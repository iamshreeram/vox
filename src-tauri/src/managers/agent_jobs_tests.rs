//! JobManager lifecycle tests (S2b, DESIGN D4, R2-4, R2-5, R2-6, R3-2, R3-4, R4-2).
//! Red by design: the JobManager bodies are todo!(). Real `sh` fake CLIs live in
//! temp dirs. Do not edit these tests to make code pass.
#![cfg(unix)]

#[path = "agent_tools/test_env_child.rs"]
mod test_env_child;

use super::{JobError, JobManager, JobObserver, JobSnapshot, JobSpec, JobState, ShutdownConfig};
use crate::managers::converse_confirm::{FileId, JobLaunch, LaunchConfig};
use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

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

/// Records the order of observer calls as "progress" / "finish" strings.
#[derive(Default)]
struct RecordingObserver {
    events: Mutex<Vec<&'static str>>,
}

impl RecordingObserver {
    fn finish_count(&self) -> usize {
        self.events
            .lock()
            .map(|e| e.iter().filter(|k| **k == "finish").count())
            .unwrap_or(0)
    }
    fn progress_count(&self) -> usize {
        self.events
            .lock()
            .map(|e| e.iter().filter(|k| **k == "progress").count())
            .unwrap_or(0)
    }
    fn last_kind(&self) -> Option<&'static str> {
        self.events.lock().ok().and_then(|e| e.last().copied())
    }
}

impl JobObserver for RecordingObserver {
    fn on_progress(&self, _snap: &JobSnapshot) {
        if let Ok(mut e) = self.events.lock() {
            e.push("progress");
        }
    }
    fn on_finish(&self, _snap: &JobSnapshot) {
        if let Ok(mut e) = self.events.lock() {
            e.push("finish");
        }
    }
}

/// Blocks in on_finish until opened; records the project of each finish only
/// after it is released, so the recorded order is the delivery order.
struct GatedObserver {
    open: Mutex<bool>,
    cv: Condvar,
    delivered: Mutex<Vec<String>>,
}

impl GatedObserver {
    fn new() -> Arc<Self> {
        Arc::new(GatedObserver {
            open: Mutex::new(false),
            cv: Condvar::new(),
            delivered: Mutex::new(Vec::new()),
        })
    }
    fn release(&self) {
        if let Ok(mut open) = self.open.lock() {
            *open = true;
        }
        self.cv.notify_all();
    }
}

impl JobObserver for GatedObserver {
    fn on_progress(&self, _snap: &JobSnapshot) {}
    fn on_finish(&self, snap: &JobSnapshot) {
        if let Ok(mut open) = self.open.lock() {
            while !*open {
                open = match self.cv.wait(open) {
                    Ok(guard) => guard,
                    Err(_) => return,
                };
            }
        }
        if let Ok(mut d) = self.delivered.lock() {
            d.push(snap.project.clone());
        }
    }
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

/// A fake CLI that records its own pid and starts a grandchild that also sleeps,
/// so tests can check that the whole process group is killed.
/// Returns (binary, leader_pidfile, grandchild_pidfile).
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

fn spec(dir: &Path, binary: &Path, task: &str) -> JobSpec {
    JobSpec {
        launch: launch(dir, binary, task, Duration::from_secs(20)),
        progress_interval: Duration::from_secs(90),
    }
}

/// Starts a job and panics with a clear message if it is refused.
fn must_start(mgr: &Arc<JobManager>, spec: JobSpec, obs: Arc<dyn JobObserver>) -> u64 {
    match mgr.start(spec, obs) {
        Ok(id) => id,
        Err(_) => panic!("start must succeed for this scenario"),
    }
}

/// Returns the error of a refused start, panicking if it was accepted.
fn refused(mgr: &Arc<JobManager>, spec: JobSpec, obs: Arc<dyn JobObserver>) -> JobError {
    match mgr.start(spec, obs) {
        Ok(_) => panic!("start must be refused for this scenario"),
        Err(e) => e,
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

fn wait_terminal(mgr: &JobManager, limit: Duration) -> Option<JobSnapshot> {
    let mut last = None;
    wait_until(limit, || {
        last = mgr.snapshot();
        last.as_ref()
            .is_some_and(|s| !matches!(s.state, JobState::Running))
    });
    last
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

fn quick_shutdown() -> ShutdownConfig {
    ShutdownConfig {
        term_grace: Duration::from_millis(200),
        drain: Duration::from_millis(200),
        deadline: Duration::from_secs(5),
    }
}

// ---- outcome states -------------------------------------------------------

#[test]
fn success_exit_zero_records_succeeded_and_finishes_once() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "echo hello-from-job");
    let mgr = JobManager::new();
    let obs = Arc::new(RecordingObserver::default());
    let id = must_start(&mgr, spec(tmp.path(), &bin, "task"), obs.clone());
    let snap = wait_terminal(&mgr, Duration::from_secs(10)).expect("snapshot");
    assert_eq!(snap.id, id);
    assert!(matches!(snap.state, JobState::Succeeded));
    assert!(snap.tail.iter().any(|l| l.contains("hello-from-job")));
    assert!(wait_until(Duration::from_secs(5), || obs.finish_count() == 1));
    assert_eq!(obs.finish_count(), 1, "on_finish must run exactly once");
}

#[test]
fn non_zero_exit_records_failed_with_code() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "exit 7");
    let mgr = JobManager::new();
    must_start(
        &mgr,
        spec(tmp.path(), &bin, "task"),
        Arc::new(RecordingObserver::default()),
    );
    let snap = wait_terminal(&mgr, Duration::from_secs(10)).expect("snapshot");
    assert!(matches!(snap.state, JobState::Failed { code: Some(7) }));
}

#[test]
fn killed_by_signal_records_failed_without_code() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "kill -9 $$");
    let mgr = JobManager::new();
    must_start(
        &mgr,
        spec(tmp.path(), &bin, "task"),
        Arc::new(RecordingObserver::default()),
    );
    let snap = wait_terminal(&mgr, Duration::from_secs(10)).expect("snapshot");
    assert!(matches!(snap.state, JobState::Failed { code: None }));
}

// ---- bad binary variants ---------------------------------------------------

#[test]
fn relative_binary_path_is_bad_binary_and_spawns_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "touch spawned.marker");
    let mut s = spec(tmp.path(), &bin, "task");
    s.launch.config.binary = PathBuf::from("cli");
    let mgr = JobManager::new();
    let err = refused(&mgr, s, Arc::new(RecordingObserver::default()));
    assert!(matches!(err, JobError::BadBinary));
    assert!(!tmp.path().join("spawned.marker").exists());
}

#[test]
fn directory_as_binary_is_bad_binary() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut s = spec(tmp.path(), tmp.path(), "task");
    s.launch.config.binary_id = file_id(tmp.path());
    let mgr = JobManager::new();
    let err = refused(&mgr, s, Arc::new(RecordingObserver::default()));
    assert!(matches!(err, JobError::BadBinary));
}

#[test]
fn non_executable_binary_is_bad_binary() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "true");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    let s = spec(tmp.path(), &bin, "task");
    let mgr = JobManager::new();
    let err = refused(&mgr, s, Arc::new(RecordingObserver::default()));
    assert!(matches!(err, JobError::BadBinary));
}

#[test]
fn overwritten_binary_fails_file_id_check_and_is_bad_binary() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "true");
    let approved = file_id(&bin);
    // Overwrite the same path with different content (new len / inode).
    write_script(tmp.path(), "cli", "echo replaced; true; true; true");
    let mut s = spec(tmp.path(), &bin, "task");
    s.launch.config.binary_id = approved;
    let mgr = JobManager::new();
    let err = refused(&mgr, s, Arc::new(RecordingObserver::default()));
    assert!(matches!(err, JobError::BadBinary));
}

// ---- admission and busy -----------------------------------------------------

#[test]
fn second_start_while_running_is_busy() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (bin, leader, grandchild) = group_script(tmp.path());
    let mgr = JobManager::new();
    must_start(
        &mgr,
        spec(tmp.path(), &bin, "task"),
        Arc::new(RecordingObserver::default()),
    );
    let _guard = GroupGuard {
        pids: vec![],
        pgids: vec![read_pid_file(&leader)],
    };
    let _gc = GroupGuard {
        pids: vec![read_pid_file(&grandchild)],
        pgids: vec![],
    };
    let err = refused(
        &mgr,
        spec(tmp.path(), &bin, "task"),
        Arc::new(RecordingObserver::default()),
    );
    assert!(matches!(err, JobError::Busy));
    assert!(mgr.cancel());
}

#[test]
fn close_admission_makes_start_return_shutting_down() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "touch should-not-run.marker");
    let mgr = JobManager::new();
    mgr.close_admission();
    let err = refused(
        &mgr,
        spec(tmp.path(), &bin, "task"),
        Arc::new(RecordingObserver::default()),
    );
    assert!(matches!(err, JobError::ShuttingDown));
    assert!(!tmp.path().join("should-not-run.marker").exists());
}

#[test]
fn saturated_finish_reservations_refuse_fifth_start_then_deliver_in_order() {
    // R3-4: 4 reservations. Observer blocked => the first 4 jobs finish and clean
    // up without waiting for it; the 5th start is Busy; unblocking delivers each
    // Finish exactly once, in order.
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "exit 0");
    let mgr = JobManager::new();
    let obs = GatedObserver::new();
    for i in 1..=4 {
        let mut s = spec(tmp.path(), &bin, "task");
        s.launch.project = format!("job-{i}");
        let id = must_start(&mgr, s, obs.clone());
        assert!(
            wait_until(Duration::from_secs(10), || mgr.snapshot().is_some_and(
                |snap| snap.id == id && !matches!(snap.state, JobState::Running)
            )),
            "job {i} must finish while the observer is blocked"
        );
    }
    let mut fifth = spec(tmp.path(), &bin, "task");
    fifth.launch.project = "job-5".to_string();
    let err = refused(&mgr, fifth, obs.clone());
    assert!(matches!(err, JobError::Busy), "5th start must be Busy");
    obs.release();
    let expected: Vec<String> = (1..=4).map(|i| format!("job-{i}")).collect();
    assert!(wait_until(Duration::from_secs(10), || obs
        .delivered
        .lock()
        .map(|d| d.len() == 4)
        .unwrap_or(false)));
    std::thread::sleep(Duration::from_millis(200));
    let delivered = obs.delivered.lock().expect("delivered").clone();
    assert_eq!(delivered, expected, "each Finish once, in order");
}

// ---- spawn contract ------------------------------------------------------------

#[test]
fn argv_is_exact_and_no_shell_expansion_happens() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Prints each argv entry on its own line.
    let bin = write_script(
        tmp.path(),
        "cli",
        "for a in \"$@\"; do printf '[%s]\\n' \"$a\"; done",
    );
    let task = "--leading dash $(touch X) `touch Y` \"q\" 'single'\nnewline";
    let mgr = JobManager::new();
    must_start(
        &mgr,
        spec(tmp.path(), &bin, task),
        Arc::new(RecordingObserver::default()),
    );
    let snap = wait_terminal(&mgr, Duration::from_secs(10)).expect("snapshot");
    assert!(matches!(snap.state, JobState::Succeeded));
    assert!(
        snap.tail.iter().any(|l| l == "[-p]"),
        "prompt flag is its own argv entry"
    );
    assert!(
        snap.tail.iter().any(|l| l.contains("--leading dash")),
        "leading-dash task stays one argv value"
    );
    assert!(
        !tmp.path().join("X").exists(),
        "$(...) must not be expanded"
    );
    assert!(
        !tmp.path().join("Y").exists(),
        "backticks must not be expanded"
    );
}

#[test]
fn cwd_is_pinned_to_original_directory_after_rename_and_replace() {
    // R2-4: the directory is pinned before the job runs; renaming the path and
    // creating a different directory under the old name must not move the job.
    let root = tempfile::tempdir().expect("tempdir");
    let proj = root.path().join("proj");
    std::fs::create_dir(&proj).expect("mkdir proj");
    let original_ino = std::fs::metadata(&proj).expect("stat").ino();
    let bin = write_script(root.path(), "cli", "ls -di . | awk '{print $1}'");
    let s = spec(&proj, &bin, "task");
    std::fs::rename(&proj, root.path().join("moved")).expect("rename");
    std::fs::create_dir(&proj).expect("replacement dir");
    let mgr = JobManager::new();
    must_start(&mgr, s, Arc::new(RecordingObserver::default()));
    let snap = wait_terminal(&mgr, Duration::from_secs(10)).expect("snapshot");
    assert!(matches!(snap.state, JobState::Succeeded));
    let reported = snap.tail.iter().find_map(|l| l.trim().parse::<u64>().ok());
    assert_eq!(
        reported,
        Some(original_ino),
        "job must run in the pinned directory"
    );
}

#[test]
fn env_is_inherited_by_the_job() {
    test_env_child::run_in_child(
        module_path!(),
        "env_is_inherited_by_the_job_inner",
        &[("VOX_JOB_TEST_VAR", "inherited-ok")],
    );
}

#[test]
#[ignore = "runs only in the child process started by env_is_inherited_by_the_job"]
fn env_is_inherited_by_the_job_inner() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(
        tmp.path(),
        "cli",
        "echo \"var=$VOX_JOB_TEST_VAR\"; [ -n \"$PATH\" ] && echo path-set",
    );
    let mgr = JobManager::new();
    must_start(
        &mgr,
        spec(tmp.path(), &bin, "task"),
        Arc::new(RecordingObserver::default()),
    );
    let snap = wait_terminal(&mgr, Duration::from_secs(10)).expect("snapshot");
    assert!(snap.tail.iter().any(|l| l == "var=inherited-ok"));
    assert!(snap.tail.iter().any(|l| l == "path-set"));
    test_env_child::child_ok("env_is_inherited_by_the_job_inner");
}

// ---- termination ------------------------------------------------------------------

#[test]
fn cancel_kills_group_including_grandchild_and_is_idempotent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (bin, leader, grandchild) = group_script(tmp.path());
    let mgr = JobManager::new();
    must_start(
        &mgr,
        spec(tmp.path(), &bin, "task"),
        Arc::new(RecordingObserver::default()),
    );
    let leader_pid = read_pid_file(&leader);
    let gc_pid = read_pid_file(&grandchild);
    let _guard = GroupGuard {
        pids: vec![gc_pid],
        pgids: vec![leader_pid],
    };
    assert!(mgr.cancel(), "cancel of a running job returns true");
    let snap = wait_terminal(&mgr, Duration::from_secs(10)).expect("snapshot");
    assert!(matches!(snap.state, JobState::Cancelled));
    assert!(
        wait_until(Duration::from_secs(5), || !pid_alive(gc_pid)),
        "grandchild must be gone after cancel"
    );
    assert!(!mgr.cancel(), "cancel when idle returns false");
}

#[test]
fn timeout_kills_group_and_records_timed_out() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (bin, leader, grandchild) = group_script(tmp.path());
    let mgr = JobManager::new();
    let mut s = spec(tmp.path(), &bin, "task");
    // Host fact: a fresh sh script needs ~450 ms before its first line runs, and
    // first exec is security-scanned by the OS, so under parallel load the scans
    // queue. The timeout must leave room for the pid files to be written first.
    s.launch.config.timeout = Duration::from_secs(12);
    must_start(&mgr, s, Arc::new(RecordingObserver::default()));
    let leader_pid = read_pid_file(&leader);
    let gc_pid = read_pid_file(&grandchild);
    let _guard = GroupGuard {
        pids: vec![gc_pid],
        pgids: vec![leader_pid],
    };
    let snap = wait_terminal(&mgr, Duration::from_secs(30)).expect("snapshot");
    assert!(matches!(snap.state, JobState::TimedOut));
    assert!(wait_until(Duration::from_secs(5), || !pid_alive(gc_pid)));
}

#[test]
fn cancel_and_timeout_together_record_one_terminal_state() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (bin, leader, grandchild) = group_script(tmp.path());
    let mgr = JobManager::new();
    let obs = Arc::new(RecordingObserver::default());
    let mut s = spec(tmp.path(), &bin, "task");
    // Load-tolerant 12 s timeout (see timeout_kills_group_and_records_timed_out).
    s.launch.config.timeout = Duration::from_secs(12);
    let started = Instant::now();
    must_start(&mgr, s, obs.clone());
    let leader_pid = read_pid_file(&leader);
    let gc_pid = read_pid_file(&grandchild);
    let _guard = GroupGuard {
        pids: vec![gc_pid],
        pgids: vec![leader_pid],
    };
    // Call cancel right around the moment the timeout fires (timeout minus 50 ms).
    let target = Duration::from_millis(11_950);
    if let Some(remaining) = target.checked_sub(started.elapsed()) {
        std::thread::sleep(remaining);
    }
    let _ = mgr.cancel();
    let snap = wait_terminal(&mgr, Duration::from_secs(30)).expect("snapshot");
    assert!(matches!(
        snap.state,
        JobState::Cancelled | JobState::TimedOut
    ));
    assert!(wait_until(Duration::from_secs(5), || obs.finish_count() == 1));
    assert!(
        wait_until(Duration::from_secs(5), || !pid_alive(gc_pid)),
        "group must be dead after the single terminal state"
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        obs.finish_count(),
        1,
        "exactly one terminal state, one finish"
    );
}

#[test]
fn sigterm_ignoring_child_is_killed_after_grace() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let pidfile = tmp.path().join("leader.pid");
    let bin = write_script(
        tmp.path(),
        "cli",
        &format!(
            "trap '' TERM; echo $$ > '{}'; while true; do sleep 1; done",
            pidfile.display()
        ),
    );
    let mgr = JobManager::with_timing(Duration::from_millis(300), Duration::from_millis(200));
    must_start(
        &mgr,
        spec(tmp.path(), &bin, "task"),
        Arc::new(RecordingObserver::default()),
    );
    let leader = read_pid_file(&pidfile);
    let _guard = GroupGuard {
        pids: vec![],
        pgids: vec![leader],
    };
    let began = Instant::now();
    assert!(mgr.cancel());
    let snap = wait_terminal(&mgr, Duration::from_secs(10)).expect("snapshot");
    assert!(matches!(snap.state, JobState::Cancelled));
    assert!(wait_until(Duration::from_secs(5), || !pid_alive(leader)));
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "SIGKILL after the 300 ms grace, not an unbounded wait"
    );
}

#[test]
fn natural_exit_while_grandchild_holds_pipe_does_not_hang() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let leader = tmp.path().join("leader.pid");
    let grandchild = tmp.path().join("grandchild.pid");
    // The leader exits at once; the backgrounded sleep inherits the stdout and
    // stderr pipes and keeps them open.
    let bin = write_script(
        tmp.path(),
        "cli",
        &format!(
            "echo $$ > '{}'; sleep 60 & echo $! > '{}'; exit 0",
            leader.display(),
            grandchild.display()
        ),
    );
    let mgr = JobManager::new();
    let started = Instant::now();
    must_start(
        &mgr,
        spec(tmp.path(), &bin, "task"),
        Arc::new(RecordingObserver::default()),
    );
    let gc_pid = read_pid_file(&grandchild);
    let _guard = GroupGuard {
        pids: vec![gc_pid],
        pgids: vec![read_pid_file(&leader)],
    };
    // The manager must terminate leftover group members once the leader exits,
    // then drain, and report a terminal state. A hang here is a failure.
    let snap = wait_terminal(&mgr, Duration::from_secs(8));
    assert!(
        snap.is_some_and(|s| matches!(s.state, JobState::Succeeded)),
        "a Succeeded terminal state must be reached within 8 s, not a hang"
    );
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "drain deadline respected"
    );
    assert!(
        wait_until(Duration::from_secs(5), || !pid_alive(gc_pid)),
        "grandchild must be dead after the job finishes"
    );
}

#[test]
fn natural_exit_kills_background_member_that_closed_its_pipes() {
    // R2-5: the group is cleaned at every terminal state. Here the leader exits
    // naturally with no pipe held open, so only the owner's group cleanup can
    // stop the background member that detached from the pipes.
    let tmp = tempfile::tempdir().expect("tempdir");
    let leader = tmp.path().join("leader.pid");
    let grandchild = tmp.path().join("grandchild.pid");
    let bin = write_script(
        tmp.path(),
        "cli",
        &format!(
            "echo $$ > '{}'; (sleep 60 >/dev/null 2>&1 </dev/null & echo $! > '{}'); exit 0",
            leader.display(),
            grandchild.display()
        ),
    );
    let mgr = JobManager::new();
    must_start(
        &mgr,
        spec(tmp.path(), &bin, "task"),
        Arc::new(RecordingObserver::default()),
    );
    let gc_pid = read_pid_file(&grandchild);
    let _guard = GroupGuard {
        pids: vec![gc_pid],
        pgids: vec![read_pid_file(&leader)],
    };
    let snap = wait_terminal(&mgr, Duration::from_secs(10));
    assert!(
        snap.is_some_and(|s| matches!(s.state, JobState::Succeeded)),
        "natural exit must record Succeeded"
    );
    assert!(
        wait_until(Duration::from_secs(5), || !pid_alive(gc_pid)),
        "background member that closed its pipes must be killed at the terminal state"
    );
}

#[test]
fn cancel_and_timeout_do_not_wait_a_full_grace_when_the_group_exits_at_once() {
    // A leader that dies on SIGTERM must not make cancel sit out the 5 s grace.
    let tmp = tempfile::tempdir().expect("tempdir");
    let pidfile = tmp.path().join("leader.pid");
    let bin = write_script(
        tmp.path(),
        "cli",
        &format!("echo $$ > '{}'; exec sleep 60", pidfile.display()),
    );
    let mgr = JobManager::with_timing(Duration::from_secs(5), Duration::from_secs(1));
    must_start(
        &mgr,
        spec(tmp.path(), &bin, "task"),
        Arc::new(RecordingObserver::default()),
    );
    let leader = read_pid_file(&pidfile);
    let _guard = GroupGuard {
        pids: vec![],
        pgids: vec![leader],
    };
    let began = Instant::now();
    assert!(mgr.cancel());
    let snap = wait_terminal(&mgr, Duration::from_secs(10)).expect("snapshot");
    assert!(matches!(snap.state, JobState::Cancelled));
    assert!(
        began.elapsed() < Duration::from_secs(3),
        "cancel waited {:?}; it must not sit out the 5 s grace",
        began.elapsed()
    );
}

#[test]
fn no_zombie_remains_after_finish() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let pidfile = tmp.path().join("leader.pid");
    let bin = write_script(
        tmp.path(),
        "cli",
        &format!("echo $$ > '{}'; exit 0", pidfile.display()),
    );
    let mgr = JobManager::new();
    must_start(
        &mgr,
        spec(tmp.path(), &bin, "task"),
        Arc::new(RecordingObserver::default()),
    );
    wait_terminal(&mgr, Duration::from_secs(10)).expect("snapshot");
    let pid = read_pid_file(&pidfile);
    // SAFETY: waitpid with WNOHANG only queries the child state.
    let rc = unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) };
    assert_eq!(rc, -1, "leader must already be reaped (no zombie)");
}

// ---- output ------------------------------------------------------------------------

#[test]
fn chatty_output_is_bounded_and_job_still_completes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(
        tmp.path(),
        "cli",
        "i=0; while [ $i -lt 100000 ]; do echo line-$i; i=$((i+1)); done; \
         head -c 10000000 /dev/zero | tr '\\0' 'x'; echo",
    );
    let mgr = JobManager::new();
    must_start(
        &mgr,
        spec(tmp.path(), &bin, "task"),
        Arc::new(RecordingObserver::default()),
    );
    let snap = wait_terminal(&mgr, Duration::from_secs(30)).expect("snapshot");
    assert!(matches!(snap.state, JobState::Succeeded));
    assert!(snap.tail.len() <= 20, "tail capped at 20 lines");
    assert!(
        snap.tail.iter().all(|l| l.len() <= 200),
        "tail lines capped at 200 bytes"
    );
}

#[test]
fn tail_is_redacted_before_it_is_visible() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let secret = format!("sk-{}", "a".repeat(30));
    let bin = write_script(tmp.path(), "cli", &format!("echo 'key={secret}'"));
    let mgr = JobManager::new();
    must_start(
        &mgr,
        spec(tmp.path(), &bin, "task"),
        Arc::new(RecordingObserver::default()),
    );
    let snap = wait_terminal(&mgr, Duration::from_secs(10)).expect("snapshot");
    assert!(
        snap.tail.iter().all(|l| !l.contains(&secret)),
        "raw secret must never appear in the tail"
    );
}

// ---- observers ---------------------------------------------------------------------

#[test]
fn progress_is_throttled_and_finish_is_last_and_once() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "sleep 0.7");
    let mut s = spec(tmp.path(), &bin, "task");
    s.progress_interval = Duration::from_millis(100);
    let mgr = JobManager::new();
    let obs = Arc::new(RecordingObserver::default());
    let started = Instant::now();
    must_start(&mgr, s, obs.clone());
    wait_terminal(&mgr, Duration::from_secs(10)).expect("snapshot");
    assert!(wait_until(Duration::from_secs(5), || obs.finish_count() == 1));
    let wall_ms = started.elapsed().as_millis();
    assert!(
        obs.progress_count() >= 1,
        "progress after the first interval"
    );
    assert!(
        obs.progress_count() as u128 <= wall_ms / 100 + 1,
        "progress at most once per 100 ms interval of measured wall time"
    );
    assert_eq!(obs.last_kind(), Some("finish"), "no progress after finish");
}

#[test]
fn panicking_observer_is_isolated_and_job_still_finishes() {
    struct Panicker;
    impl JobObserver for Panicker {
        fn on_progress(&self, _snap: &JobSnapshot) {
            panic!("observer panic (expected)");
        }
        fn on_finish(&self, _snap: &JobSnapshot) {
            panic!("observer panic (expected)");
        }
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "exit 0");
    let mgr = JobManager::new();
    must_start(&mgr, spec(tmp.path(), &bin, "task"), Arc::new(Panicker));
    let snap = wait_terminal(&mgr, Duration::from_secs(10)).expect("snapshot");
    assert!(matches!(snap.state, JobState::Succeeded));
    assert!(wait_until(Duration::from_secs(5), || !mgr.has_active_job()));
    must_start(
        &mgr,
        spec(tmp.path(), &bin, "task"),
        Arc::new(RecordingObserver::default()),
    );
}

#[test]
fn blocking_observer_does_not_delay_cancel() {
    struct Blocker;
    impl JobObserver for Blocker {
        fn on_progress(&self, _snap: &JobSnapshot) {}
        fn on_finish(&self, _snap: &JobSnapshot) {
            std::thread::sleep(Duration::from_secs(30));
        }
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let (bin, leader, grandchild) = group_script(tmp.path());
    let mgr = JobManager::new();
    must_start(&mgr, spec(tmp.path(), &bin, "task"), Arc::new(Blocker));
    let _guard = GroupGuard {
        pids: vec![read_pid_file(&grandchild)],
        pgids: vec![read_pid_file(&leader)],
    };
    let began = Instant::now();
    assert!(mgr.cancel());
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "cancel must not wait for the observer"
    );
}

// ---- shutdown ---------------------------------------------------------------------

#[test]
fn shutdown_kills_running_job_before_returning_and_is_idempotent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (bin, leader, grandchild) = group_script(tmp.path());
    let mgr = JobManager::new();
    must_start(
        &mgr,
        spec(tmp.path(), &bin, "task"),
        Arc::new(RecordingObserver::default()),
    );
    let gc_pid = read_pid_file(&grandchild);
    let _guard = GroupGuard {
        pids: vec![gc_pid],
        pgids: vec![read_pid_file(&leader)],
    };
    assert!(
        mgr.shutdown(&quick_shutdown()),
        "clean shutdown reports true"
    );
    assert!(!pid_alive(gc_pid), "group gone when shutdown returns");
    assert!(
        mgr.shutdown(&quick_shutdown()),
        "second shutdown is also clean"
    );
}

#[test]
fn start_racing_close_admission_never_leaks_a_running_job() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "sleep 60");
    let mgr = JobManager::new();
    let racer = {
        let mgr = Arc::clone(&mgr);
        let s = spec(tmp.path(), &bin, "task");
        std::thread::spawn(move || mgr.start(s, Arc::new(RecordingObserver::default())))
    };
    mgr.close_admission();
    let outcome = racer.join().expect("racer thread");
    match outcome {
        Ok(_) => assert!(mgr.has_active_job(), "a completed start must be visible"),
        Err(e) => assert!(matches!(e, JobError::ShuttingDown)),
    }
    mgr.shutdown(&quick_shutdown());
    assert!(!mgr.has_active_job(), "no job survives shutdown");
}

#[test]
fn cancel_racing_natural_exit_yields_exactly_one_terminal_state() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "sleep 0.2; exit 0");
    let mgr = JobManager::new();
    let obs = Arc::new(RecordingObserver::default());
    must_start(&mgr, spec(tmp.path(), &bin, "task"), obs.clone());
    std::thread::sleep(Duration::from_millis(190));
    let _ = mgr.cancel();
    let snap = wait_terminal(&mgr, Duration::from_secs(10)).expect("snapshot");
    assert!(matches!(
        snap.state,
        JobState::Succeeded | JobState::Cancelled
    ));
    assert!(wait_until(Duration::from_secs(5), || obs.finish_count() == 1));
    assert_eq!(obs.finish_count(), 1, "exactly one finish for one job");
}
