//! Tests for the phase-aware plist builder, the launch and install steps, and the
//! real commit environment driven through fakes (launchctl, host, real files).

use super::*;
use crate::managers::agent_jobs::JobManager;
use crate::managers::ownership::{Owner, OwnershipGate};
use crate::managers::source_update_auto::consent::AutoMarker;
use crate::managers::source_update_auto::UpdateCoordinator;
use crate::managers::source_update_commit::{commit, CommitOutcome, DeferReason};
use crate::managers::source_update_pending::{
    encode_pending, staged_app_path, CommitInfo, PendingOrigin, PendingState, PENDING_FILE_NAME,
};
use crate::managers::source_update_runner::write_private_atomic;
use crate::managers::source_update_runner::{
    build_job_plist, build_job_plist_kind, job_label, plist_path, update_state_dir, JobParams,
    RunKind, RunnerError,
};
use crate::managers::source_update_runner_run::{launch_build, write_install_plist};
use std::collections::VecDeque;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const RUN: &str = "0123456789abcdef";
const INSTALL: &str = "fedcba9876543210";
const TARGET: &str = "0123456789abcdef0123456789abcdef01234567";
const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Keys every phase carries (PR A base environment, minus the target sha).
const BASE_KEYS: [&str; 16] = [
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "PATH",
    "LANG",
    "GIT_TERMINAL_PROMPT",
    "GIT_SSH_COMMAND",
    "VOX_SOURCE_UPDATE",
    "VOX_UPDATE_RUN_ID",
    "VOX_UPDATE_STATUS_FILE",
    "VOX_UPDATE_STATE_DIR",
    "VOX_UPDATE_NONINTERACTIVE",
    "VOX_UPDATE_LOCK_DIR",
    "CARGO_TERM_PROGRESS_WHEN",
    "CARGO_TERM_PROGRESS_WIDTH",
];

type Log = Arc<Mutex<Vec<String>>>;

fn record(log: &Log, entry: String) {
    log.lock().unwrap().push(entry);
}

struct Fixture {
    _root: tempfile::TempDir,
    app_data: PathBuf,
    state: PathBuf,
    home: PathBuf,
    tmpdir: PathBuf,
    lock_dir: PathBuf,
    checkout: PathBuf,
    site: RunSite,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().to_path_buf();
    let checkout = base.join("checkout");
    fs::create_dir_all(checkout.join("scripts")).unwrap();
    fs::write(checkout.join("scripts/update-macos.sh"), "#!/bin/bash\n").unwrap();
    let app_data = base.join("app");
    fs::create_dir_all(&app_data).unwrap();
    let state = update_state_dir(&app_data);
    crate::managers::source_update_runner::ensure_state_dir(&state).unwrap();
    let home = base.join("home");
    fs::create_dir_all(&home).unwrap();
    let lock_dir = base.join("vox-update.lock");
    let site = RunSite {
        app_data: app_data.clone(),
        source_dir: checkout.clone(),
        home: home.clone(),
        tmpdir: base.clone(),
        lock_dir: lock_dir.clone(),
        proxy_env: Vec::new(),
    };
    Fixture {
        _root: root,
        app_data,
        state,
        home,
        tmpdir: base,
        lock_dir,
        checkout,
        site,
    }
}

fn built_record(state: &Path) -> PendingRun {
    PendingRun {
        run_id: RUN.to_string(),
        target_full: TARGET.to_string(),
        staged_app: staged_app_path(state, RUN),
        exe_name: "vox-bin".to_string(),
        exe_sha256: SHA.to_string(),
        info_plist_sha256: SHA.to_string(),
        origin: PendingOrigin::Auto,
        ts: 1_700_000_000,
        state: PendingState::Built,
    }
}

fn write_record(state: &Path, run: &PendingRun) {
    let bytes = encode_pending(run, state).unwrap();
    write_private_atomic(&PendingStore::new(state).path(), &bytes).unwrap();
}

fn committing_of(base: &PendingRun, state: &Path, install_run_id: &str) -> PendingRun {
    PendingRun {
        state: PendingState::Committing(CommitInfo {
            install_run_id: install_run_id.to_string(),
            install_label: job_label(install_run_id),
            install_plist: plist_path(state, install_run_id),
            committing_ts: 1_700_000_100,
        }),
        ..base.clone()
    }
}

fn params(fx: &Fixture, run_id: &str, target: &str) -> JobParams {
    JobParams {
        run_id: run_id.to_string(),
        source_dir: fx.checkout.clone(),
        status_file: fx.state.join("status.json"),
        state_dir: fx.state.clone(),
        log_file: fx.state.join(format!("run-{run_id}.log")),
        target_full: target.to_string(),
        home: fx.home.clone(),
        tmpdir: fx.tmpdir.clone(),
        lock_dir: fx.lock_dir.clone(),
        proxy_env: Vec::new(),
    }
}

fn env_keys(xml: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut inside = false;
    for line in xml.lines() {
        if line.contains("<key>EnvironmentVariables</key>") {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if line == "\t</dict>" {
            break;
        }
        if let Some(key) = line
            .strip_prefix("\t\t<key>")
            .and_then(|rest| rest.strip_suffix("</key>"))
        {
            keys.push(key.to_string());
        }
    }
    keys
}

fn env_value(xml: &str, key: &str) -> Option<String> {
    let marker = format!("\t\t<key>{key}</key>");
    let mut lines = xml.lines();
    while let Some(line) = lines.next() {
        if line == marker {
            let value = lines.next()?;
            return value
                .strip_prefix("\t\t<string>")
                .and_then(|rest| rest.strip_suffix("</string>"))
                .map(str::to_string);
        }
    }
    None
}

fn sorted(mut keys: Vec<String>) -> Vec<String> {
    keys.sort();
    keys
}

fn expected(extra: &[&str]) -> Vec<String> {
    let mut all: Vec<String> = BASE_KEYS.iter().map(|k| k.to_string()).collect();
    all.extend(extra.iter().map(|k| k.to_string()));
    sorted(all)
}

fn assert_no_secret_values(xml: &str) {
    for key in env_keys(xml) {
        let value = env_value(xml, &key).unwrap_or_default().to_lowercase();
        for needle in ["token", "secret", "password", "bearer"] {
            assert!(!value.contains(needle), "secret-like value under {key}");
        }
    }
}

// ---- fake launchctl --------------------------------------------------------

/// Records every launchctl-level call. Probes are scripted; the default is absent.
struct FakeControl {
    log: Log,
    state: PathBuf,
    probes: Mutex<VecDeque<JobProbe>>,
    fail_bootstrap: bool,
    marker_at_bootstrap: Mutex<Option<bool>>,
}

impl FakeControl {
    fn new(log: Log, state: &Path) -> Self {
        Self {
            log,
            state: state.to_path_buf(),
            probes: Mutex::new(VecDeque::new()),
            fail_bootstrap: false,
            marker_at_bootstrap: Mutex::new(None),
        }
    }

    fn with_probes(self, probes: &[JobProbe]) -> Self {
        *self.probes.lock().unwrap() = probes.iter().copied().collect();
        self
    }

    fn failing(mut self) -> Self {
        self.fail_bootstrap = true;
        self
    }

    fn calls(&self, prefix: &str) -> usize {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.starts_with(prefix))
            .count()
    }
}

impl JobControl for FakeControl {
    fn probe(&self, label: &str) -> JobProbe {
        record(&self.log, format!("probe:{label}"));
        self.probes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(JobProbe::Absent)
    }

    fn bootstrap(&self, plist: &Path) -> Result<(), RunnerError> {
        let committing = PendingStore::new(&self.state)
            .load()
            .ok()
            .flatten()
            .is_some_and(|run| matches!(run.state, PendingState::Committing(_)));
        let plist_there = plist.exists();
        let marker_there = AutoMarker::new(&self.state).exists(RUN);
        *self.marker_at_bootstrap.lock().unwrap() = Some(marker_there);
        record(
            &self.log,
            format!("bootstrap:committing={committing},plist={plist_there}"),
        );
        if self.fail_bootstrap {
            return Err(RunnerError::Launchctl {
                operation: "bootstrap",
                code: Some(5),
            });
        }
        Ok(())
    }

    fn bootout(&self, label: &str) -> Result<(), RunnerError> {
        record(&self.log, format!("bootout:{label}"));
        Ok(())
    }
}

// ---- fake host -------------------------------------------------------------

struct FakeHost {
    checks: bool,
    auto: bool,
    gate: Arc<OwnershipGate>,
    jobs: Arc<JobManager>,
    now: Instant,
    start: Instant,
    state: PathBuf,
    log: Log,
}

impl FakeHost {
    fn new(fx: &Fixture, log: Log) -> Self {
        let created = Instant::now();
        Self {
            checks: true,
            auto: true,
            gate: OwnershipGate::new(),
            jobs: JobManager::new(),
            now: created + Duration::from_secs(120),
            start: created,
            state: fx.state.clone(),
            log,
        }
    }
}

impl CommitHost for FakeHost {
    fn checks_enabled(&self) -> bool {
        self.checks
    }
    fn auto_enabled(&self) -> bool {
        self.auto
    }
    fn gate(&self) -> &Arc<OwnershipGate> {
        &self.gate
    }
    fn jobs(&self) -> Arc<JobManager> {
        Arc::clone(&self.jobs)
    }
    fn now(&self) -> Instant {
        self.now
    }
    fn process_start(&self) -> Instant {
        self.start
    }
    fn stop_build_poller(&self) -> Result<(), PollerStopError> {
        let phase = match PendingStore::new(&self.state).load() {
            Ok(Some(run)) if run.state == PendingState::Built => "built",
            _ => "other",
        };
        record(&self.log, format!("stop_poller:{phase}"));
        Ok(())
    }
    fn start_poller(&self, run_id: &str, config: PollerConfig) {
        record(&self.log, format!("start_poller:{run_id}:{}", config.label));
    }
}

// ---- helpers for the commit tests -------------------------------------------

fn new_log() -> Log {
    Arc::new(Mutex::new(Vec::new()))
}

fn snapshot(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

fn plist_files(state: &Path) -> Vec<String> {
    fs::read_dir(state)
        .unwrap()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("run-") && n.ends_with(".plist"))
        .collect()
}

fn commit_once(
    host: &FakeHost,
    control: &FakeControl,
    fx: &Fixture,
    manual: bool,
) -> CommitOutcome<Lease, UpdateReservation> {
    let env = AppCommitEnv::new(
        host,
        control,
        fx.site.clone(),
        PendingStore::new(&fx.state),
        Duration::ZERO,
    );
    let coordinator = UpdateCoordinator::new();
    commit(&env, &coordinator, manual)
}

fn reason(outcome: &CommitOutcome<Lease, UpdateReservation>) -> Option<DeferReason> {
    match outcome {
        CommitOutcome::Deferred(reason) => Some(*reason),
        _ => None,
    }
}

// ---- plist bytes -------------------------------------------------------------

#[test]
fn all_phase_bytes_are_unchanged_from_the_base_builder() {
    let fx = fixture();
    let p = params(&fx, RUN, TARGET);
    assert_eq!(
        build_job_plist(&p).unwrap(),
        build_job_plist_kind(&p, RunKind::All).unwrap()
    );
    assert_eq!(
        sorted(env_keys(&build_job_plist(&p).unwrap())),
        expected(&["VOX_UPDATE_TARGET_SHA"])
    );
}

#[test]
fn build_plist_has_exactly_the_documented_environment() {
    let fx = fixture();
    let p = params(&fx, RUN, TARGET);
    let auto = build_job_plist_kind(&p, RunKind::Build { origin_auto: true }).unwrap();
    assert_eq!(
        sorted(env_keys(&auto)),
        expected(&[
            "VOX_UPDATE_TARGET_SHA",
            "VOX_UPDATE_PHASE",
            "VOX_UPDATE_ORIGIN"
        ])
    );
    assert_eq!(
        env_value(&auto, "VOX_UPDATE_PHASE").as_deref(),
        Some("build")
    );
    assert_eq!(
        env_value(&auto, "VOX_UPDATE_ORIGIN").as_deref(),
        Some("auto")
    );
    assert_eq!(
        env_value(&auto, "VOX_UPDATE_TARGET_SHA").as_deref(),
        Some(TARGET)
    );
    let manual = build_job_plist_kind(&p, RunKind::Build { origin_auto: false }).unwrap();
    assert_eq!(
        env_value(&manual, "VOX_UPDATE_ORIGIN").as_deref(),
        Some("manual")
    );
    assert_no_secret_values(&auto);
    assert_no_secret_values(&manual);
}

#[test]
fn install_plist_has_exactly_the_documented_environment() {
    let fx = fixture();
    let built = built_record(&fx.state);
    let committing = committing_of(&built, &fx.state, INSTALL);
    let p = params(&fx, INSTALL, TARGET);
    let xml = build_job_plist_kind(
        &p,
        RunKind::Install {
            pending: &committing,
        },
    )
    .unwrap();
    assert_eq!(
        sorted(env_keys(&xml)),
        expected(&[
            "VOX_UPDATE_PHASE",
            "VOX_UPDATE_PENDING_FILE",
            "VOX_UPDATE_PERCENT_FLOOR"
        ])
    );
    assert_eq!(
        env_value(&xml, "VOX_UPDATE_PHASE").as_deref(),
        Some("install")
    );
    assert_eq!(
        env_value(&xml, "VOX_UPDATE_RUN_ID").as_deref(),
        Some(INSTALL)
    );
    assert_eq!(
        env_value(&xml, "VOX_UPDATE_PERCENT_FLOOR").as_deref(),
        Some("91")
    );
    let pending_file = fx.state.join(PENDING_FILE_NAME);
    assert_eq!(
        env_value(&xml, "VOX_UPDATE_PENDING_FILE").as_deref(),
        pending_file.to_str()
    );
    assert!(xml.contains("<key>AbandonProcessGroup</key>\n\t<true/>"));
    assert!(xml.contains(&format!("<string>{}</string>", job_label(INSTALL))));
    assert_no_secret_values(&xml);
}

#[test]
fn install_plist_refuses_a_run_id_that_is_not_the_install_id() {
    let fx = fixture();
    let committing = committing_of(&built_record(&fx.state), &fx.state, INSTALL);
    let p = params(&fx, RUN, TARGET);
    let result = build_job_plist_kind(
        &p,
        RunKind::Install {
            pending: &committing,
        },
    );
    assert!(matches!(result, Err(RunnerError::InvalidValue(_))));
}

#[test]
fn write_install_plist_writes_at_the_record_path_with_mode_0600() {
    let fx = fixture();
    let built = built_record(&fx.state);
    let committing = committing_of(&built, &fx.state, INSTALL);
    let inputs = fx.site.inputs(INSTALL, TARGET);
    let path = write_install_plist(&inputs, &committing).unwrap();
    assert_eq!(path, plist_path(&fx.state, INSTALL));
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let xml = fs::read_to_string(&path).unwrap();
    assert!(xml.contains(&job_label(INSTALL)));
    // A second write must not replace an existing plist.
    assert!(matches!(
        write_install_plist(&inputs, &committing),
        Err(RunnerError::Busy(_))
    ));
}

// ---- launch_build --------------------------------------------------------------

#[test]
fn launch_build_writes_the_marker_before_bootstrap_and_the_plist_is_0600() {
    let fx = fixture();
    let log = new_log();
    let control = FakeControl::new(Arc::clone(&log), &fx.state);
    let inputs = fx.site.inputs(RUN, TARGET);
    let launched = launch_build(&control, &inputs, true).unwrap();
    assert_eq!(*control.marker_at_bootstrap.lock().unwrap(), Some(true));
    assert!(AutoMarker::new(&fx.state).exists(RUN));
    assert_eq!(launched.plist, plist_path(&fx.state, RUN));
    assert_eq!(launched.label, job_label(RUN));
    let mode = fs::metadata(&launched.plist).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn manual_build_writes_no_marker() {
    let fx = fixture();
    let log = new_log();
    let control = FakeControl::new(Arc::clone(&log), &fx.state);
    let inputs = fx.site.inputs(RUN, TARGET);
    launch_build(&control, &inputs, false).unwrap();
    assert!(!AutoMarker::new(&fx.state).exists(RUN));
}

#[test]
fn failed_auto_launch_leaves_no_marker_and_no_plist() {
    let fx = fixture();
    let log = new_log();
    let control = FakeControl::new(Arc::clone(&log), &fx.state).failing();
    let inputs = fx.site.inputs(RUN, TARGET);
    assert!(launch_build(&control, &inputs, true).is_err());
    assert!(!AutoMarker::new(&fx.state).exists(RUN));
    assert!(plist_files(&fx.state).is_empty());
}

#[test]
fn launch_build_refuses_when_a_pending_record_exists() {
    let fx = fixture();
    write_record(&fx.state, &built_record(&fx.state));
    let log = new_log();
    let control = FakeControl::new(Arc::clone(&log), &fx.state);
    let inputs = fx.site.inputs(RUN, TARGET);
    assert!(matches!(
        launch_build(&control, &inputs, true),
        Err(RunnerError::Busy(_))
    ));
    assert_eq!(control.calls("bootstrap"), 0);
    assert!(!AutoMarker::new(&fx.state).exists(RUN));
}

#[test]
fn launch_build_refuses_when_a_run_plist_is_registered() {
    let fx = fixture();
    fs::write(plist_path(&fx.state, INSTALL), "x").unwrap();
    let log = new_log();
    let control = FakeControl::new(Arc::clone(&log), &fx.state);
    let inputs = fx.site.inputs(RUN, TARGET);
    assert!(matches!(
        launch_build(&control, &inputs, false),
        Err(RunnerError::Busy(_))
    ));
    assert_eq!(control.calls("bootstrap"), 0);
}

#[test]
fn launch_build_refuses_when_the_update_lock_is_present() {
    let fx = fixture();
    // A lock directory without a readable owner counts as held: refuse.
    fs::create_dir_all(&fx.lock_dir).unwrap();
    let log = new_log();
    let control = FakeControl::new(Arc::clone(&log), &fx.state);
    let inputs = fx.site.inputs(RUN, TARGET);
    assert!(matches!(
        launch_build(&control, &inputs, false),
        Err(RunnerError::Busy(_))
    ));
    assert_eq!(control.calls("bootstrap"), 0);
}

// ---- idle rule -------------------------------------------------------------------

#[test]
fn idle_rule_needs_a_free_gate_and_sixty_quiet_seconds() {
    let base = Instant::now();
    let t = |s: u64| base + Duration::from_secs(s);
    assert!(
        !idle_ready(t(600), None, base, base),
        "gate held: never idle"
    );
    assert!(
        !idle_ready(t(59), Some(base), base, base),
        "59 s is not enough"
    );
    assert!(idle_ready(t(60), Some(base), base, base), "60 s is enough");
    assert!(
        !idle_ready(t(100), Some(base), t(50), base),
        "recent agent activity restarts the clock"
    );
    assert!(
        !idle_ready(t(100), Some(base), base, t(70)),
        "a late process start restarts the clock"
    );
}

// ---- commit environment, end to end with fakes ------------------------------------

#[test]
fn happy_path_stops_poller_then_commits_on_disk_then_bootstraps_in_order() {
    let fx = fixture();
    let built = built_record(&fx.state);
    write_record(&fx.state, &built);
    let log = new_log();
    let host = FakeHost::new(&fx, Arc::clone(&log));
    let control = FakeControl::new(Arc::clone(&log), &fx.state);
    let outcome = commit_once(&host, &control, &fx, false);
    let CommitOutcome::Committed {
        lease,
        reservation,
        pending,
    } = outcome
    else {
        panic!("expected a commit");
    };
    assert_eq!(
        snapshot(&log),
        vec![
            "stop_poller:built".to_string(),
            "bootstrap:committing=true,plist=true".to_string(),
            format!(
                "start_poller:{}:{}",
                pending_install_id(&pending),
                job_label(&pending_install_id(&pending))
            ),
        ]
    );
    assert!(plist_path(&fx.state, &pending_install_id(&pending)).exists());
    let on_disk = PendingStore::new(&fx.state).load().unwrap().unwrap();
    assert!(matches!(on_disk.state, PendingState::Committing(_)));
    assert_eq!(host.gate.holder(), Some(Owner::Updater));
    assert!(
        host.jobs.try_reserve_for_update().is_err(),
        "reservation still held"
    );
    drop(lease);
    drop(reservation);
}

fn pending_install_id(run: &PendingRun) -> String {
    match &run.state {
        PendingState::Committing(info) => info.install_run_id.clone(),
        PendingState::Built => String::new(),
    }
}

#[test]
fn consent_off_writes_nothing_and_touches_no_guard() {
    let fx = fixture();
    write_record(&fx.state, &built_record(&fx.state));
    let log = new_log();
    let mut host = FakeHost::new(&fx, Arc::clone(&log));
    host.auto = false;
    let control = FakeControl::new(Arc::clone(&log), &fx.state);
    let outcome = commit_once(&host, &control, &fx, false);
    assert_eq!(reason(&outcome), Some(DeferReason::ConsentOff));
    assert!(snapshot(&log).is_empty());
    assert!(plist_files(&fx.state).is_empty());
    assert_eq!(host.gate.holder(), None);
}

#[test]
fn checks_disabled_blocks_even_a_manual_commit() {
    let fx = fixture();
    write_record(&fx.state, &built_record(&fx.state));
    let log = new_log();
    let mut host = FakeHost::new(&fx, Arc::clone(&log));
    host.checks = false;
    let control = FakeControl::new(Arc::clone(&log), &fx.state);
    let outcome = commit_once(&host, &control, &fx, true);
    assert_eq!(reason(&outcome), Some(DeferReason::ConsentOff));
    assert!(snapshot(&log).is_empty());
}

#[test]
fn gate_busy_writes_nothing() {
    let fx = fixture();
    write_record(&fx.state, &built_record(&fx.state));
    let log = new_log();
    let host = FakeHost::new(&fx, Arc::clone(&log));
    let _dictation = host.gate.try_acquire(Owner::Dictation).unwrap();
    let control = FakeControl::new(Arc::clone(&log), &fx.state);
    let outcome = commit_once(&host, &control, &fx, false);
    assert_eq!(reason(&outcome), Some(DeferReason::Busy));
    assert!(snapshot(&log).is_empty());
    assert!(plist_files(&fx.state).is_empty());
    assert!(PendingStore::new(&fx.state)
        .load()
        .unwrap()
        .is_some_and(|r| r.state == PendingState::Built));
}

#[test]
fn bootstrap_failure_revokes_and_releases_everything() {
    let fx = fixture();
    write_record(&fx.state, &built_record(&fx.state));
    let log = new_log();
    let host = FakeHost::new(&fx, Arc::clone(&log));
    let control = FakeControl::new(Arc::clone(&log), &fx.state).failing();
    let outcome = commit_once(&host, &control, &fx, false);
    assert_eq!(reason(&outcome), Some(DeferReason::BootstrapFailed));
    assert!(plist_files(&fx.state).is_empty(), "plist removed");
    assert_eq!(control.calls("bootout:"), 1);
    assert_eq!(control.calls("probe:"), 2, "two confirmed absent probes");
    let restored = PendingStore::new(&fx.state).load().unwrap().unwrap();
    assert_eq!(restored.state, PendingState::Built, "reverted to built");
    assert_eq!(host.gate.holder(), None, "lease released");
    assert!(
        host.jobs.try_reserve_for_update().is_ok(),
        "reservation released"
    );
}

#[test]
fn unknown_probes_keep_the_guards_and_the_committing_record() {
    let fx = fixture();
    write_record(&fx.state, &built_record(&fx.state));
    let log = new_log();
    let host = FakeHost::new(&fx, Arc::clone(&log));
    let control = FakeControl::new(Arc::clone(&log), &fx.state)
        .failing()
        .with_probes(&[JobProbe::Unknown]);
    let outcome = commit_once(&host, &control, &fx, false);
    let CommitOutcome::Unresolved {
        lease,
        reservation,
        pending,
    } = outcome
    else {
        panic!("expected unresolved");
    };
    assert_eq!(host.gate.holder(), Some(Owner::Updater));
    assert!(matches!(pending.state, PendingState::Committing(_)));
    let on_disk = PendingStore::new(&fx.state).load().unwrap().unwrap();
    assert!(matches!(on_disk.state, PendingState::Committing(_)));
    drop((lease, reservation));
}

#[test]
fn a_registered_install_label_is_adopted_not_revoked() {
    let fx = fixture();
    write_record(&fx.state, &built_record(&fx.state));
    let log = new_log();
    let host = FakeHost::new(&fx, Arc::clone(&log));
    let registered = JobProbe::Registered {
        pid: None,
        last_exit: None,
    };
    let control = FakeControl::new(Arc::clone(&log), &fx.state)
        .failing()
        .with_probes(&[registered]);
    let outcome = commit_once(&host, &control, &fx, false);
    assert!(matches!(outcome, CommitOutcome::Committed { .. }));
}

#[test]
fn admission_keeps_and_releases_the_guards() {
    let gate = OwnershipGate::new();
    let jobs = JobManager::new();
    let admission = UpdateAdmission::new();
    let lease = gate.try_acquire(Owner::Updater).unwrap();
    let reservation = jobs.try_reserve_for_update().unwrap();
    admission.store(AdmissionHold::new(lease, reservation));
    assert!(admission.is_held());
    assert_eq!(gate.holder(), Some(Owner::Updater));
    admission.release();
    assert!(!admission.is_held());
    assert_eq!(gate.holder(), None);
    assert!(jobs.try_reserve_for_update().is_ok());
}

#[test]
fn committing_record_path_is_the_run_plist_path() {
    let fx = fixture();
    let committing = committing_of(&built_record(&fx.state), &fx.state, INSTALL);
    let PendingState::Committing(info) = &committing.state else {
        panic!("committing");
    };
    assert_eq!(info.install_plist, plist_path(&fx.state, INSTALL));
    assert_eq!(info.install_label, job_label(INSTALL));
}
