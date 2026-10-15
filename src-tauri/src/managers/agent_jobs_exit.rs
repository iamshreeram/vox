//! App-exit coordination for background jobs (M3, DESIGN R4-2, R5-1). One
//! idempotent coordinator serves the tray quit and the native exit request.
//! This module never signals a process itself: all signalling is done by the
//! job owner task through the ExitTarget methods below.
//!
//! Phase A (S2c tests): todo!() bodies only. Phase B implements them.

use std::sync::Arc;
use std::time::Duration;

use super::JobManager;

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

/// What the coordinator needs from the job manager. Implemented by JobManager;
/// tests use a scripted fake. request_* return immediately.
pub(crate) trait ExitTarget: Send + Sync {
    /// Stops admission of new jobs (must be the first call).
    fn close_admission(&self);
    /// Whether a job is active.
    fn has_active_job(&self) -> bool;
    /// Asks the owner to terminate the job with the given grace.
    fn request_terminate(&self, grace: Duration);
    /// Asks the owner to force-kill the job group.
    fn request_force_kill(&self);
    /// True once the active slot is free (group cleaned and leader reaped).
    fn cleanup_confirmed(&self) -> bool;
}

impl ExitTarget for JobManager {
    fn close_admission(&self) {
        todo!()
    }

    fn has_active_job(&self) -> bool {
        todo!()
    }

    fn request_terminate(&self, grace: Duration) {
        todo!()
    }

    fn request_force_kill(&self) {
        todo!()
    }

    fn cleanup_confirmed(&self) -> bool {
        todo!()
    }
}

/// Serializes app-exit handling for both quit paths.
pub struct ExitCoordinator;

impl ExitCoordinator {
    /// Creates a coordinator.
    pub fn new() -> Arc<Self> {
        todo!()
    }

    /// Handles a native exit request (RunEvent::ExitRequested).
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

    /// Generic form of on_exit_requested over any ExitTarget.
    pub(crate) fn on_exit_requested_with(
        self: &Arc<Self>,
        target: Arc<dyn ExitTarget>,
        prevent: impl FnOnce(),
        finish: impl FnOnce(CleanupResult) + Send + 'static,
        cfg: &ExitConfig,
    ) -> ExitDecision {
        todo!()
    }

    /// Generic form of request_exit over any ExitTarget.
    pub(crate) fn request_exit_with(
        self: &Arc<Self>,
        target: Arc<dyn ExitTarget>,
        finish: impl FnOnce(CleanupResult) + Send + 'static,
        cfg: &ExitConfig,
    ) {
        todo!()
    }
}
