//! Git vetting and argv construction (DESIGN R2-1, R3-1). Refuses any repo whose
//! `.git/config` is outside the strict allowlist, before git is ever spawned.

use std::path::Path;
use std::process::Command;

#[cfg(unix)]
use super::bounded_cmd::{run_bounded, BoundedOutput, RunError};
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VetError {
    /// Names the section.key only, never values (R2-1).
    ForbiddenSetting(String),
    NotARepo,
    /// `.git` is a file, a symlink, or has commondir/alternates/config.worktree.
    UnsupportedLayout(String),
    /// Config over 64 KiB or 500 lines, non-UTF-8, NUL, or outside the grammar (R3-1).
    MalformedConfig,
    /// Only constructed on the non-unix stub path.
    #[allow(dead_code)]
    GitMissing,
}

/// Fixed trusted absolute git locations, first existing regular executable wins (R3-1).
#[cfg(unix)]
const TRUSTED_GIT: [&str; 5] = [
    "/usr/bin/git",
    "/opt/homebrew/bin/git",
    "/usr/local/bin/git",
    "/run/current-system/sw/bin/git",
    "/nix/var/nix/profiles/default/bin/git",
];

/// Fixed PATH given to the git child (R3-1).
#[cfg(unix)]
const FIXED_PATH: &str = "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin";

/// The complete child environment; nothing inherited from the parent (R2-1).
#[cfg(unix)]
const GIT_ENV: [(&str, &str); 10] = [
    ("PATH", FIXED_PATH),
    ("HOME", "/nonexistent"),
    ("LC_ALL", "C"),
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_SYSTEM", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_ATTR_NOSYSTEM", "1"),
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_PAGER", "cat"),
];

#[cfg(unix)]
const MAX_CONFIG_BYTES: usize = 64 * 1024;
#[cfg(unix)]
const MAX_CONFIG_LINES: usize = 500;
/// Git-dir entries whose presence changes what a repo reads; any of them refuses.
#[cfg(unix)]
const REFUSED_LAYOUT: [&str; 3] = ["commondir", "config.worktree", "objects/info/alternates"];
/// Allowed `core.*` keys (R2-1).
#[cfg(unix)]
const CORE_KEYS: [&str; 8] = [
    "repositoryformatversion",
    "filemode",
    "bare",
    "logallrefupdates",
    "ignorecase",
    "precomposeunicode",
    "symlinks",
    "quotepath",
];
#[cfg(unix)]
const STATUS_ARGS: [&str; 5] = [
    "status",
    "--porcelain=v1",
    "--branch",
    "--no-renames",
    "--ignore-submodules=all",
];
#[cfg(unix)]
const LOG_ARGS: [&str; 4] = ["log", "-n", "5", "--format=%h %s"];
#[cfg(unix)]
const STATUS_CAP: usize = 64 * 1024;
#[cfg(unix)]
const GIT_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(unix)]
const STATUS_LINE_CAP: usize = 40;
#[cfg(unix)]
const LOG_LINE_CAP: usize = 5;

const GIT_MISSING: &str = "git is not installed";
/// Reason text for the non-unix stub; unused on unix builds.
#[allow(dead_code)]
const UNSUPPORTED: &str = "not supported on this platform yet";

/// Checks the project directory without executing anything (R2-1, R3-1 grammar).
#[cfg(unix)]
pub fn vet_git_config(project_dir: &Path) -> Result<(), VetError> {
    vet_layout(project_dir)?;
    let bytes = read_config_bytes(project_dir)?;
    vet_config_bytes(&bytes)
}

/// Non-unix stub: git vetting is only implemented for unix in M2.
#[cfg(not(unix))]
pub fn vet_git_config(_project_dir: &Path) -> Result<(), VetError> {
    Err(VetError::UnsupportedLayout(UNSUPPORTED.to_string()))
}

/// Builds the git command: cleared env, fixed trusted PATH, no inherited GIT_* (R2-1, R3-1).
/// Production runs git through `run_git`; this builder is the inspectable form the tests use.
#[cfg(unix)]
#[allow(dead_code)]
pub fn build_git_command(project_dir: &Path, args: &[&str]) -> Result<Command, VetError> {
    let program = trusted_git().ok_or(VetError::GitMissing)?;
    let mut cmd = Command::new(program);
    cmd.args(git_argv(project_dir, args))
        .env_clear()
        .envs(git_env())
        .current_dir(project_dir);
    Ok(cmd)
}

/// Non-unix stub: git is never run on non-unix in M2.
#[cfg(not(unix))]
pub fn build_git_command(_project_dir: &Path, _args: &[&str]) -> Result<Command, VetError> {
    Err(VetError::GitMissing)
}

/// Porcelain status (<= 40 lines plus "(N more)") and last 5 commits, redacted.
#[cfg(unix)]
pub async fn git_status_summary(project_name: &str, project_dir: &Path) -> Result<String, String> {
    if trusted_git().is_none() {
        return Err(GIT_MISSING.to_string());
    }
    vet_git_config(project_dir).map_err(|e| vet_message(project_name, &e))?;
    let status = run_git(project_dir, &STATUS_ARGS)
        .await
        .map_err(|e| run_message(project_name, &e))?;
    if status.exit_code != Some(0) {
        return Err(format!("git status failed in {project_name}"));
    }
    // A repo without commits makes log exit non-zero; the summary then omits commits.
    let log = run_git(project_dir, &LOG_ARGS)
        .await
        .ok()
        .filter(|o| o.exit_code == Some(0));
    Ok(format_summary(project_name, &status, log.as_ref()))
}

/// Non-unix stub: porcelain status is only implemented for unix in M2.
#[cfg(not(unix))]
pub async fn git_status_summary(
    _project_name: &str,
    _project_dir: &Path,
) -> Result<String, String> {
    Err(UNSUPPORTED.to_string())
}

/// First trusted absolute git that exists as a regular executable file (R3-1).
#[cfg(unix)]
fn trusted_git() -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    TRUSTED_GIT.iter().map(PathBuf::from).find(|p| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

/// Full argv: fixed safety flags, pinned git-dir and work-tree, then the caller args.
#[cfg(unix)]
fn git_argv(project_dir: &Path, args: &[&str]) -> Vec<String> {
    let mut argv: Vec<String> = [
        "--no-optional-locks",
        "--no-pager",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.untrackedCache=false",
        "-c",
        "core.hooksPath=/dev/null",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    argv.push(format!("--git-dir={}/.git", project_dir.display()));
    argv.push(format!("--work-tree={}", project_dir.display()));
    argv.extend(args.iter().map(|s| s.to_string()));
    argv
}

/// The explicit child environment as owned pairs.
#[cfg(unix)]
fn git_env() -> Vec<(String, String)> {
    GIT_ENV
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Runs git through the bounded runner with the vetted argv and env.
#[cfg(unix)]
async fn run_git(project_dir: &Path, args: &[&str]) -> Result<BoundedOutput, RunError> {
    let program = trusted_git().ok_or_else(|| RunError::Spawn(GIT_MISSING.to_string()))?;
    run_bounded(
        &program,
        &git_argv(project_dir, args),
        project_dir,
        &git_env(),
        STATUS_CAP,
        GIT_TIMEOUT,
    )
    .await
}

/// Rejects anything that is not a plain `.git` directory with no redirecting entries.
#[cfg(unix)]
fn vet_layout(project_dir: &Path) -> Result<(), VetError> {
    let git_dir = project_dir.join(".git");
    let meta = std::fs::symlink_metadata(&git_dir).map_err(|_| VetError::NotARepo)?;
    if !meta.is_dir() {
        return Err(VetError::UnsupportedLayout(
            ".git is not a plain directory".to_string(),
        ));
    }
    for name in REFUSED_LAYOUT {
        if std::fs::symlink_metadata(git_dir.join(name)).is_ok() {
            return Err(VetError::UnsupportedLayout(name.to_string()));
        }
    }
    Ok(())
}

/// Reads `.git/config` as a regular file no larger than the byte cap.
#[cfg(unix)]
fn read_config_bytes(project_dir: &Path) -> Result<Vec<u8>, VetError> {
    use std::io::Read;
    let path = project_dir.join(".git").join("config");
    let meta = std::fs::symlink_metadata(&path).map_err(|_| VetError::MalformedConfig)?;
    if !meta.is_file() || meta.len() > MAX_CONFIG_BYTES as u64 {
        return Err(VetError::MalformedConfig);
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .and_then(|f| f.take(MAX_CONFIG_BYTES as u64 + 1).read_to_end(&mut bytes))
        .map_err(|_| VetError::MalformedConfig)?;
    Ok(bytes)
}

/// A section header seen so far. `allowed` is decided at the header; `keyed` records
/// whether any key line followed it (a forbidden header with no keys still refuses).
#[cfg(unix)]
struct Scope {
    name: String,
    sub: bool,
    allowed: bool,
    keyed: bool,
}

/// Fail-closed grammar and allowlist check over the raw config bytes (R3-1).
#[cfg(unix)]
fn vet_config_bytes(bytes: &[u8]) -> Result<(), VetError> {
    if bytes.len() > MAX_CONFIG_BYTES || bytes.contains(&0) {
        return Err(VetError::MalformedConfig);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| VetError::MalformedConfig)?;
    if text.lines().count() > MAX_CONFIG_LINES {
        return Err(VetError::MalformedConfig);
    }
    let mut current: Option<Scope> = None;
    for raw in text.lines() {
        let line = raw.trim_start_matches(is_blank);
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.contains('\\') {
            return Err(VetError::MalformedConfig);
        }
        if line.starts_with('[') {
            finish_scope(current.take())?;
            current = Some(parse_header(line)?);
            continue;
        }
        if line.contains('"') {
            return Err(VetError::MalformedConfig);
        }
        let key = parse_key(line).ok_or(VetError::MalformedConfig)?;
        let scope = current.as_mut().ok_or(VetError::MalformedConfig)?;
        scope.keyed = true;
        let key_lc = key.to_ascii_lowercase();
        if !scope.allowed || !key_allowed(scope, &key_lc) {
            return Err(VetError::ForbiddenSetting(format!(
                "{}.{}",
                scope.name, key_lc
            )));
        }
    }
    finish_scope(current)
}

/// A forbidden header that never received a key is refused by its section name.
#[cfg(unix)]
fn finish_scope(scope: Option<Scope>) -> Result<(), VetError> {
    match scope {
        Some(s) if !s.allowed && !s.keyed => Err(VetError::ForbiddenSetting(s.name)),
        _ => Ok(()),
    }
}

/// Parses `[name]` or `[name "sub"]` exactly; legacy dotted names and trailing text fail.
#[cfg(unix)]
fn parse_header(line: &str) -> Result<Scope, VetError> {
    let inner = line
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or(VetError::MalformedConfig)?;
    let (name, sub) = match inner.find('"') {
        None => (inner, false),
        Some(pos) => {
            let before = &inner[..pos];
            let name = before.trim_end_matches(is_blank);
            if name.len() == before.len() {
                return Err(VetError::MalformedConfig);
            }
            let sub_text = inner[pos + 1..]
                .strip_suffix('"')
                .ok_or(VetError::MalformedConfig)?;
            if !sub_text.chars().all(is_sub_char) {
                return Err(VetError::MalformedConfig);
            }
            (name, true)
        }
    };
    if !valid_name(name) {
        return Err(VetError::MalformedConfig);
    }
    let name = name.to_ascii_lowercase();
    Ok(Scope {
        allowed: section_allowed(&name, sub),
        name,
        sub,
        keyed: false,
    })
}

/// Parses `<ws>key<ws>=<ws>value` and returns the key; no `=` is refused.
#[cfg(unix)]
fn parse_key(line: &str) -> Option<&str> {
    let (lhs, _value) = line.split_once('=')?;
    let key = lhs.trim_matches(is_blank);
    valid_name(key).then_some(key)
}

/// Section names: ASCII letter first, then letters, digits or hyphens (no dots).
#[cfg(unix)]
fn valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[cfg(unix)]
fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

#[cfg(unix)]
fn is_sub_char(c: char) -> bool {
    (' '..='~').contains(&c) && c != '"' && c != '\\'
}

/// Sections permitted at all, and whether they must carry a subsection (R2-1).
#[cfg(unix)]
fn section_allowed(name: &str, sub: bool) -> bool {
    match name {
        "core" | "user" | "init" | "gc" | "pull" | "push" => !sub,
        "remote" | "branch" | "submodule" => sub,
        _ => false,
    }
}

/// Key allowlist for an already-allowed section (R2-1).
#[cfg(unix)]
fn key_allowed(scope: &Scope, key: &str) -> bool {
    match (scope.name.as_str(), scope.sub) {
        ("core", false) => CORE_KEYS.contains(&key),
        ("remote", true) => matches!(key, "url" | "pushurl" | "fetch"),
        ("branch", true) => matches!(key, "remote" | "merge" | "rebase"),
        ("user", false) => matches!(key, "name" | "email"),
        ("init" | "gc" | "pull" | "push", false) => true,
        ("submodule", true) => matches!(key, "url" | "active"),
        _ => false,
    }
}

/// Renders the bounded, redacted summary text.
#[cfg(unix)]
fn format_summary(
    project_name: &str,
    status: &BoundedOutput,
    log: Option<&BoundedOutput>,
) -> String {
    let text = String::from_utf8_lossy(&status.stdout);
    let mut branch = String::from("(unknown)");
    let mut changes: Vec<&str> = Vec::new();
    for line in text.lines() {
        if let Some(head) = line.strip_prefix("## ") {
            branch = head.to_string();
        } else if !line.is_empty() {
            changes.push(line);
        }
    }
    let mut out = format!("Project: {project_name}\nBranch: {branch}\n");
    if changes.is_empty() {
        out.push_str("Working tree clean.\n");
    } else {
        out.push_str(&format!("Changes ({}):\n", changes.len()));
        for line in changes.iter().take(STATUS_LINE_CAP) {
            out.push_str(line);
            out.push('\n');
        }
        if changes.len() > STATUS_LINE_CAP {
            out.push_str(&format!("({} more)\n", changes.len() - STATUS_LINE_CAP));
        }
    }
    if status.stdout_truncated {
        out.push_str("(status output was cut short)\n");
    }
    if let Some(log) = log {
        let subjects: Vec<String> = String::from_utf8_lossy(&log.stdout)
            .lines()
            .take(LOG_LINE_CAP)
            .map(str::to_string)
            .collect();
        if !subjects.is_empty() {
            out.push_str("Recent commits:\n");
            for subject in subjects {
                out.push_str(&subject);
                out.push('\n');
            }
        }
    }
    crate::managers::redact::redact(&out)
}

/// Fixed refusal text for a vetting failure; names the project and key only.
#[cfg(unix)]
fn vet_message(project_name: &str, err: &VetError) -> String {
    match err {
        VetError::ForbiddenSetting(name) => {
            format!("git settings in {project_name} include {name}; I won't run git there")
        }
        VetError::NotARepo => format!("{project_name} is not a git repository"),
        VetError::UnsupportedLayout(_) => {
            format!("{project_name} uses an unsupported git layout; I won't run git there")
        }
        VetError::MalformedConfig => format!(
            "git settings in {project_name} could not be checked safely; I won't run git there"
        ),
        VetError::GitMissing => GIT_MISSING.to_string(),
    }
}

/// Fixed message for a runner failure.
#[cfg(unix)]
fn run_message(project_name: &str, err: &RunError) -> String {
    match err {
        RunError::Timeout => format!("git timed out in {project_name}"),
        RunError::Spawn(msg) if msg == GIT_MISSING => GIT_MISSING.to_string(),
        _ => format!("could not run git in {project_name}"),
    }
}

#[cfg(test)]
#[path = "git_vet_tests.rs"]
mod git_vet_tests;

#[cfg(test)]
#[path = "test_env_child.rs"]
mod test_env_child;
