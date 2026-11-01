//! Real [`CommitEnv`] for the auto-update install (DESIGN R7-2, R8-2, R9-3, R9-4).
//!
//! [`AppCommitEnv`] maps each trait call onto the real effects: the pending
//! record, the install plist, the real launchd adapter (through [`JobControl`]),
//! the ownership gate, the agent-job reservation and the poller registry. Every
//! external seam is a parameter, so the tests drive the same code with fakes.
//!
//! Deadlock rule (B3b-1): [`app_commit`] takes the coordinator guard itself, so
//! it must NOT be called while the caller holds [`update_coordinator`], and it
//! must not call `write_settings`.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::job_runtime::JobWiring;
use crate::managers::agent_jobs::{JobManager, UpdateReservation};
use crate::managers::ownership::{self, idle_for, Lease, Owner, OwnershipGate};
use crate::managers::source_update::{checkout_dir, running_build_sha, SourceUpdateRuntime};
use crate::managers::source_update_auto::consent::AutoMarker;
use crate::managers::source_update_auto::{update_coordinator, AUTO_IDLE};
use crate::settings::{get_settings, update_checks_effectively_enabled};

use super::source_update_commit::{
    commit, AdmissionHold, Busy, CommitEnv, CommitOutcome, DeferReason, Pacer, StepFailed,
};
use super::source_update_pending::{PendingRun, PendingState, PendingStore};
use super::source_update_runner::{
    new_run_id, status_path, update_state_dir, JobControl, JobProbe,
};
use super::source_update_runner_poller::{
    self as poller, PollerConfig, PollerHandle, PollerStopError,
};
use super::source_update_runner_run::{self as run, RunSite};

/// Pause between two install-job probes during revocation (real launchd only).
pub const REVOKE_PAUSE: Duration = Duration::from_secs(2);

/// The process start instant. Initialised by [`note_process_start`] at runtime
/// creation, so the idle check cannot be satisfied by a clock that began late.
static PROCESS_START: OnceLock<Instant> = OnceLock::new();

/// Records and returns the process start instant (first call wins).
pub fn note_process_start() -> Instant {
    *PROCESS_START.get_or_init(Instant::now)
}

/// Pure idle rule (R7-5): the gate has been free, and no agent job ran, for
/// at least [`AUTO_IDLE`], counted from the latest of the three instants.
/// `gate_idle_since` is `None` when the gate was not free.
pub fn idle_ready(
    now: Instant,
    gate_idle_since: Option<Instant>,
    last_activity: Instant,
    process_start: Instant,
) -> bool {
    let Some(gate_idle) = gate_idle_since else {
        return false;
    };
    let since = gate_idle.max(last_activity).max(process_start);
    idle_for(now, since) >= AUTO_IDLE
}

/// Everything the commit needs from the running application. The Tauri
/// implementation is [`TauriHost`]; tests supply their own.
pub trait CommitHost {
    /// Update checks are effectively enabled (persisted setting, env override applied).
    fn checks_enabled(&self) -> bool;
    /// The owner's automatic-update switch is on.
    fn auto_enabled(&self) -> bool;
    /// The ownership gate shared with dictation, conversation and enrollment.
    fn gate(&self) -> &Arc<OwnershipGate>;
    /// The agent-job manager that takes the update reservation.
    fn jobs(&self) -> Arc<JobManager>;
    /// Monotonic now.
    fn now(&self) -> Instant;
    /// Monotonic process start.
    fn process_start(&self) -> Instant;
    /// Stops the registered build poller and waits for it (R9-4).
    fn stop_build_poller(&self) -> Result<(), PollerStopError>;
    /// Spawns the poller for `config` and registers it under `run_id`.
    fn start_poller(&self, run_id: &str, config: PollerConfig);
}

/// The real commit environment for one commit attempt.
pub struct AppCommitEnv<'a, C: JobControl, H: CommitHost> {
    host: &'a H,
    control: &'a C,
    site: RunSite,
    store: PendingStore,
    pause: Duration,
    /// The gate's idle instant read just before the lease was taken.
    gate_idle_before: Mutex<Option<Instant>>,
}

impl<'a, C: JobControl, H: CommitHost> AppCommitEnv<'a, C, H> {
    /// Builds the environment. `pause` is the wait between revocation probes.
    pub fn new(
        host: &'a H,
        control: &'a C,
        site: RunSite,
        store: PendingStore,
        pause: Duration,
    ) -> Self {
        Self {
            host,
            control,
            site,
            store,
            pause,
            gate_idle_before: Mutex::new(None),
        }
    }

    fn gate_idle_before(&self) -> MutexGuard<'_, Option<Instant>> {
        self.gate_idle_before
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl<C: JobControl, H: CommitHost> Pacer for AppCommitEnv<'_, C, H> {
    fn pause(&self) {
        if !self.pause.is_zero() {
            thread::sleep(self.pause);
        }
    }
}

impl<C: JobControl, H: CommitHost> CommitEnv for AppCommitEnv<'_, C, H> {
    type Lease = Lease;
    type Reservation = UpdateReservation;

    fn consent(&self, manual: bool) -> bool {
        self.host.checks_enabled() && (manual || self.host.auto_enabled())
    }

    fn load_pending(&self) -> Result<Option<PendingRun>, StepFailed> {
        self.store.load().map_err(|_| StepFailed)
    }

    fn acquire_gate(&self) -> Result<Lease, Busy> {
        *self.gate_idle_before() = self.host.gate().idle_since();
        self.host
            .gate()
            .try_acquire(Owner::Updater)
            .map_err(|_| Busy)
    }

    fn reserve_jobs(&self) -> Result<UpdateReservation, Busy> {
        self.host.jobs().try_reserve_for_update().map_err(|_| Busy)
    }

    fn idle_ok(&self, manual: bool) -> bool {
        if manual {
            return true;
        }
        let gate_idle = *self.gate_idle_before();
        let last_activity = self.host.jobs().last_activity();
        idle_ready(
            self.host.now(),
            gate_idle,
            last_activity,
            self.host.process_start(),
        )
    }

    fn stop_build_poller(&self) -> Result<(), StepFailed> {
        self.host.stop_build_poller().map_err(|_| StepFailed)
    }

    fn begin_commit(
        &self,
        built: &PendingRun,
        install_run_id: &str,
    ) -> Result<PendingRun, StepFailed> {
        self.store
            .begin_commit(
                built,
                install_run_id,
                crate::managers::source_update_runner::now_unix(),
            )
            .map_err(|_| StepFailed)
    }

    fn write_install_plist(&self, committing: &PendingRun, silent: bool) -> Result<(), StepFailed> {
        let PendingState::Committing(info) = &committing.state else {
            return Err(StepFailed);
        };
        if silent {
            // The marker must exist before bootstrap, so the first progress frame is silent.
            AutoMarker::new(self.store.state_dir())
                .write(&info.install_run_id)
                .map_err(|_| StepFailed)?;
        }
        let inputs = self
            .site
            .inputs(&info.install_run_id, &committing.target_full);
        run::write_install_plist(&inputs, committing)
            .map(|_| ())
            .map_err(|_| StepFailed)
    }

    fn bootstrap(&self, committing: &PendingRun) -> Result<(), StepFailed> {
        run::bootstrap_install(self.control, committing).map_err(|_| StepFailed)
    }

    fn probe_install(&self, committing: &PendingRun) -> JobProbe {
        match &committing.state {
            PendingState::Committing(info) => self.control.probe(&info.install_label),
            PendingState::Built => JobProbe::Unknown,
        }
    }

    fn remove_install_plist(&self, committing: &PendingRun) {
        run::remove_install_plist(committing);
    }

    fn bootout_install(&self, committing: &PendingRun) -> Result<(), StepFailed> {
        run::bootout_install(self.control, committing).map_err(|_| StepFailed)
    }

    /// Reverts to `built` and removes the install marker. Called only after a conclusive
    /// revocation, so a live install (adopted instead) keeps its marker and its silence.
    fn revert_to_built(&self, committing: &PendingRun) -> Result<(), StepFailed> {
        self.store
            .revert(committing)
            .map(|_| ())
            .map_err(|_| StepFailed)?;
        if let PendingState::Committing(info) = &committing.state {
            if let Err(error) = AutoMarker::new(self.store.state_dir()).remove(&info.install_run_id)
            {
                log::warn!("could not remove the install marker after revocation: {error}");
            }
        }
        Ok(())
    }

    fn start_install_poller(&self, committing: &PendingRun) {
        let PendingState::Committing(info) = &committing.state else {
            return;
        };
        let state_dir = self.store.state_dir().to_path_buf();
        let config = PollerConfig {
            run_id: info.install_run_id.clone(),
            label: info.install_label.clone(),
            status_file: status_path(&state_dir),
            state_dir,
            plist: info.install_plist.clone(),
            lock_dir: self.site.lock_dir.clone(),
            running_build_sha: running_build_sha(),
        };
        self.host.start_poller(&info.install_run_id, config);
    }

    fn new_run_id(&self) -> String {
        new_run_id()
    }
}

/// Admission state behind [`UpdateAdmission`]. One mutex guards all of it.
#[derive(Default)]
struct AdmissionState {
    /// The guards of an install in flight, once stored.
    hold: Option<AdmissionHold<Lease, UpdateReservation>>,
    /// A lease taken by a restore whose reservation was refused. Kept, so the retry only
    /// takes the reservation (B6 minor 6).
    partial_lease: Option<Lease>,
    /// A commit attempt is running and may store guards at its end.
    commit_in_flight: bool,
    /// A release arrived during the attempt, before the guards were stored (B6 minor 5).
    release_requested: bool,
}

/// Admission guards kept while an install is in flight (R7-2 step 7, R9-3).
/// Released by [`release_admission`] once the install is terminal or gone.
#[derive(Default)]
pub struct UpdateAdmission {
    state: Mutex<AdmissionState>,
}

impl UpdateAdmission {
    /// Nothing held.
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks the start of a commit attempt. The install poller can settle before the attempt
    /// stores its guards, so a release in this window is remembered (B6 minor 5).
    pub fn begin(&self) {
        let mut state = self.guard();
        state.commit_in_flight = true;
        state.release_requested = false;
    }

    /// Marks the end of a commit attempt. Nothing is remembered past this point.
    pub fn finish(&self) {
        let mut state = self.guard();
        state.commit_in_flight = false;
        state.release_requested = false;
    }

    /// Keeps `hold`. If a release was requested during the attempt, the guards are released
    /// at once instead.
    pub fn store(&self, hold: AdmissionHold<Lease, UpdateReservation>) {
        let mut state = self.guard();
        if state.release_requested {
            state.release_requested = false;
            drop(state);
            hold.release();
            return;
        }
        state.hold = Some(hold);
    }

    /// Releases the held guards (and any partial lease). With nothing held during a commit
    /// attempt, the release is remembered for [`UpdateAdmission::store`].
    pub fn release(&self) {
        let (taken, partial) = {
            let mut state = self.guard();
            let taken = state.hold.take();
            if taken.is_none() && state.commit_in_flight {
                state.release_requested = true;
            }
            (taken, state.partial_lease.take())
        };
        drop(partial);
        if let Some(hold) = taken {
            hold.release();
        }
    }

    /// True while guards are held. Only the tests observe this; production gates on the
    /// restore and release paths.
    #[cfg(test)]
    pub fn is_held(&self) -> bool {
        self.guard().hold.is_some()
    }

    /// Takes the admission for a restore. A lease from an earlier refused attempt is kept and
    /// `take_lease` is not called again; if `take_reservation` is refused, the lease stays.
    /// Nothing happens when the admission is already held.
    pub fn hold_with(
        &self,
        take_lease: impl FnOnce() -> Result<Lease, ()>,
        take_reservation: impl FnOnce() -> Result<UpdateReservation, ()>,
    ) -> Result<(), ()> {
        let mut state = self.guard();
        if state.hold.is_some() {
            return Ok(());
        }
        if state.partial_lease.is_none() {
            state.partial_lease = Some(take_lease()?);
        }
        let reservation = take_reservation()?;
        let lease = state.partial_lease.take().ok_or(())?;
        state.hold = Some(AdmissionHold::new(lease, reservation));
        Ok(())
    }

    fn guard(&self) -> MutexGuard<'_, AdmissionState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// What [`app_commit`] reports. The guards are kept in [`UpdateAdmission`] when
/// an install is in flight, never returned here.
#[derive(Debug)]
pub enum CommitStatus {
    /// The install is registered; the guards are held.
    Committed(PendingRun),
    /// Nothing was committed (guards released).
    Deferred(DeferReason),
    /// Revocation was inconclusive; the guards and the `committing` record are kept.
    Unresolved(PendingRun),
}

/// [`CommitHost`] backed by the running Tauri application.
pub struct TauriHost<'a> {
    app: &'a AppHandle,
}

impl<'a> TauriHost<'a> {
    /// Host for `app`.
    pub fn new(app: &'a AppHandle) -> Self {
        Self { app }
    }
}

impl CommitHost for TauriHost<'_> {
    fn checks_enabled(&self) -> bool {
        update_checks_effectively_enabled(&get_settings(self.app))
    }

    fn auto_enabled(&self) -> bool {
        get_settings(self.app).auto_update_enabled
    }

    fn gate(&self) -> &Arc<OwnershipGate> {
        ownership::global()
    }

    fn jobs(&self) -> Arc<JobManager> {
        self.app.state::<JobWiring>().manager.clone()
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    fn process_start(&self) -> Instant {
        note_process_start()
    }

    fn stop_build_poller(&self) -> Result<(), PollerStopError> {
        match self.app.try_state::<SourceUpdateRuntime>() {
            Some(runtime) => runtime.pollers.stop_and_join(),
            None => Err(PollerStopError),
        }
    }

    fn start_poller(&self, run_id: &str, config: PollerConfig) {
        let handle: PollerHandle = poller::spawn(self.app.clone(), config);
        if let Some(runtime) = self.app.try_state::<SourceUpdateRuntime>() {
            runtime.pollers.replace(run_id, handle);
        }
    }
}

/// Runs the commit sequence on the real application (see the module deadlock rule).
/// On `Committed` or `Unresolved` the guards are kept in the runtime's
/// [`UpdateAdmission`]; call [`release_admission`] when the install ends.
pub fn app_commit(app: &AppHandle, manual: bool) -> CommitStatus {
    let Some(runtime) = app.try_state::<SourceUpdateRuntime>() else {
        return CommitStatus::Deferred(DeferReason::Fs);
    };
    let Ok(app_data) = crate::portable::app_data_dir(app) else {
        return CommitStatus::Deferred(DeferReason::Fs);
    };
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return CommitStatus::Deferred(DeferReason::Fs);
    };
    let state_dir = update_state_dir(&app_data);
    let site = RunSite {
        app_data,
        source_dir: checkout_dir(),
        home,
        tmpdir: std::env::temp_dir(),
        lock_dir: std::path::PathBuf::from(crate::managers::source_update::update_lock_path()),
        proxy_env: crate::managers::source_update_runner::proxy_env_from(&|name| {
            std::env::var(name).ok()
        }),
    };
    let host = TauriHost::new(app);
    let control = crate::managers::source_update_runner::LaunchctlJobControl::current();
    let env = AppCommitEnv::new(
        &host,
        &control,
        site,
        PendingStore::new(&state_dir),
        REVOKE_PAUSE,
    );
    // Begin before the commit: the install poller may settle before the guards are stored.
    runtime.admission.begin();
    let status = match commit(&env, update_coordinator(), manual) {
        CommitOutcome::Committed {
            lease,
            reservation,
            pending,
        } => {
            runtime
                .admission
                .store(AdmissionHold::new(lease, reservation));
            CommitStatus::Committed(pending)
        }
        CommitOutcome::Unresolved {
            lease,
            reservation,
            pending,
        } => {
            runtime
                .admission
                .store(AdmissionHold::new(lease, reservation));
            CommitStatus::Unresolved(pending)
        }
        CommitOutcome::Deferred(reason) => CommitStatus::Deferred(reason),
    };
    runtime.admission.finish();
    status
}

/// Releases the install admission guards held by [`app_commit`], if any.
pub fn release_admission(app: &AppHandle) {
    if let Some(runtime) = app.try_state::<SourceUpdateRuntime>() {
        runtime.admission.release();
    }
}

#[cfg(test)]
#[path = "source_update_commit_env_tests.rs"]
mod tests;
