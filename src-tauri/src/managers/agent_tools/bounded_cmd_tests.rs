//! Bounded runner tests (DESIGN R2-7, R3-1). Red by design: `run_bounded` is todo!().
//! Included from bounded_cmd.rs. Do not edit to make code pass.
#![cfg(unix)]

use super::{run_bounded, BoundedOutput, RunError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const CAP_64K: usize = 64 * 1024;

fn minimal_env() -> Vec<(String, String)> {
    vec![("PATH".to_string(), "/usr/bin:/bin".to_string())]
}

async fn run_sh(
    script: &str,
    cwd: &Path,
    cap: usize,
    timeout: Duration,
) -> Result<BoundedOutput, RunError> {
    let args = vec!["-c".to_string(), script.to_string()];
    run_bounded(
        Path::new("/bin/sh"),
        &args,
        cwd,
        &minimal_env(),
        cap,
        timeout,
    )
    .await
}

fn pid_alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks existence and permission; nothing is delivered.
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

async fn wait_until_dead(pid: i32, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if !pid_alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    !pid_alive(pid)
}

async fn read_pid(path: &Path) -> i32 {
    for _ in 0..200 {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(pid) = text.trim().parse::<i32>() {
                return pid;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("pid file was never written: {}", path.display());
}

#[tokio::test]
async fn stdout_within_cap_is_returned_exactly_and_not_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_sh("printf hello", dir.path(), CAP_64K, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(out.stdout, b"hello".to_vec());
    assert!(!out.stdout_truncated);
    assert_eq!(out.exit_code, Some(0));
}

#[tokio::test]
async fn zero_exit_with_no_output_is_surfaced_as_code_zero() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_sh("exit 0", dir.path(), CAP_64K, Duration::from_secs(5))
        .await
        .unwrap();
    assert!(out.stdout.is_empty());
    assert_eq!(out.exit_code, Some(0));
}

#[tokio::test]
async fn non_zero_exit_is_surfaced_with_its_code_and_output() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_sh(
        "printf x; exit 3",
        dir.path(),
        CAP_64K,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(out.stdout, b"x".to_vec());
    assert_eq!(out.exit_code, Some(3));
}

#[tokio::test]
async fn stdout_exactly_at_cap_is_not_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_sh(
        "head -c 65536 /dev/zero",
        dir.path(),
        CAP_64K,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(out.stdout.len(), CAP_64K);
    assert!(!out.stdout_truncated);
}

#[tokio::test]
async fn stdout_over_cap_is_capped_and_marked_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let out = run_sh(
        "head -c 10485760 /dev/zero",
        dir.path(),
        CAP_64K,
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    assert!(out.stdout_truncated);
    assert_eq!(out.stdout.len(), CAP_64K);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "10 MB writer must not stall"
    );
}

#[tokio::test]
async fn zero_cap_drops_all_stdout_and_reports_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_sh("printf abc", dir.path(), 0, Duration::from_secs(5))
        .await
        .unwrap();
    assert!(out.stdout.is_empty());
    assert!(out.stdout_truncated);
}

#[tokio::test]
async fn stderr_flood_is_discarded_without_blocking_or_reaching_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let out = run_sh(
        "head -c 5242880 /dev/zero >&2; printf ok",
        dir.path(),
        CAP_64K,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(out.stdout, b"ok".to_vec());
    assert!(!out.stdout_truncated);
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[tokio::test]
async fn non_utf8_and_nul_bytes_are_returned_raw() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_sh(
        "printf '\\377\\000x'",
        dir.path(),
        CAP_64K,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(out.stdout, vec![0xff, 0x00, b'x']);
}

#[tokio::test]
async fn timeout_returns_promptly_with_timeout_error() {
    let dir = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let result = run_sh("sleep 30", dir.path(), CAP_64K, Duration::from_millis(300)).await;
    assert_eq!(result, Err(RunError::Timeout));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn child_ignoring_sigterm_is_killed_after_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile: PathBuf = dir.path().join("shell.pid");
    let script = format!(
        "trap '' TERM; echo $$ > '{}'; while :; do sleep 0.1; done",
        pidfile.display()
    );
    let started = Instant::now();
    let result = run_sh(&script, dir.path(), CAP_64K, Duration::from_millis(400)).await;
    assert_eq!(result, Err(RunError::Timeout));
    assert!(started.elapsed() < Duration::from_secs(3));
    let pid = read_pid(&pidfile).await;
    assert!(
        wait_until_dead(pid, Duration::from_secs(2)).await,
        "SIGTERM-ignoring child survived"
    );
}

#[tokio::test]
async fn timeout_kills_grandchild_in_the_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile: PathBuf = dir.path().join("grandchild.pid");
    let script = format!("sleep 30 & echo $! > '{}'; wait", pidfile.display());
    let result = run_sh(&script, dir.path(), CAP_64K, Duration::from_millis(500)).await;
    assert_eq!(result, Err(RunError::Timeout));
    let pid = read_pid(&pidfile).await;
    assert!(
        wait_until_dead(pid, Duration::from_secs(2)).await,
        "grandchild survived timeout"
    );
}

#[tokio::test]
async fn dropping_the_future_kills_the_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile: PathBuf = dir.path().join("dropped.pid");
    let script = format!("sleep 30 & echo $! > '{}'; sleep 30", pidfile.display());
    let fut = run_sh(&script, dir.path(), CAP_64K, Duration::from_secs(30));
    let outer = tokio::time::timeout(Duration::from_millis(500), fut).await;
    assert!(
        outer.is_err(),
        "the outer timeout must cancel the runner future"
    );
    let pid = read_pid(&pidfile).await;
    assert!(
        wait_until_dead(pid, Duration::from_secs(2)).await,
        "grandchild survived drop"
    );
}

#[tokio::test]
async fn stdout_overflow_kills_the_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile: PathBuf = dir.path().join("overflow.pid");
    let script = format!(
        "sleep 30 & echo $! > '{}'; head -c 10485760 /dev/zero",
        pidfile.display()
    );
    let started = Instant::now();
    let out = run_sh(&script, dir.path(), CAP_64K, Duration::from_secs(10))
        .await
        .unwrap();
    assert!(out.stdout_truncated);
    assert!(started.elapsed() < Duration::from_secs(5));
    let pid = read_pid(&pidfile).await;
    assert!(
        wait_until_dead(pid, Duration::from_secs(2)).await,
        "grandchild survived overflow"
    );
}

#[tokio::test]
async fn descendant_holding_pipes_is_bounded_and_killed() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile: PathBuf = dir.path().join("pipe-holder.pid");
    let script = format!("sleep 30 & echo $! > '{}'; printf ok", pidfile.display());
    let started = Instant::now();
    let result = run_sh(&script, dir.path(), CAP_64K, Duration::from_secs(1)).await;
    assert!(
        matches!(result, Ok(_) | Err(RunError::Timeout)),
        "unexpected result: {result:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "reap wait must be bounded"
    );
    let pid = read_pid(&pidfile).await;
    assert!(
        wait_until_dead(pid, Duration::from_secs(2)).await,
        "pipe-holding grandchild survived"
    );
}

#[tokio::test]
async fn normal_exit_leaves_no_zombie_behind() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile: PathBuf = dir.path().join("zombie.pid");
    let script = format!("echo $$ > '{}'; printf ok", pidfile.display());
    let out = run_sh(&script, dir.path(), CAP_64K, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(out.stdout, b"ok".to_vec());
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // Reaped means kill(pid, 0) fails with ESRCH; a zombie would still answer.
    assert!(!pid_alive(pid), "direct child was not reaped");
}

#[tokio::test]
async fn child_sees_only_the_explicit_minimal_env() {
    let dir = tempfile::tempdir().unwrap();
    // Parent-side markers that must NOT reach the child.
    std::env::set_var("VOX_BOUNDED_TEST_PARENT_MARK", "leak");
    std::env::set_var("GIT_CONFIG_COUNT", "1");
    std::env::set_var("GIT_CONFIG_KEY_0", "core.fsmonitor");
    let env = vec![
        ("PATH".to_string(), "/usr/bin:/bin".to_string()),
        ("VOX_EXPLICIT".to_string(), "yes".to_string()),
    ];
    let out = run_bounded(
        Path::new("/usr/bin/env"),
        &[],
        dir.path(),
        &env,
        CAP_64K,
        Duration::from_secs(5),
    )
    .await;
    std::env::remove_var("VOX_BOUNDED_TEST_PARENT_MARK");
    std::env::remove_var("GIT_CONFIG_COUNT");
    std::env::remove_var("GIT_CONFIG_KEY_0");
    let out = out.unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut lines: Vec<&str> = text.lines().collect();
    lines.sort_unstable();
    assert_eq!(lines, vec!["PATH=/usr/bin:/bin", "VOX_EXPLICIT=yes"]);
    assert!(!text.contains("HOME="), "HOME must not leak into the child");
    assert!(
        !text.contains("GIT_"),
        "inherited GIT_* must not leak into the child"
    );
}

#[tokio::test]
async fn stdin_is_null_so_a_reader_sees_eof_immediately() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_bounded(
        Path::new("/bin/cat"),
        &[],
        dir.path(),
        &minimal_env(),
        CAP_64K,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(out.stdout.is_empty());
    assert_eq!(out.exit_code, Some(0));
}

#[tokio::test]
async fn program_runs_without_a_shell_so_arguments_are_literal() {
    let dir = tempfile::tempdir().unwrap();
    let args = vec!["$HOME".to_string()];
    let out = run_bounded(
        Path::new("/bin/echo"),
        &args,
        dir.path(),
        &minimal_env(),
        CAP_64K,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(out.stdout, b"$HOME\n".to_vec());
}

#[tokio::test]
async fn working_directory_is_the_requested_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_bounded(
        Path::new("/bin/pwd"),
        &["-P".to_string()],
        dir.path(),
        &minimal_env(),
        CAP_64K,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    let printed = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let expected = std::fs::canonicalize(dir.path()).unwrap();
    assert_eq!(PathBuf::from(printed), expected);
}

#[tokio::test]
async fn missing_program_is_a_spawn_error() {
    let dir = tempfile::tempdir().unwrap();
    let result = run_bounded(
        Path::new("/nonexistent/vox-bounded-test-binary"),
        &[],
        dir.path(),
        &minimal_env(),
        CAP_64K,
        Duration::from_secs(5),
    )
    .await;
    assert!(matches!(result, Err(RunError::Spawn(_))), "got {result:?}");
}
