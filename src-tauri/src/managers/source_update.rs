//! "New version available" for builds installed from a local checkout.
//!
//! Design in one paragraph: a build made through `scripts/update-macos.sh`
//! embeds the git commit it was built from (plus an explicit opt-in flag).
//! Vox asks GitHub for the newest commits on `main`, compares, and tells the
//! UI whether the running build is behind. Installing starts a detached,
//! one-shot launchd job that runs the same update script (see
//! `source_update_runner`), outside Vox's own process tree, because the
//! installer stops and restarts Vox. Progress comes back through the status
//! file and is published as `source-update-progress`.

use log::{info, warn};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

use super::source_update_runner::{
    self as runner, file_age_secs, now_unix, plist_path, proxy_env_from, read_status_bounded,
    run_id_in_status, status_path, update_state_dir, JobControl, ReconcileInput, RunPhase,
    UpdateProgress, INTERRUPTED_AFTER_SECS,
};
use super::source_update_runner_poller::{self as poller, PollerConfig};
use super::source_update_runner_run::{
    adopt_plan, cleanup_run, launch, start_gate, AdoptPlan, LaunchInputs, StartGate,
};

/// `owner/repo` this build tracks.
pub const REPO_SLUG: &str = "iamshreeram/vox";
/// Newest commits requested from GitHub in one call.
pub const COMMIT_WINDOW: usize = 100;
/// Most change subjects shown to the user.
pub const MAX_CHANGES: usize = 5;
/// Longest change subject, in characters (including a trailing ellipsis).
pub const MAX_SUBJECT_CHARS: usize = 100;

/// Which update flow the footer should use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SourceUpdateMode {
    /// Rebuild-from-checkout flow (this module).
    Source,
    /// The pre-existing Tauri updater-plugin flow, untouched.
    Plugin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SourceUpdateState {
    UpToDate,
    Available,
    /// The build cannot be related to `main` (history rewritten, commit not on
    /// GitHub, diverged). Never installable, never prompts automatically.
    Untracked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SourceUpdateInfo {
    pub state: SourceUpdateState,
    /// First 7 hex characters of the commit this build was made from.
    pub current_commit: String,
    /// First 7 hex characters of the newest `main` commit.
    pub latest_commit: String,
    /// Full 40-hex SHA of the newest `main` commit, or empty when unknown.
    /// The update builds exactly this commit (see `pinned_target`).
    pub latest_full: String,
    /// Number of commits `main` is ahead of this build.
    pub behind_by: u32,
    /// Up to [`MAX_CHANGES`] newest-first commit subjects.
    pub changes: Vec<String>,
    /// GitHub compare page built locally from validated full SHAs.
    pub compare_url: String,
    /// Human explanation for `Untracked`.
    pub note: Option<String>,
    /// Filled in by the caller from [`install_blocker`]; `evaluate_*` leave it
    /// `false`/`None`.
    pub can_install: bool,
    pub install_blocker: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCommit {
    pub sha: String,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheckError {
    Network(String),
    RateLimited,
    Http(u16),
    Malformed(String),
    EmptyHistory,
    BadBuildCommit,
    TooLarge,
}

impl UpdateCheckError {
    /// Short, user-presentable sentence.
    pub fn user_message(&self) -> String {
        match self {
            Self::RateLimited => {
                "GitHub's rate limit was reached. Try again in a few minutes.".to_string()
            }
            Self::Network(_) => {
                "Could not reach GitHub to check for updates. Check your connection.".to_string()
            }
            Self::Http(status) => format!(
                "GitHub returned an unexpected response (HTTP {status}) while checking for updates."
            ),
            Self::Malformed(_) => {
                "GitHub returned a response Vox could not read while checking for updates."
                    .to_string()
            }
            Self::EmptyHistory => "GitHub returned no commits for main.".to_string(),
            Self::BadBuildCommit => {
                "This build does not record a valid source commit, so updates cannot be checked."
                    .to_string()
            }
            Self::TooLarge => "The update is too large to check automatically.".to_string(),
        }
    }
}

/// Outcome of looking for the build commit inside the newest-commits window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowEvaluation {
    /// Fully decided (up to date, or behind by an exact count).
    Resolved(SourceUpdateInfo),
    /// The build commit is not in the window: ask GitHub's compare API.
    NotInWindow {
        latest_sha: String,
        changes: Vec<String>,
    },
}

/// GitHub compare `status` (head = main, base = build commit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareStatus {
    /// `main` has `ahead_by` commits the build lacks.
    Ahead {
        ahead_by: u32,
    },
    Identical,
    /// The build is ahead of `main`.
    Behind,
    Diverged,
}

pub fn is_full_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn short_sha(s: &str) -> String {
    s.chars().take(7).collect::<String>().to_lowercase()
}

pub fn commits_api_url() -> String {
    format!("https://api.github.com/repos/{REPO_SLUG}/commits?sha=main&per_page={COMMIT_WINDOW}")
}

/// `None` unless `build_sha` is a full 40-hex SHA.
pub fn compare_api_url(build_sha: &str) -> Option<String> {
    if !is_full_sha(build_sha) {
        return None;
    }
    Some(format!(
        "https://api.github.com/repos/{REPO_SLUG}/compare/{build_sha}...main?per_page=1"
    ))
}

/// `None` unless both are full SHAs.
pub fn compare_page_url(build_sha: &str, latest_sha: &str) -> Option<String> {
    if !is_full_sha(build_sha) || !is_full_sha(latest_sha) {
        return None;
    }
    Some(format!(
        "https://github.com/{REPO_SLUG}/compare/{build_sha}...{latest_sha}"
    ))
}

/// `None` for 200, otherwise the error to report.
pub fn classify_status(status: u16) -> Option<UpdateCheckError> {
    match status {
        200 => None,
        403 | 429 => Some(UpdateCheckError::RateLimited),
        other => Some(UpdateCheckError::Http(other)),
    }
}

/// First line of `msg`, trimmed; if longer than `max_chars` characters it is
/// cut to `max_chars - 1` characters plus `…` (never splits a character).
pub fn first_line_truncated(msg: &str, max_chars: usize) -> String {
    let first = msg.split('\n').next().unwrap_or("");
    let first = first.strip_suffix('\r').unwrap_or(first);
    let line = first.trim();
    if line.chars().count() <= max_chars {
        return line.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    let mut out: String = line.chars().take(max_chars - 1).collect();
    out.push('…');
    out
}

fn malformed(reason: &str) -> UpdateCheckError {
    UpdateCheckError::Malformed(reason.to_string())
}

pub fn parse_commits_response(body: &str) -> Result<Vec<RemoteCommit>, UpdateCheckError> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|_| malformed("body is not JSON"))?;
    let items = value
        .as_array()
        .ok_or_else(|| malformed("body is not an array"))?;
    items
        .iter()
        .map(|item| {
            let sha = item
                .get("sha")
                .and_then(|v| v.as_str())
                .ok_or_else(|| malformed("commit has no sha"))?;
            if !is_full_sha(sha) {
                return Err(malformed("commit sha is not a full SHA"));
            }
            let message = item
                .get("commit")
                .and_then(|c| c.get("message"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| malformed("commit has no message"))?;
            let subject = first_line_truncated(message, MAX_SUBJECT_CHARS);
            let subject = if subject.is_empty() {
                "(no message)".to_string()
            } else {
                subject
            };
            Ok(RemoteCommit {
                sha: sha.to_string(),
                subject,
            })
        })
        .collect()
}

pub fn parse_compare_response(body: &str) -> Result<CompareStatus, UpdateCheckError> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|_| malformed("body is not JSON"))?;
    let status = value
        .get("status")
        .and_then(|v| v.as_str())
        .ok_or_else(|| malformed("compare response has no status"))?;
    match status {
        "ahead" => {
            let ahead_by = value
                .get("ahead_by")
                .and_then(|v| v.as_u64())
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| malformed("ahead_by is not a valid count"))?;
            Ok(CompareStatus::Ahead { ahead_by })
        }
        "identical" => Ok(CompareStatus::Identical),
        "behind" => Ok(CompareStatus::Behind),
        "diverged" => Ok(CompareStatus::Diverged),
        _ => Err(malformed("unknown compare status")),
    }
}

/// `remote` is newest-first, as GitHub returns it.
pub fn evaluate_window(
    build_sha: &str,
    remote: &[RemoteCommit],
) -> Result<WindowEvaluation, UpdateCheckError> {
    if !is_full_sha(build_sha) {
        return Err(UpdateCheckError::BadBuildCommit);
    }
    let Some(latest) = remote.first() else {
        return Err(UpdateCheckError::EmptyHistory);
    };
    let Some(index) = remote
        .iter()
        .position(|c| c.sha.eq_ignore_ascii_case(build_sha))
    else {
        return Ok(WindowEvaluation::NotInWindow {
            latest_sha: latest.sha.clone(),
            changes: subjects(remote),
        });
    };
    let compare_url = compare_page_url(build_sha, &latest.sha).unwrap_or_default();
    if index == 0 {
        return Ok(WindowEvaluation::Resolved(SourceUpdateInfo {
            state: SourceUpdateState::UpToDate,
            current_commit: short_sha(build_sha),
            latest_commit: short_sha(&latest.sha),
            latest_full: latest.sha.clone(),
            behind_by: 0,
            changes: Vec::new(),
            compare_url,
            note: None,
            can_install: false,
            install_blocker: None,
        }));
    }
    Ok(WindowEvaluation::Resolved(SourceUpdateInfo {
        state: SourceUpdateState::Available,
        current_commit: short_sha(build_sha),
        latest_commit: short_sha(&latest.sha),
        latest_full: latest.sha.clone(),
        behind_by: u32::try_from(index).unwrap_or(u32::MAX),
        changes: subjects(&remote[..index]),
        compare_url,
        note: None,
        can_install: false,
        install_blocker: None,
    }))
}

/// Newest-first subjects, capped at [`MAX_CHANGES`].
fn subjects(commits: &[RemoteCommit]) -> Vec<String> {
    commits
        .iter()
        .take(MAX_CHANGES)
        .map(|c| c.subject.clone())
        .collect()
}

/// Finish a [`WindowEvaluation::NotInWindow`] using the compare API result.
/// `Err(Http(404))` means GitHub does not know the build commit.
pub fn resolve_compare(
    build_sha: &str,
    latest_sha: &str,
    changes: Vec<String>,
    compare: Result<CompareStatus, UpdateCheckError>,
) -> Result<SourceUpdateInfo, UpdateCheckError> {
    let current_commit = short_sha(build_sha);
    let latest_commit = short_sha(latest_sha);
    let latest_full = if is_full_sha(latest_sha) {
        latest_sha.to_string()
    } else {
        String::new()
    };
    let compare_url = compare_page_url(build_sha, latest_sha).unwrap_or_default();
    let untracked = |note: &str| SourceUpdateInfo {
        state: SourceUpdateState::Untracked,
        current_commit: current_commit.clone(),
        latest_commit: latest_commit.clone(),
        latest_full: latest_full.clone(),
        behind_by: 0,
        changes: Vec::new(),
        compare_url: compare_url.clone(),
        note: Some(note.to_string()),
        can_install: false,
        install_blocker: None,
    };
    match compare {
        Ok(CompareStatus::Ahead { ahead_by }) => Ok(SourceUpdateInfo {
            state: SourceUpdateState::Available,
            current_commit: current_commit.clone(),
            latest_commit: latest_commit.clone(), latest_full: latest_full.clone(),
            behind_by: ahead_by,
            changes,
            compare_url: compare_url.clone(),
            note: None,
            can_install: false,
            install_blocker: None,
        }),
        Ok(CompareStatus::Identical | CompareStatus::Behind) => Ok(SourceUpdateInfo {
            state: SourceUpdateState::UpToDate,
            current_commit: current_commit.clone(),
            latest_commit: latest_commit.clone(), latest_full: latest_full.clone(),
            behind_by: 0,
            changes: Vec::new(),
            compare_url: compare_url.clone(),
            note: None,
            can_install: false,
            install_blocker: None,
        }),
        Ok(CompareStatus::Diverged) => Ok(untracked(
            "This build's history has diverged from main, so it cannot be updated automatically.",
        )),
        Err(UpdateCheckError::Http(404)) => Ok(untracked(
            "GitHub does not know this build's commit (history may have been rewritten), so updates cannot be tracked.",
        )),
        Err(other) => Err(other),
    }
}

/// Source mode requires the explicit opt-in, macOS, a full commit SHA and the
/// `main` branch; everything else keeps the existing plugin flow.
pub fn update_mode(opted_in: bool, is_macos: bool, commit: &str, branch: &str) -> SourceUpdateMode {
    if opted_in && is_macos && is_full_sha(commit) && branch == "main" {
        SourceUpdateMode::Source
    } else {
        SourceUpdateMode::Plugin
    }
}

/// Exact-membership check of the `origin` URL.
pub fn origin_matches(url: &str) -> bool {
    const ORIGIN_URLS: [&str; 6] = [
        "https://github.com/iamshreeram/vox",
        "https://github.com/iamshreeram/vox.git",
        "git@github.com:iamshreeram/vox",
        "git@github.com:iamshreeram/vox.git",
        "ssh://git@github.com/iamshreeram/vox",
        "ssh://git@github.com/iamshreeram/vox.git",
    ];
    ORIGIN_URLS.contains(&url)
}

/// Facts gathered by IO, judged by the pure [`install_blocker`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallFacts {
    pub is_macos: bool,
    pub checks_disabled: bool,
    pub source_dir_exists: bool,
    pub script_exists: bool,
    pub origin_url: Option<String>,
    pub branch: Option<String>,
    /// `None` = git could not tell.
    pub tracked_changes: Option<bool>,
    /// Path of an existing update lock directory, if any.
    pub update_lock: Option<String>,
}

/// First reason an install cannot start, in a fixed priority order; `None`
/// when everything is fine.
pub fn install_blocker(facts: &InstallFacts) -> Option<String> {
    if !facts.is_macos {
        return Some("Rebuild updates are only available on macOS.".to_string());
    }
    if facts.checks_disabled {
        return Some("Update checks are disabled.".to_string());
    }
    if !facts.source_dir_exists {
        return Some("The source checkout this build came from no longer exists.".to_string());
    }
    if !facts.script_exists {
        return Some("scripts/update-macos.sh is missing from the checkout.".to_string());
    }
    if !facts.origin_url.as_deref().is_some_and(origin_matches) {
        return Some("The checkout's origin remote is not github.com/iamshreeram/vox.".to_string());
    }
    match facts.branch.as_deref() {
        Some("main") => {}
        Some(branch) => {
            return Some(format!(
                "The checkout is on branch '{branch}'; switch it to 'main' to update."
            ));
        }
        None => {
            return Some(
                "Could not determine the checkout's branch (detached HEAD?); switch it to 'main'."
                    .to_string(),
            );
        }
    }
    match facts.tracked_changes {
        Some(false) => {}
        Some(true) => {
            return Some("The checkout has uncommitted changes to tracked files.".to_string());
        }
        None => {
            return Some("Could not read the checkout's working tree state.".to_string());
        }
    }
    facts.update_lock.as_ref().map(|path| {
        format!(
            "A previous update still holds its lock at {path}. If no update is running, remove it with: rm -rf {path}"
        )
    })
}

/// Runs `git -C <dir> <args>` without prompts or optional index locks.
/// `None` when git cannot be started or exits unsuccessfully.
fn run_git(dir: &Path, args: &[&str]) -> Option<std::process::Output> {
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
}

pub fn git_origin_url(dir: &Path) -> Option<String> {
    let out = run_git(dir, &["remote", "get-url", "origin"])?;
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if url.is_empty() {
        None
    } else {
        Some(url)
    }
}

/// `None` for a detached HEAD or when git fails.
pub fn git_current_branch(dir: &Path) -> Option<String> {
    let out = run_git(dir, &["branch", "--show-current"])?;
    let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}

/// Uncommitted changes to TRACKED files only (untracked files are ignored,
/// matching the update script). `None` when git fails.
pub fn git_tracked_changes(dir: &Path) -> Option<bool> {
    run_git(dir, &["status", "--porcelain", "--untracked-files=no"])
        .map(|out| !out.stdout.is_empty())
}

/// `${TMPDIR:-/tmp}/vox-update.lock`, the directory `update-macos.sh` holds
/// while an update runs.
pub fn update_lock_path() -> String {
    let base = match std::env::var("TMPDIR") {
        Ok(dir) if !dir.is_empty() => {
            let trimmed = dir.trim_end_matches('/');
            if trimmed.is_empty() {
                "/tmp".to_string()
            } else {
                trimmed.to_string()
            }
        }
        _ => "/tmp".to_string(),
    };
    format!("{base}/vox-update.lock")
}

/// Double-click protection for launching an update. This is only UX; the real
/// mutual exclusion is the lock taken by `update-macos.sh`.
pub struct LaunchGuard {
    window: Duration,
    last: Mutex<Option<Instant>>,
}

impl LaunchGuard {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            last: Mutex::new(None),
        }
    }

    /// `true` if a launch may proceed now (and reserves it).
    pub fn try_acquire(&self, now: Instant) -> bool {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(previous) = *last {
            if now.saturating_duration_since(previous) < self.window {
                return false;
            }
        }
        *last = Some(now);
        true
    }

    /// Drop the reservation (e.g. starting the update job failed).
    pub fn release(&self) {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        *last = None;
    }
}

// ---- runtime: build metadata, network, install facts, orchestration --------

/// Full commit this build was made from (`unknown` for a non-git build).
const BUILD_COMMIT: &str = env!("VOX_BUILD_COMMIT");
/// Branch this build was made from (`unknown` when not known).
const BUILD_BRANCH: &str = env!("VOX_BUILD_BRANCH");
/// `1` only when the build was made with the explicit opt-in.
const SOURCE_UPDATES_FLAG: &str = env!("VOX_SOURCE_UPDATES");
/// Root of the checkout the build came from.
const SOURCE_DIR: &str = env!("VOX_SOURCE_DIR");

/// Byte cap for the newest-commits response.
const COMMITS_MAX_BYTES: usize = 2 * 1024 * 1024;
/// Byte cap for the compare response.
const COMPARE_MAX_BYTES: usize = 5 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
/// Manual checks within this window of a good check reuse the cached result.
const DEBOUNCE: Duration = Duration::from_secs(10);
/// Minimum gap between two update launches (double-click protection).
const LAUNCH_WINDOW: Duration = Duration::from_secs(5 * 60);
const SCHEDULER_START_DELAY: Duration = Duration::from_secs(20);
const SCHEDULER_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// Tauri event carrying `Option<SourceUpdateInfo>` (`null` when cleared).
const STATUS_EVENT: &str = "source-update-status";
const NOT_AVAILABLE: &str = "Rebuild updates are not available for this build.";
const CHECKS_DISABLED: &str = "Update checks are disabled.";
const NOT_READY: &str = "Update checks are not ready yet.";
const LAUNCH_BUSY: &str = "An update was just started; wait a moment before trying again.";
const LAUNCH_FAILED: &str = "Could not start the update.";
const ALREADY_RUNNING: &str = "An update is already running.";
const NOT_PINNED: &str = "Check for updates again before updating.";
/// Tauri event carrying the latest [`UpdateProgress`] of the running update.
const PROGRESS_EVENT: &str = "source-update-progress";

/// Which update flow the footer should use for this running build.
pub fn current_mode() -> SourceUpdateMode {
    update_mode(
        SOURCE_UPDATES_FLAG == "1",
        cfg!(target_os = "macos"),
        BUILD_COMMIT,
        BUILD_BRANCH,
    )
}

/// GET `url` and return `(status, body)` without interpreting the status.
///
/// The body is read incrementally and the call fails with [`UpdateCheckError::TooLarge`]
/// as soon as more than `max_bytes` arrive. Transport failures (including the
/// timeout) become [`UpdateCheckError::Network`]; the UI never shows that text.
pub async fn fetch_text(
    url: &str,
    max_bytes: usize,
    timeout: Duration,
) -> Result<(u16, String), UpdateCheckError> {
    let network = |e: reqwest::Error| UpdateCheckError::Network(e.to_string());
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(format!("vox-update-check/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(network)?;
    let mut response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(network)?;
    let status = response.status().as_u16();
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network)? {
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(UpdateCheckError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok((status, String::from_utf8_lossy(&body).into_owned()))
}

/// The injectable orchestration: `fetch(url, max_bytes)` returns `(status, body)`.
///
/// Makes one request for the newest commits. If the build commit is in that
/// window the answer is decided locally; otherwise one compare request decides.
pub async fn check_with<F, Fut>(
    build_sha: &str,
    mut fetch: F,
) -> Result<SourceUpdateInfo, UpdateCheckError>
where
    F: FnMut(String, usize) -> Fut,
    Fut: Future<Output = Result<(u16, String), UpdateCheckError>>,
{
    let (status, body) = fetch(commits_api_url(), COMMITS_MAX_BYTES).await?;
    if let Some(err) = classify_status(status) {
        return Err(err);
    }
    let commits = parse_commits_response(&body)?;
    match evaluate_window(build_sha, &commits)? {
        WindowEvaluation::Resolved(info) => Ok(info),
        WindowEvaluation::NotInWindow {
            latest_sha,
            changes,
        } => {
            let url = compare_api_url(build_sha).ok_or(UpdateCheckError::BadBuildCommit)?;
            let (status, body) = fetch(url, COMPARE_MAX_BYTES).await?;
            let compare = match status {
                200 => parse_compare_response(&body),
                404 => Err(UpdateCheckError::Http(404)),
                other => {
                    return Err(classify_status(other).unwrap_or(UpdateCheckError::Http(other)));
                }
            };
            resolve_compare(build_sha, &latest_sha, changes, compare)
        }
    }
}

/// Gather the facts [`install_blocker`] judges. Blocking (runs git); call it
/// from `spawn_blocking`.
pub fn gather_install_facts(checks_disabled: bool) -> InstallFacts {
    let source = Path::new(SOURCE_DIR);
    let lock = update_lock_path();
    let lock_exists = Path::new(&lock).exists();
    InstallFacts {
        is_macos: cfg!(target_os = "macos"),
        checks_disabled,
        source_dir_exists: source.is_dir(),
        script_exists: source.join("scripts").join("update-macos.sh").is_file(),
        origin_url: git_origin_url(source),
        branch: git_current_branch(source),
        tracked_changes: git_tracked_changes(source),
        update_lock: lock_exists.then_some(lock),
    }
}

/// Managed state for the source-update feature.
pub struct SourceUpdateRuntime {
    cached: Mutex<Option<SourceUpdateInfo>>,
    last_ok: Mutex<Option<Instant>>,
    /// Serializes checks so two callers never run the network path at once.
    check_lock: tokio::sync::Mutex<()>,
    /// Bumped by [`clear`]; an in-flight check started before a bump is discarded.
    generation: AtomicU64,
    guard: LaunchGuard,
    /// Latest progress of the running (or last finished) update.
    progress: Mutex<Option<UpdateProgress>>,
    /// The run this process is watching, if any.
    active: Mutex<Option<ActiveRun>>,
}

/// A run this process started or adopted and is watching.
#[derive(Debug, Clone)]
pub struct ActiveRun {
    /// Run identifier (16 lowercase hex).
    pub run_id: String,
    /// Launchd label of the run's job.
    pub label: String,
    /// When this process started watching the run.
    pub started: Instant,
}

impl SourceUpdateRuntime {
    /// Empty runtime with the standard launch window.
    pub fn new() -> Self {
        Self {
            cached: Mutex::new(None),
            last_ok: Mutex::new(None),
            check_lock: tokio::sync::Mutex::new(()),
            generation: AtomicU64::new(0),
            guard: LaunchGuard::new(LAUNCH_WINDOW),
            progress: Mutex::new(None),
            active: Mutex::new(None),
        }
    }
}

impl Default for SourceUpdateRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// Poison-tolerant lock: a panicked holder never leaves the feature wedged.
fn lock_state<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether update checks are on for this run (persisted setting, minus the
/// `HANDY_DISABLE_UPDATER` override).
pub fn effective_checks_enabled(app: &AppHandle) -> bool {
    let settings = crate::settings::get_settings(app);
    crate::settings::update_checks_effectively_enabled(&settings)
}

/// A finished check may be stored only while checks are still enabled and no
/// [`clear`] happened after it started.
pub fn should_commit(enabled: bool, started_gen: u64, current_gen: u64) -> bool {
    enabled && started_gen == current_gen
}

/// Run a check, publish the result (cache, event, tray) and return it.
///
/// `automatic` skips the manual-check debounce. Errors are user-presentable.
pub async fn check_and_publish(
    app: &AppHandle,
    automatic: bool,
) -> Result<SourceUpdateInfo, String> {
    if current_mode() != SourceUpdateMode::Source {
        return Err(NOT_AVAILABLE.to_string());
    }
    if !effective_checks_enabled(app) {
        return Err(CHECKS_DISABLED.to_string());
    }
    let Some(runtime) = app.try_state::<SourceUpdateRuntime>() else {
        return Err(NOT_READY.to_string());
    };

    let _permit = runtime.check_lock.lock().await;

    if !automatic {
        let recent = lock_state(&runtime.last_ok).is_some_and(|t| t.elapsed() < DEBOUNCE);
        let cached = lock_state(&runtime.cached).clone();
        if let (true, Some(info)) = (recent, cached) {
            return Ok(info);
        }
    }

    let generation = runtime.generation.load(Ordering::SeqCst);
    let mut info = check_with(BUILD_COMMIT, |url: String, max: usize| async move {
        fetch_text(&url, max, FETCH_TIMEOUT).await
    })
    .await
    .map_err(|e| e.user_message())?;

    let checks_disabled = !effective_checks_enabled(app);
    if info.state == SourceUpdateState::Available {
        let facts = tokio::task::spawn_blocking(move || gather_install_facts(checks_disabled))
            .await
            .map_err(|_| "Could not read the checkout's state.".to_string())?;
        let blocker = install_blocker(&facts);
        info.can_install = blocker.is_none();
        info.install_blocker = blocker;
    } else {
        info.can_install = false;
        info.install_blocker = None;
    }

    let current_gen = runtime.generation.load(Ordering::SeqCst);
    if !should_commit(effective_checks_enabled(app), generation, current_gen) {
        return Err("Update checks were disabled.".to_string());
    }

    *lock_state(&runtime.cached) = Some(info.clone());
    *lock_state(&runtime.last_ok) = Some(Instant::now());
    clear_failed_progress(&runtime);
    let _ = app.emit(STATUS_EVENT, Some(info.clone()));
    crate::tray::update_tray_menu(app);
    Ok(info)
}

/// Forget the cached result and invalidate any check still in flight.
pub fn clear(app: &AppHandle) {
    if let Some(runtime) = app.try_state::<SourceUpdateRuntime>() {
        runtime.generation.fetch_add(1, Ordering::SeqCst);
        *lock_state(&runtime.cached) = None;
        *lock_state(&runtime.last_ok) = None;
    }
    let _ = app.emit(STATUS_EVENT, Option::<SourceUpdateInfo>::None);
    crate::tray::update_tray_menu(app);
}

/// The last published result, if any.
pub fn cached(app: &AppHandle) -> Option<SourceUpdateInfo> {
    app.try_state::<SourceUpdateRuntime>()
        .and_then(|runtime| lock_state(&runtime.cached).clone())
}

/// `true` when the cached result says an update is available.
pub fn update_available(app: &AppHandle) -> bool {
    cached(app).is_some_and(|info| info.state == SourceUpdateState::Available)
}

/// Fresh install facts judged now; `None` means an install may start.
pub async fn install_blocker_now(app: &AppHandle) -> Option<String> {
    if current_mode() != SourceUpdateMode::Source {
        return Some(NOT_AVAILABLE.to_string());
    }
    let checks_disabled = !effective_checks_enabled(app);
    match tokio::task::spawn_blocking(move || gather_install_facts(checks_disabled)).await {
        Ok(facts) => install_blocker(&facts),
        Err(_) => Some("Could not read the checkout's state.".to_string()),
    }
}

/// Starts an update in the background; nothing opens a window. An update this
/// process already watches is attached to (its progress is shown). Another
/// update holding the lock is refused. Otherwise the detached runner builds and
/// installs the commit the last check published (`latest_full`, pinned).
pub async fn start_update(app: &AppHandle) -> Result<(), String> {
    if current_mode() != SourceUpdateMode::Source {
        return Err(NOT_AVAILABLE.to_string());
    }
    if !effective_checks_enabled(app) {
        return Err(CHECKS_DISABLED.to_string());
    }
    let Some(runtime) = app.try_state::<SourceUpdateRuntime>() else {
        return Err(NOT_READY.to_string());
    };
    let lock_dir = PathBuf::from(update_lock_path());
    let has_active = lock_state(&runtime.active).is_some();
    match start_gate(
        has_active,
        runner::lock_state(&lock_dir, &runner::process_alive),
    ) {
        StartGate::Attach => {
            if let Some(run) = lock_state(&runtime.active).clone() {
                info!(
                    "attaching to update job {} (run {}, {}s in)",
                    run.label,
                    run.run_id,
                    run.started.elapsed().as_secs()
                );
            }
            return Ok(());
        }
        StartGate::Refuse => return Err(ALREADY_RUNNING.to_string()),
        StartGate::Proceed => {}
    }
    if let Some(blocker) = install_blocker_now(app).await {
        return Err(blocker);
    }
    let target = pinned_target(cached(app).as_ref()).ok_or_else(|| NOT_PINNED.to_string())?;
    if !runtime.guard.try_acquire(Instant::now()) {
        return Err(LAUNCH_BUSY.to_string());
    }
    let config = match launch_run(app, &lock_dir, &target) {
        Ok(config) => config,
        Err(message) => {
            runtime.guard.release();
            return Err(message);
        }
    };
    cache_progress(
        app,
        UpdateProgress {
            run_id: config.run_id.clone(),
            phase: RunPhase::Preparing,
            percent: 0,
            target: short_sha(&target),
            message: "Starting the update.".to_string(),
            estimated: false,
        },
    );
    crate::tray::update_tray_menu(app);
    start_watching(app, &runtime, config);
    info!("Source update started in the background");
    Ok(())
}

/// Writes and bootstraps the detached job for a new run. Errors are logged in
/// full (they never contain secrets) and reported to the user as one short line.
fn launch_run(app: &AppHandle, lock_dir: &Path, target: &str) -> Result<PollerConfig, String> {
    let fail = |reason: String| {
        warn!("update launch failed: {reason}");
        LAUNCH_FAILED.to_string()
    };
    let app_data = crate::portable::app_data_dir(app).map_err(|e| fail(e.to_string()))?;
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| fail("HOME is not set".to_string()))?;
    let run_id = runner::new_run_id();
    if !runner::is_valid_run_id(&run_id) {
        return Err(fail("no randomness for the run id".to_string()));
    }
    let proxies = proxy_env_from(&|name| std::env::var(name).ok());
    let source = PathBuf::from(SOURCE_DIR);
    let tmpdir = std::env::temp_dir();
    let launched = launch(
        &runner::LaunchctlJobControl::current(),
        &LaunchInputs {
            run_id: &run_id,
            app_data: &app_data,
            source_dir: &source,
            target_full: target,
            home: &home,
            tmpdir: &tmpdir,
            lock_dir,
            proxy_env: &proxies,
        },
    )
    .map_err(|e| fail(e.to_string()))?;
    Ok(PollerConfig {
        run_id,
        label: launched.label,
        state_dir: launched.state_dir,
        status_file: launched.status_file,
        plist: launched.plist,
        lock_dir: lock_dir.to_path_buf(),
        running_build_sha: running_build_sha(),
    })
}

/// Records `config` as this process's active run and starts watching it.
fn start_watching(app: &AppHandle, runtime: &SourceUpdateRuntime, config: PollerConfig) {
    info!("watching update job {}", config.label);
    *lock_state(&runtime.active) = Some(ActiveRun {
        run_id: config.run_id.clone(),
        label: config.label.clone(),
        started: Instant::now(),
    });
    poller::spawn(app.clone(), config);
}

/// The full SHA a new update must build: the latest `main` commit from the last
/// check, only when that check says an update is available and the SHA is a
/// lowercase 40-hex value. `None` otherwise.
pub fn pinned_target(info: Option<&SourceUpdateInfo>) -> Option<String> {
    let info = info?;
    let full = &info.latest_full;
    (info.state == SourceUpdateState::Available
        && is_full_sha(full)
        && full.bytes().all(|b| !b.is_ascii_uppercase()))
    .then(|| full.clone())
}

/// Full SHA of the running build, or empty when it is unknown.
pub fn running_build_sha() -> String {
    if is_full_sha(BUILD_COMMIT) {
        BUILD_COMMIT.to_string()
    } else {
        String::new()
    }
}

/// The latest progress of the running (or last finished) update, if any.
pub fn progress(app: &AppHandle) -> Option<UpdateProgress> {
    app.try_state::<SourceUpdateRuntime>()
        .and_then(|runtime| lock_state(&runtime.progress).clone())
}

/// Caches `progress` and emits it as `source-update-progress`. The tray is not
/// refreshed here: the poller coalesces those refreshes.
pub fn cache_progress(app: &AppHandle, progress: UpdateProgress) {
    if let Some(runtime) = app.try_state::<SourceUpdateRuntime>() {
        *lock_state(&runtime.progress) = Some(progress.clone());
    }
    let _ = app.emit(PROGRESS_EVENT, progress);
}

/// Forgets the active run once its poller has stopped.
pub fn run_finished(app: &AppHandle, run_id: &str) {
    if let Some(runtime) = app.try_state::<SourceUpdateRuntime>() {
        let mut active = lock_state(&runtime.active);
        if active.as_ref().is_some_and(|run| run.run_id == run_id) {
            *active = None;
        }
    }
}

/// After a successful update, re-check so the footer shows the new state.
pub fn recheck_after_done(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = check_and_publish(&app, true).await {
            warn!("post-update check failed: {error}");
        }
    });
}

/// Drops a failed progress once a fresh check has run, so the tray offers the
/// update again (the next check decides whether it is still available).
fn clear_failed_progress(runtime: &SourceUpdateRuntime) {
    let mut progress = lock_state(&runtime.progress);
    if progress
        .as_ref()
        .is_some_and(|p| p.phase == RunPhase::Failed)
    {
        *progress = None;
    }
}

/// Startup adoption (R2-2, R3-1). A run started by an earlier Vox may still be
/// running, or may have finished while Vox restarted. Watch it when something is
/// left to watch, clean it up when only leftovers remain, and do nothing else.
/// A live job is never booted out and the lock is never touched.
pub fn adopt_existing_run(app: &AppHandle) {
    if current_mode() != SourceUpdateMode::Source {
        return;
    }
    let Some(runtime) = app.try_state::<SourceUpdateRuntime>() else {
        return;
    };
    let Ok(app_data) = crate::portable::app_data_dir(app) else {
        return;
    };
    let state_dir = update_state_dir(&app_data);
    let status_file = status_path(&state_dir);
    let Ok(Some(bytes)) = read_status_bounded(&status_file) else {
        return;
    };
    let Some(run_id) = run_id_in_status(&bytes) else {
        return;
    };
    let control = runner::LaunchctlJobControl::current();
    let lock_dir = PathBuf::from(update_lock_path());
    let plist = plist_path(&state_dir, &run_id);
    let running = running_build_sha();
    let input = ReconcileInput {
        status: runner::parse_status(&bytes, &run_id, None, now_unix()).ok(),
        status_age_secs: file_age_secs(&status_file)
            .or_else(|| file_age_secs(&plist))
            .unwrap_or(0),
        job: control.state(&runner::job_label(&run_id)),
        lock: runner::lock_state(&lock_dir, &runner::process_alive),
        running_build_sha: &running,
        published: false,
        interrupted_after_secs: INTERRUPTED_AFTER_SECS,
    };
    match adopt_plan(&input, plist.exists()) {
        AdoptPlan::Nothing => {}
        AdoptPlan::CleanupNow => {
            if let Err(error) = cleanup_run(&control, &state_dir, &run_id) {
                warn!("could not clean up a finished update: {error}");
            }
        }
        AdoptPlan::Watch => {
            info!("adopting an update run that is still known to Vox");
            let config = PollerConfig {
                label: runner::job_label(&run_id),
                run_id,
                state_dir,
                status_file,
                plist,
                lock_dir,
                running_build_sha: running,
            };
            start_watching(app, &runtime, config);
        }
    }
}

/// Background checker: first run after a short delay, then every six hours,
/// only in Source mode with checks enabled.
pub fn spawn_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SCHEDULER_START_DELAY).await;
        loop {
            if current_mode() == SourceUpdateMode::Source && effective_checks_enabled(&app) {
                if let Err(e) = check_and_publish(&app, true).await {
                    warn!("source update check failed: {e}");
                }
            }
            tokio::time::sleep(SCHEDULER_INTERVAL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- helpers ------------------------------------------------------------

    fn sha(n: u32) -> String {
        format!("{:040x}", 0xabc0_0000_u128 + n as u128)
    }

    fn commit(n: u32, subject: &str) -> RemoteCommit {
        RemoteCommit {
            sha: sha(n),
            subject: subject.to_string(),
        }
    }

    /// Newest-first window of `count` commits numbered `count..=1` so that
    /// `sha(count)` is the newest.
    fn window(count: u32) -> Vec<RemoteCommit> {
        (1..=count)
            .rev()
            .map(|n| commit(n, &format!("change {n}")))
            .collect()
    }

    fn commits_json(items: &[(String, &str)]) -> String {
        let v: Vec<serde_json::Value> = items
            .iter()
            .map(|(s, m)| serde_json::json!({ "sha": s, "commit": { "message": m } }))
            .collect();
        serde_json::Value::Array(v).to_string()
    }

    fn resolved(e: WindowEvaluation) -> SourceUpdateInfo {
        match e {
            WindowEvaluation::Resolved(i) => i,
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    // ---- sha helpers & urls -------------------------------------------------

    #[test]
    fn is_full_sha_accepts_exactly_forty_hex_chars_any_case() {
        assert!(is_full_sha(&sha(1)));
        assert!(is_full_sha(&sha(1).to_uppercase()));
        assert!(!is_full_sha(""));
        assert!(!is_full_sha("abc1234"));
        assert!(!is_full_sha(&format!("{}0", sha(1)))); // 41
        assert!(!is_full_sha(&sha(1)[..39])); // 39
        assert!(!is_full_sha(&format!("{}g", &sha(1)[..39]))); // non-hex
        assert!(!is_full_sha(&format!(" {}", &sha(1)[..39]))); // whitespace
    }

    #[test]
    fn short_sha_is_first_seven_lowercased() {
        assert_eq!(short_sha(&sha(1).to_uppercase()), sha(1)[..7].to_string());
        assert_eq!(short_sha("ABC"), "abc");
        assert_eq!(short_sha(""), "");
    }

    #[test]
    fn api_urls_are_exact() {
        assert_eq!(
            commits_api_url(),
            "https://api.github.com/repos/iamshreeram/vox/commits?sha=main&per_page=100"
        );
        assert_eq!(
            compare_api_url(&sha(1)),
            Some(format!(
                "https://api.github.com/repos/iamshreeram/vox/compare/{}...main?per_page=1",
                sha(1)
            ))
        );
        assert_eq!(compare_api_url("abc1234"), None);
        assert_eq!(compare_api_url("main; rm -rf /"), None);
    }

    #[test]
    fn compare_page_url_uses_full_validated_shas() {
        assert_eq!(
            compare_page_url(&sha(1), &sha(2)),
            Some(format!(
                "https://github.com/iamshreeram/vox/compare/{}...{}",
                sha(1),
                sha(2)
            ))
        );
        assert_eq!(compare_page_url("abc", &sha(2)), None);
        assert_eq!(compare_page_url(&sha(1), "zzz"), None);
    }

    // ---- status mapping & messages -----------------------------------------

    #[test]
    fn classify_status_maps_http_codes() {
        assert_eq!(classify_status(200), None);
        assert_eq!(classify_status(403), Some(UpdateCheckError::RateLimited));
        assert_eq!(classify_status(429), Some(UpdateCheckError::RateLimited));
        assert_eq!(classify_status(404), Some(UpdateCheckError::Http(404)));
        assert_eq!(classify_status(500), Some(UpdateCheckError::Http(500)));
        assert_eq!(classify_status(304), Some(UpdateCheckError::Http(304)));
    }

    #[test]
    fn user_messages_are_specific_and_nonempty() {
        assert!(UpdateCheckError::RateLimited
            .user_message()
            .to_lowercase()
            .contains("rate limit"));
        assert!(UpdateCheckError::Network("dns".into())
            .user_message()
            .contains("GitHub"));
        assert!(UpdateCheckError::Http(500).user_message().contains("500"));
        for e in [
            UpdateCheckError::Malformed("x".into()),
            UpdateCheckError::EmptyHistory,
            UpdateCheckError::BadBuildCommit,
            UpdateCheckError::TooLarge,
        ] {
            assert!(!e.user_message().trim().is_empty(), "{e:?}");
        }
        // The raw transport error text is not leaked verbatim to the UI.
        assert!(!UpdateCheckError::Network("secret-internal-detail".into())
            .user_message()
            .contains("secret-internal-detail"));
    }

    // ---- first_line_truncated ----------------------------------------------

    #[test]
    fn first_line_short_and_exact_limit_are_unchanged() {
        assert_eq!(first_line_truncated("fix: thing", 100), "fix: thing");
        let exact = "a".repeat(100);
        assert_eq!(first_line_truncated(&exact, 100), exact);
    }

    #[test]
    fn first_line_over_limit_is_cut_with_ellipsis_within_budget() {
        let long = "a".repeat(150);
        let out = first_line_truncated(&long, 100);
        assert_eq!(out.chars().count(), 100);
        assert!(out.ends_with('…'));
        assert!(out.starts_with(&"a".repeat(99)));
    }

    #[test]
    fn first_line_never_splits_multibyte_characters() {
        let long = "é".repeat(150);
        let out = first_line_truncated(&long, 100);
        assert_eq!(out.chars().count(), 100);
        let emoji = "\u{1F642}".repeat(150);
        let out = first_line_truncated(&emoji, 100);
        assert_eq!(out.chars().count(), 100);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn first_line_takes_only_the_first_line_and_trims() {
        assert_eq!(
            first_line_truncated("  subject  \n\nlong body text", 100),
            "subject"
        );
        assert_eq!(first_line_truncated("a\r\nb", 100), "a");
    }

    // ---- parse_commits_response --------------------------------------------

    #[test]
    fn parse_commits_reads_sha_and_first_line_subject() {
        let body = commits_json(&[(sha(2), "newest\n\nbody"), (sha(1), "older")]);
        let parsed = parse_commits_response(&body).unwrap();
        assert_eq!(
            parsed,
            vec![commit(2, "newest"), commit(1, "older")],
            "order is preserved and only the first line is kept"
        );
    }

    #[test]
    fn parse_commits_empty_array_is_ok_and_empty() {
        assert_eq!(parse_commits_response("[]").unwrap(), vec![]);
    }

    #[test]
    fn parse_commits_blank_message_gets_placeholder_subject() {
        let body = commits_json(&[(sha(1), "   \n")]);
        let parsed = parse_commits_response(&body).unwrap();
        assert_eq!(parsed[0].subject, "(no message)");
    }

    #[test]
    fn parse_commits_truncates_long_unicode_subjects() {
        let long = "é".repeat(150);
        let body = commits_json(&[(sha(1), long.as_str())]);
        let parsed = parse_commits_response(&body).unwrap();
        assert_eq!(parsed[0].subject.chars().count(), MAX_SUBJECT_CHARS);
        assert!(parsed[0].subject.ends_with('…'));
    }

    #[test]
    fn parse_commits_rejects_malformed_input() {
        for body in [
            "not json",
            "{\"message\":\"Not Found\"}",
            "[{\"sha\":\"zz\",\"commit\":{\"message\":\"m\"}}]",
            "[{\"sha\":\"abc1234\",\"commit\":{\"message\":\"m\"}}]",
            "[{\"commit\":{\"message\":\"m\"}}]",
            &format!("[{{\"sha\":\"{}\"}}]", sha(1)),
            &format!("[{{\"sha\":\"{}\",\"commit\":{{}}}}]", sha(1)),
            &format!("[{{\"sha\":\"{}\",\"commit\":{{\"message\":5}}}}]", sha(1)),
        ] {
            assert!(
                matches!(
                    parse_commits_response(body),
                    Err(UpdateCheckError::Malformed(_))
                ),
                "should be Malformed: {body}"
            );
        }
    }

    // ---- parse_compare_response --------------------------------------------

    #[test]
    fn parse_compare_maps_every_known_status() {
        assert_eq!(
            parse_compare_response("{\"status\":\"ahead\",\"ahead_by\":3}").unwrap(),
            CompareStatus::Ahead { ahead_by: 3 }
        );
        assert_eq!(
            parse_compare_response("{\"status\":\"identical\"}").unwrap(),
            CompareStatus::Identical
        );
        assert_eq!(
            parse_compare_response("{\"status\":\"behind\",\"behind_by\":2}").unwrap(),
            CompareStatus::Behind
        );
        assert_eq!(
            parse_compare_response("{\"status\":\"diverged\",\"ahead_by\":1,\"behind_by\":1}")
                .unwrap(),
            CompareStatus::Diverged
        );
    }

    #[test]
    fn parse_compare_rejects_unknown_or_incomplete_payloads() {
        for body in [
            "not json",
            "[]",
            "{}",
            "{\"status\":\"weird\"}",
            "{\"status\":\"ahead\"}",
            "{\"status\":\"ahead\",\"ahead_by\":\"3\"}",
            "{\"status\":\"ahead\",\"ahead_by\":-1}",
            "{\"status\":7}",
        ] {
            assert!(
                matches!(
                    parse_compare_response(body),
                    Err(UpdateCheckError::Malformed(_))
                ),
                "should be Malformed: {body}"
            );
        }
    }

    // ---- evaluate_window ----------------------------------------------------

    #[test]
    fn up_to_date_when_build_is_the_newest_commit() {
        let info = resolved(evaluate_window(&sha(5), &window(5)).unwrap());
        assert_eq!(info.state, SourceUpdateState::UpToDate);
        assert_eq!(info.behind_by, 0);
        assert!(info.changes.is_empty());
        assert_eq!(info.current_commit, sha(5)[..7]);
        assert_eq!(info.latest_commit, sha(5)[..7]);
        assert!(!info.can_install);
        assert_eq!(info.install_blocker, None);
        assert_eq!(info.note, None);
    }

    #[test]
    fn behind_by_two_lists_the_two_newest_changes_newest_first() {
        // window newest-first: 5,4,3,2,1 ; build is 3 => behind by 2 (5 and 4)
        let info = resolved(evaluate_window(&sha(3), &window(5)).unwrap());
        assert_eq!(info.state, SourceUpdateState::Available);
        assert_eq!(info.behind_by, 2);
        assert_eq!(info.changes, vec!["change 5", "change 4"]);
        assert_eq!(info.current_commit, sha(3)[..7]);
        assert_eq!(info.latest_commit, sha(5)[..7]);
        assert_eq!(
            info.compare_url,
            format!(
                "https://github.com/iamshreeram/vox/compare/{}...{}",
                sha(3),
                sha(5)
            )
        );
    }

    #[test]
    fn behind_by_seven_reports_seven_but_lists_only_five_changes() {
        let info = resolved(evaluate_window(&sha(3), &window(10)).unwrap());
        assert_eq!(info.behind_by, 7);
        assert_eq!(info.changes.len(), MAX_CHANGES);
        assert_eq!(info.changes[0], "change 10");
        assert_eq!(info.changes[4], "change 6");
    }

    #[test]
    fn build_sha_match_is_case_insensitive() {
        let info = resolved(evaluate_window(&sha(3).to_uppercase(), &window(5)).unwrap());
        assert_eq!(info.behind_by, 2);
    }

    #[test]
    fn build_commit_missing_from_window_defers_to_compare_api() {
        match evaluate_window(&sha(99), &window(8)).unwrap() {
            WindowEvaluation::NotInWindow {
                latest_sha,
                changes,
            } => {
                assert_eq!(latest_sha, sha(8));
                assert_eq!(changes.len(), MAX_CHANGES);
                assert_eq!(changes[0], "change 8");
            }
            other => panic!("expected NotInWindow, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_window_rejects_empty_history_and_bad_build_sha() {
        assert_eq!(
            evaluate_window(&sha(1), &[]),
            Err(UpdateCheckError::EmptyHistory)
        );
        for bad in ["", "abc1234", "unknown", &format!("{}0", sha(1))] {
            assert_eq!(
                evaluate_window(bad, &window(3)),
                Err(UpdateCheckError::BadBuildCommit),
                "{bad:?}"
            );
        }
    }

    // ---- resolve_compare ----------------------------------------------------

    fn changes() -> Vec<String> {
        vec!["change 8".into(), "change 7".into()]
    }

    #[test]
    fn compare_ahead_is_available_with_the_reported_count() {
        let info = resolve_compare(
            &sha(99),
            &sha(8),
            changes(),
            Ok(CompareStatus::Ahead { ahead_by: 140 }),
        )
        .unwrap();
        assert_eq!(info.state, SourceUpdateState::Available);
        assert_eq!(info.behind_by, 140);
        assert_eq!(info.changes, changes());
        assert_eq!(info.latest_commit, sha(8)[..7]);
        assert_eq!(info.current_commit, sha(99)[..7]);
        assert!(info.compare_url.contains(&sha(99)));
        assert!(info.compare_url.contains(&sha(8)));
    }

    #[test]
    fn compare_identical_or_behind_means_no_update() {
        for status in [CompareStatus::Identical, CompareStatus::Behind] {
            let info = resolve_compare(&sha(99), &sha(8), changes(), Ok(status)).unwrap();
            assert_eq!(info.state, SourceUpdateState::UpToDate, "{status:?}");
            assert_eq!(info.behind_by, 0);
            assert!(info.changes.is_empty());
        }
    }

    #[test]
    fn compare_diverged_or_unknown_commit_is_untracked_with_a_note() {
        let diverged =
            resolve_compare(&sha(99), &sha(8), changes(), Ok(CompareStatus::Diverged)).unwrap();
        assert_eq!(diverged.state, SourceUpdateState::Untracked);
        assert!(diverged
            .note
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains("diverged"));
        assert!(diverged.changes.is_empty());
        assert_eq!(diverged.behind_by, 0);

        let unknown = resolve_compare(
            &sha(99),
            &sha(8),
            changes(),
            Err(UpdateCheckError::Http(404)),
        )
        .unwrap();
        assert_eq!(unknown.state, SourceUpdateState::Untracked);
        assert!(unknown.note.as_deref().unwrap_or("").contains("GitHub"));
    }

    #[test]
    fn compare_other_errors_propagate() {
        assert_eq!(
            resolve_compare(
                &sha(99),
                &sha(8),
                changes(),
                Err(UpdateCheckError::RateLimited)
            ),
            Err(UpdateCheckError::RateLimited)
        );
        assert_eq!(
            resolve_compare(
                &sha(99),
                &sha(8),
                changes(),
                Err(UpdateCheckError::TooLarge)
            ),
            Err(UpdateCheckError::TooLarge)
        );
    }

    // ---- update_mode --------------------------------------------------------

    #[test]
    fn source_mode_needs_opt_in_macos_full_sha_and_main() {
        let c = sha(1);
        assert_eq!(
            update_mode(true, true, &c, "main"),
            SourceUpdateMode::Source
        );
        assert_eq!(
            update_mode(false, true, &c, "main"),
            SourceUpdateMode::Plugin
        );
        assert_eq!(
            update_mode(true, false, &c, "main"),
            SourceUpdateMode::Plugin
        );
        assert_eq!(
            update_mode(true, true, "unknown", "main"),
            SourceUpdateMode::Plugin
        );
        assert_eq!(
            update_mode(true, true, "", "main"),
            SourceUpdateMode::Plugin
        );
        assert_eq!(
            update_mode(true, true, &c, "feature/x"),
            SourceUpdateMode::Plugin
        );
        assert_eq!(update_mode(true, true, &c, ""), SourceUpdateMode::Plugin);
        assert_eq!(
            update_mode(true, true, &c, "Main"),
            SourceUpdateMode::Plugin
        );
    }

    // ---- origin_matches (shared table with scripts tests) --------------------

    #[test]
    fn origin_matches_is_exact_membership() {
        for ok in [
            "https://github.com/iamshreeram/vox",
            "https://github.com/iamshreeram/vox.git",
            "git@github.com:iamshreeram/vox",
            "git@github.com:iamshreeram/vox.git",
            "ssh://git@github.com/iamshreeram/vox",
            "ssh://git@github.com/iamshreeram/vox.git",
        ] {
            assert!(origin_matches(ok), "{ok}");
        }
        for bad in [
            "",
            "https://evilgithub.com/iamshreeram/vox",
            "https://github.com.evil.com/iamshreeram/vox",
            "https://user@github.com/iamshreeram/vox",
            "https://user:pw@github.com/iamshreeram/vox.git",
            "https://github.com/iamshreeram/vox/extra",
            "https://github.com/iamshreeram/vox-evil",
            "https://github.com/iamshreeram/vox/",
            "https://github.com/someoneelse/vox",
            "https://github.com/iamshreeram/other",
            "http://github.com/iamshreeram/vox",
            "git@github.com:iamshreeram/vox.git ",
            " https://github.com/iamshreeram/vox",
            "https://github.com/iamshreeram/vox\n",
            "https://github.com/IAMSHREERAM/vox",
            "https://github.com/iamshreeram/vox.git.evil",
            "file:///tmp/iamshreeram/vox",
        ] {
            assert!(!origin_matches(bad), "{bad:?}");
        }
    }

    // ---- install_blocker ----------------------------------------------------

    fn good_facts() -> InstallFacts {
        InstallFacts {
            is_macos: true,
            checks_disabled: false,
            source_dir_exists: true,
            script_exists: true,
            origin_url: Some("https://github.com/iamshreeram/vox.git".into()),
            branch: Some("main".into()),
            tracked_changes: Some(false),
            update_lock: None,
        }
    }

    #[test]
    fn install_blocker_is_none_when_everything_is_fine() {
        assert_eq!(install_blocker(&good_facts()), None);
    }

    #[test]
    fn install_blocker_names_each_problem() {
        let msg = |f: InstallFacts| install_blocker(&f).expect("should be blocked");

        let mut f = good_facts();
        f.is_macos = false;
        assert!(msg(f).contains("macOS"));

        let mut f = good_facts();
        f.checks_disabled = true;
        assert!(msg(f).to_lowercase().contains("disabled"));

        let mut f = good_facts();
        f.source_dir_exists = false;
        assert!(msg(f).contains("checkout"));

        let mut f = good_facts();
        f.script_exists = false;
        assert!(msg(f).contains("update-macos.sh"));

        let mut f = good_facts();
        f.origin_url = None;
        assert!(msg(f).contains("origin"));

        let mut f = good_facts();
        f.origin_url = Some("https://evilgithub.com/iamshreeram/vox".into());
        assert!(msg(f).contains("origin"));

        let mut f = good_facts();
        f.branch = Some("feature/x".into());
        let m = msg(f);
        assert!(m.contains("feature/x") && m.contains("main"), "{m}");

        let mut f = good_facts();
        f.branch = None;
        assert!(msg(f).to_lowercase().contains("branch"));

        let mut f = good_facts();
        f.tracked_changes = Some(true);
        assert!(msg(f).to_lowercase().contains("uncommitted"));

        let mut f = good_facts();
        f.tracked_changes = None;
        assert!(msg(f).to_lowercase().contains("working tree"));

        let mut f = good_facts();
        f.update_lock = Some("/tmp/vox-update.lock".into());
        let m = msg(f);
        assert!(
            m.contains("/tmp/vox-update.lock") && m.contains("rm -rf"),
            "{m}"
        );
    }

    #[test]
    fn install_blocker_priority_is_fixed() {
        // macOS beats everything.
        let mut f = good_facts();
        f.is_macos = false;
        f.checks_disabled = true;
        f.branch = Some("x".into());
        assert!(install_blocker(&f).unwrap().contains("macOS"));
        // disabled beats missing checkout.
        let mut f = good_facts();
        f.checks_disabled = true;
        f.source_dir_exists = false;
        assert!(install_blocker(&f)
            .unwrap()
            .to_lowercase()
            .contains("disabled"));
        // origin beats branch beats dirty beats lock.
        let mut f = good_facts();
        f.origin_url = None;
        f.branch = Some("x".into());
        f.tracked_changes = Some(true);
        f.update_lock = Some("/l".into());
        assert!(install_blocker(&f).unwrap().contains("origin"));
        f.origin_url = good_facts().origin_url;
        assert!(install_blocker(&f).unwrap().contains("main"));
        f.branch = Some("main".into());
        assert!(install_blocker(&f)
            .unwrap()
            .to_lowercase()
            .contains("uncommitted"));
        f.tracked_changes = Some(false);
        assert!(install_blocker(&f).unwrap().contains("rm -rf"));
    }

    // ---- git helpers (throwaway repositories) -------------------------------

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .expect("git must be installed to run these tests");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("tracked.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "tracked.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        dir
    }

    #[test]
    fn git_current_branch_reports_main_and_none_when_detached() {
        let repo = init_repo();
        assert_eq!(git_current_branch(repo.path()), Some("main".to_string()));
        git(repo.path(), &["checkout", "-q", "--detach"]);
        assert_eq!(git_current_branch(repo.path()), None);
    }

    #[test]
    fn git_tracked_changes_ignores_untracked_files() {
        let repo = init_repo();
        assert_eq!(git_tracked_changes(repo.path()), Some(false));
        std::fs::write(repo.path().join("notes.md"), "untracked\n").unwrap();
        assert_eq!(
            git_tracked_changes(repo.path()),
            Some(false),
            "untracked files must not count"
        );
        std::fs::write(repo.path().join("tracked.txt"), "two\n").unwrap();
        assert_eq!(git_tracked_changes(repo.path()), Some(true));
    }

    #[test]
    fn git_origin_url_reads_the_remote() {
        let repo = init_repo();
        assert_eq!(git_origin_url(repo.path()), None);
        git(
            repo.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/iamshreeram/vox.git",
            ],
        );
        assert_eq!(
            git_origin_url(repo.path()),
            Some("https://github.com/iamshreeram/vox.git".to_string())
        );
    }

    #[test]
    fn git_helpers_return_none_for_missing_or_non_repo_dirs() {
        let missing = Path::new("/definitely/not/a/real/dir/vox-test");
        assert_eq!(git_current_branch(missing), None);
        assert_eq!(git_tracked_changes(missing), None);
        assert_eq!(git_origin_url(missing), None);
        let plain = tempfile::tempdir().unwrap();
        assert_eq!(git_current_branch(plain.path()), None);
        assert_eq!(git_tracked_changes(plain.path()), None);
    }

    // ---- update_lock_path & LaunchGuard -------------------------------------

    #[test]
    fn update_lock_path_matches_the_script_convention() {
        let p = update_lock_path();
        assert!(p.ends_with("/vox-update.lock"), "{p}");
        assert!(!p.contains("//vox-update.lock"), "no doubled slash: {p}");
    }

    #[test]
    fn launch_guard_blocks_repeat_launches_inside_the_window() {
        let guard = LaunchGuard::new(Duration::from_secs(300));
        let t0 = Instant::now();
        assert!(guard.try_acquire(t0));
        assert!(!guard.try_acquire(t0 + Duration::from_secs(1)));
        assert!(!guard.try_acquire(t0 + Duration::from_secs(299)));
        assert!(guard.try_acquire(t0 + Duration::from_secs(300)));
        assert!(!guard.try_acquire(t0 + Duration::from_secs(301)));
    }

    #[test]
    fn launch_guard_release_allows_an_immediate_retry() {
        let guard = LaunchGuard::new(Duration::from_secs(300));
        let t0 = Instant::now();
        assert!(guard.try_acquire(t0));
        guard.release();
        assert!(guard.try_acquire(t0 + Duration::from_secs(1)));
        // Releasing twice (or without a reservation) is harmless.
        guard.release();
        guard.release();
        assert!(guard.try_acquire(t0 + Duration::from_secs(2)));
    }

    #[test]
    fn launch_guard_with_zero_window_never_blocks() {
        let guard = LaunchGuard::new(Duration::ZERO);
        let t0 = Instant::now();
        assert!(guard.try_acquire(t0));
        assert!(guard.try_acquire(t0));
    }
}

#[cfg(test)]
mod orchestration_tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Arc;

    type Canned = Result<(u16, String), UpdateCheckError>;

    fn full(n: u32) -> String {
        format!("{:040x}", 0xdef0_0000_u128 + n as u128)
    }

    /// Newest-first commit list body; subjects are `subject-<n>`.
    fn commits_body(shas: &[u32]) -> String {
        let items: Vec<String> = shas
            .iter()
            .map(|n| {
                format!(
                    r#"{{"sha":"{}","commit":{{"message":"subject-{n}\n\nbody"}}}}"#,
                    full(*n)
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    type RequestLog = Arc<Mutex<Vec<(String, usize)>>>;

    /// Fetch double that answers from a queue and records every request.
    fn canned_fetch(
        responses: Vec<Canned>,
    ) -> (
        impl FnMut(String, usize) -> std::future::Ready<Canned>,
        RequestLog,
    ) {
        let log: RequestLog = Arc::new(Mutex::new(Vec::new()));
        let log_inner = Arc::clone(&log);
        let mut queue: VecDeque<Canned> = responses.into();
        let fetch = move |url: String, max: usize| {
            log_inner.lock().expect("request log").push((url, max));
            let next = queue
                .pop_front()
                .unwrap_or_else(|| Err(UpdateCheckError::Network("no canned response".into())));
            std::future::ready(next)
        };
        (fetch, log)
    }

    fn ok(body: String) -> Canned {
        Ok((200, body))
    }

    #[tokio::test]
    async fn up_to_date_when_build_is_newest_and_makes_one_request() {
        let (fetch, log) = canned_fetch(vec![ok(commits_body(&[1, 2]))]);
        let info = check_with(&full(1), fetch).await.expect("check ok");
        assert_eq!(info.state, SourceUpdateState::UpToDate);
        assert_eq!(info.behind_by, 0);
        assert_eq!(log.lock().expect("log").len(), 1);
    }

    #[tokio::test]
    async fn behind_by_two_lists_changes_newest_first_with_one_request() {
        let (fetch, log) = canned_fetch(vec![ok(commits_body(&[0, 1, 2, 3]))]);
        let info = check_with(&full(2), fetch).await.expect("check ok");
        assert_eq!(info.state, SourceUpdateState::Available);
        assert_eq!(info.behind_by, 2);
        assert_eq!(info.changes, vec!["subject-0", "subject-1"]);
        assert_eq!(log.lock().expect("log").len(), 1);
    }

    #[tokio::test]
    async fn build_outside_window_uses_compare_ahead_count_with_two_requests() {
        let build = full(999);
        let (fetch, log) = canned_fetch(vec![
            ok(commits_body(&[0, 1, 2])),
            ok(r#"{"status":"ahead","ahead_by":7}"#.to_string()),
        ]);
        let info = check_with(&build, fetch).await.expect("check ok");
        assert_eq!(info.state, SourceUpdateState::Available);
        assert_eq!(info.behind_by, 7);
        assert_eq!(info.changes, vec!["subject-0", "subject-1", "subject-2"]);
        let requests = log.lock().expect("log");
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].0, compare_api_url(&build).expect("full sha"));
    }

    #[tokio::test]
    async fn compare_404_means_untracked() {
        let (fetch, _log) =
            canned_fetch(vec![ok(commits_body(&[0, 1])), Ok((404, "{}".to_string()))]);
        let info = check_with(&full(999), fetch).await.expect("check ok");
        assert_eq!(info.state, SourceUpdateState::Untracked);
        assert!(info.note.is_some());
    }

    #[tokio::test]
    async fn compare_500_is_an_http_error() {
        let (fetch, _log) = canned_fetch(vec![
            ok(commits_body(&[0, 1])),
            Ok((500, "boom".to_string())),
        ]);
        let err = check_with(&full(999), fetch)
            .await
            .expect_err("should fail");
        assert_eq!(err, UpdateCheckError::Http(500));
    }

    #[tokio::test]
    async fn commits_403_is_rate_limited() {
        let (fetch, _log) = canned_fetch(vec![Ok((403, "{}".to_string()))]);
        let err = check_with(&full(1), fetch).await.expect_err("should fail");
        assert_eq!(err, UpdateCheckError::RateLimited);
    }

    #[tokio::test]
    async fn malformed_commits_body_is_malformed() {
        let (fetch, _log) = canned_fetch(vec![ok("not json".to_string())]);
        let err = check_with(&full(1), fetch).await.expect_err("should fail");
        assert!(matches!(err, UpdateCheckError::Malformed(_)), "{err:?}");
    }

    #[tokio::test]
    async fn empty_commit_array_is_empty_history() {
        let (fetch, _log) = canned_fetch(vec![ok("[]".to_string())]);
        let err = check_with(&full(1), fetch).await.expect_err("should fail");
        assert_eq!(err, UpdateCheckError::EmptyHistory);
    }

    #[tokio::test]
    async fn invalid_build_sha_is_bad_build_commit() {
        // The commits request happens first; the SHA is validated after parsing.
        let (fetch, _log) = canned_fetch(vec![ok(commits_body(&[0, 1]))]);
        let err = check_with("not-a-sha", fetch)
            .await
            .expect_err("should fail");
        assert_eq!(err, UpdateCheckError::BadBuildCommit);
    }

    #[tokio::test]
    async fn compare_response_too_large_propagates() {
        let (fetch, _log) = canned_fetch(vec![
            ok(commits_body(&[0, 1])),
            Err(UpdateCheckError::TooLarge),
        ]);
        let err = check_with(&full(999), fetch)
            .await
            .expect_err("should fail");
        assert_eq!(err, UpdateCheckError::TooLarge);
    }
}

#[cfg(test)]
mod io_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Accept one connection, read the request head, then either answer with
    /// `response` or (when `hold` is set) keep the socket open without replying.
    async fn serve_once(response: Vec<u8>, hold: bool) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut head = Vec::new();
            let mut chunk = [0u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                match socket.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => head.extend_from_slice(&chunk[..n]),
                }
            }
            if hold {
                tokio::time::sleep(Duration::from_secs(10)).await;
                return;
            }
            let _ = socket.write_all(&response).await;
            let _ = socket.shutdown().await;
        });
        port
    }

    fn http_response(status_line: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    #[tokio::test]
    async fn fetch_text_returns_status_and_small_body() {
        let port = serve_once(http_response("200 OK", "hello"), false).await;
        let url = format!("http://127.0.0.1:{port}/x");
        let got = fetch_text(&url, 1024, Duration::from_secs(5))
            .await
            .expect("fetch ok");
        assert_eq!(got, (200, "hello".to_string()));
    }

    #[tokio::test]
    async fn fetch_text_passes_non_200_status_through() {
        let port = serve_once(http_response("404 Not Found", "not found"), false).await;
        let url = format!("http://127.0.0.1:{port}/x");
        let got = fetch_text(&url, 1024, Duration::from_secs(5))
            .await
            .expect("fetch ok");
        assert_eq!(got, (404, "not found".to_string()));
    }

    #[tokio::test]
    async fn fetch_text_rejects_bodies_over_the_cap() {
        let body = "a".repeat(100);
        let port = serve_once(http_response("200 OK", &body), false).await;
        let url = format!("http://127.0.0.1:{port}/x");
        let err = fetch_text(&url, 10, Duration::from_secs(5))
            .await
            .expect_err("should be too large");
        assert_eq!(err, UpdateCheckError::TooLarge);
    }

    #[tokio::test]
    async fn fetch_text_times_out_as_network_error() {
        let port = serve_once(Vec::new(), true).await;
        let url = format!("http://127.0.0.1:{port}/x");
        let err = fetch_text(&url, 1024, Duration::from_millis(200))
            .await
            .expect_err("should time out");
        assert!(matches!(err, UpdateCheckError::Network(_)), "{err:?}");
    }

    #[tokio::test]
    async fn fetch_text_closed_port_is_network_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);
        let url = format!("http://127.0.0.1:{port}/x");
        let err = fetch_text(&url, 1024, Duration::from_secs(5))
            .await
            .expect_err("should fail to connect");
        assert!(matches!(err, UpdateCheckError::Network(_)), "{err:?}");
    }

    #[test]
    fn should_commit_truth_table() {
        for enabled in [false, true] {
            for started in [0u64, 1] {
                for current in [0u64, 1] {
                    let expected = enabled && started == current;
                    assert_eq!(
                        should_commit(enabled, started, current),
                        expected,
                        "enabled={enabled} started={started} current={current}"
                    );
                }
            }
        }
    }

    #[test]
    fn current_mode_is_plugin_in_a_default_build() {
        // VOX_SOURCE_UPDATES is only embedded as 1 by scripts/update-macos.sh.
        assert_eq!(current_mode(), SourceUpdateMode::Plugin);
    }

    #[test]
    fn embedded_commit_is_unknown_or_a_full_sha() {
        assert!(
            BUILD_COMMIT == "unknown" || is_full_sha(BUILD_COMMIT),
            "unexpected embedded commit: {BUILD_COMMIT}"
        );
        assert!(!SOURCE_DIR.is_empty());
    }
}

/// Network sanity checks against the real GitHub API. Ignored by default (they
/// need internet access and use the unauthenticated rate limit); run with
/// `cargo test --lib --offline live_github -- --ignored`.
#[cfg(test)]
mod live_github {
    use super::*;

    async fn real_fetch(url: String, max: usize) -> Result<(u16, String), UpdateCheckError> {
        fetch_text(&url, max, Duration::from_secs(20)).await
    }

    #[tokio::test]
    #[ignore = "uses the network"]
    async fn a_build_a_few_commits_behind_is_reported_as_available() {
        // dc6896f was the head of main before the CI fix landed.
        let info = check_with("dc6896fcc19d74af70d15202ed3dad43a63a6da9", real_fetch)
            .await
            .expect("live check");
        assert_eq!(info.state, SourceUpdateState::Available, "{info:?}");
        assert!(info.behind_by >= 1, "{info:?}");
        assert!(!info.changes.is_empty() && info.changes.len() <= MAX_CHANGES);
        assert!(info
            .compare_url
            .starts_with("https://github.com/iamshreeram/vox/compare/"));
    }

    #[tokio::test]
    #[ignore = "uses the network"]
    async fn a_build_older_than_the_window_goes_through_the_compare_api() {
        // The repository's root commit is far outside the newest-100 window.
        let info = check_with("cdb307e2120bb7d7a7af2589262dcf133eaaa0ec", real_fetch)
            .await
            .expect("live check");
        assert_eq!(info.state, SourceUpdateState::Available, "{info:?}");
        assert!(info.behind_by > COMMIT_WINDOW as u32, "{info:?}");
        assert_eq!(info.changes.len(), MAX_CHANGES);
    }

    #[tokio::test]
    #[ignore = "uses the network"]
    async fn a_commit_github_does_not_know_is_untracked() {
        let info = check_with("0123456789abcdef0123456789abcdef01234567", real_fetch)
            .await
            .expect("live check");
        assert_eq!(info.state, SourceUpdateState::Untracked, "{info:?}");
        assert!(info.note.is_some());
    }
}

#[cfg(test)]
#[path = "source_update_wiring_tests.rs"]
mod wiring_tests;
