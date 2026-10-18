//! Bounded vault IO worker: one dedicated thread for filesystem-touching jobs (DESIGN Rev 3, 3.3).
//!
//! Contract: a single std thread named `vault-io` with a bounded job queue (capacity 4). Callers
//! block on their own thread (use `spawn_blocking` from async code) until the job finishes or its
//! deadline passes. When the queue is full, or the worker has been busy on one job for more than
//! 10 s (wedged), new submissions fail at once with `IoError::Unavailable`. No extra worker is
//! ever spawned after a timeout, so at most one wedged thread can exist.

// Not yet called by the app; nothing in this change calls this module.
#![allow(dead_code)]

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Capacity of the bounded job queue (DESIGN 3.3).
const QUEUE_CAPACITY: usize = 4;

/// A job running for longer than this makes the worker wedged (DESIGN 3.3).
const WEDGE_AFTER: Duration = Duration::from_secs(10);

/// A type-erased unit of work queued on the worker thread.
pub(crate) type Job = Box<dyn FnOnce() + Send>;

/// Why a job did not produce a value (DESIGN 3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoError {
    /// Queue full, worker wedged, or worker gone. The job was not run.
    Unavailable,
    /// The job did not finish before its deadline.
    TimedOut,
}

/// Handle to the single bounded vault-IO worker thread (DESIGN 3.3).
pub struct VaultIoWorker {
    /// Bounded sender into the worker's queue (capacity 4).
    jobs: SyncSender<Job>,
    /// Start time of the job currently running on the worker, if any.
    busy_since: Arc<Mutex<Option<Instant>>>,
    /// Join handle of the `vault-io` thread; `None` when the spawn failed.
    handle: Option<JoinHandle<()>>,
    /// Set to true by the worker thread just before it exits.
    exited: Arc<AtomicBool>,
    /// Number of `vault-io` threads spawned by this handle (0 or 1).
    spawned: usize,
}

/// Locks the busy marker, recovering the value if a previous holder panicked.
fn lock_busy(busy: &Mutex<Option<Instant>>) -> MutexGuard<'_, Option<Instant>> {
    busy.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Body of the `vault-io` thread: runs queued jobs in order until every sender is gone.
/// A panicking job is contained so the thread keeps serving later jobs; the panicking job's
/// result sender is dropped during the unwind, which the caller sees as `Unavailable`.
fn run_worker(rx: Receiver<Job>, busy: Arc<Mutex<Option<Instant>>>, exited: Arc<AtomicBool>) {
    while let Ok(job) = rx.recv() {
        *lock_busy(&busy) = Some(Instant::now());
        let _ = catch_unwind(AssertUnwindSafe(job));
        *lock_busy(&busy) = None;
    }
    exited.store(true, Ordering::SeqCst);
}

impl VaultIoWorker {
    /// Spawns the `vault-io` thread and returns its handle (DESIGN 3.3).
    ///
    /// If the OS refuses the thread, the handle is returned anyway with no thread: every
    /// later submission then fails with `Unavailable`, and `thread_count` reports 0.
    pub fn start() -> Self {
        let (jobs, rx) = mpsc::sync_channel::<Job>(QUEUE_CAPACITY);
        let busy_since = Arc::new(Mutex::new(None));
        let exited = Arc::new(AtomicBool::new(false));
        let spawned_busy = Arc::clone(&busy_since);
        let spawned_exit = Arc::clone(&exited);
        let spawn = thread::Builder::new()
            .name("vault-io".to_owned())
            .spawn(move || run_worker(rx, spawned_busy, spawned_exit));
        let (handle, spawned) = match spawn {
            Ok(handle) => (Some(handle), 1),
            Err(_) => (None, 0),
        };
        VaultIoWorker {
            jobs,
            busy_since,
            handle,
            exited,
            spawned,
        }
    }

    /// Runs `job` on the worker and blocks the calling thread until it finishes or `deadline`
    /// passes (DESIGN 3.3). Returns `Unavailable` at once when the queue is full or wedged.
    ///
    /// Returns `TimedOut` when the deadline passes first; the job keeps running on the worker
    /// and its result is discarded. Returns `Unavailable` when the job panics.
    pub fn submit<T: Send + 'static>(
        &self,
        job: Box<dyn FnOnce() -> T + Send>,
        deadline: Duration,
    ) -> Result<T, IoError> {
        if self.wedged() {
            return Err(IoError::Unavailable);
        }
        let (result_tx, result_rx) = mpsc::sync_channel::<T>(1);
        let wrapped: Job = Box::new(move || {
            let value = job();
            let _ = result_tx.send(value);
        });
        match self.jobs.try_send(wrapped) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                return Err(IoError::Unavailable);
            }
        }
        match result_rx.recv_timeout(deadline) {
            Ok(value) => Ok(value),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(IoError::TimedOut),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(IoError::Unavailable),
        }
    }

    /// True when the current job has been running for more than 10 s (DESIGN 3.3).
    pub fn wedged(&self) -> bool {
        match *lock_busy(&self.busy_since) {
            Some(started) => started.elapsed() > WEDGE_AFTER,
            None => false,
        }
    }

    /// Number of `vault-io` threads ever spawned by this handle; must stay 1 (DESIGN 3.3).
    pub fn thread_count(&self) -> usize {
        self.spawned
    }

    /// Flag set to true by the `vault-io` thread just before it exits, so tests can observe that
    /// dropping the worker joins the thread (DESIGN 3.3).
    pub fn exit_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.exited)
    }
}

/// Longest time `Drop` waits for the `vault-io` thread before detaching it (DESIGN 3.0 F5).
const DROP_WAIT: Duration = Duration::from_secs(2);

/// Waits up to `budget` for `handle`'s thread to finish, then joins it if it has. When the
/// budget passes first the handle is dropped, which detaches the thread: a wedged job can
/// never block the owner's drop.
fn join_within(handle: JoinHandle<()>, budget: Duration) {
    let deadline = Instant::now() + budget;
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let _ = handle.join();
}

impl Drop for VaultIoWorker {
    /// Closes the queue, waits at most 2 s for the `vault-io` thread, then detaches it if a
    /// job is still running. Drop never blocks longer than that budget.
    fn drop(&mut self) {
        let (idle, _) = mpsc::sync_channel::<Job>(1);
        // Replacing the sender drops the last live sender, which ends the worker loop.
        drop(std::mem::replace(&mut self.jobs, idle));
        if let Some(handle) = self.handle.take() {
            join_within(handle, DROP_WAIT);
        }
    }
}
