//! Background coding jobs: one job at a time, a process group per job, and a
//! shutdown path that cleans up before the app exits (M3, DESIGN D4, R2-5,
//! R2-6, R3-4, R4-2). Unix only: on other targets start() returns Unsupported.

#![allow(dead_code, unused_variables)]

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::managers::agent_jobs_output::TailRing;
use crate::managers::converse_confirm::JobLaunch;

#[path = "agent_jobs_delivery.rs"]
mod agent_jobs_delivery;

#[path = "agent_jobs_owner.rs"]
mod agent_jobs_owner;

#[path = "agent_jobs_spawn.rs"]
mod agent_jobs_spawn;

use agent_jobs_delivery::Delivery;
use agent_jobs_owner::{Msg, Owner, Record};

/// Default grace between SIGTERM and SIGKILL.
const DEFAULT_GRACE: Duration = Duration::from_secs(3);
/// Default output drain budget after a job stops.
const DEFAULT_DRAIN: Duration = Duration::from_secs(2);

/// Parameters for starting one job.
pub struct JobSpec {
    /// The prepared launch.
    pub launch: JobLaunch,
    /// Minimum time between progress announcements.
    pub progress_interval: Duration,
}

/// Lifecycle state of a job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobState {
    /// Still running.
    Running,
    /// Exited with status zero.
    Succeeded,
    /// Exited with a failure status.
    Failed {
        /// Exit code, if one was reported.
        code: Option<i32>,
    },
    /// Cancelled by the user.
    Cancelled,
    /// Stopped for exceeding its time limit.
    TimedOut,
    /// The process could not be started.
    SpawnFailed,
}

/// A point-in-time view of a job.
pub struct JobSnapshot {
    /// Job id.
    pub id: u64,
    /// Project name.
    pub project: String,
    /// Current state.
    pub state: JobState,
    /// Seconds since the job started.
    pub elapsed_secs: u64,
    /// The most recent output lines.
    pub tail: Vec<String>,
}

/// Why a job could not be started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobError {
    /// Another job is active, or no Finish reservation is left.
    Busy,
    /// Jobs are not supported here.
    Unsupported,
    /// The coding CLI binary is not usable.
    BadBinary,
    /// The project directory is not usable.
    BadDir,
    /// The process failed to spawn.
    SpawnFailed,
    /// The app is closing; no new jobs are admitted.
    ShuttingDown,
}

/// Receives job progress and completion.
pub trait JobObserver: Send + Sync {
    /// Called with each progress snapshot.
    fn on_progress(&self, snap: &JobSnapshot);
    /// Called once when the job reaches a final state.
    fn on_finish(&self, snap: &JobSnapshot);
}

/// Budgets for a graceful shutdown of the job manager.
pub struct ShutdownConfig {
    /// Time between terminate and kill.
    pub term_grace: Duration,
    /// Time to drain output after the job stops.
    pub drain: Duration,
    /// Overall shutdown deadline.
    pub deadline: Duration,
}

/// Budgets for the app-exit coordinator.
pub struct ExitConfig {
    /// Time allowed for a clean job stop before forcing it.
    pub deadline: Duration,
    /// Time between terminate and kill.
    pub term_grace: Duration,
    /// Time to drain output after the job stops.
    pub drain: Duration,
    /// Last resort: how long to wait after the deadline before giving up.
    pub hard_cap: Duration,
}

impl Default for ExitConfig {
    fn default() -> Self {
        ExitConfig {
            deadline: Duration::from_secs(5),
            term_grace: Duration::from_secs(1),
            drain: Duration::from_secs(1),
            hard_cap: Duration::from_secs(15),
        }
    }
}

/// The active job as seen by the manager.
struct Active {
    /// Command channel of the owner task.
    tx: Sender<Msg>,
    /// Shared record (state, tail, timing).
    record: Arc<Mutex<Record>>,
    /// True once the owner has finished the job and released the slot.
    done: Arc<Mutex<bool>>,
}

/// Mutable manager state guarded by one mutex.
struct State {
    /// Next job id.
    next_id: u64,
    /// Admission is closed (close_admission).
    closed: bool,
    /// Current or most recent job.
    job: Option<Active>,
}

/// Owns the single active background job.
pub struct JobManager {
    state: Mutex<State>,
    delivery: Arc<Delivery>,
    /// Default time between SIGTERM and SIGKILL.
    term_grace: Duration,
    /// Default output drain budget.
    drain: Duration,
}

fn lock_state(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl JobManager {
    /// Creates a manager with the default termination grace.
    pub fn new() -> Arc<Self> {
        Self::with_timing(DEFAULT_GRACE, DEFAULT_DRAIN)
    }

    /// Creates a manager with an explicit termination grace.
    pub fn with_grace(term_grace: Duration) -> Arc<Self> {
        Self::with_timing(term_grace, DEFAULT_DRAIN)
    }

    /// Creates a manager with explicit termination grace and output drain budgets (tests and shutdown).
    pub fn with_timing(term_grace: Duration, drain: Duration) -> Arc<Self> {
        Arc::new(JobManager {
            state: Mutex::new(State {
                next_id: 1,
                closed: false,
                job: None,
            }),
            delivery: Arc::new(Delivery::spawn()),
            term_grace,
            drain,
        })
    }

    /// Starts a job. Returns its id.
    pub fn start(
        self: &Arc<Self>,
        spec: JobSpec,
        observer: Arc<dyn JobObserver>,
    ) -> Result<u64, JobError> {
        let mut st = lock_state(&self.state);
        if st.closed {
            return Err(JobError::ShuttingDown);
        }
        if let Some(active) = st.job.as_ref() {
            if !*lock_done(&active.done) {
                return Err(JobError::Busy);
            }
        }
        agent_jobs_spawn::check_binary(&spec.launch.config)?;
        agent_jobs_spawn::check_dir(&spec.launch.dir)?;
        if !self.delivery.try_reserve() {
            return Err(JobError::Busy);
        }
        let child = match agent_jobs_spawn::spawn_job(&spec.launch) {
            Ok(child) => child,
            Err(e) => {
                self.delivery.cancel_reservation();
                return Err(e);
            }
        };
        let id = st.next_id;
        st.next_id += 1;
        let pgid = child.id() as i32;
        let (tx, rx) = mpsc::channel();
        let tail = Arc::new(Mutex::new(TailRing::new()));
        let started = Instant::now();
        let record = Arc::new(Mutex::new(Record {
            id,
            project: spec.launch.project.clone(),
            started,
            ended: None,
            state: JobState::Running,
            tail,
        }));
        let done = Arc::new(Mutex::new(false));
        let done_flag = Arc::clone(&done);
        let owner = Owner {
            child,
            pgid,
            rx,
            tx: tx.clone(),
            record: Arc::clone(&record),
            observer,
            delivery: Arc::clone(&self.delivery),
            deadline: started + spec.launch.config.timeout,
            progress_interval: spec.progress_interval,
            next_progress: started + spec.progress_interval,
            term_grace: self.term_grace,
            drain: self.drain,
            readers_left: 0,
            on_done: Some(Box::new(move || {
                *lock_done(&done_flag) = true;
            })),
        };
        std::thread::Builder::new()
            .name("agent-job-owner".to_string())
            .spawn(move || owner.run())
            .map_err(|_| JobError::SpawnFailed)?;
        st.job = Some(Active { tx, record, done });
        Ok(id)
    }

    /// Cancels the active job. Returns `true` if one was cancelled.
    pub fn cancel(&self) -> bool {
        self.send_stop(self.term_grace, self.drain)
    }

    /// Snapshot of the current or last job.
    pub fn snapshot(&self) -> Option<JobSnapshot> {
        let st = lock_state(&self.state);
        st.job.as_ref().map(|a| lock_record(&a.record).snapshot())
    }

    /// Whether a job is active.
    pub fn has_active_job(&self) -> bool {
        let st = lock_state(&self.state);
        st.job.as_ref().is_some_and(|a| !*lock_done(&a.done))
    }

    /// Stops admission of new jobs. Synchronous; a racing start either completes first or fails.
    pub fn close_admission(&self) {
        lock_state(&self.state).closed = true;
    }

    /// Stops the active job within the budgets. Returns `true` when it is clean.
    pub fn shutdown(&self, cfg: &ShutdownConfig) -> bool {
        self.close_admission();
        let until = Instant::now() + cfg.deadline;
        self.send_stop(cfg.term_grace, cfg.drain);
        while self.has_active_job() {
            if Instant::now() >= until {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }

    /// Asks the owner to terminate the job with the given budgets. Returns
    /// `true` when a running job was asked to stop.
    fn send_stop(&self, grace: Duration, drain: Duration) -> bool {
        let st = lock_state(&self.state);
        match st.job.as_ref() {
            Some(a) if !*lock_done(&a.done) => a.tx.send(Msg::Stop { grace, drain }).is_ok(),
            _ => false,
        }
    }
}

fn lock_done(done: &Mutex<bool>) -> MutexGuard<'_, bool> {
    done.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_record(record: &Mutex<Record>) -> MutexGuard<'_, Record> {
    record
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Result of the exit cleanup.
pub enum CleanupResult {
    /// The job group is gone and the leader reaped.
    Confirmed,
    /// Cleanup could not be confirmed within the hard cap.
    Unconfirmed,
}

/// Decision returned to the exit event.
pub enum ExitDecision {
    /// Let the exit proceed.
    Allow,
    /// Keep the process alive until cleanup finishes.
    Deferred,
}

/// Serializes app-exit handling for both quit paths.
pub struct ExitCoordinator;

impl ExitCoordinator {
    /// Creates a coordinator.
    pub fn new() -> Arc<Self> {
        todo!()
    }

    /// Handles a native exit request.
    pub fn on_exit_requested(
        self: &Arc<Self>,
        manager: &Arc<JobManager>,
        prevent: impl FnOnce(),
        finish: impl FnOnce(CleanupResult) + Send + 'static,
        cfg: &ExitConfig,
    ) -> ExitDecision {
        todo!()
    }

    /// Handles a tray quit request.
    pub fn request_exit(
        self: &Arc<Self>,
        manager: &Arc<JobManager>,
        finish: impl FnOnce(CleanupResult) + Send + 'static,
        cfg: &ExitConfig,
    ) {
        todo!()
    }
}

#[cfg(test)]
#[path = "agent_jobs_tests.rs"]
mod agent_jobs_tests;
