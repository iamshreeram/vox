use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use super::*;
use crate::managers::source_update_pending::{CommitInfo, PendingOrigin};

const RUN: &str = "0123456789abcdef";
const INSTALL: &str = "fedcba9876543210";
const TARGET: &str = "0123456789abcdef0123456789abcdef01234567";
const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn built() -> PendingRun {
    PendingRun {
        run_id: RUN.to_string(),
        target_full: TARGET.to_string(),
        staged_app: PathBuf::from("/state/staged/0123456789abcdef/Vox.app"),
        exe_name: "vox-bin".to_string(),
        exe_sha256: SHA.to_string(),
        info_plist_sha256: SHA.to_string(),
        origin: PendingOrigin::Auto,
        ts: 1_700_000_000,
        state: PendingState::Built,
    }
}

fn committing_of(base: &PendingRun, install_run_id: &str) -> PendingRun {
    PendingRun {
        state: PendingState::Committing(CommitInfo {
            install_run_id: install_run_id.to_string(),
            install_label: format!("com.iamshreeram.vox.update.{install_run_id}"),
            install_plist: PathBuf::from(format!("/state/run-{install_run_id}.plist")),
            committing_ts: 1_700_000_100,
        }),
        ..base.clone()
    }
}

fn is_committing(run: &Option<PendingRun>) -> bool {
    matches!(
        run.as_ref().map(|r| &r.state),
        Some(PendingState::Committing(_))
    )
}

/// One lock-counted resource (gate or agent-job reservation).
struct Slot {
    held: AtomicBool,
    drops: AtomicUsize,
}

impl Slot {
    fn new() -> Arc<Slot> {
        Arc::new(Slot {
            held: AtomicBool::new(false),
            drops: AtomicUsize::new(0),
        })
    }

    fn take(self: &Arc<Self>) -> Result<Token, Busy> {
        if self.held.swap(true, SeqCst) {
            Err(Busy)
        } else {
            Ok(Token {
                slot: Arc::clone(self),
            })
        }
    }

    fn is_held(&self) -> bool {
        self.held.load(SeqCst)
    }

    fn drops(&self) -> usize {
        self.drops.load(SeqCst)
    }
}

/// Guard handed out by the fake. Dropping it releases the slot and counts.
struct Token {
    slot: Arc<Slot>,
}

impl Drop for Token {
    fn drop(&mut self) {
        self.slot.held.store(false, SeqCst);
        self.slot.drops.fetch_add(1, SeqCst);
    }
}

/// Injectable failure points.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fail {
    Stop,
    BeginCommit,
    PlistWrite,
    Bootstrap,
    Bootout,
    Revert,
}

#[derive(Clone, Copy)]
struct Cfg {
    checks_on: bool,
    auto_on: bool,
    idle: bool,
    fail: Option<Fail>,
    fail_too: Option<Fail>,
}

fn cfg() -> Cfg {
    Cfg {
        checks_on: true,
        auto_on: true,
        idle: true,
        fail: None,
        fail_too: None,
    }
}

/// Fake env. Every call is logged, checks that the coordinator is held, and
/// sleeps briefly while flagging any overlapping call.
struct Fake<'a> {
    coord: &'a UpdateCoordinator,
    cfg: Cfg,
    gate: Arc<Slot>,
    jobs: Arc<Slot>,
    log: Mutex<Vec<&'static str>>,
    pending: Mutex<Option<PendingRun>>,
    probes: Mutex<VecDeque<JobProbe>>,
    state_at_bootstrap: Mutex<Option<bool>>,
    unlocked_calls: AtomicUsize,
    overlaps: AtomicUsize,
    active: AtomicUsize,
    pauses: AtomicUsize,
}

impl<'a> Fake<'a> {
    fn new(coord: &'a UpdateCoordinator, cfg: Cfg) -> Self {
        Self {
            coord,
            cfg,
            gate: Slot::new(),
            jobs: Slot::new(),
            log: Mutex::new(Vec::new()),
            pending: Mutex::new(Some(built())),
            probes: Mutex::new(VecDeque::new()),
            state_at_bootstrap: Mutex::new(None),
            unlocked_calls: AtomicUsize::new(0),
            overlaps: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            pauses: AtomicUsize::new(0),
        }
    }

    fn with_probes(self, probes: &[JobProbe]) -> Self {
        *lock(&self.probes) = probes.iter().copied().collect();
        self
    }

    fn step(&self, name: &'static str) {
        if !self.coord.is_held() {
            self.unlocked_calls.fetch_add(1, SeqCst);
        }
        if self.active.fetch_add(1, SeqCst) != 0 {
            self.overlaps.fetch_add(1, SeqCst);
        }
        lock(&self.log).push(name);
        std::thread::sleep(Duration::from_millis(2));
        self.active.fetch_sub(1, SeqCst);
    }

    fn result(&self, fail: Fail) -> Result<(), StepFailed> {
        let fails = self.cfg.fail == Some(fail) || self.cfg.fail_too == Some(fail);
        if fails {
            Err(StepFailed)
        } else {
            Ok(())
        }
    }

    fn calls(&self) -> Vec<&'static str> {
        lock(&self.log).clone()
    }

    fn pending_state_is_committing(&self) -> bool {
        is_committing(&lock(&self.pending))
    }

    fn assert_clean(&self) {
        assert_eq!(
            self.unlocked_calls.load(SeqCst),
            0,
            "env call outside coordinator"
        );
        assert_eq!(self.overlaps.load(SeqCst), 0, "overlapping env calls");
    }
}

impl Pacer for Fake<'_> {
    fn pause(&self) {
        self.pauses.fetch_add(1, SeqCst);
    }
}

impl CommitEnv for Fake<'_> {
    type Lease = Token;
    type Reservation = Token;

    fn consent(&self, manual: bool) -> bool {
        self.step("consent");
        self.cfg.checks_on && (manual || self.cfg.auto_on)
    }

    fn load_pending(&self) -> Result<Option<PendingRun>, StepFailed> {
        self.step("load");
        Ok(lock(&self.pending).clone())
    }

    fn acquire_gate(&self) -> Result<Token, Busy> {
        self.step("gate");
        self.gate.take()
    }

    fn reserve_jobs(&self) -> Result<Token, Busy> {
        self.step("jobs");
        self.jobs.take()
    }

    fn idle_ok(&self, manual: bool) -> bool {
        self.step("idle");
        self.cfg.idle || manual
    }

    fn stop_build_poller(&self) -> Result<(), StepFailed> {
        self.step("stop");
        self.result(Fail::Stop)
    }

    fn new_run_id(&self) -> String {
        self.step("run_id");
        INSTALL.to_string()
    }

    fn begin_commit(&self, built: &PendingRun, id: &str) -> Result<PendingRun, StepFailed> {
        self.step("begin");
        self.result(Fail::BeginCommit)?;
        let committing = committing_of(built, id);
        *lock(&self.pending) = Some(committing.clone());
        Ok(committing)
    }

    fn write_install_plist(&self, _: &PendingRun) -> Result<(), StepFailed> {
        self.step("plist");
        self.result(Fail::PlistWrite)
    }

    fn bootstrap(&self, _: &PendingRun) -> Result<(), StepFailed> {
        self.step("bootstrap");
        *lock(&self.state_at_bootstrap) = Some(self.pending_state_is_committing());
        self.result(Fail::Bootstrap)
    }

    fn probe_install(&self, _: &PendingRun) -> JobProbe {
        self.step("probe");
        lock(&self.probes).pop_front().unwrap_or(JobProbe::Unknown)
    }

    fn remove_install_plist(&self, _: &PendingRun) {
        self.step("rm_plist");
    }

    fn bootout_install(&self, _: &PendingRun) -> Result<(), StepFailed> {
        self.step("bootout");
        self.result(Fail::Bootout)
    }

    fn revert_to_built(&self, committing: &PendingRun) -> Result<(), StepFailed> {
        self.step("revert");
        self.result(Fail::Revert)?;
        *lock(&self.pending) = Some(PendingRun {
            state: PendingState::Built,
            ..committing.clone()
        });
        Ok(())
    }

    fn start_install_poller(&self, _: &PendingRun) {
        self.step("start_poller");
    }
}

fn registered() -> JobProbe {
    JobProbe::Registered {
        pid: Some(7),
        last_exit: None,
    }
}

fn registered_idle() -> JobProbe {
    JobProbe::Registered {
        pid: None,
        last_exit: Some(0),
    }
}

fn reason(outcome: &CommitOutcome<Token, Token>) -> DeferReason {
    match outcome {
        CommitOutcome::Deferred(reason) => *reason,
        CommitOutcome::Committed { .. } => panic!("expected Deferred, got Committed"),
        CommitOutcome::Unresolved { .. } => panic!("expected Deferred, got Unresolved"),
    }
}

/// True when the outcome is `Unresolved` and both guards are still held.
fn unresolved_guards_held(outcome: &CommitOutcome<Token, Token>) -> bool {
    match outcome {
        CommitOutcome::Unresolved {
            lease, reservation, ..
        } => lease.slot.is_held() && reservation.slot.is_held(),
        _ => false,
    }
}

/// Calls up to and including the prefix the test cares about.
fn tail_after(calls: &[&'static str], marker: &str) -> Vec<&'static str> {
    let at = calls
        .iter()
        .position(|c| *c == marker)
        .unwrap_or(calls.len());
    calls[at..].to_vec()
}

const HAPPY: &[&str] = &[
    "consent",
    "load",
    "gate",
    "jobs",
    "idle",
    "stop",
    "run_id",
    "begin",
    "plist",
    "bootstrap",
    "start_poller",
];

const PLIST_REVOKE: &[&str] = &["plist", "rm_plist", "bootout", "probe", "probe", "revert"];

const BOOTSTRAP_REVOKE: &[&str] = &[
    "bootstrap",
    "rm_plist",
    "bootout",
    "probe",
    "probe",
    "revert",
];

#[test]
fn happy_path_calls_in_contract_order_and_keeps_guards() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(&coord, cfg());
    let outcome = commit(&fake, &coord, false);
    assert_eq!(fake.calls(), HAPPY);
    fake.assert_clean();
    let CommitOutcome::Committed {
        lease,
        reservation,
        pending,
    } = outcome
    else {
        panic!("expected Committed");
    };
    assert!(is_committing(&Some(pending)));
    assert!(fake.gate.is_held() && fake.jobs.is_held());
    assert_eq!(fake.gate.drops(), 0);
    drop((lease, reservation));
    assert!(!fake.gate.is_held() && !fake.jobs.is_held());
    assert_eq!((fake.gate.drops(), fake.jobs.drops()), (1, 1));
}

#[test]
fn stop_build_poller_runs_strictly_before_begin_commit() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(&coord, cfg());
    let _ = commit(&fake, &coord, false);
    let calls = fake.calls();
    let stop = calls.iter().position(|c| *c == "stop");
    let begin = calls.iter().position(|c| *c == "begin");
    assert!(stop.is_some() && begin.is_some() && stop < begin);
}

#[test]
fn begin_commit_is_durable_before_bootstrap() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(&coord, cfg());
    let _ = commit(&fake, &coord, false);
    assert_eq!(*lock(&fake.state_at_bootstrap), Some(true));
}

#[test]
fn consent_off_never_touches_gate_or_jobs() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(
        &coord,
        Cfg {
            auto_on: false,
            ..cfg()
        },
    );
    let outcome = commit(&fake, &coord, false);
    assert_eq!(reason(&outcome), DeferReason::ConsentOff);
    assert_eq!(fake.calls(), ["consent"]);
    assert_eq!((fake.gate.drops(), fake.jobs.drops()), (0, 0));
    fake.assert_clean();
}

#[test]
fn checks_disabled_blocks_manual_commit_too() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(
        &coord,
        Cfg {
            checks_on: false,
            ..cfg()
        },
    );
    assert_eq!(
        reason(&commit(&fake, &coord, true)),
        DeferReason::ConsentOff
    );
    assert_eq!(fake.calls(), ["consent"]);
}

#[test]
fn no_built_record_defers_after_load_without_touching_gate() {
    for pending in [None, Some(committing_of(&built(), INSTALL))] {
        let coord = UpdateCoordinator::new();
        let fake = Fake::new(&coord, cfg());
        *lock(&fake.pending) = pending;
        let outcome = commit(&fake, &coord, false);
        assert_eq!(reason(&outcome), DeferReason::NoBuiltRun);
        assert_eq!(fake.calls(), ["consent", "load"]);
        assert_eq!(fake.gate.drops(), 0);
    }
}

#[test]
fn gate_busy_defers_without_reserving_jobs() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(&coord, cfg());
    fake.gate.held.store(true, SeqCst);
    assert_eq!(reason(&commit(&fake, &coord, false)), DeferReason::Busy);
    assert_eq!(fake.calls(), ["consent", "load", "gate"]);
    assert!(!fake.jobs.is_held());
}

#[test]
fn reserve_busy_releases_the_lease() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(&coord, cfg());
    fake.jobs.held.store(true, SeqCst);
    assert_eq!(reason(&commit(&fake, &coord, false)), DeferReason::Busy);
    assert_eq!(fake.calls(), ["consent", "load", "gate", "jobs"]);
    assert_eq!((fake.gate.drops(), fake.gate.is_held()), (1, false));
}

#[test]
fn not_idle_releases_both_guards() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(
        &coord,
        Cfg {
            idle: false,
            ..cfg()
        },
    );
    assert_eq!(reason(&commit(&fake, &coord, false)), DeferReason::NotIdle);
    assert!(!fake.calls().contains(&"stop"));
    assert_eq!((fake.gate.drops(), fake.jobs.drops()), (1, 1));
    assert!(!fake.gate.is_held() && !fake.jobs.is_held());
}

#[test]
fn poller_stop_failure_releases_both_and_never_begins() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(
        &coord,
        Cfg {
            fail: Some(Fail::Stop),
            ..cfg()
        },
    );
    assert_eq!(
        reason(&commit(&fake, &coord, false)),
        DeferReason::PollerStop
    );
    assert!(!fake.calls().contains(&"begin"));
    assert_eq!((fake.gate.drops(), fake.jobs.drops()), (1, 1));
}

#[test]
fn begin_commit_failure_releases_both_and_leaves_built() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(
        &coord,
        Cfg {
            fail: Some(Fail::BeginCommit),
            ..cfg()
        },
    );
    assert_eq!(reason(&commit(&fake, &coord, false)), DeferReason::Fs);
    assert!(!fake.calls().contains(&"plist"));
    assert!(!fake.pending_state_is_committing());
    assert_eq!((fake.gate.drops(), fake.jobs.drops()), (1, 1));
}

#[test]
fn plist_write_failure_revokes_in_contract_order() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(
        &coord,
        Cfg {
            fail: Some(Fail::PlistWrite),
            ..cfg()
        },
    )
    .with_probes(&[JobProbe::Absent, JobProbe::Absent]);
    let outcome = commit(&fake, &coord, false);
    assert_eq!(reason(&outcome), DeferReason::BootstrapFailed);
    assert_eq!(tail_after(&fake.calls(), "plist"), PLIST_REVOKE);
    assert!(!fake.calls().contains(&"bootstrap"));
    assert_eq!(fake.pauses.load(SeqCst), 1);
    assert!(!fake.pending_state_is_committing());
    assert_eq!((fake.gate.drops(), fake.jobs.drops()), (1, 1));
    fake.assert_clean();
}

#[test]
fn bootstrap_failure_revokes_then_releases() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(
        &coord,
        Cfg {
            fail: Some(Fail::Bootstrap),
            ..cfg()
        },
    )
    .with_probes(&[JobProbe::Absent, JobProbe::Absent]);
    assert_eq!(
        reason(&commit(&fake, &coord, false)),
        DeferReason::BootstrapFailed
    );
    assert_eq!(tail_after(&fake.calls(), "bootstrap"), BOOTSTRAP_REVOKE);
    assert!(!fake.calls().contains(&"start_poller"));
    assert!(!fake.pending_state_is_committing());
    assert_eq!((fake.gate.drops(), fake.jobs.drops()), (1, 1));
}

#[test]
fn revoke_inconclusive_via_unknown_keeps_guards_and_record() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(
        &coord,
        Cfg {
            fail: Some(Fail::Bootstrap),
            ..cfg()
        },
    )
    .with_probes(&[JobProbe::Unknown]);
    let outcome = commit(&fake, &coord, false);
    let CommitOutcome::Unresolved { pending, .. } = &outcome else {
        panic!("expected Unresolved");
    };
    assert!(is_committing(&Some(pending.clone())));
    assert_eq!(
        tail_after(&fake.calls(), "bootstrap"),
        ["bootstrap", "rm_plist", "bootout", "probe"]
    );
    assert!(fake.gate.is_held() && fake.jobs.is_held());
    assert_eq!((fake.gate.drops(), fake.jobs.drops()), (0, 0));
    drop(outcome);
    assert!(!fake.gate.is_held() && !fake.jobs.is_held());
}

#[test]
fn revoke_registered_then_absent_adopts_and_keeps_guards() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(
        &coord,
        Cfg {
            fail: Some(Fail::Bootstrap),
            ..cfg()
        },
    )
    .with_probes(&[registered_idle(), JobProbe::Absent]);
    let outcome = commit(&fake, &coord, false);
    assert!(matches!(outcome, CommitOutcome::Committed { .. }));
    assert_eq!(fake.calls().last(), Some(&"start_poller"));
    assert!(fake.gate.is_held() && fake.jobs.is_held());
    assert!(fake.pending_state_is_committing());
}

#[test]
fn revoke_absent_then_registered_adopts_and_keeps_guards() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(
        &coord,
        Cfg {
            fail: Some(Fail::PlistWrite),
            ..cfg()
        },
    )
    .with_probes(&[JobProbe::Absent, registered()]);
    let outcome = commit(&fake, &coord, false);
    assert!(matches!(outcome, CommitOutcome::Committed { .. }));
    assert!(fake.gate.is_held() && fake.jobs.is_held());
    assert_eq!(fake.calls().iter().filter(|c| **c == "revert").count(), 0);
}

#[test]
fn revoke_absent_then_unknown_is_inconclusive_and_keeps_guards() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(
        &coord,
        Cfg {
            fail: Some(Fail::PlistWrite),
            ..cfg()
        },
    )
    .with_probes(&[JobProbe::Absent, JobProbe::Unknown]);
    let outcome = commit(&fake, &coord, false);
    assert!(unresolved_guards_held(&outcome));
    assert!(!fake.calls().contains(&"revert"));
    assert!(fake.gate.is_held() && fake.jobs.is_held());
    assert_eq!((fake.gate.drops(), fake.jobs.drops()), (0, 0));
}

#[test]
fn revoke_bootout_failure_is_inconclusive_without_probing() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(
        &coord,
        Cfg {
            fail: Some(Fail::Bootstrap),
            fail_too: Some(Fail::Bootout),
            ..cfg()
        },
    );
    let outcome = commit(&fake, &coord, false);
    assert!(unresolved_guards_held(&outcome));
    assert!(!fake.calls().contains(&"probe"));
    assert!(fake.gate.is_held());
}

#[test]
fn revert_failure_after_conclusive_absent_keeps_guards() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(
        &coord,
        Cfg {
            fail: Some(Fail::Bootstrap),
            fail_too: Some(Fail::Revert),
            ..cfg()
        },
    )
    .with_probes(&[JobProbe::Absent, JobProbe::Absent]);
    let outcome = commit(&fake, &coord, false);
    assert!(unresolved_guards_held(&outcome));
    assert!(fake.gate.is_held() && fake.jobs.is_held());
}

#[test]
fn manual_skips_auto_opt_in_and_idle_but_takes_both_guards() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(
        &coord,
        Cfg {
            auto_on: false,
            idle: false,
            ..cfg()
        },
    );
    let outcome = commit(&fake, &coord, true);
    assert!(matches!(outcome, CommitOutcome::Committed { .. }));
    assert!(fake.calls().contains(&"gate") && fake.calls().contains(&"jobs"));
}

#[test]
fn admission_hold_release_drops_both_guards_once() {
    let slot_gate = Slot::new();
    let slot_jobs = Slot::new();
    let (Ok(lease), Ok(reservation)) = (slot_gate.take(), slot_jobs.take()) else {
        panic!("fresh slots must be takeable");
    };
    let hold = AdmissionHold::new(lease, reservation);
    assert!(slot_gate.is_held() && slot_jobs.is_held());
    hold.release();
    assert!(!slot_gate.is_held() && !slot_jobs.is_held());
    assert_eq!((slot_gate.drops(), slot_jobs.drops()), (1, 1));
}

#[test]
fn concurrent_commits_serialise_and_only_one_commits() {
    let coord = UpdateCoordinator::new();
    let fake = Fake::new(&coord, cfg());
    let mut tags: Vec<u8> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| match commit(&fake, &coord, false) {
                    CommitOutcome::Committed { .. } => 1_u8,
                    CommitOutcome::Deferred(_) => 2,
                    CommitOutcome::Unresolved { .. } => 3,
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or(0)).collect()
    });
    tags.sort_unstable();
    assert_eq!(tags, vec![1, 2]);
    fake.assert_clean();
}
