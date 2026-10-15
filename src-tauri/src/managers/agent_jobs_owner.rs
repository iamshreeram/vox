//! Owner task of one background job (S2b, R2-5, R3-4, R4-2). One owner thread
//! per job owns the Child, the output readers, the timer and the command
//! channel. It is the only code that signals the job's process group, and it
//! signals only while the leader is unreaped (the leader is detected with
//! waitid WNOWAIT, so its pid and pgid cannot be reused until child.wait()).

use std::io::{self, Read};
use std::process::Child;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use super::agent_jobs_delivery::Delivery;
use super::{JobObserver, JobSnapshot, JobState};
use crate::managers::agent_jobs_output::{OutputPipeline, TailRing, CHUNK_BYTES};

/// Lines of output kept in every snapshot.
const TAIL_LINES: usize = 20;

/// Messages the owner thread receives.
pub(super) enum Msg {
    /// Stop the job: terminate the group, wait `grace`, then kill it. Output
    /// is drained for at most `drain` after the group is gone.
    Stop { grace: Duration, drain: Duration },
    /// Kill the group immediately (no grace).
    ForceKill,
    /// The leader exited (detected with WNOWAIT; the leader is not reaped yet).
    Exited,
    /// One output reader reached EOF or failed.
    ReaderDone,
}

/// Shared job record read by snapshot() and updated by the owner.
pub(super) struct Record {
    /// Job id.
    pub(super) id: u64,
    /// Project name.
    pub(super) project: String,
    /// Start instant.
    pub(super) started: Instant,
    /// End instant, set with the terminal state.
    pub(super) ended: Option<Instant>,
    /// Current state.
    pub(super) state: JobState,
    /// Output ring shared by both readers.
    pub(super) tail: Arc<Mutex<TailRing>>,
}

impl Record {
    /// Builds a point-in-time snapshot of this record.
    pub(super) fn snapshot(&self) -> JobSnapshot {
        let end = self.ended.unwrap_or_else(Instant::now);
        let tail = lock_ring(&self.tail).last(TAIL_LINES);
        JobSnapshot {
            id: self.id,
            project: self.project.clone(),
            state: self.state,
            elapsed_secs: end.duration_since(self.started).as_secs(),
            tail,
        }
    }
}

fn lock_ring(ring: &Mutex<TailRing>) -> MutexGuard<'_, TailRing> {
    ring.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Grace between SIGTERM and SIGKILL when the leader exited on its own.
const NATURAL_GRACE_MS: u64 = 300;

/// Why the owner stopped waiting for the job to run.
#[derive(Clone, Copy)]
enum Trigger {
    /// The leader exited on its own.
    Natural,
    /// A stop request (cancel, shutdown, force kill).
    Stop,
    /// The time limit passed.
    Timeout,
}

/// Everything the owner thread needs for one job.
pub(super) struct Owner {
    /// Job child (stdin null, stdout and stderr piped, own process group).
    pub(super) child: Child,
    /// Process group id (equal to the leader pid).
    pub(super) pgid: i32,
    /// Receives commands, exit and reader notifications.
    pub(super) rx: Receiver<Msg>,
    /// Sender cloned into the readers and the waiter.
    pub(super) tx: Sender<Msg>,
    /// Shared record for snapshots.
    pub(super) record: Arc<Mutex<Record>>,
    /// Observer of this job.
    pub(super) observer: Arc<dyn JobObserver>,
    /// Delivery thread of the manager.
    pub(super) delivery: Arc<Delivery>,
    /// Time limit of the job.
    pub(super) deadline: Instant,
    /// Minimum time between progress events.
    pub(super) progress_interval: Duration,
    /// Next instant a progress event may be offered.
    pub(super) next_progress: Instant,
    /// Grace between terminate and kill (manager default; a Stop may override).
    pub(super) term_grace: Duration,
    /// Output drain budget (manager default; a Stop may override).
    pub(super) drain: Duration,
    /// Readers still running.
    pub(super) readers_left: usize,
    /// The leader exit was observed (leader still unreaped).
    pub(super) leader_exited: bool,
    /// Called after the terminal state is stored and the finish is queued.
    pub(super) on_done: Option<Box<dyn FnOnce() + Send>>,
}

impl Owner {
    /// Runs the job to a terminal state, then releases the manager slot.
    pub(super) fn run(mut self) {
        if self.start_io().is_err() {
            self.stop_group(Duration::ZERO);
            let _ = self.child.wait();
            self.finish(JobState::SpawnFailed);
            return;
        }
        let trigger = self.wait_trigger();
        let status = self.cleanup(trigger);
        let state = terminal_state(trigger, status);
        self.finish(state);
    }

    /// Starts the two output readers and the leader waiter.
    fn start_io(&mut self) -> io::Result<()> {
        let ring = Arc::clone(&lock_record(&self.record).tail);
        if let Some(out) = self.child.stdout.take() {
            spawn_reader(out, OutputPipeline::new(Arc::clone(&ring)), self.tx.clone())?;
            self.readers_left += 1;
        }
        if let Some(err) = self.child.stderr.take() {
            spawn_reader(err, OutputPipeline::new(ring), self.tx.clone())?;
            self.readers_left += 1;
        }
        spawn_waiter(self.pgid, self.tx.clone())?;
        Ok(())
    }

    /// Waits for natural exit, a stop request or the time limit.
    fn wait_trigger(&mut self) -> Trigger {
        loop {
            match self.pump(self.deadline) {
                Some(Msg::Exited) => {
                    self.leader_exited = true;
                    return Trigger::Natural;
                }
                Some(Msg::Stop { grace, drain }) => {
                    self.term_grace = grace;
                    self.drain = drain;
                    return Trigger::Stop;
                }
                Some(Msg::ForceKill) => {
                    self.term_grace = Duration::ZERO;
                    return Trigger::Stop;
                }
                Some(Msg::ReaderDone) | None => {
                    if Instant::now() >= self.deadline {
                        return Trigger::Timeout;
                    }
                }
            }
        }
    }

    /// Cleans the process group, reaps the leader and drains the readers.
    fn cleanup(&mut self, trigger: Trigger) -> Option<std::process::ExitStatus> {
        // The group is always cleaned, even when the leader exited on its own:
        // a background member may still hold the pipes or keep running.
        let grace = match trigger {
            Trigger::Natural => self.term_grace.min(Duration::from_millis(NATURAL_GRACE_MS)),
            Trigger::Stop | Trigger::Timeout => self.term_grace,
        };
        self.stop_group(grace);
        let status = self.child.wait().ok();
        let drain_until = Instant::now() + self.drain;
        self.wait_readers(drain_until);
        status
    }

    /// SIGTERM to the group, wait up to `grace` (stopping early once the leader
    /// has exited and all readers are done), then ALWAYS SIGKILL the group. The
    /// leader is still unreaped here, so the pgid is ours. Honors ForceKill.
    fn stop_group(&mut self, grace: Duration) {
        signal_group(self.pgid, libc::SIGTERM);
        let until = Instant::now() + grace;
        while Instant::now() < until {
            if self.leader_exited && self.readers_left == 0 {
                break;
            }
            if let Some(Msg::ForceKill) = self.pump(until) {
                break;
            }
        }
        signal_group(self.pgid, libc::SIGKILL);
    }

    /// Waits until all readers are done or `until`. Returns true when all are done.
    fn wait_readers(&mut self, until: Instant) -> bool {
        while self.readers_left > 0 {
            if Instant::now() >= until {
                return false;
            }
            self.pump(until);
        }
        true
    }

    /// Receives the next non-reader message, or None at `until`. Offers a
    /// progress event whenever the interval elapses while running.
    fn pump(&mut self, until: Instant) -> Option<Msg> {
        loop {
            let now = Instant::now();
            self.maybe_progress(now);
            if now >= until {
                return None;
            }
            let wake = until.min(self.next_progress);
            match self.rx.recv_timeout(wake.saturating_duration_since(now)) {
                Ok(Msg::ReaderDone) => self.readers_left = self.readers_left.saturating_sub(1),
                Ok(Msg::Exited) => {
                    self.leader_exited = true;
                    return Some(Msg::Exited);
                }
                Ok(msg) => return Some(msg),
                Err(RecvTimeoutError::Timeout) => {}
                // The owner holds a sender, so this cannot happen; treat it as a kill.
                Err(RecvTimeoutError::Disconnected) => return Some(Msg::ForceKill),
            }
        }
    }

    /// Offers one progress snapshot when the interval has elapsed while running.
    fn maybe_progress(&mut self, now: Instant) {
        if now < self.next_progress {
            return;
        }
        let snap = {
            let rec = lock_record(&self.record);
            if !matches!(rec.state, JobState::Running) {
                return;
            }
            rec.snapshot()
        };
        self.delivery
            .offer_progress(Arc::clone(&self.observer), snap);
        self.next_progress = now + self.progress_interval;
    }

    /// Stores the terminal state, queues the finish and releases the slot.
    fn finish(&mut self, state: JobState) {
        let snap = {
            let mut rec = lock_record(&self.record);
            rec.state = state;
            rec.ended = Some(Instant::now());
            rec.snapshot()
        };
        self.delivery
            .enqueue_finish(Arc::clone(&self.observer), snap);
        if let Some(done) = self.on_done.take() {
            done();
        }
    }
}

fn lock_record(record: &Mutex<Record>) -> MutexGuard<'_, Record> {
    record
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Maps the trigger and the reaped status to the single terminal state.
fn terminal_state(trigger: Trigger, status: Option<std::process::ExitStatus>) -> JobState {
    match trigger {
        Trigger::Stop => JobState::Cancelled,
        Trigger::Timeout => JobState::TimedOut,
        Trigger::Natural => match status {
            Some(s) if s.success() => JobState::Succeeded,
            Some(s) => JobState::Failed { code: s.code() },
            None => JobState::Failed { code: None },
        },
    }
}

/// Sends `sig` to the process group. Only called while the leader is unreaped.
pub(super) fn signal_group(pgid: i32, sig: i32) {
    // SAFETY: killpg only sends a signal. `pgid` is the pid of the job leader,
    // which the owner has not reaped yet, so the group id cannot have been reused;
    // ESRCH (group already gone) is harmless and ignored.
    unsafe {
        libc::killpg(pgid, sig);
    }
}

/// Reads one output stream into the pipeline until EOF, then notifies the owner.
fn spawn_reader<R: Read + Send + 'static>(
    mut src: R,
    mut pipe: OutputPipeline,
    tx: Sender<Msg>,
) -> io::Result<()> {
    thread::Builder::new()
        .name("agent-job-reader".to_string())
        .spawn(move || {
            let mut buf = vec![0u8; CHUNK_BYTES];
            loop {
                match src.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => pipe.feed(&buf[..n]),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => {
                        pipe.read_error();
                        break;
                    }
                }
            }
            pipe.finish();
            let _ = tx.send(Msg::ReaderDone);
        })
        .map(|_| ())
}

/// Waits for the leader to exit without reaping it, then notifies the owner.
fn spawn_waiter(pgid: i32, tx: Sender<Msg>) -> io::Result<()> {
    thread::Builder::new()
        .name("agent-job-waiter".to_string())
        .spawn(move || {
            wait_leader_exit(pgid);
            let _ = tx.send(Msg::Exited);
        })
        .map(|_| ())
}

/// Blocks until the leader has exited; the leader stays a zombie (unreaped).
fn wait_leader_exit(pid: i32) {
    loop {
        // SAFETY: siginfo_t is plain C data and an all-zero value is valid.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: waitid with WNOWAIT only reports the state of our own child.
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if rc == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return;
        }
    }
}
