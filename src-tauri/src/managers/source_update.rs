//! "New version available" for builds installed from a local checkout.
//!
//! SCAFFOLD (test-first): every function body below is `todo!()` until the
//! implementation step replaces it. The `tests` module at the bottom is the
//! authoritative specification and must not be weakened.
//!
//! Design in one paragraph: a build made through `scripts/update-macos.sh`
//! embeds the git commit it was built from (plus an explicit opt-in flag).
//! Vox asks GitHub for the newest commits on `main`, compares, and tells the
//! UI whether the running build is behind. Installing means opening a
//! Terminal window that runs the same update script, so the build runs
//! outside Vox's own process tree (the installer stops and restarts Vox).

use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

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
        todo!()
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

pub fn is_full_sha(_s: &str) -> bool {
    todo!()
}

pub fn short_sha(_s: &str) -> String {
    todo!()
}

pub fn commits_api_url() -> String {
    todo!()
}

/// `None` unless `build_sha` is a full 40-hex SHA.
pub fn compare_api_url(_build_sha: &str) -> Option<String> {
    todo!()
}

/// `None` unless both are full SHAs.
pub fn compare_page_url(_build_sha: &str, _latest_sha: &str) -> Option<String> {
    todo!()
}

/// `None` for 200, otherwise the error to report.
pub fn classify_status(_status: u16) -> Option<UpdateCheckError> {
    todo!()
}

/// First line of `msg`, trimmed; if longer than `max_chars` characters it is
/// cut to `max_chars - 1` characters plus `…` (never splits a character).
pub fn first_line_truncated(_msg: &str, _max_chars: usize) -> String {
    todo!()
}

pub fn parse_commits_response(_body: &str) -> Result<Vec<RemoteCommit>, UpdateCheckError> {
    todo!()
}

pub fn parse_compare_response(_body: &str) -> Result<CompareStatus, UpdateCheckError> {
    todo!()
}

/// `remote` is newest-first, as GitHub returns it.
pub fn evaluate_window(
    _build_sha: &str,
    _remote: &[RemoteCommit],
) -> Result<WindowEvaluation, UpdateCheckError> {
    todo!()
}

/// Finish a [`WindowEvaluation::NotInWindow`] using the compare API result.
/// `Err(Http(404))` means GitHub does not know the build commit.
pub fn resolve_compare(
    _build_sha: &str,
    _latest_sha: &str,
    _changes: Vec<String>,
    _compare: Result<CompareStatus, UpdateCheckError>,
) -> Result<SourceUpdateInfo, UpdateCheckError> {
    todo!()
}

/// Source mode requires the explicit opt-in, macOS, a full commit SHA and the
/// `main` branch; everything else keeps the existing plugin flow.
pub fn update_mode(
    _opted_in: bool,
    _is_macos: bool,
    _commit: &str,
    _branch: &str,
) -> SourceUpdateMode {
    todo!()
}

/// Exact-membership check of the `origin` URL.
pub fn origin_matches(_url: &str) -> bool {
    todo!()
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
pub fn install_blocker(_facts: &InstallFacts) -> Option<String> {
    todo!()
}

pub fn git_origin_url(_dir: &Path) -> Option<String> {
    todo!()
}

/// `None` for a detached HEAD or when git fails.
pub fn git_current_branch(_dir: &Path) -> Option<String> {
    todo!()
}

/// Uncommitted changes to TRACKED files only (untracked files are ignored,
/// matching the update script). `None` when git fails.
pub fn git_tracked_changes(_dir: &Path) -> Option<bool> {
    todo!()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptError {
    UnsafePath,
}

/// POSIX single-quote escaping (`'` becomes `'\''`). Rejects NUL, CR and LF.
pub fn shell_single_quote(_s: &str) -> Result<String, ScriptError> {
    todo!()
}

/// Bash text opened in Terminal to run the update.
pub fn build_update_script(_source_dir: &str) -> Result<String, ScriptError> {
    todo!()
}

/// `${TMPDIR:-/tmp}/vox-update.lock`, the directory `update-macos.sh` holds
/// while an update runs.
pub fn update_lock_path() -> String {
    todo!()
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
    pub fn try_acquire(&self, _now: Instant) -> bool {
        let _ = (&self.window, &self.last);
        todo!()
    }

    /// Drop the reservation (e.g. opening Terminal failed).
    pub fn release(&self) {
        todo!()
    }
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

    // ---- shell quoting & the generated Terminal script ----------------------

    #[test]
    fn shell_single_quote_escapes_everything_literally() {
        assert_eq!(
            shell_single_quote("/Users/a b/vox").unwrap(),
            "'/Users/a b/vox'"
        );
        assert_eq!(shell_single_quote("it's").unwrap(), "'it'\\''s'");
        assert_eq!(shell_single_quote("$(rm -rf /)").unwrap(), "'$(rm -rf /)'");
        assert_eq!(shell_single_quote("`id`").unwrap(), "'`id`'");
        assert_eq!(shell_single_quote("").unwrap(), "''");
        assert_eq!(shell_single_quote("''").unwrap(), "''\\'''\\'''");
    }

    #[test]
    fn shell_single_quote_rejects_control_characters() {
        for bad in ["a\nb", "a\rb", "a\0b", "\n"] {
            assert_eq!(
                shell_single_quote(bad),
                Err(ScriptError::UnsafePath),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn update_script_has_the_expected_structure() {
        let script = build_update_script("/Users/me/vox").unwrap();
        assert!(script.starts_with("#!/bin/bash\n"), "{script}");
        assert!(script.contains("cd '/Users/me/vox' ||"));
        assert!(script.contains("VOX_SOURCE_UPDATE=1 ./scripts/update-macos.sh"));
        assert!(script.contains(".cargo/bin"));
        assert!(script.contains(".bun/bin"));
        assert!(script.contains(".local/bin"));
        assert!(script.contains("rm -f -- \"$0\""));
        assert!(script.contains("read -r"));
        assert!(
            script.contains("./scripts/update-macos.sh; if Vox is missing")
                || script.contains("scripts/install-macos.sh"),
            "recovery hint must be present: {script}"
        );
    }

    #[test]
    fn update_script_rejects_unsafe_paths() {
        assert_eq!(
            build_update_script("/tmp/a\nb"),
            Err(ScriptError::UnsafePath)
        );
        assert_eq!(
            build_update_script("/tmp/a\0b"),
            Err(ScriptError::UnsafePath)
        );
    }

    #[cfg(unix)]
    mod execution {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        /// A fake checkout whose directory name contains a space, a single
        /// quote and a dollar sign, with a stub update script recording what it
        /// saw and exiting with the code stored in `exit_code`.
        fn fake_checkout(exit_code: i32) -> (tempfile::TempDir, std::path::PathBuf) {
            let root = tempfile::tempdir().unwrap();
            let dir = root.path().join("my vox's $HOME dir");
            std::fs::create_dir_all(dir.join("scripts")).unwrap();
            let stub = dir.join("scripts/update-macos.sh");
            std::fs::write(
                &stub,
                format!(
                    "#!/bin/bash\n\
                     echo \"cwd=$PWD\" > \"$PWD/marker\"\n\
                     echo \"flag=$VOX_SOURCE_UPDATE\" >> \"$PWD/marker\"\n\
                     exit {exit_code}\n"
                ),
            )
            .unwrap();
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
            (root, dir)
        }

        fn run_script(script_text: &str) -> (std::process::ExitStatus, std::path::PathBuf) {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("run.command");
            std::fs::write(&path, script_text).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            let status = std::process::Command::new("bash")
                .arg(&path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            // Keep the temp dir alive until after the run, then hand the path
            // back so the caller can assert the script deleted itself.
            let p = path.clone();
            std::mem::forget(tmp);
            (status, p)
        }

        #[test]
        fn runs_the_update_script_inside_a_hostile_directory_name() {
            let (_root, dir) = fake_checkout(0);
            let script = build_update_script(dir.to_str().unwrap()).unwrap();
            let (status, script_path) = run_script(&script);
            assert!(status.success());
            let marker = std::fs::read_to_string(dir.join("marker")).unwrap();
            assert!(marker.contains("flag=1"), "{marker}");
            let canonical = std::fs::canonicalize(&dir).unwrap();
            assert!(
                marker.contains(&format!("cwd={}", canonical.display()))
                    || marker.contains(&format!("cwd={}", dir.display())),
                "{marker}"
            );
            assert!(
                !script_path.exists(),
                "the .command file must delete itself"
            );
        }

        #[test]
        fn propagates_the_update_scripts_exit_status() {
            let (_root, dir) = fake_checkout(3);
            let script = build_update_script(dir.to_str().unwrap()).unwrap();
            let (status, _) = run_script(&script);
            assert_eq!(status.code(), Some(3));
        }

        #[test]
        fn a_missing_checkout_exits_one_without_running_anything() {
            let script = build_update_script("/definitely/not/here/vox").unwrap();
            let (status, _) = run_script(&script);
            assert_eq!(status.code(), Some(1));
        }
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
