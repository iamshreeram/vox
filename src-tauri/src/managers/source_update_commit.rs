//! Commit sequence and revocation for the auto-update install (R7-2, R7-3,
//! R9-3, R9-4).
//!
//! This is pure orchestration over [`CommitEnv`]. It does no launchd and no
//! filesystem work itself: every effect goes through the trait, and the real
//! implementation is supplied by the caller. The order of the calls is the
//! contract; the fake-env tests in `source_update_commit_tests.rs` pin it.
//!
//! Everything from the consent check to the spawn runs under the single
//! [`UpdateCoordinator`] guard, so consent cannot change between the check and
//! the commit.

use super::source_update_auto::UpdateCoordinator;
use super::source_update_pending::{PendingRun, PendingState};
use super::source_update_runner::JobProbe;

/// A filesystem or launchd step failed. Carries no detail; callers log specifics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepFailed;

/// The admission lease or the agent-job reservation is held by someone else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Busy;

/// Waiting between install-job probes during revocation. Real implementations
/// sleep; test implementations return at once.
pub trait Pacer {
    /// Waits before the next probe.
    fn pause(&self);
}

/// Every effect the commit sequence needs. All methods take `&self`.
///
/// `Lease` and `Reservation` are the guards for the ownership gate and the
/// agent-job reservation. The orchestration keeps them alive until the install
/// is terminal, conclusively revoked, or confirmed gone.
pub trait CommitEnv: Pacer {
    /// Guard for `Owner::Updater` on the ownership gate.
    type Lease;
    /// Guard for the agent-job update reservation.
    type Reservation;

    /// Effective consent: checks enabled, and auto-update enabled unless `manual`.
    fn consent(&self, manual: bool) -> bool;
    /// Loads and validates `pending.json`. `Ok(None)` when there is no record.
    fn load_pending(&self) -> Result<Option<PendingRun>, StepFailed>;
    /// Takes the admission lease for `Owner::Updater`.
    fn acquire_gate(&self) -> Result<Self::Lease, Busy>;
    /// Takes the agent-job reservation.
    fn reserve_jobs(&self) -> Result<Self::Reservation, Busy>;
    /// Idleness check. Always true when `manual`.
    fn idle_ok(&self, manual: bool) -> bool;
    /// Stops the build poller and waits until it has really stopped (R9-4).
    fn stop_build_poller(&self) -> Result<(), StepFailed>;
    /// Durably records `committing` with the install ids. Returns the record as written.
    fn begin_commit(
        &self,
        built: &PendingRun,
        install_run_id: &str,
    ) -> Result<PendingRun, StepFailed>;
    /// Writes the install job's plist.
    fn write_install_plist(&self, committing: &PendingRun) -> Result<(), StepFailed>;
    /// Registers the install job with launchd.
    fn bootstrap(&self, committing: &PendingRun) -> Result<(), StepFailed>;
    /// Three-valued probe of the install label.
    fn probe_install(&self, committing: &PendingRun) -> JobProbe;
    /// Removes the install plist. Infallible for the caller; the real impl logs.
    fn remove_install_plist(&self, committing: &PendingRun);
    /// Boots the install label out of launchd.
    fn bootout_install(&self, committing: &PendingRun) -> Result<(), StepFailed>;
    /// Moves a `committing` record back to `built`.
    fn revert_to_built(&self, committing: &PendingRun) -> Result<(), StepFailed>;
    /// Starts the install poller bound to the committing run.
    fn start_install_poller(&self, committing: &PendingRun);
    /// A fresh install run id (16 lowercase hex).
    fn new_run_id(&self) -> String;
}

/// Why a commit did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferReason {
    /// Consent is off (checks disabled, or auto-update off for an automatic run).
    ConsentOff,
    /// No valid `built` record (none, invalid, or already committing).
    NoBuiltRun,
    /// The gate or the agent-job reservation is held by someone else.
    Busy,
    /// Agent jobs or work-start activity made the machine non-idle.
    NotIdle,
    /// The build poller did not confirm it stopped.
    PollerStop,
    /// A filesystem step failed before the install was bootstrapped.
    Fs,
    /// Bootstrap failed and the install was conclusively revoked; state is `built` again.
    BootstrapFailed,
}

/// Result of [`commit`].
pub enum CommitOutcome<L, R> {
    /// The install job is registered. The guards must be kept until the install
    /// is terminal or confirmed gone.
    Committed {
        /// Admission lease, kept.
        lease: L,
        /// Agent-job reservation, kept.
        reservation: R,
        /// The durable `committing` record.
        pending: PendingRun,
    },
    /// Nothing was committed; any guards taken were released.
    Deferred(DeferReason),
    /// Revocation could not prove the install absent. Guards and the
    /// `committing` record are kept; recovery must keep retrying.
    Unresolved {
        /// Admission lease, kept.
        lease: L,
        /// Agent-job reservation, kept.
        reservation: R,
        /// The durable `committing` record.
        pending: PendingRun,
    },
}

/// Outcome of [`revoke_install`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeResult {
    /// Two consecutive Absent probes, and the record is back to `built`.
    Revoked,
    /// launchd reports the label registered. The install is treated as live.
    Adopted,
    /// Not provable either way (or the revert failed). Keep everything.
    Inconclusive,
}

/// Owns the admission guards for an install that is in flight.
///
/// Release only on: terminal install status, a conclusive revoke, or when the
/// install process is confirmed gone.
pub struct AdmissionHold<L, R> {
    lease: L,
    reservation: R,
}

impl<L, R> AdmissionHold<L, R> {
    /// Wraps the two guards.
    pub fn new(lease: L, reservation: R) -> Self {
        Self { lease, reservation }
    }

    /// Releases the lease, then the reservation.
    pub fn release(self) {
        let Self { lease, reservation } = self;
        drop(lease);
        drop(reservation);
    }
}

/// Revokes an install whose bootstrap or plist write failed (R9-3).
///
/// Order is fixed: remove the plist, boot the label out, then probe. Two
/// consecutive `Absent` probes (with one [`Pacer::pause`] between) are required
/// before the record is reverted to `built`. Any `Registered` observation means
/// the install is live and is adopted. `Unknown`, or a failed bootout or revert,
/// is inconclusive. Nothing here times out.
pub fn revoke_install<E: CommitEnv>(env: &E, committing: &PendingRun) -> RevokeResult {
    env.remove_install_plist(committing);
    if env.bootout_install(committing).is_err() {
        return RevokeResult::Inconclusive;
    }
    match env.probe_install(committing) {
        JobProbe::Absent => {}
        JobProbe::Registered { .. } => return RevokeResult::Adopted,
        JobProbe::Unknown => return RevokeResult::Inconclusive,
    }
    env.pause();
    match env.probe_install(committing) {
        JobProbe::Absent => {}
        JobProbe::Registered { .. } => return RevokeResult::Adopted,
        JobProbe::Unknown => return RevokeResult::Inconclusive,
    }
    match env.revert_to_built(committing) {
        Ok(()) => RevokeResult::Revoked,
        Err(StepFailed) => RevokeResult::Inconclusive,
    }
}

/// Runs the commit sequence (R7-2, R9-4) under the coordinator guard.
///
/// Order: consent, load (must be `built`), gate, reservation, idle, stop the
/// build poller, new run id, durable `committing`, plist, bootstrap, then the
/// install poller. Failures before `begin_commit` release everything. Failures
/// after it go through [`revoke_install`]; only a conclusive revoke releases
/// the guards.
pub fn commit<E: CommitEnv>(
    env: &E,
    coordinator: &UpdateCoordinator,
    manual: bool,
) -> CommitOutcome<E::Lease, E::Reservation> {
    let _serial = coordinator.guard();
    if !env.consent(manual) {
        return CommitOutcome::Deferred(DeferReason::ConsentOff);
    }
    let built = match env.load_pending() {
        Ok(Some(run)) if run.state == PendingState::Built => run,
        Ok(_) => return CommitOutcome::Deferred(DeferReason::NoBuiltRun),
        Err(StepFailed) => return CommitOutcome::Deferred(DeferReason::Fs),
    };
    let lease = match env.acquire_gate() {
        Ok(lease) => lease,
        Err(Busy) => return CommitOutcome::Deferred(DeferReason::Busy),
    };
    let reservation = match env.reserve_jobs() {
        Ok(reservation) => reservation,
        Err(Busy) => return CommitOutcome::Deferred(DeferReason::Busy),
    };
    if !env.idle_ok(manual) {
        return CommitOutcome::Deferred(DeferReason::NotIdle);
    }
    if env.stop_build_poller().is_err() {
        return CommitOutcome::Deferred(DeferReason::PollerStop);
    }
    let install_run_id = env.new_run_id();
    let committing = match env.begin_commit(&built, &install_run_id) {
        Ok(committing) => committing,
        Err(StepFailed) => return CommitOutcome::Deferred(DeferReason::Fs),
    };
    if env.write_install_plist(&committing).is_err() || env.bootstrap(&committing).is_err() {
        return after_failed_spawn(env, lease, reservation, committing);
    }
    env.start_install_poller(&committing);
    CommitOutcome::Committed {
        lease,
        reservation,
        pending: committing,
    }
}

/// Decides what a failed plist write or bootstrap leaves behind.
fn after_failed_spawn<E: CommitEnv>(
    env: &E,
    lease: E::Lease,
    reservation: E::Reservation,
    committing: PendingRun,
) -> CommitOutcome<E::Lease, E::Reservation> {
    match revoke_install(env, &committing) {
        RevokeResult::Revoked => CommitOutcome::Deferred(DeferReason::BootstrapFailed),
        RevokeResult::Adopted => {
            env.start_install_poller(&committing);
            CommitOutcome::Committed {
                lease,
                reservation,
                pending: committing,
            }
        }
        RevokeResult::Inconclusive => CommitOutcome::Unresolved {
            lease,
            reservation,
            pending: committing,
        },
    }
}

#[cfg(test)]
#[path = "source_update_commit_tests.rs"]
mod tests;
