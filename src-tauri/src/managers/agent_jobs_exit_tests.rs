//! App-exit coordinator tests (S2c; DESIGN R4-2 (a)-(f) and R5-1).
//! Red by design: ExitCoordinator and the ExitTarget impl for JobManager are
//! todo!() stubs in agent_jobs_exit.rs. Integration cases use real JobManager
//! jobs with `sh` fake CLIs in temp dirs; every spawned process group is
//! killed by a drop guard. Do not edit these tests to make code pass.
#![cfg(unix)]

use super::exit::ExitTarget;
use super::{
    CleanupResult, ExitConfig, ExitCoordinator, ExitDecision, JobError, JobManager, JobObserver,
    JobSnapshot, JobSpec,
};
use crate::managers::converse_confirm::{FileId, JobLaunch, LaunchConfig};
use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

/// Locks a test mutex, ignoring poisoning so a failing test reports cleanly.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Kills recorded pids and process groups when dropped, so a failing test never
/// leaves fake CLI processes running. A stale id at worst signals nothing.
struct GroupGuard {
    pids: Vec<i32>,
    pgids: Vec<i32>,
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        for pgid in &self.pgids {
            // SAFETY: SIGKILL to a process group id recorded by this test (the
            // leader pid of a job spawned with process_group(0)); ESRCH is harmless.
            unsafe {
                libc::killpg(*pgid, libc::SIGKILL);
            }
        }
        for pid in &self.pids {
            // SAFETY: SIGKILL to a pid recorded by this test; ESRCH is harmless.
            unsafe {
                libc::kill(*pid, libc::SIGKILL);
            }
        }
    }
}

/// Cancels whatever job the manager still runs when dropped, so a panicking test
/// never leaves a fake CLI group alive (cancel is a no-op when idle).
struct CancelOnDrop(Arc<JobManager>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// Observer that ignores everything (the exit tests only care about cleanup).
struct NoopObserver;

impl JobObserver for NoopObserver {
    fn on_progress(&self, _snap: &JobSnapshot) {}
    fn on_finish(&self, _snap: &JobSnapshot) {}
}

fn file_id(path: &Path) -> FileId {
    let meta = std::fs::metadata(path).expect("stat fake binary");
    FileId {
        dev: meta.dev(),
        ino: meta.ino(),
        len: meta.len(),
        mtime_secs: meta.mtime(),
    }
}

/// Writes an executable `sh` script named `name` into `dir` and returns its path.
fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake cli");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

/// A fake CLI that records its pid and starts a sleeping grandchild, so tests can
/// check that the whole process group dies. Returns (binary, leader, grandchild).
fn group_script(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let leader = dir.join("leader.pid");
    let grandchild = dir.join("grandchild.pid");
    let bin = write_script(
        dir,
        "group-cli",
        &format!(
            "echo $$ > '{}'; sleep 60 & echo $! > '{}'; wait",
            leader.display(),
            grandchild.display()
        ),
    );
    (bin, leader, grandchild)
}

/// A fake CLI that only records its pid and then sleeps. Returns (binary, leader).
fn sleeper_script(dir: &Path) -> (PathBuf, PathBuf) {
    let leader = dir.join("sleeper.pid");
    let bin = write_script(
        dir,
        "sleeper-cli",
        &format!("echo $$ > '{}'; sleep 60", leader.display()),
    );
    (bin, leader)
}

fn pinned_dir(dir: &Path) -> OwnedFd {
    std::fs::File::open(dir).expect("open project dir").into()
}

fn launch(dir: &Path, binary: &Path, task: &str, timeout: Duration) -> JobLaunch {
    JobLaunch {
        project: "proj".to_string(),
        dir: pinned_dir(dir),
        task: task.to_string(),
        config: LaunchConfig {
            binary: binary.to_path_buf(),
            binary_id: file_id(binary),
            prompt_flag: "-p".to_string(),
            timeout,
        },
    }
}

/// A job spec with a 12 s job timeout (script starts are slow under load).
fn spec(dir: &Path, binary: &Path, task: &str) -> JobSpec {
    JobSpec {
        launch: launch(dir, binary, task, Duration::from_secs(12)),
        progress_interval: Duration::from_secs(90),
    }
}

/// Starts a job and panics with a clear message if it is refused.
fn must_start(mgr: &Arc<JobManager>, spec: JobSpec, obs: Arc<dyn JobObserver>) -> u64 {
    match mgr.start(spec, obs) {
        Ok(id) => id,
        Err(e) => panic!("start must succeed for this scenario, got {e:?}"),
    }
}

/// Polls `cond` until true or `limit` elapses. Returns the final value.
fn wait_until(limit: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    cond()
}

fn pid_alive(pid: i32) -> bool {
    // SAFETY: kill with signal 0 only probes for the existence of the pid.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn read_pid_file(path: &Path) -> i32 {
    let mut pid = None;
    wait_until(Duration::from_secs(15), || {
        pid = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok());
        pid.is_some()
    });
    match pid {
        Some(p) => p,
        None => panic!("pid file {} must hold a pid", path.display()),
    }
}

/// Exit budgets with short injected durations: deadline 300 ms, hard cap 300 ms.
fn fast_cfg() -> ExitConfig {
    ExitConfig {
        deadline: Duration::from_millis(300),
        term_grace: Duration::from_millis(100),
        drain: Duration::from_millis(100),
        hard_cap: Duration::from_millis(300),
    }
}

/// Exit budgets for real jobs: a long deadline (never forced in these tests).
fn real_cfg() -> ExitConfig {
    ExitConfig {
        deadline: Duration::from_secs(5),
        term_grace: Duration::from_millis(100),
        drain: Duration::from_millis(100),
        hard_cap: Duration::from_secs(15),
    }
}

/// Shared ordering log for the fake target and the finish closures.
type Log = Arc<Mutex<Vec<&'static str>>>;

fn new_log() -> Log {
    Arc::new(Mutex::new(Vec::new()))
}

fn events(log: &Log) -> Vec<&'static str> {
    lock(log).clone()
}

// ---- scripted fake target --------------------------------------------------

/// Scripted ExitTarget for the timing logic. `confirm_delay` is measured from the
/// moment request_terminate is called; `None` means the target never confirms.
struct FakeTarget {
    /// Whether a job is still running (cleared once confirmed).
    active: AtomicBool,
    /// Delay after request_terminate before cleanup is confirmed.
    confirm_delay: Option<Duration>,
    /// Ordering log shared with the finish closures.
    log: Log,
    /// When request_terminate was called.
    terminate_at: Mutex<Option<Instant>>,
    /// When request_force_kill was called.
    force_at: Mutex<Option<Instant>>,
    /// Thread that called close_admission.
    close_thread: Mutex<Option<ThreadId>>,
    close_calls: AtomicUsize,
    terminate_calls: AtomicUsize,
    force_calls: AtomicUsize,
    /// Grace value passed to the last request_terminate.
    last_grace: Mutex<Option<Duration>>,
}

impl FakeTarget {
    fn new(active: bool, confirm_delay: Option<Duration>, log: Log) -> Arc<Self> {
        Arc::new(FakeTarget {
            active: AtomicBool::new(active),
            confirm_delay,
            log,
            terminate_at: Mutex::new(None),
            force_at: Mutex::new(None),
            close_thread: Mutex::new(None),
            close_calls: AtomicUsize::new(0),
            terminate_calls: AtomicUsize::new(0),
            force_calls: AtomicUsize::new(0),
            last_grace: Mutex::new(None),
        })
    }
}

impl ExitTarget for FakeTarget {
    fn close_admission(&self) {
        self.close_calls.fetch_add(1, SeqCst);
        *lock(&self.close_thread) = Some(std::thread::current().id());
        lock(&self.log).push("close");
    }

    fn has_active_job(&self) -> bool {
        self.active.load(SeqCst)
    }

    fn request_terminate(&self, grace: Duration) {
        self.terminate_calls.fetch_add(1, SeqCst);
        *lock(&self.last_grace) = Some(grace);
        *lock(&self.terminate_at) = Some(Instant::now());
        lock(&self.log).push("terminate");
    }

    fn request_force_kill(&self) {
        self.force_calls.fetch_add(1, SeqCst);
        *lock(&self.force_at) = Some(Instant::now());
        lock(&self.log).push("force_kill");
    }

    fn cleanup_confirmed(&self) -> bool {
        if !self.active.load(SeqCst) {
            return true;
        }
        let terminated = *lock(&self.terminate_at);
        match (terminated, self.confirm_delay) {
            (Some(t), Some(delay)) if Instant::now() >= t + delay => {
                self.active.store(false, SeqCst);
                true
            }
            _ => false,
        }
    }
}

/// What one finish call observed.
#[derive(Clone, Copy)]
struct FinishRecord {
    /// True for CleanupResult::Confirmed, false for Unconfirmed.
    confirmed: bool,
    /// When finish ran.
    at: Instant,
    /// Whether the target reported cleanup confirmed at the moment finish ran.
    target_confirmed_at_finish: bool,
}

type Finishes = Arc<Mutex<Vec<FinishRecord>>>;

fn new_finishes() -> Finishes {
    Arc::new(Mutex::new(Vec::new()))
}

/// A finish closure that records its call (and what the target said at that time)
/// into `finishes` and the ordering log.
fn recording_finish(
    finishes: Finishes,
    log: Log,
    target: Arc<dyn ExitTarget>,
) -> impl FnOnce(CleanupResult) + Send + 'static {
    move |result| {
        let confirmed = matches!(result, CleanupResult::Confirmed);
        let record = FinishRecord {
            confirmed,
            at: Instant::now(),
            target_confirmed_at_finish: target.cleanup_confirmed(),
        };
        lock(&log).push("finish");
        lock(&finishes).push(record);
    }
}

fn finish_count(finishes: &Finishes) -> usize {
    lock(finishes).len()
}

fn wait_for_finish(finishes: &Finishes, limit: Duration) -> bool {
    wait_until(limit, || finish_count(finishes) >= 1)
}

/// A prevent closure that counts its calls.
fn counting_prevent(count: Arc<AtomicUsize>) -> impl FnOnce() + Send + 'static {
    move || {
        count.fetch_add(1, SeqCst);
    }
}

/// Slack for timing comparisons that start from separate clocks.
const TIMING_SLACK: Duration = Duration::from_millis(50);
// ---- timing logic against the scripted target (no processes) ---------------

/// Case 2 (fake): no active job => Allow, no prevent, no finish; admission closed first.
#[test]
fn idle_target_native_request_is_allow_without_prevent_or_finish() {
    let log = new_log();
    let target = FakeTarget::new(false, None, Arc::clone(&log));
    let dyn_target: Arc<dyn ExitTarget> = target.clone();
    let finishes = new_finishes();
    let prevents = Arc::new(AtomicUsize::new(0));
    let coord = ExitCoordinator::new();
    let decision = coord.on_exit_requested_with(
        dyn_target.clone(),
        counting_prevent(Arc::clone(&prevents)),
        recording_finish(Arc::clone(&finishes), Arc::clone(&log), dyn_target),
        &fast_cfg(),
    );
    assert!(matches!(decision, ExitDecision::Allow));
    assert_eq!(
        prevents.load(SeqCst),
        0,
        "idle native exit must not prevent"
    );
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        finish_count(&finishes),
        0,
        "native idle path must not finish"
    );
    assert_eq!(target.close_calls.load(SeqCst), 1, "admission closed once");
    assert_eq!(
        target.terminate_calls.load(SeqCst),
        0,
        "nothing to terminate"
    );
}

/// Case 1 (fake): active job => prevent() once, Deferred, terminate issued, finish
/// only after confirmation (the target reports confirmed at the moment finish runs).
#[test]
fn active_target_native_request_is_deferred_and_finishes_only_after_confirmation() {
    let log = new_log();
    let target = FakeTarget::new(true, Some(Duration::from_millis(200)), Arc::clone(&log));
    let dyn_target: Arc<dyn ExitTarget> = target.clone();
    let finishes = new_finishes();
    let prevents = Arc::new(AtomicUsize::new(0));
    let coord = ExitCoordinator::new();
    let decision = coord.on_exit_requested_with(
        dyn_target.clone(),
        counting_prevent(Arc::clone(&prevents)),
        recording_finish(Arc::clone(&finishes), Arc::clone(&log), dyn_target),
        &fast_cfg(),
    );
    assert!(matches!(decision, ExitDecision::Deferred));
    assert_eq!(
        prevents.load(SeqCst),
        1,
        "prevent must be called exactly once"
    );
    assert!(
        wait_for_finish(&finishes, Duration::from_secs(5)),
        "finish must run after confirmation"
    );
    let records: Vec<FinishRecord> = lock(&finishes).clone();
    assert_eq!(records.len(), 1, "finish runs exactly once");
    assert!(records[0].confirmed, "finish receives Confirmed");
    assert!(
        records[0].target_confirmed_at_finish,
        "finish must not run before the target confirmed the group is gone"
    );
    assert_eq!(
        target.terminate_calls.load(SeqCst),
        1,
        "one terminate request"
    );
}

/// Case 4 (fake): confirmation delayed beyond 1 s. finish is uncalled until the
/// confirmation, runs exactly once, and force kill is issued only after the deadline.
#[test]
fn confirmation_delayed_beyond_one_second_defers_finish_until_confirmed() {
    let log = new_log();
    let target = FakeTarget::new(true, Some(Duration::from_millis(1200)), Arc::clone(&log));
    let dyn_target: Arc<dyn ExitTarget> = target.clone();
    let finishes = new_finishes();
    // hard_cap is long enough that the last resort cannot fire before the
    // 1200 ms confirmation (R4-2(e): deferral is retained until confirmed).
    let cfg = ExitConfig {
        deadline: Duration::from_millis(300),
        term_grace: Duration::from_millis(100),
        drain: Duration::from_millis(100),
        hard_cap: Duration::from_secs(5),
    };
    let coord = ExitCoordinator::new();
    let decision = coord.on_exit_requested_with(
        dyn_target.clone(),
        || {},
        recording_finish(Arc::clone(&finishes), Arc::clone(&log), dyn_target),
        &cfg,
    );
    assert!(matches!(decision, ExitDecision::Deferred));
    std::thread::sleep(Duration::from_millis(900));
    assert_eq!(
        finish_count(&finishes),
        0,
        "finish must stay uncalled while confirmation is pending"
    );
    assert!(
        wait_for_finish(&finishes, Duration::from_secs(5)),
        "finish must run once confirmed"
    );
    assert_eq!(finish_count(&finishes), 1, "finish runs exactly once");
    assert!(lock(&finishes)[0].confirmed);
    let terminated = lock(&target.terminate_at).expect("terminate was requested");
    let forced = lock(&target.force_at).expect("force kill issued after the deadline");
    assert!(
        forced + TIMING_SLACK >= terminated + cfg.deadline,
        "force kill must not be issued before the deadline"
    );
    assert_eq!(target.force_calls.load(SeqCst), 1, "force kill issued once");
}

/// Case 4b (fake): confirmed before the deadline => no force kill at all.
#[test]
fn confirmation_before_deadline_never_force_kills() {
    let log = new_log();
    let target = FakeTarget::new(true, Some(Duration::from_millis(50)), Arc::clone(&log));
    let dyn_target: Arc<dyn ExitTarget> = target.clone();
    let finishes = new_finishes();
    let coord = ExitCoordinator::new();
    let decision = coord.on_exit_requested_with(
        dyn_target.clone(),
        || {},
        recording_finish(Arc::clone(&finishes), Arc::clone(&log), dyn_target),
        &fast_cfg(),
    );
    assert!(matches!(decision, ExitDecision::Deferred));
    assert!(wait_for_finish(&finishes, Duration::from_secs(5)));
    assert_eq!(
        target.force_calls.load(SeqCst),
        0,
        "no force kill when confirmed in time"
    );
    assert_eq!(target.terminate_calls.load(SeqCst), 1);
    assert!(lock(&finishes)[0].confirmed);
}

/// Case 5 (fake): a target that never confirms => finish only after
/// deadline + hard_cap, with Unconfirmed, never earlier, exactly once.
#[test]
fn never_confirmed_target_finishes_unconfirmed_only_after_deadline_plus_hard_cap() {
    let log = new_log();
    let target = FakeTarget::new(true, None, Arc::clone(&log));
    let dyn_target: Arc<dyn ExitTarget> = target.clone();
    let finishes = new_finishes();
    let cfg = fast_cfg();
    let coord = ExitCoordinator::new();
    let decision = coord.on_exit_requested_with(
        dyn_target.clone(),
        || {},
        recording_finish(Arc::clone(&finishes), Arc::clone(&log), dyn_target),
        &cfg,
    );
    assert!(matches!(decision, ExitDecision::Deferred));
    assert!(
        wait_for_finish(&finishes, Duration::from_secs(5)),
        "finish must run after the hard cap"
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(finish_count(&finishes), 1, "finish runs exactly once");
    let record = lock(&finishes)[0];
    assert!(
        !record.confirmed,
        "never-confirmed cleanup reports Unconfirmed"
    );
    assert!(!record.target_confirmed_at_finish);
    let terminated = lock(&target.terminate_at).expect("terminate was requested");
    assert!(
        record.at + TIMING_SLACK >= terminated + cfg.deadline + cfg.hard_cap,
        "finish must not run before deadline + hard_cap"
    );
    assert!(
        target.force_calls.load(SeqCst) >= 1,
        "force kill issued at the deadline"
    );
}

/// Case 6 (fake): repeated native requests during Cleaning => prevent() each time,
/// exactly one terminate (one cleanup thread), exactly one finish.
#[test]
fn repeated_native_requests_while_cleaning_prevent_each_and_finish_once() {
    let log = new_log();
    let target = FakeTarget::new(true, Some(Duration::from_millis(300)), Arc::clone(&log));
    let dyn_target: Arc<dyn ExitTarget> = target.clone();
    let finishes = new_finishes();
    let prevents = Arc::new(AtomicUsize::new(0));
    let coord = ExitCoordinator::new();
    for _ in 0..3 {
        let decision = coord.on_exit_requested_with(
            dyn_target.clone(),
            counting_prevent(Arc::clone(&prevents)),
            recording_finish(Arc::clone(&finishes), Arc::clone(&log), dyn_target.clone()),
            &fast_cfg(),
        );
        assert!(matches!(decision, ExitDecision::Deferred));
    }
    assert_eq!(
        prevents.load(SeqCst),
        3,
        "prevent() is called for every request"
    );
    assert!(wait_for_finish(&finishes, Duration::from_secs(5)));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(finish_count(&finishes), 1, "exactly one finish");
    assert_eq!(
        target.terminate_calls.load(SeqCst),
        1,
        "exactly one cleanup thread"
    );
    assert_eq!(target.close_calls.load(SeqCst), 1, "admission closed once");
}

/// Adversarial (fake): many threads request exit at the same instant while a job
/// is active => one Deferred path, one terminate, exactly one finish.
#[test]
fn concurrent_native_requests_produce_one_cleanup_and_one_finish() {
    let log = new_log();
    let target = FakeTarget::new(true, Some(Duration::from_millis(200)), Arc::clone(&log));
    let dyn_target: Arc<dyn ExitTarget> = target.clone();
    let finishes = new_finishes();
    let coord = ExitCoordinator::new();
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let mut handles = Vec::new();
    for _ in 0..8 {
        let coord = Arc::clone(&coord);
        let dyn_target = dyn_target.clone();
        let finishes = Arc::clone(&finishes);
        let log = Arc::clone(&log);
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            coord.on_exit_requested_with(
                dyn_target.clone(),
                || {},
                recording_finish(finishes, log, dyn_target),
                &fast_cfg(),
            )
        }));
    }
    let mut deferred = 0;
    for handle in handles {
        let decision = handle.join().expect("exit request thread must not panic");
        if matches!(decision, ExitDecision::Deferred) {
            deferred += 1;
        }
    }
    assert_eq!(
        deferred, 8,
        "every request during an active job is Deferred"
    );
    assert!(wait_for_finish(&finishes, Duration::from_secs(5)));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        finish_count(&finishes),
        1,
        "exactly one finish under concurrency"
    );
    assert_eq!(
        target.terminate_calls.load(SeqCst),
        1,
        "exactly one cleanup thread"
    );
}

/// Case 7 (fake): after finish (Phase Cleaned) a re-entrant native request is Allow
/// with no prevent, which is what app.exit's own ExitRequested needs.
#[test]
fn reentrant_native_request_after_finish_is_allow() {
    let log = new_log();
    let target = FakeTarget::new(true, Some(Duration::from_millis(100)), Arc::clone(&log));
    let dyn_target: Arc<dyn ExitTarget> = target.clone();
    let finishes = new_finishes();
    let prevents = Arc::new(AtomicUsize::new(0));
    let coord = ExitCoordinator::new();
    let first = coord.on_exit_requested_with(
        dyn_target.clone(),
        counting_prevent(Arc::clone(&prevents)),
        recording_finish(Arc::clone(&finishes), Arc::clone(&log), dyn_target.clone()),
        &fast_cfg(),
    );
    assert!(matches!(first, ExitDecision::Deferred));
    assert!(wait_for_finish(&finishes, Duration::from_secs(5)));
    let again = coord.on_exit_requested_with(
        dyn_target.clone(),
        counting_prevent(Arc::clone(&prevents)),
        recording_finish(Arc::clone(&finishes), Arc::clone(&log), dyn_target),
        &fast_cfg(),
    );
    assert!(
        matches!(again, ExitDecision::Allow),
        "re-entrant exit after finish is Allow"
    );
    assert_eq!(
        prevents.load(SeqCst),
        1,
        "re-entrant exit must not prevent again"
    );
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        finish_count(&finishes),
        1,
        "re-entrant exit must not finish again"
    );
}

/// Case 9 (fake, R5-1): idle tray request_exit => finish gets Confirmed exactly
/// once across three repeated tray requests.
#[test]
fn idle_tray_request_finishes_confirmed_exactly_once_across_repeats() {
    let log = new_log();
    let target = FakeTarget::new(false, None, Arc::clone(&log));
    let dyn_target: Arc<dyn ExitTarget> = target.clone();
    let finishes = new_finishes();
    let coord = ExitCoordinator::new();
    for _ in 0..3 {
        coord.request_exit_with(
            dyn_target.clone(),
            recording_finish(Arc::clone(&finishes), Arc::clone(&log), dyn_target.clone()),
            &fast_cfg(),
        );
    }
    assert!(wait_for_finish(&finishes, Duration::from_secs(5)));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        finish_count(&finishes),
        1,
        "tray finish is issued exactly once"
    );
    assert!(
        lock(&finishes)[0].confirmed,
        "idle tray finish receives Confirmed"
    );
    assert_eq!(
        target.terminate_calls.load(SeqCst),
        0,
        "idle tray quit terminates nothing"
    );
}

/// Case 9b (fake, R5-1): tray request_exit with a running job => finish only after
/// the target confirmed, exactly once across repeated tray requests.
#[test]
fn active_tray_request_finishes_only_after_cleanup_exactly_once() {
    let log = new_log();
    let target = FakeTarget::new(true, Some(Duration::from_millis(250)), Arc::clone(&log));
    let dyn_target: Arc<dyn ExitTarget> = target.clone();
    let finishes = new_finishes();
    let coord = ExitCoordinator::new();
    for _ in 0..3 {
        coord.request_exit_with(
            dyn_target.clone(),
            recording_finish(Arc::clone(&finishes), Arc::clone(&log), dyn_target.clone()),
            &fast_cfg(),
        );
    }
    assert_eq!(
        finish_count(&finishes),
        0,
        "tray finish must wait for the job"
    );
    assert!(wait_for_finish(&finishes, Duration::from_secs(5)));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        finish_count(&finishes),
        1,
        "tray finish is issued exactly once"
    );
    let record = lock(&finishes)[0];
    assert!(record.confirmed);
    assert!(
        record.target_confirmed_at_finish,
        "finish only after cleanup"
    );
    assert_eq!(target.terminate_calls.load(SeqCst), 1);
}

/// Case 10 (fake): ordering log: close_admission precedes request_terminate, and
/// request_terminate precedes request_force_kill.
#[test]
fn ordering_close_then_terminate_then_force_kill() {
    let log = new_log();
    let target = FakeTarget::new(true, None, Arc::clone(&log));
    let dyn_target: Arc<dyn ExitTarget> = target.clone();
    let finishes = new_finishes();
    let coord = ExitCoordinator::new();
    let decision = coord.on_exit_requested_with(
        dyn_target.clone(),
        || {},
        recording_finish(Arc::clone(&finishes), Arc::clone(&log), dyn_target),
        &fast_cfg(),
    );
    assert!(matches!(decision, ExitDecision::Deferred));
    assert!(wait_for_finish(&finishes, Duration::from_secs(5)));
    let seen = events(&log);
    let pos = |name: &str| seen.iter().position(|e| *e == name);
    let close = pos("close").expect("close_admission recorded");
    let terminate = pos("terminate").expect("request_terminate recorded");
    let force = pos("force_kill").expect("request_force_kill recorded");
    assert!(
        close < terminate,
        "close_admission must precede request_terminate: {seen:?}"
    );
    assert!(
        terminate < force,
        "request_terminate must precede request_force_kill: {seen:?}"
    );
    assert_eq!(
        seen.first().copied(),
        Some("close"),
        "close is the first call: {seen:?}"
    );
}

/// The grace passed to request_terminate is the configured term_grace.
#[test]
fn terminate_request_carries_the_configured_term_grace() {
    let log = new_log();
    let target = FakeTarget::new(true, Some(Duration::from_millis(50)), Arc::clone(&log));
    let dyn_target: Arc<dyn ExitTarget> = target.clone();
    let finishes = new_finishes();
    let cfg = fast_cfg();
    let coord = ExitCoordinator::new();
    coord.on_exit_requested_with(
        dyn_target.clone(),
        || {},
        recording_finish(Arc::clone(&finishes), Arc::clone(&log), dyn_target),
        &cfg,
    );
    assert!(wait_for_finish(&finishes, Duration::from_secs(5)));
    assert_eq!(*lock(&target.last_grace), Some(cfg.term_grace));
}

/// close_admission runs on the calling thread, before anything else (R4-2(b)).
#[test]
fn close_admission_runs_on_the_calling_thread_first() {
    let log = new_log();
    let target = FakeTarget::new(false, None, Arc::clone(&log));
    let dyn_target: Arc<dyn ExitTarget> = target.clone();
    let finishes = new_finishes();
    let coord = ExitCoordinator::new();
    coord.request_exit_with(
        dyn_target.clone(),
        recording_finish(Arc::clone(&finishes), Arc::clone(&log), dyn_target),
        &fast_cfg(),
    );
    assert_eq!(
        target.close_calls.load(SeqCst),
        1,
        "close_admission is called synchronously"
    );
    assert_eq!(
        *lock(&target.close_thread),
        Some(std::thread::current().id()),
        "close_admission must run on the calling thread"
    );
    assert_eq!(events(&log).first().copied(), Some("close"));
    assert!(wait_for_finish(&finishes, Duration::from_secs(5)));
}

// ---- integration: real JobManager + sh fake CLIs ----------------------------

/// True when `needle` occurs as a call token (not as part of a longer identifier
/// such as `request_force_kill(`).
fn contains_call_token(src: &str, needle: &str) -> bool {
    src.match_indices(needle).any(|(i, _)| {
        src[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'))
    })
}

/// Case 1 (real): native ExitRequested with a running job => Deferred, prevent
/// called, finish not called until the group is dead; the finish closure records
/// whether the group leader and its grandchild were gone at that moment.
#[test]
fn native_exit_with_running_job_defers_and_finishes_after_group_is_gone() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (bin, leader, grandchild) = group_script(tmp.path());
    let mgr = JobManager::new();
    let _cancel = CancelOnDrop(Arc::clone(&mgr));
    must_start(&mgr, spec(tmp.path(), &bin, "task"), Arc::new(NoopObserver));
    let leader_pid = read_pid_file(&leader);
    let gc_pid = read_pid_file(&grandchild);
    let _guard = GroupGuard {
        pids: vec![gc_pid],
        pgids: vec![leader_pid],
    };
    let coord = ExitCoordinator::new();
    let prevents = Arc::new(AtomicUsize::new(0));
    let leader_alive_at_finish = Arc::new(Mutex::new(None::<bool>));
    let grandchild_alive_at_finish = Arc::new(Mutex::new(None::<bool>));
    let confirmed_at_finish = Arc::new(Mutex::new(Vec::<bool>::new()));
    let (la, ga, cf) = (
        Arc::clone(&leader_alive_at_finish),
        Arc::clone(&grandchild_alive_at_finish),
        Arc::clone(&confirmed_at_finish),
    );
    let decision = coord.on_exit_requested(
        &mgr,
        counting_prevent(Arc::clone(&prevents)),
        move |result| {
            // Runs only after the owner confirmed: leader reaped, group gone.
            let gc_gone = wait_until(Duration::from_secs(2), || !pid_alive(gc_pid));
            *lock(&la) = Some(pid_alive(leader_pid));
            *lock(&ga) = Some(!gc_gone);
            lock(&cf).push(matches!(result, CleanupResult::Confirmed));
        },
        &real_cfg(),
    );
    assert!(matches!(decision, ExitDecision::Deferred));
    assert_eq!(prevents.load(SeqCst), 1, "prevent called exactly once");
    assert!(
        wait_until(Duration::from_secs(15), || !lock(&confirmed_at_finish)
            .is_empty()),
        "finish must run once the group is gone"
    );
    assert_eq!(
        *lock(&leader_alive_at_finish),
        Some(false),
        "leader gone at finish"
    );
    assert_eq!(
        *lock(&grandchild_alive_at_finish),
        Some(false),
        "grandchild gone at finish"
    );
    assert_eq!(
        *lock(&confirmed_at_finish),
        vec![true],
        "finish runs once, Confirmed"
    );
    assert!(!mgr.has_active_job(), "no active job after finish");
}

/// Case 2 (real): no active job => Allow, no prevent, no finish.
#[test]
fn native_exit_with_no_job_is_allow_without_prevent_or_finish() {
    let mgr = JobManager::new();
    let coord = ExitCoordinator::new();
    let finish_calls = Arc::new(AtomicUsize::new(0));
    let fc = Arc::clone(&finish_calls);
    let prevents = Arc::new(AtomicUsize::new(0));
    let decision = coord.on_exit_requested(
        &mgr,
        counting_prevent(Arc::clone(&prevents)),
        move |_result| {
            fc.fetch_add(1, SeqCst);
        },
        &real_cfg(),
    );
    assert!(matches!(decision, ExitDecision::Allow));
    assert_eq!(
        prevents.load(SeqCst),
        0,
        "idle native exit must not prevent"
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        finish_calls.load(SeqCst),
        0,
        "idle native exit must not finish"
    );
}

/// Case 9 (real, R5-1): idle tray request_exit => finish gets Confirmed exactly
/// once across three repeated tray requests.
#[test]
fn idle_tray_request_exit_finishes_confirmed_once_with_real_manager() {
    let mgr = JobManager::new();
    let coord = ExitCoordinator::new();
    let finish_calls = Arc::new(AtomicUsize::new(0));
    let confirmed = Arc::new(Mutex::new(Vec::<bool>::new()));
    for _ in 0..3 {
        let fc = Arc::clone(&finish_calls);
        let cf = Arc::clone(&confirmed);
        coord.request_exit(
            &mgr,
            move |result| {
                fc.fetch_add(1, SeqCst);
                lock(&cf).push(matches!(result, CleanupResult::Confirmed));
            },
            &real_cfg(),
        );
    }
    assert!(wait_until(Duration::from_secs(5), || finish_calls
        .load(SeqCst)
        >= 1));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        finish_calls.load(SeqCst),
        1,
        "tray finish issued exactly once"
    );
    assert_eq!(
        *lock(&confirmed),
        vec![true],
        "idle tray finish is Confirmed"
    );
}

/// Reads the pid file if the leader wrote it within `limit`; `None` when the job
/// was stopped before its script ran (the pid file is then legitimately absent).
fn pid_if_written(path: &Path, limit: Duration) -> Option<i32> {
    wait_until(limit, || path.exists());
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
}

/// Case 3 (real): a start() racing close_admission either completes before it
/// (the job is then seen as active and cleaned before finish) or returns
/// ShuttingDown. In every round: after finish the manager has no active job, a
/// later start returns ShuttingDown, and IF a leader pid was recorded its process
/// is dead. Repeated 8 times with a growing race offset.
#[test]
fn start_racing_close_admission_never_leaks_a_running_job() {
    for round in 0..8u64 {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (bin, leader) = sleeper_script(tmp.path());
        let mgr = JobManager::new();
        let _cancel = CancelOnDrop(Arc::clone(&mgr));
        let coord = ExitCoordinator::new();
        let starter_mgr = Arc::clone(&mgr);
        let starter_spec = spec(tmp.path(), &bin, "task");
        let starter =
            std::thread::spawn(move || starter_mgr.start(starter_spec, Arc::new(NoopObserver)));
        std::thread::sleep(Duration::from_millis(round * 3));
        let finish_calls = Arc::new(AtomicUsize::new(0));
        let active_at_finish = Arc::new(Mutex::new(None::<bool>));
        let (fc, af) = (Arc::clone(&finish_calls), Arc::clone(&active_at_finish));
        let finish_mgr = Arc::clone(&mgr);
        let decision = coord.on_exit_requested(
            &mgr,
            || {},
            move |_result| {
                *lock(&af) = Some(finish_mgr.has_active_job());
                fc.fetch_add(1, SeqCst);
            },
            &real_cfg(),
        );
        let started = starter.join().expect("starter thread must not panic");
        let started_ok = started.is_ok();
        match &started {
            Ok(_) => assert!(
                matches!(decision, ExitDecision::Deferred),
                "round {round}: a started job must be seen as active"
            ),
            Err(e) => assert!(
                matches!(e, JobError::ShuttingDown),
                "round {round}: a racing start must be ShuttingDown, got {e:?}"
            ),
        }
        if matches!(decision, ExitDecision::Deferred) {
            assert!(
                wait_until(Duration::from_secs(15), || finish_calls.load(SeqCst) == 1),
                "round {round}: finish must run exactly once"
            );
            assert_eq!(
                *lock(&active_at_finish),
                Some(false),
                "round {round}: no running job at finish"
            );
        }
        // Whatever the race outcome, admission stays closed afterwards.
        let late = mgr.start(spec(tmp.path(), &bin, "task"), Arc::new(NoopObserver));
        assert!(
            matches!(late, Err(JobError::ShuttingDown)),
            "round {round}: a start after exit must return ShuttingDown"
        );
        // The pid is recorded only when the script ran before the stop; guard it.
        let recorded = pid_if_written(&leader, Duration::from_millis(300));
        let _guard = GroupGuard {
            pids: vec![],
            pgids: recorded.into_iter().collect(),
        };
        if let Some(pid) = recorded {
            assert!(
                started_ok,
                "round {round}: a pid was recorded, so a start must have succeeded"
            );
            assert!(
                !pid_alive(pid),
                "round {round}: recorded leader must be dead"
            );
        }
    }
}

/// The real JobManager is an ExitTarget: close_admission makes start refuse, and
/// with no job cleanup is confirmed.
#[test]
fn job_manager_exit_target_close_admission_refuses_later_starts() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "touch should-not-run.marker");
    let mgr = JobManager::new();
    let target: Arc<dyn ExitTarget> = Arc::clone(&mgr) as Arc<dyn ExitTarget>;
    target.close_admission();
    assert!(!target.has_active_job());
    let err = match mgr.start(spec(tmp.path(), &bin, "task"), Arc::new(NoopObserver)) {
        Ok(_) => panic!("start must be refused after close_admission"),
        Err(e) => e,
    };
    assert!(matches!(err, JobError::ShuttingDown));
    assert!(
        target.cleanup_confirmed(),
        "no job means cleanup is confirmed"
    );
    assert!(!tmp.path().join("should-not-run.marker").exists());
}

/// Case 8 (source contract): the coordinator module never calls kill/killpg or
/// touches libc; only the owner task in agent_jobs_owner.rs signals the group.
/// Token-based: `request_force_kill(` is a method name, not a signal call.
#[test]
fn coordinator_source_contains_no_signal_calls() {
    let src = include_str!("agent_jobs_exit.rs");
    for needle in ["kill(", "killpg(", "libc::"] {
        assert!(
            !contains_call_token(src, needle),
            "agent_jobs_exit.rs must not call `{needle}`"
        );
    }
}
