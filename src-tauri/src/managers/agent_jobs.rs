//! Background coding jobs: one job at a time, a process group per job, and a
//! shutdown path that cleans up before the app exits (M3). Scaffolding only:
//! bodies are filled by later M3 steps.

#![allow(dead_code, unused_variables)]

use std::sync::Arc;
use std::time::Duration;

use crate::managers::converse_confirm::JobLaunch;

#[path = "agent_jobs_spawn.rs"]
mod agent_jobs_spawn;

#[path = "agent_jobs_delivery.rs"]
mod agent_jobs_delivery;

#[path = "agent_jobs_owner.rs"]
mod agent_jobs_owner;

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
pub enum JobError {
    /// Another job is active.
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

/// Owns the single active background job.
pub struct JobManager;

impl JobManager {
    /// Creates a manager with the default termination grace.
    pub fn new() -> Arc<Self> {
        todo!()
    }

    /// Creates a manager with an explicit termination grace.
    pub fn with_grace(term_grace: Duration) -> Arc<Self> {
        todo!()
    }

    /// Creates a manager with explicit termination grace and output drain budgets (tests and shutdown).
    pub fn with_timing(term_grace: Duration, drain: Duration) -> Arc<Self> {
        todo!()
    }

    /// Starts a job. Returns its id.
    pub fn start(
        self: &Arc<Self>,
        spec: JobSpec,
        observer: Arc<dyn JobObserver>,
    ) -> Result<u64, JobError> {
        todo!()
    }

    /// Cancels the active job. Returns `true` if one was cancelled.
    pub fn cancel(&self) -> bool {
        todo!()
    }

    /// Snapshot of the current or last job.
    pub fn snapshot(&self) -> Option<JobSnapshot> {
        todo!()
    }

    /// Whether a job is active.
    pub fn has_active_job(&self) -> bool {
        todo!()
    }

    /// Stops admission of new jobs. Synchronous; a racing start either completes first or fails.
    pub fn close_admission(&self) {
        todo!()
    }

    /// Stops the active job within the budgets. Returns `true` when it is clean.
    pub fn shutdown(&self, cfg: &ShutdownConfig) -> bool {
        todo!()
    }
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
