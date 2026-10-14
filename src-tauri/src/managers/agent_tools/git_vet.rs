// M2 stub: bodies arrive in M2-1..M2-3, this allow is removed in M2-3
#![allow(dead_code)]

//! Git vetting and argv construction (DESIGN R2-1, R3-1). Refuses any repo whose
//! `.git/config` is outside the strict allowlist, before git is ever spawned.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VetError {
    /// Names the section.key only, never values (R2-1).
    ForbiddenSetting(String),
    NotARepo,
    /// `.git` is a file, a symlink, or has commondir/alternates/config.worktree.
    UnsupportedLayout(String),
    /// Config over 64 KiB or 500 lines, non-UTF-8, NUL, or outside the grammar (R3-1).
    MalformedConfig,
    GitMissing,
}

/// Checks the project directory without executing anything (R2-1, R3-1 grammar).
pub fn vet_git_config(project_dir: &Path) -> Result<(), VetError> {
    let _ = project_dir;
    todo!()
}

/// Builds the git command: cleared env, fixed trusted PATH, no inherited GIT_* (R2-1, R3-1).
pub fn build_git_command(project_dir: &Path, args: &[&str]) -> Result<Command, VetError> {
    let _ = (project_dir, args);
    todo!()
}

/// Porcelain status (<= 40 lines plus "(N more)") and last 5 commits, redacted.
pub async fn git_status_summary(project_name: &str, project_dir: &Path) -> Result<String, String> {
    let _ = (project_name, project_dir);
    todo!()
}

#[cfg(test)]
#[path = "git_vet_tests.rs"]
mod git_vet_tests;
