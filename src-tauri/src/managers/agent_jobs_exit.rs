//! App-exit coordination for background jobs (M3, DESIGN R4-2, R5-1). One
//! idempotent coordinator serves the tray quit and the native exit request.
//! This module never signals a process itself: all signalling is done by the
//! job owner task through the ExitTarget methods below.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::agent_jobs_owner::Msg;
use super::JobManager;

/// How often the cleanup thread polls the target for confirmation.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Budgets for the app-exit coordinator.
#[derive(Clone, Copy, Debug)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupResult {
    /// The job group is gone and the leader reaped.
    Confirmed,
    /// Cleanup could not be confirmed within the hard cap.
    Unconfirmed,
}

/// Decision returned to the exit event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    // The path-qualified calls below resolve to the inherent JobManager methods
    // of the same name, so they do not recurse.
    fn close_admission(&self) {
        JobManager::close_admission(self);
    }

    fn has_active_job(&self) -> bool {
        JobManager::has_active_job(self)
    }

    fn request_terminate(&self, grace: Duration) {
        self.send_active(Msg::Stop {
            grace,
            drain: self.drain,
        });
    }

    fn request_force_kill(&self) {
        self.send_active(Msg::ForceKill);
    }

    fn cleanup_confirmed(&self) -> bool {
        !JobManager::has_active_job(self)
    }
}

/// Exit phase of the coordinator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// No exit has been requested yet.
    Running,
    /// A job is being cleaned up; finish is pending.
    Cleaning,
    /// Finish has been issued (or the idle exit was allowed).
    Cleaned,
}

/// Serializes app-exit handling for both quit paths.
pub struct ExitCoordinator {
    phase: Mutex<Phase>,
}

fn lock_phase(phase: &Mutex<Phase>) -> MutexGuard<'_, Phase> {
    phase
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_slot<F>(slot: &Mutex<Option<F>>) -> MutexGuard<'_, Option<F>> {
    slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl ExitCoordinator {
    /// Creates a coordinator.
    pub fn new() -> Arc<Self> {
        Arc::new(ExitCoordinator {
            phase: Mutex::new(Phase::Running),
        })
    }

    /// Handles a native exit request (RunEvent::ExitRequested).
    pub fn on_exit_requested(
        self: &Arc<Self>,
        manager: &Arc<JobManager>,
        prevent: impl FnOnce(),
        finish: impl FnOnce(CleanupResult) + Send + 'static,
        cfg: &ExitConfig,
    ) -> ExitDecision {
        let target: Arc<dyn ExitTarget> = manager.clone();
        self.on_exit_requested_with(target, prevent, finish, cfg)
    }

    /// Handles a tray quit request.
    pub fn request_exit(
        self: &Arc<Self>,
        manager: &Arc<JobManager>,
        finish: impl FnOnce(CleanupResult) + Send + 'static,
        cfg: &ExitConfig,
    ) {
        let target: Arc<dyn ExitTarget> = manager.clone();
        self.request_exit_with(target, finish, cfg);
    }

    /// Generic form of on_exit_requested over any ExitTarget.
    pub(crate) fn on_exit_requested_with(
        self: &Arc<Self>,
        target: Arc<dyn ExitTarget>,
        prevent: impl FnOnce(),
        finish: impl FnOnce(CleanupResult) + Send + 'static,
        cfg: &ExitConfig,
    ) -> ExitDecision {
        self.decide(target, prevent, finish, *cfg, false)
    }

    /// Generic form of request_exit over any ExitTarget.
    pub(crate) fn request_exit_with(
        self: &Arc<Self>,
        target: Arc<dyn ExitTarget>,
        finish: impl FnOnce(CleanupResult) + Send + 'static,
        cfg: &ExitConfig,
    ) {
        self.decide(target, || {}, finish, *cfg, true);
    }

    /// Shared decision for both quit paths. `tray` makes an idle Allow call
    /// `finish(Confirmed)` itself (R5-1); a native Allow never finishes.
    fn decide<P, F>(
        self: &Arc<Self>,
        target: Arc<dyn ExitTarget>,
        prevent: P,
        finish: F,
        cfg: ExitConfig,
        tray: bool,
    ) -> ExitDecision
    where
        P: FnOnce(),
        F: FnOnce(CleanupResult) + Send + 'static,
    {
        let mut phase = lock_phase(&self.phase);
        match *phase {
            // Exit is already under way (or the idle exit was allowed): no
            // second finish, and the native re-entrant request is allowed.
            Phase::Cleaned => return ExitDecision::Allow,
            Phase::Cleaning => {
                prevent();
                return ExitDecision::Deferred;
            }
            Phase::Running => {}
        }
        target.close_admission();
        if !target.has_active_job() {
            *phase = Phase::Cleaned;
            drop(phase);
            if tray {
                finish(CleanupResult::Confirmed);
            }
            return ExitDecision::Allow;
        }
        prevent();
        *phase = Phase::Cleaning;
        drop(phase);
        self.start_cleanup(target, finish, cfg);
        ExitDecision::Deferred
    }

    /// Spawns the single cleanup thread that owns the pending finish.
    fn start_cleanup<F>(self: &Arc<Self>, target: Arc<dyn ExitTarget>, finish: F, cfg: ExitConfig)
    where
        F: FnOnce(CleanupResult) + Send + 'static,
    {
        let slot: Mutex<Option<F>> = Mutex::new(Some(finish));
        let slot = Arc::new(slot);
        let worker_slot = Arc::clone(&slot);
        let coord = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("agent-exit-cleanup".to_string())
            .spawn(move || coord.run_cleanup(target.as_ref(), &worker_slot, cfg));
        if spawned.is_err() {
            log::error!("could not start the job cleanup thread; cleanup reported unconfirmed");
            self.complete(&slot, CleanupResult::Unconfirmed);
        }
    }

    /// Asks the target to stop the job, forces it at the deadline, and waits for
    /// confirmation. Finish runs only after confirmation, or after the hard cap.
    fn run_cleanup<F>(&self, target: &dyn ExitTarget, slot: &Mutex<Option<F>>, cfg: ExitConfig)
    where
        F: FnOnce(CleanupResult),
    {
        target.request_terminate(cfg.term_grace);
        let started = Instant::now();
        let deadline = started + cfg.deadline;
        let hard_stop = deadline + cfg.hard_cap;
        let mut forced = false;
        let result = loop {
            if target.cleanup_confirmed() {
                break CleanupResult::Confirmed;
            }
            let now = Instant::now();
            if !forced && now >= deadline {
                target.request_force_kill();
                forced = true;
            }
            if now >= hard_stop {
                log::warn!("job cleanup not confirmed after the hard cap; exiting anyway");
                break CleanupResult::Unconfirmed;
            }
            std::thread::sleep(POLL_INTERVAL.min(hard_stop.saturating_duration_since(now)));
        };
        self.complete(slot, result);
    }

    /// Marks the coordinator Cleaned, then issues the single finish. The phase is
    /// set first so a re-entrant exit request from finish is allowed.
    fn complete<F>(&self, slot: &Mutex<Option<F>>, result: CleanupResult)
    where
        F: FnOnce(CleanupResult),
    {
        *lock_phase(&self.phase) = Phase::Cleaned;
        let finish = lock_slot(slot).take();
        if let Some(finish) = finish {
            finish(result);
        }
    }
}
