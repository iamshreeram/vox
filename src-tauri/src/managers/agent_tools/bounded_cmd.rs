// M2 stub: bodies arrive in M2-1..M2-3, this allow is removed in M2-3
#![allow(dead_code)]

//! Bounded subprocess runner (DESIGN R2-7): concurrent capped stdout and stderr drains, a
//! process group killed on success, error, overflow, timeout or drop.

use std::path::Path;
use std::time::Duration;

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
pub async fn run_bounded(
    program: &Path,
    args: &[String],
    cwd: &Path,
    env: &[(String, String)],
    stdout_cap: usize,
    timeout: Duration,
) -> Result<BoundedOutput, RunError> {
    let _ = (program, args, cwd, env, stdout_cap, timeout);
    todo!()
}
