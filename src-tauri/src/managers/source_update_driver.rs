//! Driver of the automatic update (DESIGN R6-5, R6-6, R7-2, R7-3, R7-6, R9-1..R9-4).
//!
//! One std thread wakes every 15 s (every 5 s while an install is outstanding) and at once
//! when [`kick`] is called. Each wake runs [`tick`] over a [`DriverEnv`]: a resting build is
//! committed, cleaned up or left alone, and otherwise the start decision runs. Everything that
//! blocks (launchd, git, the verified-commit request, the commit itself) runs on this thread,
//! never on the async runtime.
//!
//! Startup recovery is [`restore_admission`], called before any listener starts. The same
//! planner, [`restore_with`], runs again from the driver while an install is outstanding and
//! its poller is not running.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use log::{info, warn};
use tauri::{AppHandle, Manager};

use crate::job_runtime::JobWiring;
use crate::managers::ownership::{self, idle_for, Owner};
use crate::managers::source_update::{
    active_run_id, cached, checkout_dir, current_mode, install_blocker, pinned_target,
    start_auto_build, update_lock_path, SourceUpdateMode, SourceUpdateState,
};
use crate::managers::source_update_auto::consent::{
    cleanup_after_build_without_consent, take_manual_commit_request, AutoMarker, CleanupPlan,
};
use crate::managers::source_update_auto::status::emit_auto_update_changed;
use crate::managers::source_update_auto::{
    auto_update_decision, load_auto_state, save_auto_state, should_retry, update_coordinator,
    AutoDecision, AutoFacts, AutoResult, AutoState, SkipReason, VerifiedState,
};
use crate::managers::source_update_commit::{
    revoke_install, AdmissionHold, CommitEnv, DeferReason, RevokeResult,
};
use crate::managers::source_update_commit_env::{
    app_commit, release_admission, AppCommitEnv, CommitStatus, TauriHost, REVOKE_PAUSE,
};
use crate::managers::source_update_commit_plan::{
    plan_recovery, PendingInput, RecoveryAction, RecoveryInput, StatusInput,
};
use crate::managers::source_update_pending::{PendingRun, PendingState, PendingStore};
use crate::managers::source_update_runner::{
    now_unix, parse_status, proxy_env_from, read_status_bounded, status_path, update_state_dir,
    JobControl, JobProbe, LaunchctlJobControl, RunPhase, UpdateProgress, INSTALL_PERCENT_FLOOR,
};
use crate::managers::source_update_runner_run::{cleanup_run, RunSite};
use crate::managers::source_update_verify::{check_commit, GithubCommitVerifier};
use crate::settings::{auto_update_effective, get_settings, update_checks_effectively_enabled};

/// Wake interval while nothing is outstanding (DESIGN R6-5 cadence).
pub const TICK_INTERVAL: Duration = Duration::from_secs(15);
/// Wake interval while an install is outstanding (R9-3 retry).
pub const RETRY_INTERVAL: Duration = Duration::from_secs(5);

static WAKE_FLAG: Mutex<bool> = Mutex::new(false);
static WAKE_SIGNAL: Condvar = Condvar::new();
static STARTED: AtomicBool = AtomicBool::new(false);
static REVOKING: AtomicBool = AtomicBool::new(false);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Wakes the driver now (a check ended Available, or a poller ended).
pub fn kick() {
    *lock(&WAKE_FLAG) = true;
    WAKE_SIGNAL.notify_all();
}

fn wait_for_wake(timeout: Duration) {
    let guard = lock(&WAKE_FLAG);
    let (mut woken, _) = WAKE_SIGNAL
        .wait_timeout_while(guard, timeout, |flag| !*flag)
        .unwrap_or_else(PoisonError::into_inner);
    *woken = false;
}

/// What the driver does next: wake after the short or the normal interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Next {
    /// True while an install is outstanding and recovery must be retried soon.
    pub fast: bool,
}

const fn slow() -> Next {
    Next { fast: false }
}

const fn fast() -> Next {
    Next { fast: true }
}

/// What the driver remembers between ticks, so logs and records change only on change.
#[derive(Debug, Default)]
pub struct DriverMemory {
    last_record: Option<(String, &'static str)>,
    last_defer: Option<DeferReason>,
}

/// Facts gathered for one start decision. Everything except the verified-commit check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvFacts {
    /// The build is a source build.
    pub source_mode: bool,
    /// Update checks are effectively enabled.
    pub checks_effective: bool,
    /// The owner switched automatic updates on.
    pub auto_enabled: bool,
    /// The published check says an update is available.
    pub state_available: bool,
    /// No install blocker is present.
    pub install_blocker_none: bool,
    /// An update run is active in this process.
    pub run_active: bool,
    /// Full pinned target from the last check, when it is a valid offer.
    pub target: Option<String>,
    /// Persisted attempt state; `Err` when it could not be read (fail closed).
    pub attempt: Result<Option<AutoState>, ()>,
    /// The admission gate is free right now.
    pub gate_free: bool,
    /// No agent job runs.
    pub jobs_idle: bool,
    /// Time since the last activity of any kind.
    pub idle_for: Duration,
    /// Unix seconds now.
    pub now: u64,
}

/// Everything [`tick`] reads and does. The Tauri implementation is [`AppDriverEnv`].
pub trait DriverEnv {
    /// Automatic updates are effective (source build, checks on, switch on).
    fn auto_effective(&self) -> bool;
    /// Update checks are effectively enabled.
    fn checks_effective(&self) -> bool;
    /// The owner's automatic-update switch is on.
    fn auto_enabled(&self) -> bool;
    /// Loads `pending.json`; `Err` when it is unreadable (nothing is done then).
    fn pending(&self) -> Result<Option<PendingRun>, ()>;
    /// Gathers the start facts.
    fn gather(&self) -> EnvFacts;
    /// Runs the verified-commit check for `full_sha`. Only called when everything else says Start.
    fn verify(&self, full_sha: &str) -> VerifiedState;
    /// Starts the automatic build of `target`.
    fn launch_build(&self, target: &str) -> Result<(), ()>;
    /// Persists the attempt state and announces the change.
    fn save_state(&self, state: &AutoState);
    /// An update run is active in this process.
    fn run_active(&self) -> bool;
    /// Consumes a manual-commit request.
    fn take_manual_request(&self) -> bool;
    /// True when the `run-<id>.auto` marker of `run_id` exists.
    fn marker_exists(&self, run_id: &str) -> bool;
    /// Removes a built record that has lost its consent (R9-1).
    fn cleanup_built(&self);
    /// Runs the commit sequence (R7-2).
    fn commit(&self, manual: bool) -> CommitStatus;
    /// True while the install poller of `committing` is running.
    fn install_running(&self, committing: &PendingRun) -> bool;
    /// Re-runs the restore planner for an outstanding install.
    fn recover(&self) -> RestoreOutcome;
}

/// Pure: the facts the start decision reads, with the verified-commit result given.
pub fn auto_facts(facts: &EnvFacts, verified: VerifiedState) -> AutoFacts {
    let backoff_clear = match (&facts.attempt, facts.target.as_deref()) {
        (Err(()), _) => false,
        (Ok(state), Some(target)) => state
            .as_ref()
            .is_none_or(|state| should_retry(state, target, facts.now)),
        (Ok(_), None) => true,
    };
    AutoFacts {
        source_mode: facts.source_mode,
        checks_effective: facts.checks_effective,
        auto_enabled: facts.auto_enabled,
        state_available: facts.state_available && facts.target.is_some(),
        install_blocker_none: facts.install_blocker_none,
        run_active: facts.run_active,
        backoff_clear,
        verified,
        gate_free: facts.gate_free,
        jobs_idle: facts.jobs_idle,
        idle_for: facts.idle_for,
    }
}

/// Pure: the attempt state after a Wait or Skip for `target`, or `None` when nothing is recorded.
/// `start` and `no_update` are never recorded. A completed update for the same target is kept.
pub fn skip_record(previous: &AutoState, target: &str, code: &str, now: u64) -> Option<AutoState> {
    if matches!(code, "start" | "no_update") {
        return None;
    }
    let kept_updated = previous.last_result == Some(AutoResult::Updated)
        && previous.last_target.as_deref() == Some(target);
    (!kept_updated).then(|| previous.with_skipped(target, code, now))
}

/// Pure: the manual start decision (R7-7 c). Attaching wins; a built record is committed now;
/// otherwise a fresh single-phase run starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManualStart {
    /// Attach to the run this process already watches.
    Attach,
    /// Commit the built automatic update now.
    CommitBuilt,
    /// Launch a fresh run.
    Launch,
}

/// Decides a manual start from the attachable run and the built record.
pub fn manual_start_plan(attachable: bool, built_pending: bool) -> ManualStart {
    if attachable {
        ManualStart::Attach
    } else if built_pending {
        ManualStart::CommitBuilt
    } else {
        ManualStart::Launch
    }
}

/// One wake of the driver.
pub fn tick<E: DriverEnv>(env: &E, memory: &mut DriverMemory) -> Next {
    let pending = match env.pending() {
        Ok(pending) => pending,
        Err(()) => return slow(),
    };
    match pending {
        Some(run) => match run.state {
            PendingState::Committing(_) => {
                if !env.install_running(&run) {
                    env.recover();
                }
                fast()
            }
            PendingState::Built => tick_built(env, memory, &run),
        },
        None => tick_idle(env, memory),
    }
}

/// A resting built record: commit it, clean it up, or wait for the next wake.
fn tick_built<E: DriverEnv>(env: &E, memory: &mut DriverMemory, run: &PendingRun) -> Next {
    if env.run_active() {
        return slow();
    }
    if !env.checks_effective() {
        env.cleanup_built();
        return slow();
    }
    let manual = env.take_manual_request() || !env.marker_exists(&run.run_id);
    if !manual && !env.auto_enabled() {
        env.cleanup_built();
        return slow();
    }
    match env.commit(manual) {
        CommitStatus::Committed(_) => {
            memory.last_defer = None;
            slow()
        }
        CommitStatus::Deferred(reason) => {
            if memory.last_defer != Some(reason) {
                memory.last_defer = Some(reason);
                info!("automatic update commit deferred");
            }
            slow()
        }
        CommitStatus::Unresolved(committing) => {
            // Retry soon unless the install poller already runs (bootstrap completed later).
            if env.install_running(&committing) {
                slow()
            } else {
                fast()
            }
        }
    }
}

/// No pending record: the start decision for a new automatic build.
fn tick_idle<E: DriverEnv>(env: &E, memory: &mut DriverMemory) -> Next {
    if !env.auto_effective() {
        return slow();
    }
    let facts = env.gather();
    let provisional = auto_update_decision(&auto_facts(&facts, VerifiedState::Verified));
    let decision = match (provisional, facts.target.as_deref()) {
        (AutoDecision::Start, Some(target)) => {
            auto_update_decision(&auto_facts(&facts, env.verify(target)))
        }
        (AutoDecision::Start, None) => AutoDecision::Skip(SkipReason::NoUpdate),
        (other, _) => other,
    };
    match (decision, facts.target.as_deref()) {
        (AutoDecision::Start, Some(target)) => {
            if env.launch_build(target).is_err() {
                record(env, memory, &facts, target, "spawn_failed");
            }
        }
        (other, Some(target)) => record(env, memory, &facts, target, other.code()),
        (_, None) => {}
    }
    slow()
}

/// Records a Wait or Skip reason once per change (debounce).
fn record<E: DriverEnv>(
    env: &E,
    memory: &mut DriverMemory,
    facts: &EnvFacts,
    target: &str,
    code: &'static str,
) {
    let Ok(previous) = &facts.attempt else {
        return;
    };
    if memory
        .last_record
        .as_ref()
        .is_some_and(|(seen, seen_code)| seen == target && *seen_code == code)
    {
        return;
    }
    memory.last_record = Some((target.to_string(), code));
    let previous = previous.clone().unwrap_or_default();
    let next = if code == "spawn_failed" {
        Some(previous.with_failure(target, code, facts.now))
    } else {
        skip_record(&previous, target, code, facts.now)
    };
    if let Some(next) = next {
        env.save_state(&next);
    }
}

/// What startup recovery did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreOutcome {
    /// Nothing to hold or retry.
    Nothing,
    /// Admission is held and an install is being adopted or revoked.
    Held,
    /// Admission could not be taken now; the driver retries.
    Retry,
}

/// Effects of the startup restore (R7-3, R9-3). Real: [`AppRestore`]; tests: fakes.
pub trait RestoreEnv {
    /// Three-valued probe of an install label.
    fn probe(&self, label: &str) -> JobProbe;
    /// Phase of the install run's status, when the status file holds that run.
    fn install_phase(&self, install_run_id: &str) -> Option<RunPhase>;
    /// Takes the admission (gate lease and job reservation); a no-op when already held.
    fn hold(&self) -> Result<(), ()>;
    /// Starts the install poller for a committing record that is still live.
    fn adopt(&self, committing: &PendingRun);
    /// Revokes a committing record in the background, retrying while inconclusive.
    fn revoke_in_background(&self, committing: &PendingRun);
    /// Settles a finished install (`done` or failed) and releases admission.
    fn finish(&self, committing: &PendingRun, done: bool);
    /// Removes an unreadable pending record and records it.
    fn discard_invalid(&self);
    /// Best-effort removal of orphaned staged directories.
    fn sweep(&self, keep_run_id: Option<&str>);
}

fn status_input(phase: Option<RunPhase>) -> StatusInput {
    match phase {
        None => StatusInput::None,
        Some(phase) if phase.is_terminal() => StatusInput::Terminal,
        Some(_) => StatusInput::NonTerminal,
    }
}

/// Startup recovery table (R7-3, R8-2, R9-3). Reads the record, probes, plans, and acts.
/// The admission is taken before any revocation starts.
pub fn restore_with<E: RestoreEnv>(env: &E, store: &PendingStore) -> RestoreOutcome {
    let (input_pending, committing, keep) = match store.load() {
        Ok(None) => (PendingInput::Absent, None, None),
        Err(_) => (PendingInput::Invalid, None, None),
        Ok(Some(run)) => {
            let keep = Some(run.run_id.clone());
            match run.state {
                PendingState::Built => (PendingInput::Built, None, keep),
                PendingState::Committing(_) => (PendingInput::Committing, Some(run), keep),
            }
        }
    };
    let (probe, phase) = match committing.as_ref().map(|run| &run.state) {
        Some(PendingState::Committing(info)) => (
            env.probe(&info.install_label),
            env.install_phase(&info.install_run_id),
        ),
        _ => (JobProbe::Absent, None),
    };
    let input = RecoveryInput {
        pending: input_pending,
        probe,
        install_status: status_input(phase),
    };
    let outcome = match (plan_recovery(&input), committing.as_ref()) {
        (RecoveryAction::AdoptInstall { .. }, Some(run)) => match env.hold() {
            Ok(()) => {
                env.adopt(run);
                RestoreOutcome::Held
            }
            Err(()) => RestoreOutcome::Retry,
        },
        (RecoveryAction::RevokeInstall, Some(run)) => match env.hold() {
            Ok(()) => {
                env.revoke_in_background(run);
                RestoreOutcome::Held
            }
            Err(()) => RestoreOutcome::Retry,
        },
        (RecoveryAction::FinishInstall, Some(run)) => {
            env.finish(run, phase == Some(RunPhase::Done));
            RestoreOutcome::Nothing
        }
        (RecoveryAction::DiscardInvalid, _) => {
            env.discard_invalid();
            RestoreOutcome::Nothing
        }
        _ => RestoreOutcome::Nothing,
    };
    env.sweep(keep.as_deref());
    outcome
}

/// Startup restore for the running app. Called from `initialize_core_logic` before any
/// listener starts (see the comment there).
pub fn restore_admission(app: &AppHandle) {
    let Some(state_dir) = state_dir_of(app) else {
        warn!("automatic update restore skipped: no state directory");
        return;
    };
    let outcome = restore_with(
        &AppRestore::new(app, state_dir.clone()),
        &PendingStore::new(&state_dir),
    );
    if outcome != RestoreOutcome::Nothing {
        info!("automatic update admission restored");
    }
}

/// True when `pending.json` holds an install that is outstanding. Adoption of a build run
/// leaves such a record to the restore step.
pub fn committing_on_disk(app: &AppHandle) -> bool {
    let Some(state_dir) = state_dir_of(app) else {
        return false;
    };
    matches!(
        PendingStore::new(&state_dir).load(),
        Ok(Some(PendingRun {
            state: PendingState::Committing(_),
            ..
        }))
    )
}

/// Runs the commit sequence on the app and announces the install it started (R7-7 a).
pub fn commit_now(app: &AppHandle, manual: bool) -> CommitStatus {
    let status = app_commit(app, manual);
    if let CommitStatus::Committed(run) = &status {
        publish_install_started(app, run);
    }
    status
}

/// Marks a started install as silent and publishes its first progress (phase install, 91%).
fn publish_install_started(app: &AppHandle, run: &PendingRun) {
    let PendingState::Committing(info) = &run.state else {
        return;
    };
    if let Some(state_dir) = state_dir_of(app) {
        if let Err(error) = AutoMarker::new(&state_dir).write(&info.install_run_id) {
            warn!("could not mark the automatic install as silent: {error}");
        }
    }
    crate::managers::source_update::cache_progress(
        app,
        UpdateProgress {
            run_id: info.install_run_id.clone(),
            phase: RunPhase::Installing,
            percent: INSTALL_PERCENT_FLOOR,
            target: crate::managers::source_update::short_sha(&run.target_full),
            message: "Installing the update.".to_string(),
            estimated: false,
            silent: true,
        },
    );
}

/// Called by the poller when a run reaches its outcome (done or failed).
pub fn on_terminal(app: &AppHandle, run_id: &str, progress: &UpdateProgress) {
    let Some(state_dir) = state_dir_of(app) else {
        return;
    };
    let done = progress.phase == RunPhase::Done;
    let pending = PendingStore::new(&state_dir).load().ok().flatten();
    if let Some(run) = pending.as_ref() {
        if install_run_of(run) == Some(run_id) {
            finish_install(app, &state_dir, run, done);
            return;
        }
    }
    let markers = AutoMarker::new(&state_dir);
    if !done && markers.exists(run_id) {
        record_build_failed(app, &state_dir);
        if let Err(error) = markers.remove(run_id) {
            warn!("could not remove the automatic marker: {error}");
        }
    }
}

fn record_build_failed(app: &AppHandle, state_dir: &Path) {
    let Some(target) = pinned_target(cached(app).as_ref()) else {
        warn!("automatic update build failed without a known target");
        return;
    };
    let previous = load_auto_state(state_dir)
        .ok()
        .flatten()
        .unwrap_or_default();
    let next = previous.with_failure(&target, "build_failed", now_unix());
    save_or_warn(state_dir, &next);
    emit_auto_update_changed(app);
}

/// Settles a terminal install: records the outcome, removes the record and the marker, and
/// releases admission. Idempotent.
fn finish_install(app: &AppHandle, state_dir: &Path, run: &PendingRun, done: bool) {
    let Some(install_id) = install_run_of(run) else {
        return;
    };
    let previous = load_auto_state(state_dir)
        .ok()
        .flatten()
        .unwrap_or_default();
    let now = now_unix();
    let next = if done {
        previous.with_updated(&run.target_full, now)
    } else {
        previous.with_failure(&run.target_full, "install_failed", now)
    };
    save_or_warn(state_dir, &next);
    if let Err(error) = AutoMarker::new(state_dir).remove(install_id) {
        warn!("could not remove the install marker: {error}");
    }
    if let Err(error) = PendingStore::new(state_dir).remove_all() {
        warn!("could not remove the finished update: {error}");
    }
    release_admission(app);
    emit_auto_update_changed(app);
    info!("automatic update install finished");
}

/// Settles a revoked install: failed with `interrupted`, record removed, admission released.
fn revoked_install(app: &AppHandle, state_dir: &Path, run: &PendingRun) {
    let previous = load_auto_state(state_dir)
        .ok()
        .flatten()
        .unwrap_or_default();
    let next = previous.with_failure(&run.target_full, "interrupted", now_unix());
    save_or_warn(state_dir, &next);
    if let Err(error) = PendingStore::new(state_dir).remove_all() {
        warn!("could not remove the revoked update: {error}");
    }
    release_admission(app);
    emit_auto_update_changed(app);
    info!("interrupted automatic install was revoked");
}

/// Removes an unreadable pending record and records `invalid_pending` (no target is known).
fn discard_invalid_record(app: &AppHandle, state_dir: &Path) {
    if let Err(error) = PendingStore::new(state_dir).remove_all() {
        warn!("could not remove an unreadable pending update: {error}");
    }
    let previous = load_auto_state(state_dir)
        .ok()
        .flatten()
        .unwrap_or_default();
    let next = AutoState {
        last_result: Some(AutoResult::Failed),
        reason_code: Some("invalid_pending".to_string()),
        ts: now_unix(),
        ..previous
    };
    save_or_warn(state_dir, &next);
    emit_auto_update_changed(app);
    warn!("removed an unreadable pending automatic update");
}

fn save_or_warn(state_dir: &Path, state: &AutoState) {
    if let Err(error) = save_auto_state(state_dir, state) {
        warn!("could not record the automatic update state: {error}");
    }
}

fn install_run_of(run: &PendingRun) -> Option<&str> {
    match &run.state {
        PendingState::Committing(info) => Some(info.install_run_id.as_str()),
        PendingState::Built => None,
    }
}

fn state_dir_of(app: &AppHandle) -> Option<PathBuf> {
    crate::portable::app_data_dir(app)
        .ok()
        .map(|dir| update_state_dir(&dir))
}

/// The paths and environment a real install job needs (mirrors `app_commit`).
fn run_site(app: &AppHandle) -> Option<RunSite> {
    let app_data = crate::portable::app_data_dir(app).ok()?;
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(RunSite {
        app_data,
        source_dir: checkout_dir(),
        home,
        tmpdir: std::env::temp_dir(),
        lock_dir: PathBuf::from(update_lock_path()),
        proxy_env: proxy_env_from(&|name| std::env::var(name).ok()),
    })
}

/// Real [`RestoreEnv`] over the running app.
struct AppRestore<'a> {
    app: &'a AppHandle,
    state_dir: PathBuf,
}

impl<'a> AppRestore<'a> {
    fn new(app: &'a AppHandle, state_dir: PathBuf) -> Self {
        Self { app, state_dir }
    }
}

impl RestoreEnv for AppRestore<'_> {
    fn probe(&self, label: &str) -> JobProbe {
        LaunchctlJobControl::current().probe(label)
    }

    fn install_phase(&self, install_run_id: &str) -> Option<RunPhase> {
        let bytes = read_status_bounded(&status_path(&self.state_dir))
            .ok()
            .flatten()?;
        parse_status(&bytes, install_run_id, None, now_unix())
            .ok()
            .map(|status| status.phase)
    }

    fn hold(&self) -> Result<(), ()> {
        let _serial = update_coordinator().guard();
        let runtime = self
            .app
            .try_state::<crate::managers::source_update::SourceUpdateRuntime>()
            .ok_or(())?;
        if runtime.admission.is_held() {
            return Ok(());
        }
        let lease = ownership::global()
            .try_acquire(Owner::Updater)
            .map_err(|_| ())?;
        let jobs = self.app.try_state::<JobWiring>().ok_or(())?;
        let reservation = jobs.manager.try_reserve_for_update().map_err(|_| ())?;
        runtime
            .admission
            .store(AdmissionHold::new(lease, reservation));
        Ok(())
    }

    fn adopt(&self, committing: &PendingRun) {
        let PendingState::Committing(info) = &committing.state else {
            return;
        };
        let Some(runtime) = self
            .app
            .try_state::<crate::managers::source_update::SourceUpdateRuntime>()
        else {
            return;
        };
        if runtime.pollers.is_running(&info.install_run_id) {
            return;
        }
        let Some(site) = run_site(self.app) else {
            warn!("could not adopt the outstanding install: paths unavailable");
            return;
        };
        let host = TauriHost::new(self.app);
        let control = LaunchctlJobControl::current();
        let env = AppCommitEnv::new(
            &host,
            &control,
            site,
            PendingStore::new(&self.state_dir),
            REVOKE_PAUSE,
        );
        env.start_install_poller(committing);
        info!("adopted an outstanding automatic install");
    }

    fn revoke_in_background(&self, committing: &PendingRun) {
        if REVOKING.swap(true, Ordering::SeqCst) {
            return;
        }
        let app = self.app.clone();
        let state_dir = self.state_dir.clone();
        let committing = committing.clone();
        let spawned = thread::Builder::new()
            .name("source-update-revoke".into())
            .spawn(move || {
                revoke_until_settled(&app, &state_dir, &committing);
                REVOKING.store(false, Ordering::SeqCst);
            });
        if spawned.is_err() {
            REVOKING.store(false, Ordering::SeqCst);
            warn!("could not start the revocation of an outstanding install");
        }
    }

    fn finish(&self, committing: &PendingRun, done: bool) {
        if let PendingState::Committing(info) = &committing.state {
            let control = LaunchctlJobControl::current();
            if let Err(error) = cleanup_run(&control, &self.state_dir, &info.install_run_id) {
                warn!("update cleanup did not finish: {error}");
            }
        }
        finish_install(self.app, &self.state_dir, committing, done);
    }

    fn discard_invalid(&self) {
        discard_invalid_record(self.app, &self.state_dir);
    }

    fn sweep(&self, keep_run_id: Option<&str>) {
        let status_run = read_status_bounded(&status_path(&self.state_dir))
            .ok()
            .flatten()
            .and_then(|bytes| crate::managers::source_update_runner::run_id_in_status(&bytes));
        let removed =
            PendingStore::new(&self.state_dir).sweep_orphans(keep_run_id.or(status_run.as_deref()));
        if removed > 0 {
            info!("removed orphaned automatic update staging");
        }
    }
}

/// Revokes until conclusive (R9-3). Inconclusive results retry every [`RETRY_INTERVAL`].
fn revoke_until_settled(app: &AppHandle, state_dir: &Path, committing: &PendingRun) {
    loop {
        let Some(site) = run_site(app) else {
            thread::sleep(RETRY_INTERVAL);
            continue;
        };
        let host = TauriHost::new(app);
        let control = LaunchctlJobControl::current();
        let env = AppCommitEnv::new(
            &host,
            &control,
            site,
            PendingStore::new(state_dir),
            REVOKE_PAUSE,
        );
        match revoke_install(&env, committing) {
            RevokeResult::Revoked => {
                revoked_install(app, state_dir, committing);
                return;
            }
            RevokeResult::Adopted => {
                env.start_install_poller(committing);
                info!("outstanding automatic install is live; adopted");
                return;
            }
            RevokeResult::Inconclusive => thread::sleep(RETRY_INTERVAL),
        }
    }
}

/// The Tauri-backed [`DriverEnv`] for one tick.
struct AppDriverEnv<'a> {
    app: &'a AppHandle,
    state_dir: PathBuf,
}

impl DriverEnv for AppDriverEnv<'_> {
    fn auto_effective(&self) -> bool {
        auto_update_effective(&get_settings(self.app))
    }

    fn checks_effective(&self) -> bool {
        update_checks_effectively_enabled(&get_settings(self.app))
    }

    fn auto_enabled(&self) -> bool {
        get_settings(self.app).auto_update_enabled
    }

    fn pending(&self) -> Result<Option<PendingRun>, ()> {
        PendingStore::new(&self.state_dir).load().map_err(|_| ())
    }

    fn gather(&self) -> EnvFacts {
        let settings = get_settings(self.app);
        let checks_effective = update_checks_effectively_enabled(&settings);
        let info = cached(self.app);
        let target = pinned_target(info.as_ref());
        let state_available = info
            .as_ref()
            .is_some_and(|info| info.state == SourceUpdateState::Available);
        let install_blocker_none = install_blocker(
            &crate::managers::source_update::gather_install_facts(!checks_effective),
        )
        .is_none();
        let gate_idle = ownership::global().idle_since();
        let now = Instant::now();
        let process_start = crate::managers::source_update_commit_env::note_process_start();
        let jobs = self.app.try_state::<JobWiring>();
        let last_activity = jobs.as_ref().map(|jobs| jobs.manager.last_activity());
        let jobs_idle = jobs
            .as_ref()
            .is_some_and(|jobs| !jobs.manager.has_active_job());
        let quiet_since = gate_idle.map(|gate| {
            gate.max(last_activity.unwrap_or(process_start))
                .max(process_start)
        });
        EnvFacts {
            source_mode: current_mode() == SourceUpdateMode::Source,
            checks_effective,
            auto_enabled: settings.auto_update_enabled,
            state_available,
            install_blocker_none,
            run_active: active_run_id(self.app).is_some(),
            target,
            attempt: load_auto_state(&self.state_dir).map_err(|_| ()),
            gate_free: gate_idle.is_some(),
            jobs_idle,
            idle_for: quiet_since.map_or(Duration::ZERO, |since| idle_for(now, since)),
            now: now_unix(),
        }
    }

    fn verify(&self, full_sha: &str) -> VerifiedState {
        check_commit(&GithubCommitVerifier::default(), full_sha)
    }

    fn launch_build(&self, target: &str) -> Result<(), ()> {
        start_auto_build(self.app, target).map_err(|_| ())
    }

    fn save_state(&self, state: &AutoState) {
        save_or_warn(&self.state_dir, state);
        emit_auto_update_changed(self.app);
    }

    fn run_active(&self) -> bool {
        active_run_id(self.app).is_some()
    }

    fn take_manual_request(&self) -> bool {
        take_manual_commit_request()
    }

    fn marker_exists(&self, run_id: &str) -> bool {
        AutoMarker::new(&self.state_dir).exists(run_id)
    }

    fn cleanup_built(&self) {
        match cleanup_after_build_without_consent(&self.state_dir, false, now_unix()) {
            Ok(CleanupPlan::RemoveBuilt) => {
                info!("removed a built automatic update after consent was withdrawn");
                emit_auto_update_changed(self.app);
            }
            Ok(CleanupPlan::Keep) => {}
            Err(error) => warn!("could not clean up after withdrawn consent: {error}"),
        }
    }

    fn commit(&self, manual: bool) -> CommitStatus {
        commit_now(self.app, manual)
    }

    fn install_running(&self, committing: &PendingRun) -> bool {
        let PendingState::Committing(info) = &committing.state else {
            return false;
        };
        self.app
            .try_state::<crate::managers::source_update::SourceUpdateRuntime>()
            .is_some_and(|runtime| runtime.pollers.is_running(&info.install_run_id))
    }

    fn recover(&self) -> RestoreOutcome {
        restore_with(
            &AppRestore::new(self.app, self.state_dir.clone()),
            &PendingStore::new(&self.state_dir),
        )
    }
}

/// Starts the driver thread once. Each wake runs one [`tick`]; a missing state directory
/// skips the wake.
pub fn start(app: AppHandle) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = thread::Builder::new()
        .name("source-update-driver".into())
        .spawn(move || {
            let mut memory = DriverMemory::default();
            loop {
                let next = match state_dir_of(&app) {
                    Some(state_dir) => {
                        let env = AppDriverEnv {
                            app: &app,
                            state_dir,
                        };
                        tick(&env, &mut memory)
                    }
                    None => slow(),
                };
                wait_for_wake(if next.fast {
                    RETRY_INTERVAL
                } else {
                    TICK_INTERVAL
                });
            }
        });
    if spawned.is_err() {
        STARTED.store(false, Ordering::SeqCst);
        warn!("could not start the automatic update driver");
    }
}

#[cfg(test)]
#[path = "source_update_driver_tests.rs"]
mod tests;
