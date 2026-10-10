//! User-defined voice shortcuts: a spoken phrase mapped to opening a URL, a
//! folder, or an installed app. Whole-utterance, normalized match only.
//!
//! Safety contract (see the design review):
//! - URLs must be http/https with a host (no javascript:/file:/data:).
//! - Paths must resolve to an existing DIRECTORY at execution time (opening a
//!   file could execute it through its default handler). `~/` is expanded in
//!   Rust from the home directory -- never by a shell -- and any `..`
//!   component is rejected.
//! - Apps resolve through [`CommandRouter`] and ONLY a `Matched{OpenApp}`
//!   result is accepted; any other router result (folder, URL, no match) is an
//!   error, so an app shortcut can never open something of a different kind.
//! - Validation runs when saving AND defensively at match time, so a
//!   hand-edited or corrupted settings file can never execute an invalid
//!   entry.

#![allow(dead_code)]

use crate::managers::command_router::{CommandAction, CommandRouter, RouteDecision};
use crate::managers::voice_common::{normalize_phrase, HookEvent, VoiceHookOutcome};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const MAX_SHORTCUTS: usize = 50;
pub const MIN_PHRASE_CHARS: usize = 2;
pub const MAX_PHRASE_CHARS: usize = 60;
pub const MAX_URL_CHARS: usize = 2048;
pub const MAX_PATH_CHARS: usize = 1024;
pub const MAX_APP_NAME_CHARS: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct VoiceShortcut {
    pub phrase: String,
    pub action: VoiceShortcutAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VoiceShortcutAction {
    OpenUrl { url: String },
    OpenPath { path: String },
    OpenApp { name: String },
}

/// Validates one shortcut. `Err` carries a user-presentable reason.
pub fn validate_shortcut(shortcut: &VoiceShortcut) -> Result<(), String> {
    let phrase_chars = normalize_phrase(&shortcut.phrase).chars().count();
    if !(MIN_PHRASE_CHARS..=MAX_PHRASE_CHARS).contains(&phrase_chars) {
        return Err(format!(
            "Shortcut phrase must be {MIN_PHRASE_CHARS} to {MAX_PHRASE_CHARS} characters"
        ));
    }
    match &shortcut.action {
        VoiceShortcutAction::OpenUrl { url } => validate_url(url),
        VoiceShortcutAction::OpenPath { path } => validate_path(path),
        VoiceShortcutAction::OpenApp { name } => validate_app(name),
    }
}

fn validate_url(target: &str) -> Result<(), String> {
    if target.chars().count() > MAX_URL_CHARS {
        return Err(format!("URL must be at most {MAX_URL_CHARS} characters"));
    }
    if target.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("URL must not contain spaces or control characters".into());
    }
    // Require the scheme and a non-empty authority up front: the URL parser
    // would otherwise read "https:///path" as having the host "path".
    let after_scheme = ["https://", "http://"]
        .iter()
        .find_map(|prefix| {
            target
                .get(..prefix.len())
                .filter(|head| head.eq_ignore_ascii_case(prefix))
                .map(|_| &target[prefix.len()..])
        })
        .ok_or_else(|| "URL must start with http:// or https://".to_string())?;
    if after_scheme.is_empty() || after_scheme.starts_with(['/', '\\', '?', '#']) {
        return Err("URL must include a host".into());
    }
    let parsed = url::Url::parse(target).map_err(|_| "URL is not valid".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("URL must use http or https".into());
    }
    match parsed.host_str() {
        Some(host) if !host.is_empty() => Ok(()),
        _ => Err("URL must include a host".into()),
    }
}

fn validate_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("Folder path must not be empty".into());
    }
    if path.chars().count() > MAX_PATH_CHARS {
        return Err(format!(
            "Folder path must be at most {MAX_PATH_CHARS} characters"
        ));
    }
    if path.contains('\0') {
        return Err("Folder path must not contain NUL characters".into());
    }
    if path.starts_with('~') {
        if path != "~" && !path.starts_with("~/") {
            return Err("Only ~ or ~/ home-relative folder paths are supported".into());
        }
    } else if !(path.starts_with('/') || Path::new(path).is_absolute()) {
        return Err("Folder path must be absolute or start with ~/".into());
    }
    if has_parent_component(path) {
        return Err("Folder path must not contain ..".into());
    }
    Ok(())
}

fn validate_app(name: &str) -> Result<(), String> {
    let chars = name.trim().chars().count();
    if (1..=MAX_APP_NAME_CHARS).contains(&chars) {
        Ok(())
    } else {
        Err(format!(
            "App name must be 1 to {MAX_APP_NAME_CHARS} characters"
        ))
    }
}

fn has_parent_component(path: &str) -> bool {
    path.split(['/', '\\']).any(|part| part == "..")
}

/// Validates a whole list: at most [`MAX_SHORTCUTS`], every entry valid, and
/// no two entries sharing the same normalized phrase.
pub fn validate_all(shortcuts: &[VoiceShortcut]) -> Result<(), String> {
    if shortcuts.len() > MAX_SHORTCUTS {
        return Err(format!(
            "At most {MAX_SHORTCUTS} voice shortcuts are allowed"
        ));
    }
    let mut seen = HashSet::new();
    for shortcut in shortcuts {
        validate_shortcut(shortcut)?;
        let key = normalize_phrase(&shortcut.phrase);
        if !seen.insert(key.clone()) {
            return Err(format!("More than one shortcut uses the phrase \"{key}\""));
        }
    }
    Ok(())
}

/// Whole-utterance match on the normalized phrase. Invalid entries are
/// skipped; among valid entries the first wins for a given phrase.
pub fn match_shortcut<'a>(
    shortcuts: &'a [VoiceShortcut],
    transcript: &str,
) -> Option<&'a VoiceShortcut> {
    let spoken = normalize_phrase(transcript);
    shortcuts.iter().find(|shortcut| {
        normalize_phrase(&shortcut.phrase) == spoken && validate_shortcut(shortcut).is_ok()
    })
}

/// Expands a leading `~` / `~/` using `home`. Rejects `~user/...`, any `..`
/// component, a missing home when `~` is used, NUL, and non-absolute results.
pub fn expand_path(path: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("Folder path must not be empty".into());
    }
    if path.contains('\0') {
        return Err("Folder path must not contain NUL characters".into());
    }
    if has_parent_component(path) {
        return Err("Folder path must not contain ..".into());
    }
    if path == "~" || path.starts_with("~/") {
        let home = home.ok_or_else(|| "Home folder is unknown; cannot expand ~".to_string())?;
        if !home.is_absolute() {
            return Err("Home folder is not an absolute path".into());
        }
        // `path[1..]` drops the `~`; trimming leading slashes stops a
        // `~//x` input from being treated as an absolute path by `join`.
        let rest = path[1..].trim_start_matches('/');
        return Ok(if rest.is_empty() {
            home.to_path_buf()
        } else {
            home.join(rest)
        });
    }
    if path.starts_with('~') {
        return Err("Only ~ or ~/ home-relative folder paths are supported".into());
    }
    if path.starts_with('/') || Path::new(path).is_absolute() {
        Ok(PathBuf::from(path))
    } else {
        Err("Folder path must be absolute or start with ~/".into())
    }
}

pub trait ShortcutOpener {
    fn open_url(&self, url: &str) -> Result<(), String>;
    fn open_path(&self, path: &Path) -> Result<(), String>;
    fn open_app(&self, app_path: &Path) -> Result<(), String>;
}

/// Executes a shortcut. Ok carries a short description that never contains
/// the full URL/path (it is shown in the UI/logs and may be spoken).
pub fn execute_shortcut(
    shortcut: &VoiceShortcut,
    router: &CommandRouter,
    home: Option<&Path>,
    opener: &dyn ShortcutOpener,
) -> Result<String, String> {
    // Settings may have been hand-edited since they were saved.
    validate_shortcut(shortcut)?;
    match &shortcut.action {
        VoiceShortcutAction::OpenUrl { url } => opener.open_url(url)?,
        VoiceShortcutAction::OpenPath { path } => {
            let resolved = expand_path(path, home)?;
            let meta =
                std::fs::metadata(&resolved).map_err(|_| "Folder does not exist".to_string())?;
            if !meta.is_dir() {
                return Err("Path is not a folder".into());
            }
            opener.open_path(&resolved)?;
        }
        VoiceShortcutAction::OpenApp { name } => {
            let spoken = format!("open {}", name.trim());
            match router.route(&spoken) {
                RouteDecision::Matched {
                    action: CommandAction::OpenApp { resolved_path, .. },
                } => opener.open_app(&resolved_path)?,
                _ => return Err(format!("No installed app matches \"{}\"", name.trim())),
            }
        }
    }
    Ok(describe(&shortcut.phrase))
}

/// Short, URL- and path-free description of a successful run.
fn describe(phrase: &str) -> String {
    let label = normalize_phrase(phrase);
    if label.contains(['/', '\\']) {
        "Ran voice shortcut".into()
    } else {
        format!("Ran shortcut \"{label}\"")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutHookResult {
    pub outcome: VoiceHookOutcome,
    pub event: HookEvent,
}

/// The whole hook minus Tauri. `None` => no shortcut matched (or voice
/// commands are off); the caller continues. `Some` => the utterance is
/// consumed (`HANDLED`) whether execution succeeded or failed.
pub fn run_shortcut_hook(
    voice_commands_enabled: bool,
    shortcuts: &[VoiceShortcut],
    transcript: &str,
    router: &CommandRouter,
    home: Option<&Path>,
    opener: &dyn ShortcutOpener,
) -> Option<ShortcutHookResult> {
    if !voice_commands_enabled {
        return None;
    }
    let shortcut = match_shortcut(shortcuts, transcript)?;
    let event = match execute_shortcut(shortcut, router, home, opener) {
        Ok(description) => HookEvent::Executed(description),
        Err(message) => HookEvent::Error(message),
    };
    Some(ShortcutHookResult {
        outcome: VoiceHookOutcome::HANDLED,
        event,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managers::command_router::{AppDiscovery, CommandRouter, HomeDirProvider};
    use std::sync::Mutex;
    use tempfile::TempDir;

    // ---------- helpers ----------

    fn url_sc(phrase: &str, url: &str) -> VoiceShortcut {
        VoiceShortcut {
            phrase: phrase.into(),
            action: VoiceShortcutAction::OpenUrl { url: url.into() },
        }
    }
    fn path_sc(phrase: &str, path: &str) -> VoiceShortcut {
        VoiceShortcut {
            phrase: phrase.into(),
            action: VoiceShortcutAction::OpenPath { path: path.into() },
        }
    }
    fn app_sc(phrase: &str, name: &str) -> VoiceShortcut {
        VoiceShortcut {
            phrase: phrase.into(),
            action: VoiceShortcutAction::OpenApp { name: name.into() },
        }
    }

    // ---------- validate_shortcut: phrase ----------

    #[test]
    fn phrase_length_is_measured_on_the_normalized_text() {
        assert!(validate_shortcut(&url_sc("hi", "https://example.com")).is_ok());
        assert!(validate_shortcut(&url_sc("a", "https://example.com")).is_err());
        assert!(validate_shortcut(&url_sc("", "https://example.com")).is_err());
        assert!(validate_shortcut(&url_sc("   ...  ", "https://example.com")).is_err());
        assert!(validate_shortcut(&url_sc(&"a".repeat(60), "https://example.com")).is_ok());
        assert!(validate_shortcut(&url_sc(&"a".repeat(61), "https://example.com")).is_err());
        // Trailing punctuation does not count toward the length.
        assert!(validate_shortcut(&url_sc(
            &format!("{}!!!", "a".repeat(60)),
            "https://example.com"
        ))
        .is_ok());
    }

    // ---------- validate_shortcut: url ----------

    #[test]
    fn url_must_be_http_or_https_with_a_host() {
        for ok in [
            "https://example.com",
            "http://example.com",
            "https://example.com/path?q=1#frag",
            "http://localhost:3000",
            "https://sub.example.co.uk/a/b",
        ] {
            assert!(validate_shortcut(&url_sc("work", ok)).is_ok(), "{ok}");
        }
        for bad in [
            "javascript:alert(1)",
            "JAVASCRIPT:alert(1)",
            "file:///etc/passwd",
            "data:text/html,hi",
            "ftp://example.com",
            "mailto:a@b.com",
            "example.com",
            "//example.com",
            "https://",
            "https:///path",
            "https://a b.com",
            "https://example.com/a b",
            "https://exa\u{0}mple.com",
            "https://example.com/\n",
            "",
            "   ",
        ] {
            assert!(validate_shortcut(&url_sc("work", bad)).is_err(), "{bad:?}");
        }
        let long = format!("https://example.com/{}", "a".repeat(2048));
        assert!(validate_shortcut(&url_sc("work", &long)).is_err());
    }

    // ---------- validate_shortcut: path / app ----------

    #[test]
    fn path_must_be_absolute_or_home_relative_without_dotdot() {
        for ok in ["/tmp", "/Users/me/Documents", "~/Documents", "~/a/b", "~"] {
            assert!(validate_shortcut(&path_sc("docs", ok)).is_ok(), "{ok}");
        }
        for bad in [
            "relative/path",
            "./here",
            "",
            "/a/../b",
            "~/../x",
            "..",
            "~root/x",
            "/tmp/\u{0}x",
        ] {
            assert!(validate_shortcut(&path_sc("docs", bad)).is_err(), "{bad:?}");
        }
        assert!(validate_shortcut(&path_sc("docs", &format!("/{}", "a".repeat(1025)))).is_err());
    }

    #[test]
    fn app_name_must_be_nonempty_and_bounded() {
        assert!(validate_shortcut(&app_sc("browser", "Safari")).is_ok());
        assert!(validate_shortcut(&app_sc("browser", "")).is_err());
        assert!(validate_shortcut(&app_sc("browser", "   ")).is_err());
        assert!(validate_shortcut(&app_sc("browser", &"a".repeat(80))).is_ok());
        assert!(validate_shortcut(&app_sc("browser", &"a".repeat(81))).is_err());
    }

    // ---------- validate_all ----------

    #[test]
    fn validate_all_accepts_empty_and_valid_lists() {
        assert!(validate_all(&[]).is_ok());
        assert!(validate_all(&[
            url_sc("work", "https://a.com"),
            url_sc("home", "https://b.com")
        ])
        .is_ok());
    }

    #[test]
    fn validate_all_rejects_the_whole_list_on_any_invalid_entry() {
        let list = [
            url_sc("work", "https://a.com"),
            url_sc("bad", "javascript:1"),
        ];
        assert!(validate_all(&list).is_err());
    }

    #[test]
    fn validate_all_rejects_duplicate_normalized_phrases() {
        let list = [
            url_sc("Open Work", "https://a.com"),
            url_sc("open work!", "https://b.com"),
        ];
        assert!(validate_all(&list).is_err());
        let list = [
            url_sc("please open work", "https://a.com"),
            url_sc("open work", "https://b.com"),
        ];
        assert!(validate_all(&list).is_err());
    }

    #[test]
    fn validate_all_caps_the_list_at_fifty() {
        let fifty: Vec<_> = (0..50)
            .map(|i| url_sc(&format!("shortcut {i}"), "https://a.com"))
            .collect();
        assert!(validate_all(&fifty).is_ok());
        let fifty_one: Vec<_> = (0..51)
            .map(|i| url_sc(&format!("shortcut {i}"), "https://a.com"))
            .collect();
        assert!(validate_all(&fifty_one).is_err());
    }

    // ---------- match_shortcut ----------

    #[test]
    fn match_is_whole_utterance_and_normalized() {
        let list = [url_sc("Open Work", "https://work.example")];
        for hit in [
            "open work",
            "Open Work.",
            "  OPEN   WORK  ",
            "please open work!",
        ] {
            assert!(match_shortcut(&list, hit).is_some(), "{hit}");
        }
        for miss in [
            "open work now",
            "please open work now",
            "open",
            "work",
            "open works",
            "",
        ] {
            assert!(match_shortcut(&list, miss).is_none(), "{miss}");
        }
    }

    #[test]
    fn first_valid_entry_wins_for_a_phrase() {
        let list = [
            url_sc("go", "https://first.example"),
            url_sc("go", "https://second.example"),
        ];
        // (A list like this fails validate_all, but a corrupted file can hold it.)
        let hit = match_shortcut(&list, "go").expect("match");
        assert_eq!(
            hit.action,
            VoiceShortcutAction::OpenUrl {
                url: "https://first.example".into()
            }
        );
    }

    #[test]
    fn invalid_entries_are_skipped_never_returned() {
        let list = [
            url_sc("go", "javascript:alert(1)"),
            url_sc("go", "https://ok.example"),
        ];
        let hit = match_shortcut(&list, "go").expect("the valid duplicate is used");
        assert_eq!(
            hit.action,
            VoiceShortcutAction::OpenUrl {
                url: "https://ok.example".into()
            }
        );
        let only_bad = [url_sc("go", "javascript:alert(1)")];
        assert!(match_shortcut(&only_bad, "go").is_none());
    }

    #[test]
    fn match_never_panics_on_odd_input() {
        let list = [url_sc("go", "https://a.com")];
        let _ = match_shortcut(&list, &"é".repeat(10_000));
        let _ = match_shortcut(&list, "\u{0}\u{1}");
        assert!(match_shortcut(&[], "go").is_none());
    }

    // ---------- expand_path ----------

    #[test]
    fn expand_path_handles_home_forms() {
        let home = Path::new("/Users/me");
        assert_eq!(
            expand_path("~/Documents", Some(home)).unwrap(),
            PathBuf::from("/Users/me/Documents")
        );
        assert_eq!(
            expand_path("~", Some(home)).unwrap(),
            PathBuf::from("/Users/me")
        );
        assert_eq!(
            expand_path("/tmp", Some(home)).unwrap(),
            PathBuf::from("/tmp")
        );
        assert_eq!(expand_path("/tmp", None).unwrap(), PathBuf::from("/tmp"));
    }

    #[test]
    fn expand_path_rejects_unsafe_forms() {
        let home = Path::new("/Users/me");
        assert!(expand_path("~/Documents", None).is_err());
        assert!(expand_path("~root/x", Some(home)).is_err());
        assert!(expand_path("~/../etc", Some(home)).is_err());
        assert!(expand_path("/a/../b", Some(home)).is_err());
        assert!(expand_path("relative", Some(home)).is_err());
        assert!(expand_path("", Some(home)).is_err());
        assert!(expand_path("/x\u{0}y", Some(home)).is_err());
    }

    // ---------- execute_shortcut ----------

    #[derive(Default)]
    struct FakeOpener {
        calls: Mutex<Vec<String>>,
        fail: bool,
    }
    impl FakeOpener {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
        fn record(&self, entry: String) -> Result<(), String> {
            self.calls.lock().unwrap().push(entry);
            if self.fail {
                Err("opener failed".into())
            } else {
                Ok(())
            }
        }
    }
    impl ShortcutOpener for FakeOpener {
        fn open_url(&self, url: &str) -> Result<(), String> {
            self.record(format!("url:{url}"))
        }
        fn open_path(&self, path: &Path) -> Result<(), String> {
            self.record(format!("path:{}", path.display()))
        }
        fn open_app(&self, app_path: &Path) -> Result<(), String> {
            self.record(format!("app:{}", app_path.display()))
        }
    }

    struct FakeDiscovery(Vec<PathBuf>);
    impl AppDiscovery for FakeDiscovery {
        fn scan_installed_apps(&self) -> Vec<PathBuf> {
            self.0.clone()
        }
    }
    struct FakeHome(PathBuf);
    impl HomeDirProvider for FakeHome {
        fn home_dir(&self) -> Option<PathBuf> {
            Some(self.0.clone())
        }
    }

    /// Router with the given installed apps and a temp home containing `Downloads`.
    fn router(apps: &[&str]) -> (TempDir, CommandRouter) {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("Downloads")).unwrap();
        let router = CommandRouter::with_home(
            Box::new(FakeDiscovery(apps.iter().map(PathBuf::from).collect())),
            Box::new(FakeHome(dir.path().to_path_buf())),
        );
        (dir, router)
    }

    #[test]
    fn url_shortcut_opens_the_url_and_description_hides_it() {
        let (_home, router) = router(&[]);
        let opener = FakeOpener::default();
        let sc = url_sc("work", "https://secret.example/private?token=abc");
        let description = execute_shortcut(&sc, &router, None, &opener).expect("ok");
        assert_eq!(
            opener.calls(),
            vec!["url:https://secret.example/private?token=abc"]
        );
        assert!(!description.contains("secret.example"), "{description}");
        assert!(!description.contains("token"), "{description}");
    }

    #[test]
    fn invalid_url_is_rejected_at_execution_even_if_it_slipped_into_settings() {
        let (_home, router) = router(&[]);
        let opener = FakeOpener::default();
        assert!(
            execute_shortcut(&url_sc("x1", "javascript:alert(1)"), &router, None, &opener).is_err()
        );
        assert!(opener.calls().is_empty());
    }

    #[test]
    fn path_shortcut_opens_an_existing_directory_only() {
        let (_home, router) = router(&[]);
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("script.sh");
        std::fs::write(&file, "echo hi").unwrap();

        let opener = FakeOpener::default();
        let dir_sc = path_sc("stuff", tmp.path().to_str().unwrap());
        assert!(execute_shortcut(&dir_sc, &router, None, &opener).is_ok());
        assert_eq!(opener.calls().len(), 1);

        let opener = FakeOpener::default();
        let file_sc = path_sc("stuff", file.to_str().unwrap());
        assert!(
            execute_shortcut(&file_sc, &router, None, &opener).is_err(),
            "files are refused"
        );
        assert!(opener.calls().is_empty());

        let missing = path_sc("stuff", tmp.path().join("nope").to_str().unwrap());
        assert!(execute_shortcut(&missing, &router, None, &opener).is_err());
        assert!(opener.calls().is_empty());
    }

    #[test]
    fn path_shortcut_expands_tilde_from_the_provided_home() {
        let (_h, router) = router(&[]);
        let home = TempDir::new().unwrap();
        std::fs::create_dir(home.path().join("Projects")).unwrap();
        let opener = FakeOpener::default();
        let sc = path_sc("projects", "~/Projects");
        assert!(execute_shortcut(&sc, &router, Some(home.path()), &opener).is_ok());
        assert_eq!(
            opener.calls(),
            vec![format!("path:{}", home.path().join("Projects").display())]
        );
    }

    #[test]
    fn app_shortcut_opens_only_a_router_open_app_result() {
        let (_home, router) = router(&["/Applications/Safari.app"]);
        let opener = FakeOpener::default();
        assert!(execute_shortcut(&app_sc("browser", "safari"), &router, None, &opener).is_ok());
        assert_eq!(opener.calls(), vec!["app:/Applications/Safari.app"]);
    }

    #[test]
    fn app_shortcut_rejects_folder_url_and_unknown_router_results() {
        let (_home, router) = router(&["/Applications/Safari.app"]);
        for name in ["downloads", "example.com", "website example", "no such app"] {
            let opener = FakeOpener::default();
            assert!(
                execute_shortcut(&app_sc("thing", name), &router, None, &opener).is_err(),
                "{name}"
            );
            assert!(opener.calls().is_empty(), "{name} must not open anything");
        }
    }

    #[test]
    fn opener_failure_is_an_error() {
        let (_home, router) = router(&[]);
        let opener = FakeOpener {
            fail: true,
            ..Default::default()
        };
        assert!(
            execute_shortcut(&url_sc("work", "https://a.com"), &router, None, &opener).is_err()
        );
    }

    // ---------- hook: matched failures never paste or escalate ----------

    #[test]
    fn hook_is_none_when_voice_commands_off_or_no_match() {
        let (_home, router) = router(&[]);
        let opener = FakeOpener::default();
        let list = [url_sc("work", "https://a.com")];
        assert_eq!(
            run_shortcut_hook(false, &list, "work", &router, None, &opener),
            None
        );
        assert_eq!(
            run_shortcut_hook(true, &list, "something else", &router, None, &opener),
            None
        );
        assert_eq!(
            run_shortcut_hook(true, &[], "work", &router, None, &opener),
            None
        );
        assert!(opener.calls().is_empty());
    }

    #[test]
    fn hook_success_is_handled_with_executed_event() {
        let (_home, router) = router(&[]);
        let opener = FakeOpener::default();
        let list = [url_sc("work", "https://a.com")];
        let result =
            run_shortcut_hook(true, &list, "Work.", &router, None, &opener).expect("matched");
        assert_eq!(result.outcome, VoiceHookOutcome::HANDLED);
        assert!(matches!(result.event, HookEvent::Executed(_)));
    }

    #[test]
    fn hook_failure_is_still_handled_never_pastes_or_escalates() {
        let (_home, router) = router(&[]);
        let opener = FakeOpener {
            fail: true,
            ..Default::default()
        };
        let list = [
            url_sc("work", "https://a.com"),
            app_sc("thing", "downloads"),
        ];
        for utterance in ["work", "thing"] {
            let result =
                run_shortcut_hook(true, &list, utterance, &router, None, &opener).expect("matched");
            assert!(result.outcome.skip_paste, "{utterance}");
            assert!(!result.outcome.escalate_to_agent, "{utterance}");
            assert!(matches!(result.event, HookEvent::Error(_)), "{utterance}");
        }
    }
}
