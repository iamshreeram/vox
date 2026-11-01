//! Tests for the automatic-update driver: the start decision table, the tick over fakes,
//! the startup restore table over real pending files, the manual start plan and the
//! source-order guard for the startup placement in `lib.rs`.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::time::Duration;

use super::*;
use crate::managers::source_update_auto::consent::{
    apply_manual_conversion, cleanup_after_build_without_consent, silent_for_run,
    take_manual_commit_request, AutoMarker, CleanupPlan,
};
use crate::managers::source_update_auto::consent::{
    auto_pending, manual_commit_requested, request_manual_commit, AutoPendingFacts,
};
use crate::managers::source_update_auto::load_auto_state;
use crate::managers::source_update_pending::{
    encode_pending, staged_app_path, CommitInfo, PendingOrigin,
};
use crate::managers::source_update_runner::{
    job_label, plist_path, write_private_atomic, JobProbe, RunPhase,
};

const RUN: &str = "0123456789abcdef";
const INSTALL: &str = "fedcba9876543210";
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const OTHER_SHA: &str = "89abcdef0123456789abcdef0123456789abcdef";
const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const NOW: u64 = 1_700_000_000;

/// One row of the no-start table: a name, a change to the facts and the code recorded, if any.
type Case = (
    &'static str,
    Box<dyn Fn(&mut EnvFacts)>,
    Option<&'static str>,
);

fn facts() -> EnvFacts {
    EnvFacts {
        source_mode: true,
        checks_effective: true,
        auto_enabled: true,
        state_available: true,
        install_blocker_none: true,
        run_active: false,
        target: Some(SHA.to_string()),
        attempt: Ok(None),
        gate_free: true,
        jobs_idle: true,
        idle_for: Duration::from_secs(120),
        now: NOW,
    }
}

fn built_run(state: &Path) -> PendingRun {
    PendingRun {
        run_id: RUN.to_string(),
        target_full: SHA.to_string(),
        staged_app: staged_app_path(state, RUN),
        exe_name: "vox-bin".to_string(),
        exe_sha256: DIGEST.to_string(),
        info_plist_sha256: DIGEST.to_string(),
        origin: PendingOrigin::Auto,
        ts: NOW - 100,
        state: PendingState::Built,
    }
}

fn committing_run(state: &Path) -> PendingRun {
    PendingRun {
        state: PendingState::Committing(CommitInfo {
            install_run_id: INSTALL.to_string(),
            install_label: job_label(INSTALL),
            install_plist: plist_path(state, INSTALL),
            committing_ts: NOW - 10,
        }),
        ..built_run(state)
    }
}

fn write_record(state: &Path, run: &PendingRun) {
    let bytes = encode_pending(run, state).expect("encode");
    write_private_atomic(&PendingStore::new(state).path(), &bytes).expect("write");
}

/// Fake of every effect the tick reads or performs.
struct FakeEnv {
    facts: RefCell<EnvFacts>,
    pending: Option<PendingRun>,
    verify_result: VerifiedState,
    verify_calls: Cell<u32>,
    launches: RefCell<Vec<String>>,
    launch_fails: bool,
    saves: RefCell<Vec<AutoState>>,
    marker: bool,
    manual_request_run: Option<&'static str>,
    commit_script: RefCell<VecDeque<CommitStatus>>,
    commits: RefCell<Vec<bool>>,
    cleanups: Cell<u32>,
    install_running: bool,
    recovers: Cell<u32>,
}

impl FakeEnv {
    fn new(facts: EnvFacts) -> Self {
        Self {
            facts: RefCell::new(facts),
            pending: None,
            verify_result: VerifiedState::Verified,
            verify_calls: Cell::new(0),
            launches: RefCell::new(Vec::new()),
            launch_fails: false,
            saves: RefCell::new(Vec::new()),
            marker: true,
            manual_request_run: None,
            commit_script: RefCell::new(VecDeque::new()),
            commits: RefCell::new(Vec::new()),
            cleanups: Cell::new(0),
            install_running: false,
            recovers: Cell::new(0),
        }
    }
}

impl DriverEnv for FakeEnv {
    fn auto_effective(&self) -> bool {
        let f = self.facts.borrow();
        f.auto_enabled && f.checks_effective && f.source_mode
    }
    fn checks_effective(&self) -> bool {
        self.facts.borrow().checks_effective
    }
    fn auto_enabled(&self) -> bool {
        self.facts.borrow().auto_enabled
    }
    fn pending(&self) -> Result<Option<PendingRun>, ()> {
        Ok(self.pending.clone())
    }
    fn gather(&self) -> EnvFacts {
        self.facts.borrow().clone()
    }
    fn verify(&self, _full_sha: &str) -> VerifiedState {
        self.verify_calls.set(self.verify_calls.get() + 1);
        self.verify_result
    }
    fn launch_build(&self, target: &str) -> Result<(), ()> {
        self.launches.borrow_mut().push(target.to_string());
        if self.launch_fails {
            Err(())
        } else {
            self.facts.borrow_mut().run_active = true;
            Ok(())
        }
    }
    fn save_state(&self, state: &AutoState) {
        self.saves.borrow_mut().push(state.clone());
    }
    fn run_active(&self) -> bool {
        self.facts.borrow().run_active
    }
    fn manual_request_for(&self, run_id: &str) -> bool {
        self.manual_request_run == Some(run_id)
    }
    fn marker_exists(&self, _run_id: &str) -> bool {
        self.marker
    }
    fn cleanup_built(&self) {
        self.cleanups.set(self.cleanups.get() + 1);
    }
    fn commit(&self, manual: bool) -> CommitStatus {
        self.commits.borrow_mut().push(manual);
        self.commit_script
            .borrow_mut()
            .pop_front()
            .unwrap_or(CommitStatus::Deferred(DeferReason::NoBuiltRun))
    }
    fn install_running(&self, _committing: &PendingRun) -> bool {
        self.install_running
    }
    fn recover(&self) -> RestoreOutcome {
        self.recovers.set(self.recovers.get() + 1);
        RestoreOutcome::Nothing
    }
}

fn tick_once(env: &FakeEnv) -> Next {
    tick(env, &mut DriverMemory::default())
}

#[test]
fn start_decision_starts_for_the_pinned_target_after_verification_and_idle() {
    let env = FakeEnv::new(facts());
    tick_once(&env);
    assert_eq!(*env.launches.borrow(), vec![SHA.to_string()]);
    assert_eq!(env.verify_calls.get(), 1);
    assert!(env.saves.borrow().is_empty(), "nothing recorded on a start");
}

#[test]
fn no_start_combinations_never_verify_or_launch() {
    // (name, facts change, expected reason code recorded, verify expected)
    let cases: Vec<Case> = vec![
        ("auto off", Box::new(|f| f.auto_enabled = false), None),
        ("checks off", Box::new(|f| f.checks_effective = false), None),
        ("not source", Box::new(|f| f.source_mode = false), None),
        ("no update", Box::new(|f| f.state_available = false), None),
        (
            "blocked",
            Box::new(|f| f.install_blocker_none = false),
            Some("blocked"),
        ),
        ("busy gate", Box::new(|f| f.gate_free = false), Some("busy")),
        (
            "jobs busy",
            Box::new(|f| f.jobs_idle = false),
            Some("not_idle"),
        ),
        (
            "idle 30 s",
            Box::new(|f| f.idle_for = Duration::from_secs(30)),
            Some("not_idle"),
        ),
        (
            "idle 59 s",
            Box::new(|f| f.idle_for = Duration::from_secs(59)),
            Some("not_idle"),
        ),
        (
            "active run",
            Box::new(|f| f.run_active = true),
            Some("active_run"),
        ),
        (
            "backoff active",
            Box::new(|f| {
                f.attempt = Ok(Some(AutoState {
                    failed_target: Some(SHA.to_string()),
                    failed_ts: Some(NOW - 60),
                    ..AutoState::default()
                }))
            }),
            Some("backoff"),
        ),
        // Unreadable state: no start, and nothing can be recorded without the previous state.
        (
            "unreadable attempt state",
            Box::new(|f| f.attempt = Err(())),
            None,
        ),
    ];
    for (name, change, code) in cases {
        let mut f = facts();
        change(&mut f);
        let env = FakeEnv::new(f);
        tick_once(&env);
        assert!(env.launches.borrow().is_empty(), "{name}: must not launch");
        assert_eq!(env.verify_calls.get(), 0, "{name}: must not verify");
        match code {
            // B6 major 3: only terminal refusals are persisted. A wait or backoff is not a result.
            Some("blocked") => {
                let saved = env.saves.borrow();
                assert_eq!(saved.len(), 1, "{name}: one record");
                assert_eq!(saved[0].reason_code.as_deref(), Some("blocked"), "{name}");
                assert_eq!(saved[0].last_result, Some(AutoResult::Skipped), "{name}");
            }
            Some(wait) => assert!(
                env.saves.borrow().is_empty(),
                "{name}: the wait ({wait}) is not persisted"
            ),
            None => assert!(env.saves.borrow().is_empty(), "{name}: nothing recorded"),
        }
    }
}

#[test]
fn unverified_and_verify_error_never_start_and_record_the_reason() {
    let mut unverified = FakeEnv::new(facts());
    unverified.verify_result = VerifiedState::Unverified;
    tick_once(&unverified);
    assert!(unverified.launches.borrow().is_empty());
    assert_eq!(unverified.verify_calls.get(), 1);
    assert_eq!(
        unverified.saves.borrow()[0].reason_code.as_deref(),
        Some("unverified")
    );

    let mut failed = FakeEnv::new(facts());
    failed.verify_result = VerifiedState::Failed;
    tick_once(&failed);
    assert!(failed.launches.borrow().is_empty());
    assert_eq!(
        failed.saves.borrow()[0].reason_code.as_deref(),
        Some("verify_failed")
    );
}

#[test]
fn tick_never_launches_two_builds() {
    let env = FakeEnv::new(facts());
    let mut memory = DriverMemory::default();
    tick(&env, &mut memory);
    tick(&env, &mut memory);
    tick(&env, &mut memory);
    assert_eq!(env.launches.borrow().len(), 1);
}

#[test]
fn failed_launch_records_spawn_failure_and_does_not_retry_at_once() {
    let mut env = FakeEnv::new(facts());
    env.launch_fails = true;
    let mut memory = DriverMemory::default();
    tick(&env, &mut memory);
    let recorded = env.saves.borrow()[0].clone();
    assert_eq!(recorded.reason_code.as_deref(), Some("spawn_failed"));
    assert_eq!(recorded.last_result, Some(AutoResult::Failed));
    assert!(recorded.failed_target.is_some());
    // The persisted failure now drives the backoff: the same target is not launched again.
    env.facts.borrow_mut().attempt = Ok(Some(recorded));
    tick(&env, &mut memory);
    assert_eq!(env.launches.borrow().len(), 1);
}

#[test]
fn skip_reason_is_recorded_once_per_change() {
    // B6 major 3: a wait (gate busy) is never persisted; a terminal refusal is persisted once.
    let mut f = facts();
    f.gate_free = false;
    let mut env = FakeEnv::new(f.clone());
    let mut memory = DriverMemory::default();
    tick(&env, &mut memory);
    tick(&env, &mut memory);
    assert!(env.saves.borrow().is_empty(), "a wait is not a result");
    env.verify_result = VerifiedState::Unverified;
    env.facts.borrow_mut().gate_free = true;
    tick(&env, &mut memory);
    tick(&env, &mut memory);
    assert_eq!(env.saves.borrow().len(), 1, "the refusal is recorded once");
    assert_eq!(
        env.saves.borrow()[0].reason_code.as_deref(),
        Some("unverified")
    );
}

// B6 major 3: waits are never persisted, so a healthy run is not reported as failed.
#[test]
fn b6_wait_reasons_are_never_persisted() {
    for code in ["not_idle", "busy", "verifying", "active_run", "backoff"] {
        assert_eq!(
            skip_record(&AutoState::default(), SHA, code, NOW),
            None,
            "{code} must not be persisted"
        );
    }
    assert!(skip_record(&AutoState::default(), SHA, "unverified", NOW).is_some());
    assert!(skip_record(&AutoState::default(), SHA, "verify_failed", NOW).is_some());
}

// B6 major 3: Wait -> Start -> build, wait and install all keep the notice hidden.
#[test]
fn b6_start_after_a_wait_keeps_auto_pending_through_build_wait_and_install() {
    let dir = tempfile::tempdir().expect("dir");
    let mut f = facts();
    f.attempt = Ok(Some(AutoState::default().with_skipped(
        SHA,
        "unverified",
        NOW - 50,
    )));
    f.idle_for = Duration::from_secs(5);
    let env = FakeEnv::new(f);
    let mut memory = DriverMemory::default();
    tick(&env, &mut memory);
    assert!(env.saves.borrow().is_empty(), "the wait is not persisted");
    env.facts.borrow_mut().idle_for = Duration::from_secs(120);
    tick(&env, &mut memory);
    assert_eq!(env.launches.borrow().len(), 1, "the build starts");
    let started = env.saves.borrow().last().cloned().expect("started record");
    assert_eq!(started.last_result, None);
    assert_eq!(started.last_target.as_deref(), Some(SHA));
    let during_build = AutoPendingFacts {
        auto_effective: true,
        active_run_marked: true,
        pending: None,
        target: SHA,
        last: Some(&started),
    };
    assert!(auto_pending(&during_build), "during the build");
    let built = built_run(dir.path());
    let waiting = AutoPendingFacts {
        active_run_marked: false,
        pending: Some(&built),
        ..during_build
    };
    assert!(auto_pending(&waiting), "while the built record waits");
    let committing = committing_run(dir.path());
    let installing = AutoPendingFacts {
        active_run_marked: false,
        pending: Some(&committing),
        ..during_build
    };
    assert!(auto_pending(&installing), "during the install");
}

// B6 major 3: an unverified skip brings the notice back; a build failure keeps it back.
#[test]
fn b6_terminal_skip_and_build_failure_decide_auto_pending() {
    let mut env = FakeEnv::new(facts());
    env.verify_result = VerifiedState::Unverified;
    tick_once(&env);
    let skipped = env.saves.borrow().last().cloned().expect("skip recorded");
    let facts_skipped = AutoPendingFacts {
        auto_effective: true,
        active_run_marked: true,
        pending: None,
        target: SHA,
        last: Some(&skipped),
    };
    assert!(!auto_pending(&facts_skipped), "unverified: notice returns");
    let failed = AutoState::default().with_failure(SHA, "build_failed", NOW);
    let facts_failed = AutoPendingFacts {
        last: Some(&failed),
        ..facts_skipped
    };
    assert!(
        !auto_pending(&facts_failed),
        "build failure: notice returns"
    );
}

#[test]
fn b6_started_record_clears_only_a_failure_or_skip_for_the_same_target() {
    let skipped = AutoState::default().with_skipped(SHA, "unverified", NOW - 5);
    let started = started_record(&skipped, SHA, NOW).expect("cleared");
    assert_eq!(started.last_result, None);
    assert_eq!(started.reason_code, None);
    assert_eq!(started_record(&skipped, OTHER_SHA, NOW), None);
    let updated = AutoState::default().with_updated(SHA, NOW - 5);
    assert_eq!(started_record(&updated, SHA, NOW), None);
}

// B6 major 2: a request left for run A never commits a later automatic build B.
#[test]
fn b6_stale_manual_request_never_commits_a_later_auto_build() {
    const FIRST: &str = "aaaaaaaaaaaaaaaa";
    let dir = tempfile::tempdir().expect("dir");
    request_manual_commit(FIRST);
    let mut f = facts();
    f.auto_enabled = false;
    let mut env = FakeEnv::new(f);
    env.pending = Some(built_run(dir.path()));
    tick_once(&env);
    assert!(env.commits.borrow().is_empty(), "no manual commit of run B");
    assert_eq!(env.cleanups.get(), 1, "withdrawn consent removes run B");
    assert!(!manual_commit_requested(RUN), "no flag for run B");
    assert!(
        take_manual_commit_request(FIRST),
        "run A's flag is still its own"
    );
}

// B6 minor 4: a manual install writes no marker and is not silent; an automatic one does.
#[test]
fn b6_install_start_marks_only_a_silent_install() {
    let dir = tempfile::tempdir().expect("dir");
    let run = committing_run(dir.path());
    let manual = install_started_progress(dir.path(), &run, false).expect("progress");
    assert!(!manual.silent);
    assert!(
        !AutoMarker::new(dir.path()).exists(INSTALL),
        "no marker for manual"
    );
    let auto = install_started_progress(dir.path(), &run, true).expect("progress");
    assert!(auto.silent);
    assert_eq!(auto.phase, RunPhase::Installing);
    assert_eq!(auto.run_id, INSTALL);
    assert!(
        AutoMarker::new(dir.path()).exists(INSTALL),
        "marker for silent"
    );
}

#[test]
fn b6_install_started_progress_is_none_for_a_built_record() {
    let dir = tempfile::tempdir().expect("dir");
    assert!(install_started_progress(dir.path(), &built_run(dir.path()), true).is_none());
}

#[test]
fn skip_record_keeps_a_completed_update_and_ignores_start() {
    let updated = AutoState::default().with_updated(SHA, NOW - 5);
    assert_eq!(skip_record(&updated, SHA, "not_idle", NOW), None);
    assert_eq!(
        skip_record(&updated, OTHER_SHA, "unverified", NOW).is_some(),
        true
    );
    assert_eq!(skip_record(&updated, SHA, "unverified", NOW), None);
    assert_eq!(skip_record(&AutoState::default(), SHA, "start", NOW), None);
    assert_eq!(
        skip_record(&AutoState::default(), SHA, "no_update", NOW),
        None
    );
}

#[test]
fn pending_built_with_consent_is_committed_automatically() {
    let dir = tempfile::tempdir().expect("dir");
    let mut env = FakeEnv::new(facts());
    env.pending = Some(built_run(dir.path()));
    env.commit_script
        .borrow_mut()
        .push_back(CommitStatus::Committed(built_run(dir.path())));
    tick_once(&env);
    assert_eq!(*env.commits.borrow(), vec![false]);
    assert!(
        env.launches.borrow().is_empty(),
        "no new build while pending"
    );
}

#[test]
fn deferred_commit_is_retried_on_the_next_tick() {
    let dir = tempfile::tempdir().expect("dir");
    let mut env = FakeEnv::new(facts());
    env.pending = Some(built_run(dir.path()));
    env.commit_script
        .borrow_mut()
        .push_back(CommitStatus::Deferred(DeferReason::NotIdle));
    env.commit_script
        .borrow_mut()
        .push_back(CommitStatus::Committed(built_run(dir.path())));
    let mut memory = DriverMemory::default();
    tick(&env, &mut memory);
    tick(&env, &mut memory);
    assert_eq!(env.commits.borrow().len(), 2, "retried next tick");
}

#[test]
fn unresolved_commit_asks_for_a_fast_retry() {
    let dir = tempfile::tempdir().expect("dir");
    let mut env = FakeEnv::new(facts());
    env.pending = Some(built_run(dir.path()));
    env.commit_script
        .borrow_mut()
        .push_back(CommitStatus::Unresolved(committing_run(dir.path())));
    assert!(tick_once(&env).fast);
}

#[test]
fn opt_out_while_pending_built_removes_it_and_never_commits() {
    let dir = tempfile::tempdir().expect("dir");
    let mut f = facts();
    f.auto_enabled = false;
    let mut env = FakeEnv::new(f);
    env.pending = Some(built_run(dir.path()));
    tick_once(&env);
    assert_eq!(env.cleanups.get(), 1);
    assert!(env.commits.borrow().is_empty());
}

#[test]
fn checks_off_while_pending_built_removes_it() {
    let dir = tempfile::tempdir().expect("dir");
    let mut f = facts();
    f.checks_effective = false;
    let mut env = FakeEnv::new(f);
    env.pending = Some(built_run(dir.path()));
    tick_once(&env);
    assert_eq!(env.cleanups.get(), 1);
    assert!(env.commits.borrow().is_empty());
}

#[test]
fn consent_removal_on_real_files_deletes_the_built_record_and_records_disabled() {
    let dir = tempfile::tempdir().expect("dir");
    write_record(dir.path(), &built_run(dir.path()));
    let plan = cleanup_after_build_without_consent(dir.path(), false, NOW).expect("cleanup");
    assert_eq!(plan, CleanupPlan::RemoveBuilt);
    assert!(!PendingStore::new(dir.path()).path().exists());
    let state = load_auto_state(dir.path()).expect("state").expect("some");
    assert_eq!(state.reason_code.as_deref(), Some("disabled"));
}

#[test]
fn manual_conversion_without_marker_commits_manually() {
    let dir = tempfile::tempdir().expect("dir");
    let mut env = FakeEnv::new(facts());
    env.marker = false;
    env.pending = Some(built_run(dir.path()));
    tick_once(&env);
    assert_eq!(*env.commits.borrow(), vec![true]);
}

#[test]
fn manual_request_commits_manually_even_with_the_marker() {
    let dir = tempfile::tempdir().expect("dir");
    let mut env = FakeEnv::new(facts());
    env.manual_request_run = Some(RUN);
    env.pending = Some(built_run(dir.path()));
    tick_once(&env);
    assert_eq!(*env.commits.borrow(), vec![true]);
}

#[test]
fn an_active_build_blocks_the_commit() {
    let dir = tempfile::tempdir().expect("dir");
    let mut f = facts();
    f.run_active = true;
    let mut env = FakeEnv::new(f);
    env.pending = Some(built_run(dir.path()));
    tick_once(&env);
    assert!(env.commits.borrow().is_empty());
}

#[test]
fn committing_record_with_a_running_poller_is_left_alone() {
    let dir = tempfile::tempdir().expect("dir");
    let mut env = FakeEnv::new(facts());
    env.install_running = true;
    env.pending = Some(committing_run(dir.path()));
    tick_once(&env);
    assert_eq!(env.recovers.get(), 0);
    env.install_running = false;
    tick_once(&env);
    assert_eq!(
        env.recovers.get(),
        1,
        "recovery runs when no poller is live"
    );
}

#[test]
fn manual_start_plan_table() {
    use OutstandingKind as K;
    assert_eq!(manual_start_plan(true, K::None), ManualStart::Attach);
    assert_eq!(manual_start_plan(true, K::Built), ManualStart::Attach);
    assert_eq!(manual_start_plan(true, K::Committing), ManualStart::Attach);
    assert_eq!(manual_start_plan(false, K::Built), ManualStart::CommitBuilt);
    assert_eq!(
        manual_start_plan(false, K::Committing),
        ManualStart::InProgress
    );
    assert_eq!(manual_start_plan(false, K::None), ManualStart::Launch);
}

/// Fake of the startup restore effects; records the order of the calls.
struct FakeRestore {
    calls: RefCell<Vec<&'static str>>,
    probe: JobProbe,
    phase: Option<RunPhase>,
    hold_ok: bool,
}

impl FakeRestore {
    fn new(probe: JobProbe, phase: Option<RunPhase>) -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            probe,
            phase,
            hold_ok: true,
        }
    }
    fn saw(&self, name: &str) -> bool {
        self.calls.borrow().contains(&name)
    }
    fn position(&self, name: &str) -> Option<usize> {
        self.calls.borrow().iter().position(|call| *call == name)
    }
}

impl RestoreEnv for FakeRestore {
    fn probe(&self, _label: &str) -> JobProbe {
        self.calls.borrow_mut().push("probe");
        self.probe
    }
    fn install_phase(&self, _install_run_id: &str) -> Option<RunPhase> {
        self.calls.borrow_mut().push("phase");
        self.phase
    }
    fn hold(&self) -> Result<(), ()> {
        self.calls.borrow_mut().push("hold");
        if self.hold_ok {
            Ok(())
        } else {
            Err(())
        }
    }
    fn adopt(&self, _committing: &PendingRun) {
        self.calls.borrow_mut().push("adopt");
    }
    fn revoke_in_background(&self, _committing: &PendingRun) {
        self.calls.borrow_mut().push("revoke");
    }
    fn finish(&self, _committing: &PendingRun, done: bool) {
        self.calls
            .borrow_mut()
            .push(if done { "finish:done" } else { "finish:failed" });
    }
    fn discard_invalid(&self) {
        self.calls.borrow_mut().push("discard");
    }
    fn sweep(&self, _keep: Option<&str>) {
        self.calls.borrow_mut().push("sweep");
    }
}

fn live() -> JobProbe {
    JobProbe::Registered {
        pid: Some(4242),
        last_exit: None,
    }
}

fn registered_idle() -> JobProbe {
    JobProbe::Registered {
        pid: None,
        last_exit: None,
    }
}

#[test]
fn restore_none_does_nothing_but_sweep() {
    let dir = tempfile::tempdir().expect("dir");
    let fake = FakeRestore::new(JobProbe::Absent, None);
    let outcome = restore_with(&fake, &PendingStore::new(dir.path()));
    assert_eq!(outcome, RestoreOutcome::Nothing);
    assert_eq!(*fake.calls.borrow(), vec!["sweep"]);
}

#[test]
fn restore_built_rests_without_holding() {
    let dir = tempfile::tempdir().expect("dir");
    write_record(dir.path(), &built_run(dir.path()));
    let fake = FakeRestore::new(JobProbe::Absent, None);
    let outcome = restore_with(&fake, &PendingStore::new(dir.path()));
    assert_eq!(outcome, RestoreOutcome::Nothing);
    assert!(!fake.saw("hold"));
    assert!(PendingStore::new(dir.path()).path().exists(), "built kept");
}

#[test]
fn restore_invalid_record_is_discarded() {
    let dir = tempfile::tempdir().expect("dir");
    fs::write(PendingStore::new(dir.path()).path(), b"not json").expect("write");
    let fake = FakeRestore::new(JobProbe::Absent, None);
    restore_with(&fake, &PendingStore::new(dir.path()));
    assert!(fake.saw("discard"));
    assert!(!fake.saw("hold"));
}

#[test]
fn restore_committing_registered_without_status_adopts_under_hold() {
    let dir = tempfile::tempdir().expect("dir");
    write_record(dir.path(), &committing_run(dir.path()));
    let fake = FakeRestore::new(registered_idle(), None);
    let outcome = restore_with(&fake, &PendingStore::new(dir.path()));
    assert_eq!(outcome, RestoreOutcome::Held);
    assert!(fake.position("hold") < fake.position("adopt"));
    assert!(!fake.saw("revoke"));
    assert!(PendingStore::new(dir.path()).path().exists());
}

#[test]
fn restore_committing_unknown_holds_and_never_deletes() {
    let dir = tempfile::tempdir().expect("dir");
    write_record(dir.path(), &committing_run(dir.path()));
    let fake = FakeRestore::new(JobProbe::Unknown, None);
    let outcome = restore_with(&fake, &PendingStore::new(dir.path()));
    assert_eq!(outcome, RestoreOutcome::Held);
    assert!(fake.saw("hold"));
    assert!(!fake.saw("revoke"));
    assert!(!fake.saw("finish:failed") && !fake.saw("finish:done"));
    assert!(
        PendingStore::new(dir.path()).path().exists(),
        "unknown keeps the record"
    );
}

#[test]
fn restore_committing_absent_takes_hold_then_revokes() {
    let dir = tempfile::tempdir().expect("dir");
    write_record(dir.path(), &committing_run(dir.path()));
    let fake = FakeRestore::new(JobProbe::Absent, None);
    let outcome = restore_with(&fake, &PendingStore::new(dir.path()));
    assert_eq!(outcome, RestoreOutcome::Held);
    assert!(fake.position("hold").expect("hold") < fake.position("revoke").expect("revoke"));
    assert!(!fake.saw("adopt"));
}

#[test]
fn restore_retries_when_the_hold_cannot_be_taken() {
    let dir = tempfile::tempdir().expect("dir");
    write_record(dir.path(), &committing_run(dir.path()));
    let mut fake = FakeRestore::new(JobProbe::Absent, None);
    fake.hold_ok = false;
    let outcome = restore_with(&fake, &PendingStore::new(dir.path()));
    assert_eq!(outcome, RestoreOutcome::Retry);
    assert!(!fake.saw("revoke"), "no revocation without the hold");
}

#[test]
fn restore_committing_terminal_and_absent_finishes_with_the_outcome() {
    let dir = tempfile::tempdir().expect("dir");
    write_record(dir.path(), &committing_run(dir.path()));
    let done = FakeRestore::new(JobProbe::Absent, Some(RunPhase::Done));
    restore_with(&done, &PendingStore::new(dir.path()));
    assert!(done.saw("finish:done"));

    let failed = FakeRestore::new(JobProbe::Absent, Some(RunPhase::Failed));
    restore_with(&failed, &PendingStore::new(dir.path()));
    assert!(failed.saw("finish:failed"));
    assert!(!failed.saw("hold"));
}

#[test]
fn restore_terminal_with_live_process_adopts_instead_of_finishing() {
    let dir = tempfile::tempdir().expect("dir");
    write_record(dir.path(), &committing_run(dir.path()));
    let fake = FakeRestore::new(live(), Some(RunPhase::Done));
    restore_with(&fake, &PendingStore::new(dir.path()));
    assert!(fake.saw("adopt"));
    assert!(!fake.saw("finish:done"));
}

#[test]
fn restore_adopts_an_install_from_pending_when_status_is_missing() {
    let dir = tempfile::tempdir().expect("dir");
    write_record(dir.path(), &committing_run(dir.path()));
    // No status file for the install run: the record alone decides.
    let fake = FakeRestore::new(registered_idle(), None);
    assert_eq!(
        restore_with(&fake, &PendingStore::new(dir.path())),
        RestoreOutcome::Held
    );
}

#[test]
fn lib_rs_restores_admission_before_any_listener_starts() {
    let source = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
        .expect("read lib.rs");
    let start = source
        .find("fn initialize_core_logic(app_handle: &AppHandle) {")
        .expect("initialize_core_logic");
    let body_end = source[start..]
        .find("\n}\n")
        .map(|i| start + i)
        .expect("end");
    let body = &source[start..body_end];
    let restore = body
        .find("managers::source_update_driver::restore_admission(app_handle);")
        .expect("restore call in initialize_core_logic");
    let jobs = body
        .find("app_handle.manage(job_wiring);")
        .expect("job wiring managed");
    assert!(jobs < restore, "restore follows the managed job manager");
    // Listeners, shortcuts, wake word, ambient, conversation and tray starts that follow.
    let listeners = [
        "register_tts_listeners(app_handle",
        "wakeword_listener.start()",
        "ambient_listener.start()",
        "ConverseController::new(",
        "bubble_native::init(",
        "vault_rag_runtime::start_for_app(",
        "managers::source_update::spawn_scheduler(",
        "managers::source_update::adopt_existing_run(",
    ];
    for name in listeners {
        let at = body
            .find(name)
            .unwrap_or_else(|| panic!("{name} not in body"));
        assert!(restore < at, "restore_admission must precede {name}");
    }
}

#[test]
fn driver_log_lines_never_name_a_secret() {
    let source = include_str!("source_update_driver.rs");
    for line in source.lines() {
        let logs = ["info!(", "warn!(", "debug!(", "error!(", "trace!("]
            .iter()
            .any(|mac| line.contains(mac));
        if logs {
            let lower = line.to_ascii_lowercase();
            for word in ["token", "secret", "password"] {
                assert!(!lower.contains(word), "log line names a secret: {line}");
            }
        }
    }
}

#[test]
fn stable_reason_codes_cover_every_code_the_driver_records() {
    for code in [
        "interrupted",
        "install_failed",
        "build_failed",
        "invalid_pending",
        "spawn_failed",
        "verify_failed",
        "not_idle",
        "busy",
        "backoff",
        "unverified",
        "active_run",
        "blocked",
        "disabled",
        "not_source",
        "no_update",
    ] {
        assert!(
            crate::managers::source_update_auto::status::stable_reason_code(code).is_some(),
            "{code} must be a stable code"
        );
    }
}

#[test]
fn terminal_action_table() {
    let dir = tempfile::tempdir().expect("dir");
    let install = committing_run(dir.path());
    // The install's own terminal outcome settles it.
    assert_eq!(
        terminal_action(Some(&install), INSTALL, RunPhase::Done, false),
        TerminalAction::FinishInstall
    );
    assert_eq!(
        terminal_action(Some(&install), INSTALL, RunPhase::Failed, true),
        TerminalAction::FinishInstall
    );
    // A non-terminal phase never settles anything, so nothing is released.
    assert_eq!(
        terminal_action(Some(&install), INSTALL, RunPhase::Installing, false),
        TerminalAction::Ignore
    );
    // Another run's outcome does not settle the install.
    assert_eq!(
        terminal_action(Some(&install), "aaaaaaaaaaaaaaaa", RunPhase::Failed, false),
        TerminalAction::Ignore
    );
    // A failed automatic build is recorded; a finished one is not.
    assert_eq!(
        terminal_action(None, RUN, RunPhase::Failed, true),
        TerminalAction::BuildFailed
    );
    assert_eq!(
        terminal_action(None, RUN, RunPhase::Done, true),
        TerminalAction::Ignore
    );
    assert_eq!(
        terminal_action(None, RUN, RunPhase::Failed, false),
        TerminalAction::Ignore
    );
}

/// Fake of the settle effects; records the order of the calls.
#[derive(Default)]
struct FakeSettle {
    previous: AutoState,
    calls: RefCell<Vec<String>>,
    saved: RefCell<Option<AutoState>>,
}

impl FakeSettle {
    fn push(&self, entry: String) {
        self.calls.borrow_mut().push(entry);
    }
}

impl SettleEnv for FakeSettle {
    fn previous(&self) -> AutoState {
        self.previous.clone()
    }
    fn save(&self, state: &AutoState) {
        self.push("save".to_string());
        *self.saved.borrow_mut() = Some(state.clone());
    }
    fn remove_marker(&self, run_id: &str) {
        self.push(format!("marker:{run_id}"));
    }
    fn remove_pending(&self) {
        self.push("pending".to_string());
    }
    fn release(&self) {
        self.push("release".to_string());
    }
    fn announce(&self) {
        self.push("announce".to_string());
    }
}

#[test]
fn settled_install_records_updated_and_cleans_everything() {
    let dir = tempfile::tempdir().expect("dir");
    let run = committing_run(dir.path());
    let env = FakeSettle::default();
    settle_install(&env, &run, INSTALL, true, NOW);
    let saved = env.saved.borrow().clone().expect("state saved");
    assert_eq!(saved.last_result, Some(AutoResult::Updated));
    assert_eq!(saved.last_target.as_deref(), Some(SHA));
    assert_eq!(
        *env.calls.borrow(),
        [
            "save".to_string(),
            format!("marker:{INSTALL}"),
            "pending".to_string(),
            "release".to_string(),
            "announce".to_string(),
        ]
    );
}

#[test]
fn settled_failed_install_records_install_failed_and_still_cleans_up() {
    let dir = tempfile::tempdir().expect("dir");
    let run = committing_run(dir.path());
    let env = FakeSettle::default();
    settle_install(&env, &run, INSTALL, false, NOW);
    let saved = env.saved.borrow().clone().expect("state saved");
    assert_eq!(saved.last_result, Some(AutoResult::Failed));
    assert_eq!(saved.reason_code.as_deref(), Some("install_failed"));
    assert_eq!(saved.last_target.as_deref(), Some(SHA));
    assert_eq!(
        env.calls.borrow().last().map(String::as_str),
        Some("announce")
    );
    assert!(env.calls.borrow().contains(&"release".to_string()));
    assert!(env.calls.borrow().contains(&format!("marker:{INSTALL}")));
    assert!(env.calls.borrow().contains(&"pending".to_string()));
}

/// A manual install that fails also records `failed` for its target (the settle rule has no
/// origin check). The automatic path then waits out the 24 h cooldown for that target only.
#[test]
fn failed_manual_install_backs_off_the_automatic_path_for_its_target() {
    let dir = tempfile::tempdir().expect("dir");
    let mut run = committing_run(dir.path());
    run.origin = PendingOrigin::Manual;
    let saved = settled_state(&AutoState::default(), &run, false, NOW);
    assert!(!should_retry(&saved, SHA, NOW + 1));
    assert!(!should_retry(&saved, SHA, NOW + 23 * 60 * 60));
    assert!(should_retry(&saved, SHA, NOW + 25 * 60 * 60));
    assert!(should_retry(&saved, OTHER_SHA, NOW + 1));
}

#[test]
fn commit_built_outcome_table() {
    let dir = tempfile::tempdir().expect("dir");
    let committing = committing_run(dir.path());
    assert!(commit_built_outcome(&CommitStatus::Committed(committing.clone()), false).is_ok());
    // The driver committed the same record a moment earlier: the start is attached.
    assert!(commit_built_outcome(&CommitStatus::Deferred(DeferReason::NoBuiltRun), true).is_ok());
    assert!(commit_built_outcome(&CommitStatus::Deferred(DeferReason::NoBuiltRun), false).is_err());
    assert!(commit_built_outcome(&CommitStatus::Deferred(DeferReason::Busy), true).is_err());
    assert!(commit_built_outcome(&CommitStatus::Unresolved(committing), false).is_err());
}

#[test]
fn manual_start_over_a_built_update_requests_the_commit_and_launches_nothing() {
    let dir = tempfile::tempdir().expect("dir");
    write_record(dir.path(), &built_run(dir.path()));
    let plan = apply_manual_conversion(dir.path(), None).expect("plan");
    assert!(plan.request_commit);
    let outstanding =
        outstanding_kind(PendingStore::new(dir.path()).load().ok().flatten().as_ref());
    assert_eq!(outstanding, OutstandingKind::Built);
    assert_eq!(
        manual_start_plan(false, outstanding),
        ManualStart::CommitBuilt,
        "no new build is launched"
    );
    // Consume the one-shot request so the process-wide flag does not leak into other tests.
    assert!(take_manual_commit_request(RUN));
}

#[test]
fn manual_start_over_a_running_auto_build_converts_it_and_attaches() {
    let dir = tempfile::tempdir().expect("dir");
    AutoMarker::new(dir.path()).write(RUN).expect("marker");
    let plan = apply_manual_conversion(dir.path(), Some(RUN)).expect("plan");
    assert!(plan.remove_active_marker && !plan.request_commit);
    assert!(
        !silent_for_run(dir.path(), RUN),
        "progress is no longer silent"
    );
    assert_eq!(
        manual_start_plan(true, OutstandingKind::None),
        ManualStart::Attach
    );
}

#[test]
fn manual_start_over_an_outstanding_install_never_starts_another() {
    let dir = tempfile::tempdir().expect("dir");
    write_record(dir.path(), &committing_run(dir.path()));
    let plan = apply_manual_conversion(dir.path(), None).expect("plan");
    assert!(
        !plan.request_commit,
        "a committing record is never re-committed"
    );
    let outstanding =
        outstanding_kind(PendingStore::new(dir.path()).load().ok().flatten().as_ref());
    assert_eq!(outstanding, OutstandingKind::Committing);
    assert_eq!(
        manual_start_plan(false, outstanding),
        ManualStart::InProgress
    );
}

#[test]
fn restore_unknown_probe_with_a_running_install_releases_nothing() {
    let dir = tempfile::tempdir().expect("dir");
    write_record(dir.path(), &committing_run(dir.path()));
    let fake = FakeRestore::new(JobProbe::Unknown, Some(RunPhase::Installing));
    restore_with(&fake, &PendingStore::new(dir.path()));
    assert!(!fake.saw("finish:done") && !fake.saw("finish:failed"));
    assert!(!fake.saw("revoke"));
    assert!(PendingStore::new(dir.path()).path().exists());
}
