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

use std::fs;
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
    cleanup_after_build_without_consent, install_is_silent, manual_commit_requested,
    sweep_stale_markers, take_manual_commit_request, AutoMarker, CleanupPlan,
};
use crate::managers::source_update_auto::status::emit_auto_update_changed;
use crate::managers::source_update_auto::{
    auto_update_decision, load_auto_state, save_auto_state, should_retry, update_coordinator,
    AutoDecision, AutoFacts, AutoResult, AutoState, SkipReason, VerifiedState, WaitReason,
};
use crate::managers::source_update_commit::{revoke_install, CommitEnv, DeferReason, RevokeResult};
use crate::managers::source_update_commit_env::{
    app_commit, release_admission, AppCommitEnv, CommitStatus, TauriHost, REVOKE_PAUSE,
};
use crate::managers::source_update_commit_plan::{
    plan_recovery, PendingInput, RecoveryAction, RecoveryInput, StatusInput,
};
use crate::managers::source_update_pending::{PendingRun, PendingState, PendingStore};
use crate::managers::source_update_runner::RunnerError;
use crate::managers::source_update_runner::{
    is_lower_hex, now_unix, parse_status, proxy_env_from, read_status_bounded, run_id_in_status,
    status_path, update_state_dir, JobControl, JobProbe, LaunchctlJobControl, RunPhase,
    UpdateProgress, INSTALL_PERCENT_FLOOR,
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
    last_wait: Option<&'static str>,
}

/// Why the launcher refused to start a build (see [`DriverEnv::launch_build`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchRefusal {
    /// The launcher is busy: an update run, a pending record, the lock or a registered run
    /// exists. A wait, not a failed attempt (issue #160).
    Busy,
    /// Any other launch error: a failed attempt.
    Failed,
}

/// Pure: the refusal of a launcher error. Only the runner's Busy refusal is a wait (issue #160).
pub fn launch_refusal_of(error: &RunnerError) -> LaunchRefusal {
    match error {
        RunnerError::Busy(_) => LaunchRefusal::Busy,
        _ => LaunchRefusal::Failed,
    }
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
    fn launch_build(&self, target: &str) -> Result<(), LaunchRefusal>;
    /// Persists the attempt state and announces the change.
    fn save_state(&self, state: &AutoState);
    /// An update run is active in this process.
    fn run_active(&self) -> bool;
    /// True while a manual-commit request names exactly `run_id` (B6 major 2). Does not consume it:
    /// the commit consumes it once performed.
    fn manual_request_for(&self, run_id: &str) -> bool;
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
/// Only terminal refusals are persisted (B6 major 3): a wait is never a result, because a
/// healthy run would then look failed. `start`, `no_update`, `backoff` and the wait reasons are
/// never recorded. A completed update for the same target is kept.
pub fn skip_record(previous: &AutoState, target: &str, code: &str, now: u64) -> Option<AutoState> {
    if !is_terminal_skip(code) {
        return None;
    }
    let kept_updated = previous.last_result == Some(AutoResult::Updated)
        && previous.last_target.as_deref() == Some(target);
    (!kept_updated).then(|| previous.with_skipped(target, code, now))
}

/// Skip codes that are persisted as the last result: refusals that stay refused until the owner
/// or the published state changes. Everything else is either a wait or a transient refusal.
/// `blocked` is not terminal (B7 major): an install blocker may be our own update lock, so it is
/// never persisted as a result.
fn is_terminal_skip(code: &str) -> bool {
    matches!(
        code,
        "disabled" | "not_source" | "unverified" | "verify_failed"
    )
}

/// Pure: the attempt state after a launch of `target` (B6 major 3). A previous failure or skip
/// for the same target is cleared to a neutral started state, so the notice stays hidden during
/// the build, the wait and the install. The failure stamp (cooldown) is kept. `None` when the
/// previous result is not a failure or skip for `target`.
pub fn started_record(previous: &AutoState, target: &str, now: u64) -> Option<AutoState> {
    let failed_or_skipped = previous.last_target.as_deref() == Some(target)
        && matches!(
            previous.last_result,
            Some(AutoResult::Failed | AutoResult::Skipped)
        );
    failed_or_skipped.then(|| AutoState {
        last_target: Some(target.to_string()),
        last_result: None,
        reason_code: None,
        ts: now,
        ..previous.clone()
    })
}

/// The outstanding automatic record on disk, as far as a manual start cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutstandingKind {
    /// No record (or an unreadable one: the launch itself refuses while a record exists).
    None,
    /// A built update waiting for a commit.
    Built,
    /// An install is outstanding; nothing new may start.
    Committing,
}

/// Pure: the outstanding kind of a loaded record.
pub fn outstanding_kind(pending: Option<&PendingRun>) -> OutstandingKind {
    match pending.map(|run| &run.state) {
        None => OutstandingKind::None,
        Some(PendingState::Built) => OutstandingKind::Built,
        Some(PendingState::Committing(_)) => OutstandingKind::Committing,
    }
}

/// Pure: the manual start decision (R7-7 c). Attaching wins; a built record is committed now;
/// an outstanding install is reported as in progress and never started again; otherwise a
/// fresh single-phase run starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManualStart {
    /// Attach to the run this process already watches.
    Attach,
    /// Commit the built automatic update now (a manual commit, no new build).
    CommitBuilt,
    /// An install is already outstanding: report it as in progress, start nothing.
    InProgress,
    /// Launch a fresh run.
    Launch,
}

/// Decides a manual start from the attachable run and the outstanding record.
pub fn manual_start_plan(attachable: bool, outstanding: OutstandingKind) -> ManualStart {
    match (attachable, outstanding) {
        (true, _) => ManualStart::Attach,
        (false, OutstandingKind::Built) => ManualStart::CommitBuilt,
        (false, OutstandingKind::Committing) => ManualStart::InProgress,
        (false, OutstandingKind::None) => ManualStart::Launch,
    }
}

/// Pure: the answer of a manual "Update now" to a commit of a built record. `Ok` when the
/// commit happened, or when the driver committed the same record a moment earlier (the
/// record is then committing on disk and the start is attached). Anything else is a failure.
pub fn commit_built_outcome(status: &CommitStatus, committing_on_disk: bool) -> Result<(), ()> {
    match status {
        CommitStatus::Committed(_) => Ok(()),
        CommitStatus::Deferred(DeferReason::NoBuiltRun) if committing_on_disk => Ok(()),
        CommitStatus::Deferred(_) | CommitStatus::Unresolved(_) => Err(()),
    }
}

/// What the poller's terminal report of a run does (see [`on_terminal`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalAction {
    /// `pending.json` names this run as its install: settle from the record.
    FinishInstall,
    /// An automatic install ended after its record was already removed (the script removes
    /// `pending.json` before `done`): settle with the target from the status file or memory.
    FinishRecorded,
    /// An automatic build failed: record the build failure and drop its marker.
    BuildFailed,
    /// Nothing to do.
    Ignore,
}

/// Pure: decides the effect of a terminal report. A non-terminal phase never settles anything.
///
/// `remembered` is the install run id this process committed (automatic only). An automatic
/// install is recognised by that memory, or after a restart by its marker: a `done` install, or
/// a failure at or above the install band (`percent >= INSTALL_PERCENT_FLOOR`; a build never
/// reaches that band before `waiting`). Anything else with a marker is a failed build.
pub fn terminal_action(
    pending: Option<&PendingRun>,
    run_id: &str,
    phase: RunPhase,
    percent: u8,
    marker_exists: bool,
    remembered: Option<&str>,
) -> TerminalAction {
    if !phase.is_terminal() {
        return TerminalAction::Ignore;
    }
    if pending.and_then(install_run_of) == Some(run_id) {
        return TerminalAction::FinishInstall;
    }
    let remembered_install = remembered == Some(run_id);
    let marked_install =
        marker_exists && (phase == RunPhase::Done || percent >= INSTALL_PERCENT_FLOOR);
    if remembered_install || marked_install {
        return TerminalAction::FinishRecorded;
    }
    if phase != RunPhase::Done && marker_exists {
        TerminalAction::BuildFailed
    } else {
        TerminalAction::Ignore
    }
}

/// Effects of settling a terminal install. Real: [`AppSettle`]; tests: fakes.
pub trait SettleEnv {
    /// The persisted attempt state before this settle (default when none).
    fn previous(&self) -> AutoState;
    /// Persists the attempt state and announces nothing.
    fn save(&self, state: &AutoState);
    /// Removes the install marker of `run_id`.
    fn remove_marker(&self, run_id: &str);
    /// Removes `pending.json` and the staged directory.
    fn remove_pending(&self);
    /// Releases the install admission (gate lease and job reservation).
    fn release(&self);
    /// Tells the footer and the tray that the state changed.
    fn announce(&self);
}

/// Pure: the attempt state after an install ends. A finished install records `updated`. A failed
/// install records `failed` with `install_failed` for its target. This holds for a manual install
/// too: a failed manual install also puts its target in the 24 h automatic cooldown, so the
/// automatic path does not retry that same commit at once.
pub fn settled_state(previous: &AutoState, run: &PendingRun, done: bool, now: u64) -> AutoState {
    settled_for_target(previous, &run.target_full, done, now)
}

/// [`settled_state`] for a target given directly.
fn settled_for_target(previous: &AutoState, target: &str, done: bool, now: u64) -> AutoState {
    if done {
        previous.with_updated(target, now)
    } else {
        previous.with_failure(target, "install_failed", now)
    }
}

/// Settles an automatic install whose record is no longer on disk (native finding: the script
/// removes `pending.json` before `done`). Records the attempt when the target is known, removes
/// the install marker, releases admission and announces. It never touches `pending.json`, which
/// may belong to another run. Idempotent.
pub fn settle_recorded<E: SettleEnv>(
    env: &E,
    run_id: &str,
    target: Option<&str>,
    done: bool,
    now: u64,
) {
    if let Some(target) = target {
        let next = settled_for_target(&env.previous(), target, done, now);
        env.save(&next);
    }
    env.remove_marker(run_id);
    env.release();
    env.announce();
}

/// Settles a terminal install (state, marker, record, admission, announcement), in that order.
/// Idempotent: each step is a no-op when already done.
pub fn settle_install<E: SettleEnv>(
    env: &E,
    run: &PendingRun,
    install_run_id: &str,
    done: bool,
    now: u64,
) {
    let next = settled_state(&env.previous(), run, done, now);
    env.save(&next);
    env.remove_marker(install_run_id);
    env.remove_pending();
    env.release();
    env.announce();
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
    let manual = env.manual_request_for(&run.run_id) || !env.marker_exists(&run.run_id);
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
    // B7 major: while a run is active (our own build holds the update lock, so the blocker is
    // reported too) nothing is decided and nothing is recorded.
    if env.run_active() {
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
        (AutoDecision::Start, Some(target)) => match env.launch_build(target) {
            Ok(()) => {
                memory.last_wait = None;
                mark_started(env, &facts, target);
            }
            // Issue #160: a Busy refusal is a wait. Nothing is persisted and there is no backoff;
            // the next tick retries. The wait is logged once per change.
            Err(LaunchRefusal::Busy) => {
                if memory.last_wait != Some(WaitReason::Busy.code()) {
                    memory.last_wait = Some(WaitReason::Busy.code());
                    info!("automatic update waits: the launcher is busy");
                }
            }
            Err(LaunchRefusal::Failed) => {
                memory.last_wait = None;
                record(env, memory, &facts, target, "spawn_failed");
            }
        },
        (other, Some(target)) => record(env, memory, &facts, target, other.code()),
        (_, None) => {}
    }
    slow()
}

/// Clears an earlier failure or skip for `target` once its build has started (B6 major 3).
fn mark_started<E: DriverEnv>(env: &E, facts: &EnvFacts, target: &str) {
    let Ok(previous) = &facts.attempt else {
        return;
    };
    let previous = previous.clone().unwrap_or_default();
    if let Some(next) = started_record(&previous, target, facts.now) {
        env.save_state(&next);
    }
}

/// Records a terminal Skip or a failure once per change (debounce). Wait reasons are only
/// remembered in memory for the debounce; they are never persisted (B6 major 3).
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

/// The outstanding automatic record on disk, for a manual start. An unreadable record counts as
/// none; the launch itself refuses while any record exists.
pub fn outstanding_kind_on_disk(app: &AppHandle) -> OutstandingKind {
    let Some(state_dir) = state_dir_of(app) else {
        return OutstandingKind::None;
    };
    outstanding_kind(PendingStore::new(&state_dir).load().ok().flatten().as_ref())
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

/// Runs the commit sequence on the app and announces the install it started (R7-7 a). A manual
/// commit consumes the manual request of the built run it performed (B6 major 2).
pub fn commit_now(app: &AppHandle, manual: bool) -> CommitStatus {
    let built_id = built_run_id_on_disk(app);
    let status = app_commit(app, manual);
    if let Some(run_id) = built_id.as_deref().filter(|_| manual) {
        take_manual_commit_request(run_id);
    }
    if let CommitStatus::Committed(run) = &status {
        let silent = install_is_silent(run.origin, manual);
        if let Some(install) = install_memory(run) {
            remember_install(install);
        }
        if let Some(state_dir) = state_dir_of(app) {
            if let Some(progress) = install_started_progress(&state_dir, run, silent) {
                crate::managers::source_update::cache_progress(app, progress);
            }
        }
    }
    status
}

/// The run id of the built record on disk, if there is one.
fn built_run_id_on_disk(app: &AppHandle) -> Option<String> {
    let state_dir = state_dir_of(app)?;
    match PendingStore::new(&state_dir).load() {
        Ok(Some(run)) if run.state == PendingState::Built => Some(run.run_id),
        _ => None,
    }
}

/// Progress of a started install (phase install, 91%). A silent install also gets its marker,
/// written here so the first frame is silent; a manual install gets neither (B6 minor 4).
/// `None` for a record that is not committing.
pub fn install_started_progress(
    state_dir: &Path,
    run: &PendingRun,
    silent: bool,
) -> Option<UpdateProgress> {
    let PendingState::Committing(info) = &run.state else {
        return None;
    };
    if silent {
        if let Err(error) = AutoMarker::new(state_dir).write(&info.install_run_id) {
            warn!("could not mark the automatic install as silent: {error}");
        }
    }
    Some(UpdateProgress {
        run_id: info.install_run_id.clone(),
        phase: RunPhase::Installing,
        percent: INSTALL_PERCENT_FLOOR,
        target: crate::managers::source_update::short_sha(&run.target_full),
        message: "Installing the update.".to_string(),
        estimated: false,
        silent,
    })
}

/// The inputs of one terminal report, read from the record, the marker, the memory and the
/// status file before anything is settled.
#[derive(Debug)]
struct TerminalReport<'a> {
    run_id: &'a str,
    pending: Option<&'a PendingRun>,
    phase: RunPhase,
    percent: u8,
    marker_exists: bool,
    remembered: Option<&'a RememberedInstall>,
    status_target: Option<&'a str>,
    now: u64,
}

/// Decides and settles one terminal report through `env` (the effects of [`settle_install`],
/// [`settle_recorded`] or nothing). A build failure is left to the caller, which needs the app.
fn settle_terminal<E: SettleEnv>(env: &E, report: &TerminalReport<'_>) -> TerminalAction {
    let remembered_id = report.remembered.map(|install| install.run_id.as_str());
    let action = terminal_action(
        report.pending,
        report.run_id,
        report.phase,
        report.percent,
        report.marker_exists,
        remembered_id,
    );
    let done = report.phase == RunPhase::Done;
    match action {
        TerminalAction::FinishInstall => {
            if let Some(run) = report.pending {
                settle_install(env, run, report.run_id, done, report.now);
            }
        }
        TerminalAction::FinishRecorded => {
            let target = report.status_target.map(str::to_string).or_else(|| {
                report
                    .remembered
                    .filter(|install| install.run_id == report.run_id)
                    .map(|install| install.target.clone())
            });
            settle_recorded(env, report.run_id, target.as_deref(), done, report.now);
        }
        TerminalAction::BuildFailed | TerminalAction::Ignore => {}
    }
    action
}

/// Called by the poller when a run reaches its outcome (done or failed). The decision is
/// [`terminal_action`]; the effects are [`settle_terminal`] or the build-failure record.
pub fn on_terminal(app: &AppHandle, run_id: &str, progress: &UpdateProgress) {
    let Some(state_dir) = state_dir_of(app) else {
        return;
    };
    let pending = PendingStore::new(&state_dir).load().ok().flatten();
    let marker_exists = AutoMarker::new(&state_dir).exists(run_id);
    let remembered = remembered_install();
    let status = status_target(&state_dir, run_id);
    let report = TerminalReport {
        run_id,
        pending: pending.as_ref(),
        phase: progress.phase,
        percent: progress.percent,
        marker_exists,
        remembered: remembered.as_ref(),
        status_target: status.as_deref(),
        now: now_unix(),
    };
    let env = AppSettle {
        app,
        state_dir: &state_dir,
    };
    match settle_terminal(&env, &report) {
        TerminalAction::FinishInstall | TerminalAction::FinishRecorded => {
            forget_install(run_id);
            info!("automatic update install finished");
        }
        TerminalAction::BuildFailed => {
            record_build_failed(app, &state_dir);
            if let Err(error) = AutoMarker::new(&state_dir).remove(run_id) {
                warn!("could not remove the automatic marker: {error}");
            }
        }
        TerminalAction::Ignore => {}
    }
    sweep_markers(app);
}

/// An install this process committed (manual or automatic): its run id and full target. Kept only
/// in memory so that its terminal report can be settled after `pending.json` is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RememberedInstall {
    run_id: String,
    target: String,
}

static REMEMBERED: Mutex<Option<RememberedInstall>> = Mutex::new(None);

/// The install a committed record starts, to be remembered (F2 major 2). Every committed install
/// is remembered, manual ones included: their terminal report must settle admission too.
fn install_memory(run: &PendingRun) -> Option<RememberedInstall> {
    match &run.state {
        PendingState::Committing(info) => Some(RememberedInstall {
            run_id: info.install_run_id.clone(),
            target: run.target_full.clone(),
        }),
        PendingState::Built => None,
    }
}

fn remember_install(install: RememberedInstall) {
    *lock(&REMEMBERED) = Some(install);
}

fn remembered_install() -> Option<RememberedInstall> {
    lock(&REMEMBERED).clone()
}

fn forget_install(run_id: &str) {
    let mut remembered = lock(&REMEMBERED);
    if remembered
        .as_ref()
        .is_some_and(|install| install.run_id == run_id)
    {
        *remembered = None;
    }
}

/// The full target a run's status file names, when it names a well-formed one.
fn status_target(state_dir: &Path, run_id: &str) -> Option<String> {
    let bytes = read_status_bounded(&status_path(state_dir))
        .ok()
        .flatten()?;
    let status = parse_status(&bytes, run_id, None, now_unix()).ok()?;
    (!status.target_full.is_empty() && is_lower_hex(&status.target_full, 40))
        .then_some(status.target_full)
}

/// The run id of the status file, whatever its phase (F2 major 1: a terminal status keeps its
/// install marker until the settle removes it). A status whose run id is readable counts as live
/// even when the rest does not parse (fail closed: its marker is kept).
fn live_status_run(state_dir: &Path) -> Option<String> {
    let bytes = read_status_bounded(&status_path(state_dir))
        .ok()
        .flatten()?;
    run_id_in_status(&bytes)
}

/// Removes the automatic markers no run needs (best effort). Live: the pending record's build and
/// install runs, the active run and a non-terminal status run. Skipped when `pending.json` cannot
/// be read, because its ids are then unknown.
pub fn sweep_markers(app: &AppHandle) {
    let Some(state_dir) = state_dir_of(app) else {
        return;
    };
    let Ok(pending) = PendingStore::new(&state_dir).load() else {
        return;
    };
    let live = live_marker_ids(&state_dir, pending.as_ref(), active_run_id(app));
    let refs: Vec<&str> = live.iter().map(String::as_str).collect();
    if sweep_stale_markers(&state_dir, &refs) > 0 {
        info!("removed stale automatic update markers");
    }
}

/// The run ids whose markers must stay: the pending record's build and install runs, the active
/// run, the status file's run (terminal or not, until its settle removes the marker) and every
/// run that still has a `run-<id>.plist` (a build marker written before its status file exists).
pub fn live_marker_ids(
    state_dir: &Path,
    pending: Option<&PendingRun>,
    active: Option<String>,
) -> Vec<String> {
    let mut live: Vec<String> = Vec::new();
    if let Some(run) = pending {
        live.push(run.run_id.clone());
        live.extend(install_run_of(run).map(str::to_string));
    }
    live.extend(active);
    live.extend(live_status_run(state_dir));
    live.extend(plist_run_ids(state_dir));
    live
}

/// The run ids that have a `run-<id>.plist` in the state directory.
fn plist_run_ids(state_dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(state_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let id = name
                .to_str()?
                .strip_prefix("run-")?
                .strip_suffix(".plist")?;
            is_lower_hex(id, 16).then(|| id.to_string())
        })
        .collect()
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

/// The Tauri-backed [`SettleEnv`] of one install.
struct AppSettle<'a> {
    app: &'a AppHandle,
    state_dir: &'a Path,
}

impl SettleEnv for AppSettle<'_> {
    fn previous(&self) -> AutoState {
        load_auto_state(self.state_dir)
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    fn save(&self, state: &AutoState) {
        save_or_warn(self.state_dir, state);
    }

    fn remove_marker(&self, run_id: &str) {
        if let Err(error) = AutoMarker::new(self.state_dir).remove(run_id) {
            warn!("could not remove the install marker: {error}");
        }
    }

    fn remove_pending(&self) {
        let store = PendingStore::new(self.state_dir);
        if let Ok(Some(run)) = store.load() {
            if let Err(error) = AutoMarker::new(self.state_dir).remove(&run.run_id) {
                warn!("could not remove the build marker: {error}");
            }
        }
        if let Err(error) = store.remove_all() {
            warn!("could not remove the finished update: {error}");
        }
    }

    fn release(&self) {
        release_admission(self.app);
    }

    fn announce(&self) {
        emit_auto_update_changed(self.app);
    }
}

/// Settles a terminal install through [`settle_install`]. Idempotent.
fn finish_install(app: &AppHandle, state_dir: &Path, run: &PendingRun, done: bool) {
    let Some(install_id) = install_run_of(run) else {
        return;
    };
    settle_install(
        &AppSettle { app, state_dir },
        run,
        install_id,
        done,
        now_unix(),
    );
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
    sweep_markers(app);
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
        let jobs = self.app.try_state::<JobWiring>().ok_or(())?;
        // A lease from a refused attempt is kept; only the reservation is retried (B6 minor 6).
        runtime.admission.hold_with(
            || {
                ownership::global()
                    .try_acquire(Owner::Updater)
                    .map_err(|_| ())
            },
            || jobs.manager.try_reserve_for_update().map_err(|_| ()),
        )
    }

    fn adopt(&self, committing: &PendingRun) {
        let PendingState::Committing(info) = &committing.state else {
            return;
        };
        if let Some(install) = install_memory(committing) {
            remember_install(install);
        }
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
        sweep_markers(self.app);
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
                if let Some(install) = install_memory(committing) {
                    remember_install(install);
                }
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

    fn launch_build(&self, target: &str) -> Result<(), LaunchRefusal> {
        start_auto_build(self.app, target)
    }

    fn save_state(&self, state: &AutoState) {
        save_or_warn(&self.state_dir, state);
        emit_auto_update_changed(self.app);
    }

    fn run_active(&self) -> bool {
        active_run_id(self.app).is_some()
    }

    fn manual_request_for(&self, run_id: &str) -> bool {
        manual_commit_requested(run_id)
    }

    fn marker_exists(&self, run_id: &str) -> bool {
        AutoMarker::new(&self.state_dir).exists(run_id)
    }

    fn cleanup_built(&self) {
        match cleanup_after_build_without_consent(&self.state_dir, false, now_unix()) {
            Ok(CleanupPlan::RemoveBuilt) => {
                info!("removed a built automatic update after consent was withdrawn");
                sweep_markers(self.app);
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
