// M2 stub: bodies arrive in M2-1..M2-3, this allow is removed in M2-3
#![allow(dead_code)]

//! Bounded subprocess runner (DESIGN R2-7): concurrent capped stdout and stderr drains, a
//! process group killed on success, error, overflow, timeout or drop.

use std::path::Path;
use std::time::Duration;

#[cfg(unix)]
use std::process::Stdio;
#[cfg(unix)]
use tokio::io::AsyncReadExt;
#[cfg(unix)]
use tokio::process::{Child, ChildStderr, ChildStdout};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedOutput {
    pub stdout: Vec<u8>,
    pub stdout_truncated: bool,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    Spawn(String),
    Timeout,
    Overflow,
    Io(String),
}

/// Runs `program` with `args` directly (no shell), `env_clear` plus `env`, cwd `cwd`,
/// stdin null. `stdout_cap` bytes are kept; stderr is drained and discarded.
#[cfg(unix)]
pub async fn run_bounded(
    program: &Path,
    args: &[String],
    cwd: &Path,
    env: &[(String, String)],
    stdout_cap: usize,
    timeout: Duration,
) -> Result<BoundedOutput, RunError> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    let mut child = cmd
        .spawn()
        .map_err(|_| RunError::Spawn(SPAWN_ERROR.to_string()))?;
    let pgid = child
        .id()
        .and_then(|pid| i32::try_from(pid).ok())
        .unwrap_or(0);
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // Declared after the child so it drops first: the group is killed while the
    // leader is still unreaped, so its pgid cannot be reused in between.
    let mut group = GroupGuard { pgid, armed: true };
    let (Some(stdout), Some(stderr)) = (stdout, stderr) else {
        return Err(RunError::Io(IO_ERROR.to_string()));
    };
    let outcome = tokio::time::timeout(
        timeout,
        collect(&mut child, pgid, stdout, stderr, stdout_cap),
    )
    .await;
    match outcome {
        Ok(Ok(out)) => {
            group.armed = false;
            Ok(out)
        }
        Ok(Err(err)) => Err(err),
        Err(_elapsed) => {
            if terminate(&mut child, pgid).await {
                group.armed = false;
            }
            Err(RunError::Timeout)
        }
    }
}

/// Non-unix stub: the bounded runner is only implemented for unix in M2.
#[cfg(not(unix))]
pub async fn run_bounded(
    program: &Path,
    args: &[String],
    cwd: &Path,
    env: &[(String, String)],
    stdout_cap: usize,
    timeout: Duration,
) -> Result<BoundedOutput, RunError> {
    let _ = (program, args, cwd, env, stdout_cap, timeout);
    Err(RunError::Spawn(UNSUPPORTED.to_string()))
}

const UNSUPPORTED: &str = "not supported on this platform yet";
const SPAWN_ERROR: &str = "could not start the command";
const IO_ERROR: &str = "could not read the command output";
const WAIT_ERROR: &str = "could not collect the command";

/// Grace period between SIGTERM and SIGKILL on timeout.
#[cfg(unix)]
const KILL_GRACE: Duration = Duration::from_millis(250);
/// Bound on each reap wait after the group has been killed.
#[cfg(unix)]
const REAP_WAIT: Duration = Duration::from_secs(1);

/// Drains stdout (capped) and stderr (discarded) concurrently, kills the group once
/// stdout is finished, then reaps the leader.
#[cfg(unix)]
async fn collect(
    child: &mut Child,
    pgid: i32,
    stdout: ChildStdout,
    stderr: ChildStderr,
    cap: usize,
) -> Result<BoundedOutput, RunError> {
    let drain = DrainTask(tokio::spawn(drain_discard(stderr)));
    let (bytes, truncated) = read_capped(stdout, pgid, cap).await?;
    kill_group(pgid, libc::SIGKILL);
    drop(drain);
    let status = child
        .wait()
        .await
        .map_err(|_| RunError::Io(WAIT_ERROR.to_string()))?;
    Ok(BoundedOutput {
        stdout: bytes,
        stdout_truncated: truncated,
        exit_code: status.code(),
    })
}

/// Timeout path: SIGTERM the group, wait a short grace for the leader, then SIGKILL the
/// group and reap within REAP_WAIT. Returns true when the leader was reaped.
#[cfg(unix)]
async fn terminate(child: &mut Child, pgid: i32) -> bool {
    kill_group(pgid, libc::SIGTERM);
    let _ = tokio::time::timeout(KILL_GRACE, child.wait()).await;
    kill_group(pgid, libc::SIGKILL);
    matches!(
        tokio::time::timeout(REAP_WAIT, child.wait()).await,
        Ok(Ok(_))
    )
}

/// Reads stdout keeping at most `cap` bytes. One byte past the cap kills the group at
/// once and reports truncation; reaching EOF within the cap is not truncation.
#[cfg(unix)]
async fn read_capped(
    mut out: ChildStdout,
    pgid: i32,
    cap: usize,
) -> Result<(Vec<u8>, bool), RunError> {
    let mut kept: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = out
            .read(&mut chunk)
            .await
            .map_err(|_| RunError::Io(IO_ERROR.to_string()))?;
        if n == 0 {
            return Ok((kept, false));
        }
        let room = cap.saturating_sub(kept.len());
        if n > room {
            kept.extend_from_slice(&chunk[..room]);
            kill_group(pgid, libc::SIGKILL);
            return Ok((kept, true));
        }
        kept.extend_from_slice(&chunk[..n]);
    }
}

/// Discards stderr until EOF so the child never blocks on a full pipe.
#[cfg(unix)]
async fn drain_discard(mut err: ChildStderr) {
    let mut sink = [0u8; 8192];
    loop {
        match err.read(&mut sink).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

/// Sends `signal` to the process group. pgid 0 or less is never signalled.
#[cfg(unix)]
fn kill_group(pgid: i32, signal: libc::c_int) {
    if pgid > 0 {
        // SAFETY: killpg only delivers a signal to the group; the result is ignored.
        unsafe {
            libc::killpg(pgid, signal);
        }
    }
}

/// Kills the whole group on drop while armed (any early exit, cancellation included).
#[cfg(unix)]
struct GroupGuard {
    pgid: i32,
    armed: bool,
}

#[cfg(unix)]
impl Drop for GroupGuard {
    fn drop(&mut self) {
        if self.armed {
            kill_group(self.pgid, libc::SIGKILL);
        }
    }
}

/// Aborts the stderr drain task when dropped.
#[cfg(unix)]
struct DrainTask(tokio::task::JoinHandle<()>);

#[cfg(unix)]
impl Drop for DrainTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
#[path = "bounded_cmd_tests.rs"]
mod bounded_cmd_tests;

#[cfg(test)]
#[path = "test_env_child.rs"]
mod test_env_child;
