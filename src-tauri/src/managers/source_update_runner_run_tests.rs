//! Tests for `source_update_runner_run`: launch, cleanup, start gate and
//! adoption planning, against a fake launchd and temporary directories.
//! Wired from the run module with `#[path]`.

use super::*;
use crate::managers::source_update_runner::{
    parse_status, proxy_env_from, read_status_bounded, run_id_in_status, write_status_file,
    JobProbe, RunPhase, RunStatus, StatusMethod, INTERRUPTED_AFTER_SECS, MAX_STATUS_BYTES,
};
use std::collections::HashMap;
use std::fs;
use std::sync::Mutex;

use crate::managers::source_update_auto::consent::sweep_stale_markers;
use crate::managers::source_update_pending::PENDING_FILE_NAME;

const RUN: &str = "0123456789abcdef";
const FULL: &str = "0123456789abcdef0123456789abcdef01234567";
const NOW: u64 = 1_800_000_000;

struct FakeJobControl {
    state: Mutex<JobProbe>,
    bootstraps: Mutex<Vec<PathBuf>>,
    bootouts: Mutex<Vec<String>>,
    fail_bootstrap: bool,
}

impl FakeJobControl {
    fn new(state: JobProbe) -> Self {
        Self {
            state: Mutex::new(state),
            bootstraps: Mutex::new(Vec::new()),
            bootouts: Mutex::new(Vec::new()),
            fail_bootstrap: false,
        }
    }

    fn failing() -> Self {
        Self {
            fail_bootstrap: true,
            ..Self::new(JobProbe::Absent)
        }
    }
}

impl JobControl for FakeJobControl {
    fn probe(&self, _label: &str) -> JobProbe {
        *self.state.lock().unwrap()
    }

    fn bootstrap(&self, plist: &Path) -> Result<(), RunnerError> {
        self.bootstraps.lock().unwrap().push(plist.to_path_buf());
        if self.fail_bootstrap {
            return Err(RunnerError::Launchctl {
                operation: "bootstrap",
                code: Some(5),
            });
        }
        Ok(())
    }

    fn bootout(&self, label: &str) -> Result<(), RunnerError> {
        self.bootouts.lock().unwrap().push(label.to_string());
        Ok(())
    }
}

/// A checkout with a script, an app-data directory, and the other paths a launch needs.
struct Fixture {
    _root: tempfile::TempDir,
    checkout: PathBuf,
    app_data: PathBuf,
    home: PathBuf,
    tmpdir: PathBuf,
    lock_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().to_path_buf();
        let checkout = base.join("checkout");
        fs::create_dir_all(checkout.join("scripts")).unwrap();
        fs::write(checkout.join("scripts/update-macos.sh"), "#!/bin/bash\n").unwrap();
        let app_data = base.join("app");
        fs::create_dir_all(&app_data).unwrap();
        let home = base.join("home");
        fs::create_dir_all(&home).unwrap();
        Self {
            _root: root,
            checkout,
            app_data,
            home,
            tmpdir: base.clone(),
            lock_dir: base.join("vox-update.lock"),
        }
    }

    fn inputs<'a>(&'a self, run_id: &'a str, proxy: &'a [(String, String)]) -> LaunchInputs<'a> {
        LaunchInputs {
            run_id,
            app_data: &self.app_data,
            source_dir: &self.checkout,
            target_full: FULL,
            home: &self.home,
            tmpdir: &self.tmpdir,
            lock_dir: &self.lock_dir,
            proxy_env: proxy,
        }
    }

    fn state_dir(&self) -> PathBuf {
        update_state_dir(&self.app_data)
    }
}

fn status(phase: RunPhase, pct: u8, message: &str) -> RunStatus {
    RunStatus {
        run_id: RUN.to_string(),
        phase,
        percent: pct,
        target: "0123456".to_string(),
        target_full: FULL.to_string(),
        message: message.to_string(),
        method: StatusMethod::Exact,
        ts: NOW,
    }
}

fn live() -> JobProbe {
    JobProbe::Registered {
        pid: Some(77),
        last_exit: None,
    }
}

fn registered_dead() -> JobProbe {
    JobProbe::Registered {
        pid: None,
        last_exit: None,
    }
}

fn reconcile_input(
    status: Option<RunStatus>,
    age: u64,
    job: JobProbe,
    lock: LockState,
    published: bool,
) -> ReconcileInput<'static> {
    ReconcileInput {
        status,
        status_age_secs: age,
        job,
        lock,
        running_build_sha: FULL,
        published,
        interrupted_after_secs: INTERRUPTED_AFTER_SECS,
        pending_built: false,
    }
}

// ---- start gate -------------------------------------------------------------

#[test]
fn start_gate_table() {
    use LockState::{Free, HeldAlive, HeldDead};
    assert_eq!(start_gate(false, Free), StartGate::Proceed);
    assert_eq!(start_gate(false, HeldDead), StartGate::Proceed);
    assert_eq!(start_gate(false, HeldAlive), StartGate::Refuse);
    assert_eq!(start_gate(true, Free), StartGate::Attach);
    assert_eq!(start_gate(true, HeldAlive), StartGate::Attach);
    assert_eq!(start_gate(true, HeldDead), StartGate::Attach);
}

// ---- launch -----------------------------------------------------------------

#[test]
fn launch_writes_the_exact_plist_and_bootstraps_exactly_once() {
    let fx = Fixture::new();
    let control = FakeJobControl::new(JobProbe::Absent);
    let proxy: Vec<(String, String)> = Vec::new();
    let launched = launch(&control, &fx.inputs(RUN, &proxy)).unwrap();

    assert_eq!(launched.label, job_label(RUN));
    assert_eq!(launched.plist, plist_path(&fx.state_dir(), RUN));
    let written = fs::read_to_string(&launched.plist).unwrap();
    let expected = build_job_plist(&JobParams {
        run_id: RUN.to_string(),
        source_dir: fx.checkout.clone(),
        status_file: status_path(&fx.state_dir()),
        state_dir: fx.state_dir(),
        log_file: log_path(&fx.state_dir(), RUN),
        target_full: FULL.to_string(),
        home: fx.home.clone(),
        tmpdir: fx.tmpdir.clone(),
        lock_dir: fx.lock_dir.clone(),
        proxy_env: Vec::new(),
    })
    .unwrap();
    assert_eq!(written, expected);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&launched.plist).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    assert_eq!(*control.bootstraps.lock().unwrap(), vec![launched.plist]);
    assert!(control.bootouts.lock().unwrap().is_empty());
}

#[test]
fn launch_refuses_a_bad_run_id_without_touching_launchd() {
    let fx = Fixture::new();
    let control = FakeJobControl::new(JobProbe::Absent);
    let proxy: Vec<(String, String)> = Vec::new();
    let result = launch(&control, &fx.inputs("NOT-A-RUN-ID", &proxy));
    assert!(matches!(result, Err(RunnerError::InvalidValue("run_id"))));
    assert!(control.bootstraps.lock().unwrap().is_empty());
}

#[test]
fn launch_refuses_a_checkout_without_the_script() {
    let fx = Fixture::new();
    fs::remove_file(fx.checkout.join("scripts/update-macos.sh")).unwrap();
    let control = FakeJobControl::new(JobProbe::Absent);
    let proxy: Vec<(String, String)> = Vec::new();
    let result = launch(&control, &fx.inputs(RUN, &proxy));
    assert!(matches!(result, Err(RunnerError::ScriptMissing)));
    assert!(control.bootstraps.lock().unwrap().is_empty());
    assert!(!plist_path(&fx.state_dir(), RUN).exists());
}

#[cfg(unix)]
#[test]
fn launch_refuses_a_group_writable_state_directory() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fixture::new();
    fs::create_dir_all(fx.state_dir()).unwrap();
    fs::set_permissions(fx.state_dir(), fs::Permissions::from_mode(0o770)).unwrap();
    let control = FakeJobControl::new(JobProbe::Absent);
    let proxy: Vec<(String, String)> = Vec::new();
    let result = launch(&control, &fx.inputs(RUN, &proxy));
    assert!(matches!(result, Err(RunnerError::Io(_))));
    assert!(control.bootstraps.lock().unwrap().is_empty());
}

#[test]
fn failed_bootstrap_removes_the_plist_and_reports_the_launchctl_error() {
    let fx = Fixture::new();
    let control = FakeJobControl::failing();
    let proxy: Vec<(String, String)> = Vec::new();
    let result = launch(&control, &fx.inputs(RUN, &proxy));
    assert!(matches!(
        result,
        Err(RunnerError::Launchctl {
            operation: "bootstrap",
            code: Some(5)
        })
    ));
    assert_eq!(control.bootstraps.lock().unwrap().len(), 1);
    assert!(!plist_path(&fx.state_dir(), RUN).exists());
    assert!(control.bootouts.lock().unwrap().is_empty());
}

#[test]
fn launch_rotates_old_runs_and_keeps_the_newest_three() {
    let fx = Fixture::new();
    let state = fx.state_dir();
    fs::create_dir_all(&state).unwrap();
    let old_ids = [
        "1111111111111111",
        "2222222222222222",
        "3333333333333333",
        "4444444444444444",
    ];
    for id in old_ids {
        fs::write(log_path(&state, id), "old").unwrap();
    }
    let control = FakeJobControl::new(JobProbe::Absent);
    let proxy: Vec<(String, String)> = Vec::new();
    launch(&control, &fx.inputs(RUN, &proxy)).unwrap();
    let remaining = old_ids
        .iter()
        .filter(|id| log_path(&state, id).exists())
        .count();
    assert_eq!(remaining, KEEP_RUN_LOGS - 1, "new run plus two old ones");
    assert!(plist_path(&state, RUN).exists());
}

#[test]
fn proxy_variables_are_forwarded_only_when_given() {
    let fx = Fixture::new();
    let control = FakeJobControl::new(JobProbe::Absent);
    let proxy = vec![("HTTPS_PROXY".to_string(), "http://proxy:3128".to_string())];
    let launched = launch(&control, &fx.inputs(RUN, &proxy)).unwrap();
    let written = fs::read_to_string(&launched.plist).unwrap();
    assert!(written.contains("HTTPS_PROXY"));
    assert!(!written.contains("http_proxy"));
}

#[test]
fn proxy_env_from_reads_only_allow_listed_non_empty_names() {
    let lookup = |name: &str| match name {
        "HTTPS_PROXY" => Some("http://p:1".to_string()),
        "NO_PROXY" => Some(String::new()),
        "SECRET_TOKEN" => Some("nope".to_string()),
        _ => None,
    };
    assert_eq!(
        proxy_env_from(&lookup),
        vec![("HTTPS_PROXY".to_string(), "http://p:1".to_string())]
    );
}

// ---- cleanup ----------------------------------------------------------------

#[test]
fn cleanup_boots_out_removes_the_plist_and_never_touches_the_lock() {
    let fx = Fixture::new();
    let state = fx.state_dir();
    let proxy: Vec<(String, String)> = Vec::new();
    let control = FakeJobControl::new(JobProbe::Absent);
    launch(&control, &fx.inputs(RUN, &proxy)).unwrap();
    fs::create_dir_all(&fx.lock_dir).unwrap();
    fs::write(fx.lock_dir.join("owner"), "999999-1-abc").unwrap();

    cleanup_run(&control, &state, RUN).unwrap();

    assert_eq!(*control.bootouts.lock().unwrap(), vec![job_label(RUN)]);
    assert!(!plist_path(&state, RUN).exists());
    assert!(
        fx.lock_dir.join("owner").exists(),
        "the lock is never touched"
    );
}

#[test]
fn cleanup_is_idempotent_and_refuses_a_bad_run_id() {
    let fx = Fixture::new();
    let state = fx.state_dir();
    fs::create_dir_all(&state).unwrap();
    let control = FakeJobControl::new(JobProbe::Absent);
    cleanup_run(&control, &state, RUN).unwrap();
    cleanup_run(&control, &state, RUN).unwrap();
    assert!(matches!(
        cleanup_run(&control, &state, "../../etc"),
        Err(RunnerError::InvalidValue("run_id"))
    ));
}

// ---- adoption ---------------------------------------------------------------

#[test]
fn adoption_table() {
    use LockState::{Free, HeldAlive};
    let compiling = || status(RunPhase::Compiling, 40, "building");
    let done = || status(RunPhase::Done, 100, "");
    let failed = || status(RunPhase::Failed, 40, "boom");

    // A live job is always watched, whatever its status says.
    assert_eq!(
        adopt_plan(
            &reconcile_input(Some(compiling()), 1, live(), HeldAlive, false),
            true
        ),
        AdoptPlan::Watch
    );
    assert_eq!(
        adopt_plan(
            &reconcile_input(Some(done()), 1, live(), HeldAlive, false),
            true
        ),
        AdoptPlan::Watch
    );
    // Finished, plist still present (not cleaned up yet): publish and clean up.
    assert_eq!(
        adopt_plan(
            &reconcile_input(Some(done()), 1, registered_dead(), Free, false),
            true
        ),
        AdoptPlan::Watch
    );
    assert_eq!(
        adopt_plan(
            &reconcile_input(Some(failed()), 1, JobProbe::Absent, Free, false),
            true
        ),
        AdoptPlan::Watch
    );
    // Finished and already cleaned up by an earlier instance: stay quiet.
    assert_eq!(
        adopt_plan(
            &reconcile_input(Some(done()), 1, JobProbe::Absent, Free, false),
            false
        ),
        AdoptPlan::Nothing
    );
    // Old, dead, non-terminal: interrupted, so watched to publish the failure.
    assert_eq!(
        adopt_plan(
            &reconcile_input(
                Some(compiling()),
                INTERRUPTED_AFTER_SECS,
                JobProbe::Absent,
                Free,
                false
            ),
            true
        ),
        AdoptPlan::Watch
    );
    // Fresh, dead, non-terminal: may still be starting, leave it.
    assert_eq!(
        adopt_plan(
            &reconcile_input(Some(compiling()), 5, JobProbe::Absent, Free, false),
            true
        ),
        AdoptPlan::Nothing
    );
    // No status, registered without a process, old: the stop is published first
    // (A5 fix 2), so the run is watched until its cleanup.
    assert_eq!(
        adopt_plan(
            &reconcile_input(None, INTERRUPTED_AFTER_SECS, registered_dead(), Free, false),
            true
        ),
        AdoptPlan::Watch
    );
    // No status and nothing registered: nothing to do.
    assert_eq!(
        adopt_plan(
            &reconcile_input(None, 0, JobProbe::Absent, Free, false),
            false
        ),
        AdoptPlan::Nothing
    );
    // Finished but the lock is held alive: watch (the poller waits for the lock).
    assert_eq!(
        adopt_plan(
            &reconcile_input(Some(done()), 1, registered_dead(), HeldAlive, false),
            true
        ),
        AdoptPlan::Watch
    );
}

// ---- status bytes -------------------------------------------------------------

#[test]
fn written_status_file_round_trips_through_the_parser() {
    let fx = Fixture::new();
    let state = fx.state_dir();
    fs::create_dir_all(&state).unwrap();
    let path = status_path(&state);
    let interrupted = RunStatus {
        phase: RunPhase::Failed,
        message: "interrupted".to_string(),
        ..status(RunPhase::Compiling, 40, "building")
    };
    write_status_file(&path, &interrupted).unwrap();
    let bytes = read_status_bounded(&path).unwrap().unwrap();
    let parsed = parse_status(&bytes, RUN, None, NOW).unwrap();
    assert_eq!(parsed, interrupted);
    assert_eq!(run_id_in_status(&bytes).as_deref(), Some(RUN));
}

#[test]
fn bounded_read_reports_oversize_as_longer_than_the_cap() {
    let fx = Fixture::new();
    let state = fx.state_dir();
    fs::create_dir_all(&state).unwrap();
    let path = status_path(&state);
    fs::write(&path, vec![b' '; MAX_STATUS_BYTES * 3]).unwrap();
    let bytes = read_status_bounded(&path).unwrap().unwrap();
    assert_eq!(bytes.len(), MAX_STATUS_BYTES + 1);
    assert!(read_status_bounded(&state.join("missing.json"))
        .unwrap()
        .is_none());
}

#[test]
fn run_id_in_status_needs_a_valid_run_id_and_nothing_else() {
    assert_eq!(
        run_id_in_status(br#"{"run_id":"0123456789abcdef"}"#).as_deref(),
        Some(RUN)
    );
    assert_eq!(run_id_in_status(br#"{"run_id":"ABCDEF"}"#), None);
    assert_eq!(run_id_in_status(b"not json"), None);
    assert_eq!(run_id_in_status(br#"{"run_id":5}"#), None);
}

#[test]
fn resting_build_cleanup_boots_out_its_job_and_removes_its_plist_only() {
    // A finished build job (exit 0, no pid) is still registered and its plist is on disk.
    let fx = Fixture::new();
    let state = fx.state_dir();
    let proxy: Vec<(String, String)> = Vec::new();
    let control = FakeJobControl::new(JobProbe::Unknown);
    launch(&control, &fx.inputs(RUN, &proxy)).unwrap();
    let marker = state.join(format!("run-{RUN}.auto"));
    fs::write(&marker, b"auto\n").unwrap();

    cleanup_run(&control, &state, RUN).unwrap();

    assert_eq!(*control.bootouts.lock().unwrap(), vec![job_label(RUN)]);
    assert!(!plist_path(&state, RUN).exists());
    assert!(
        marker.exists(),
        "the build marker is removed with the pending record, not here"
    );
}

// F4: startup self-heal of finished, unreferenced update jobs.
const FINISHED: &str = "1111111111111111";
const LIVE_PID: &str = "2222222222222222";
const UNKNOWN: &str = "3333333333333333";
const PENDING_ID: &str = "4444444444444444";
const LINKED: &str = "5555555555555555";

/// Fake launchd with one probe per label; every other label is absent.
struct ProbeMap {
    probes: HashMap<String, JobProbe>,
    bootouts: Mutex<Vec<String>>,
}

impl ProbeMap {
    fn with(probes: &[(&str, JobProbe)]) -> Self {
        Self {
            probes: probes
                .iter()
                .map(|(id, probe)| (job_label(id), *probe))
                .collect(),
            bootouts: Mutex::new(Vec::new()),
        }
    }
}

impl JobControl for ProbeMap {
    fn probe(&self, label: &str) -> JobProbe {
        self.probes.get(label).copied().unwrap_or(JobProbe::Absent)
    }

    fn bootstrap(&self, _plist: &Path) -> Result<(), RunnerError> {
        Ok(())
    }

    fn bootout(&self, label: &str) -> Result<(), RunnerError> {
        self.bootouts.lock().unwrap().push(label.to_string());
        Ok(())
    }
}

fn finished_without_pid() -> JobProbe {
    JobProbe::Registered {
        pid: None,
        last_exit: Some(0),
    }
}

fn write_plist_for(state: &Path, run_id: &str) {
    fs::write(plist_path(state, run_id), b"<plist/>").unwrap();
}

#[test]
fn leftover_finished_job_is_booted_out_and_only_its_plist_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    write_plist_for(state, FINISHED);
    let log = log_path(state, FINISHED);
    fs::write(&log, b"log").unwrap();
    let marker = state.join(format!("run-{FINISHED}.auto"));
    fs::write(&marker, b"auto\n").unwrap();
    let status = status_path(state);
    fs::write(&status, b"status").unwrap();
    let pending = state.join(PENDING_FILE_NAME);
    fs::write(&pending, b"pending").unwrap();
    let jobs = ProbeMap::with(&[(FINISHED, finished_without_pid())]);

    assert_eq!(
        heal_finished_jobs(state, &[], &jobs, old_enough(state, FINISHED)),
        1
    );

    assert_eq!(*jobs.bootouts.lock().unwrap(), vec![job_label(FINISHED)]);
    assert!(!plist_path(state, FINISHED).exists());
    assert_eq!(fs::read(&log).unwrap(), b"log");
    assert_eq!(fs::read(&status).unwrap(), b"status");
    assert_eq!(fs::read(&pending).unwrap(), b"pending");
    assert!(marker.exists(), "the heal never removes markers itself");
    assert_eq!(
        sweep_stale_markers(state, &[]),
        1,
        "the sweep then removes it"
    );
    assert!(!marker.exists());
}

#[test]
fn running_unknown_and_live_listed_jobs_are_never_healed() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    for id in [LIVE_PID, UNKNOWN, PENDING_ID] {
        write_plist_for(state, id);
    }
    let jobs = ProbeMap::with(&[
        (
            LIVE_PID,
            JobProbe::Registered {
                pid: Some(4242),
                last_exit: None,
            },
        ),
        (UNKNOWN, JobProbe::Unknown),
        (PENDING_ID, finished_without_pid()),
    ]);

    assert_eq!(
        heal_finished_jobs(
            state,
            &[PENDING_ID.to_string()],
            &jobs,
            old_enough(state, PENDING_ID)
        ),
        0
    );

    assert!(jobs.bootouts.lock().unwrap().is_empty());
    for id in [LIVE_PID, UNKNOWN, PENDING_ID] {
        assert!(plist_path(state, id).exists(), "{id} keeps its plist");
    }
}

#[test]
fn symlinked_plist_is_never_followed_or_removed() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let elsewhere = dir.path().join("elsewhere.plist");
    fs::write(&elsewhere, b"x").unwrap();
    std::os::unix::fs::symlink(&elsewhere, plist_path(state, LINKED)).unwrap();
    let jobs = ProbeMap::with(&[(LINKED, finished_without_pid())]);

    let now = old_enough(state, LINKED);
    assert_eq!(heal_finished_jobs(state, &[], &jobs, now), 0);

    assert!(fs::symlink_metadata(plist_path(state, LINKED))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read(&elsewhere).unwrap(), b"x");
    assert!(jobs.bootouts.lock().unwrap().is_empty());
}

#[test]
fn only_regular_run_plists_with_a_valid_id_are_considered() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let names = [
        "run-XYZ.plist",
        "run-0123456789abcdef.plist.bak",
        "run-0123456789ABCDEF.plist",
        "other.plist",
        "run-0123456789abcdef.log",
    ];
    for name in names {
        fs::write(state.join(name), b"x").unwrap();
    }
    // Every label would probe as Absent, so only the name rule keeps these files.
    let jobs = ProbeMap::with(&[]);

    assert_eq!(heal_finished_jobs(state, &[], &jobs, u64::MAX), 0);

    assert!(jobs.bootouts.lock().unwrap().is_empty());
    for name in names {
        assert!(state.join(name).exists(), "{name} is not a run plist");
    }
}

// F4b: a plist younger than HEAL_MIN_AGE_SECS is never healed, and an unreadable or future mtime
// is never healed either. The clock is passed in, so the ages are exact.
fn mtime_secs(path: &Path) -> u64 {
    fs::metadata(path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// The clock that makes the plist of `run_id` exactly old enough to heal.
fn old_enough(state: &Path, run_id: &str) -> u64 {
    mtime_secs(&plist_path(state, run_id)) + HEAL_MIN_AGE_SECS
}

#[test]
fn young_plist_is_not_healed_even_when_its_job_looks_finished() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    write_plist_for(state, FINISHED);
    let jobs = ProbeMap::with(&[(FINISHED, finished_without_pid())]);
    let young = mtime_secs(&plist_path(state, FINISHED)) + 30;

    assert_eq!(heal_finished_jobs(state, &[], &jobs, young), 0);

    assert!(jobs.bootouts.lock().unwrap().is_empty());
    assert!(plist_path(state, FINISHED).exists());
}

#[test]
fn plist_just_under_the_minimum_age_is_not_healed_and_at_the_minimum_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    write_plist_for(state, FINISHED);
    let jobs = ProbeMap::with(&[(FINISHED, finished_without_pid())]);
    let mtime = mtime_secs(&plist_path(state, FINISHED));

    assert_eq!(
        heal_finished_jobs(state, &[], &jobs, mtime + HEAL_MIN_AGE_SECS - 1),
        0
    );
    assert!(plist_path(state, FINISHED).exists());
    assert_eq!(
        heal_finished_jobs(state, &[], &jobs, mtime + HEAL_MIN_AGE_SECS),
        1
    );
    assert!(!plist_path(state, FINISHED).exists());
}

#[test]
fn plist_with_a_future_mtime_is_not_healed() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    write_plist_for(state, FINISHED);
    let jobs = ProbeMap::with(&[(FINISHED, finished_without_pid())]);
    // The clock is behind the mtime: the age cannot be trusted, so nothing is healed.
    let behind = mtime_secs(&plist_path(state, FINISHED)) - 10;

    assert_eq!(heal_finished_jobs(state, &[], &jobs, behind), 0);

    assert!(jobs.bootouts.lock().unwrap().is_empty());
    assert!(plist_path(state, FINISHED).exists());
}

#[test]
fn repeated_heals_of_a_young_plist_do_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    write_plist_for(state, FINISHED);
    let jobs = ProbeMap::with(&[(FINISHED, finished_without_pid())]);
    let young = mtime_secs(&plist_path(state, FINISHED)) + 30;

    for _ in 0..3 {
        assert_eq!(heal_finished_jobs(state, &[], &jobs, young), 0);
    }

    assert!(jobs.bootouts.lock().unwrap().is_empty());
    assert!(plist_path(state, FINISHED).exists());
}
