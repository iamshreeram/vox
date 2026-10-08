//! Deterministic voice-command routing -- no LLM, no fuzzy ML intent
//! classification. Ports vox's `src/vox/commands/router.py` +
//! `app_registry.py` + `path_registry.py` matching algorithm exactly
//! (verified against their test suites), rather than inventing a new
//! similarity-threshold scheme: exact-normalized-name match first, then
//! prefix tolerance for trailing version suffixes ("iterm" -> "iterm2"),
//! never a "closest guess above zero" fuzzy score.
//!
//! Real `/Applications` scanning is macOS-only (see [`MacAppDiscovery`]);
//! other platforms get [`NullAppDiscovery`], which always returns no apps
//! (app-open commands correctly become `NoMatch` there, per the phase's
//! non-goals). The router itself and its matching logic are fully
//! cross-platform and exercised everywhere via [`AppDiscovery`] fakes.
//!
//! Deliberately NOT ported from Python (documented gaps, not oversights):
//! - Python aliases each app under both its `.app` folder name AND its
//!   `CFBundleDisplayName`/`CFBundleName` from Info.plist (e.g. Visual
//!   Studio Code's bundle name is just "Code"). This phase only indexes
//!   by folder name -- full Info.plist parsing needs a new dependency
//!   (a plist-parsing crate) and isn't required by any test case here.
//!   Follow-up.
//! - Python's static YAML phrase allowlist (tier 1, for curated STT
//!   mishearing corrections like "eater" -> iTerm, and non-app actions
//!   like `open_url`) isn't implemented yet -- this phase's non-goals
//!   explicitly defer command *execution* wiring to a later phase.
//!
//! Wired into the post-transcription pipeline in `actions.rs`, gated by
//! the `voice_commands_enabled` setting (default off): when enabled, a
//! recognized "open X" transcript is executed (via the OS opener) instead
//! of being pasted. `SafetyPolicy`'s confirmation gate has no destructive
//! action to guard yet (`OpenApp`/`OpenPath`/`OpenUrl` are all
//! non-destructive), so it remains library-only until a later phase adds
//! an action worth confirming.
//!
//! `CommandAction::OpenUrl` is produced by two trigger shapes: an explicit
//! "website" keyword (`"open website facebook"` -> defaults to `.com` if
//! the remainder has no dot) and a bare spoken/literal domain with the
//! "go to"/"open" verbs (`"go to google dot com"`, `"open google.com"`).
//! Only bare domains are ever accepted -- no scheme, path, port, or
//! userinfo -- since the result is handed straight to the OS URL opener;
//! see `match_url`/`is_valid_domain` below.

use once_cell::sync::Lazy;
use regex::Regex;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Result of routing a transcript through the command matcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteDecision {
    /// Matched a known deterministic action.
    Matched { action: CommandAction },
    /// No deterministic match -- caller (a later phase) decides whether to
    /// escalate to the agent bridge or do nothing; the router doesn't.
    NoMatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // Open* naming is this phase doc's own spec
pub enum CommandAction {
    OpenApp {
        name: String,
        resolved_path: PathBuf,
    },
    OpenPath {
        path: PathBuf,
    },
    OpenUrl {
        url: String,
    },
}

/// Supplies installed application bundle paths. Real filesystem scanning
/// is macOS-only; tests inject a fixed fake list instead of touching disk.
pub trait AppDiscovery: Send + Sync {
    fn scan_installed_apps(&self) -> Vec<PathBuf>;
}

/// Always reports no installed apps -- the production discovery for
/// non-macOS targets, where app-open commands are out of scope for this
/// phase and must deterministically return `NoMatch`.
pub struct NullAppDiscovery;

impl AppDiscovery for NullAppDiscovery {
    fn scan_installed_apps(&self) -> Vec<PathBuf> {
        Vec::new()
    }
}

/// Scans the standard macOS application directories for `.app` bundles.
/// Mirrors vox's `AppRegistry._DEFAULT_SEARCH_DIRS` exactly (four dirs,
/// not just `/Applications` -- plenty of built-in apps on modern macOS
/// live in `/System/Applications`).
#[cfg(target_os = "macos")]
pub struct MacAppDiscovery;

#[cfg(target_os = "macos")]
impl AppDiscovery for MacAppDiscovery {
    fn scan_installed_apps(&self) -> Vec<PathBuf> {
        let mut dirs = vec![
            PathBuf::from("/Applications"),
            PathBuf::from("/System/Applications"),
            PathBuf::from("/System/Applications/Utilities"),
        ];
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join("Applications"));
        }
        let mut apps = Vec::new();
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("app") {
                    apps.push(path);
                }
            }
        }
        apps
    }
}

/// Supplies the user's home directory. Injectable so tests can point at a
/// `tempfile::TempDir` instead of the real `$HOME` -- needed because known
/// folders are only matched if they actually exist on disk (mirrors
/// vox's `PathRegistry.find`, which checks `candidate.path.exists()`).
pub trait HomeDirProvider: Send + Sync {
    fn home_dir(&self) -> Option<PathBuf>;
}

pub struct RealHomeDir;

impl HomeDirProvider for RealHomeDir {
    fn home_dir(&self) -> Option<PathBuf> {
        #[cfg(windows)]
        {
            std::env::var_os("USERPROFILE").map(PathBuf::from)
        }
        #[cfg(not(windows))]
        {
            std::env::var_os("HOME").map(PathBuf::from)
        }
    }
}

struct AppCache {
    apps: Vec<PathBuf>,
    fetched_at: Instant,
}

/// Deterministic command router. Holds an [`AppDiscovery`] (real scan or
/// fake) plus a TTL cache over it (NFR2 -- must not rescan the filesystem
/// on every [`CommandRouter::route`] call).
pub struct CommandRouter {
    discovery: Box<dyn AppDiscovery>,
    home: Box<dyn HomeDirProvider>,
    cache: Mutex<Option<AppCache>>,
    cache_ttl: Duration,
}

const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(30);

/// Mirrors vox's `_OPEN_NAME_RE` verb set exactly (plus an optional
/// "please " prefix, stripped separately in `route()`).
const OPEN_VERBS: &[&str] = &["open ", "launch ", "start ", "switch to ", "go to "];

/// Mirrors vox's `PathRegistry._STANDARD_FOLDERS` exactly.
const KNOWN_FOLDERS: &[(&str, &str)] = &[
    ("home", ""),
    ("downloads", "Downloads"),
    ("documents", "Documents"),
    ("desktop", "Desktop"),
    ("pictures", "Pictures"),
    ("movies", "Movies"),
    ("music", "Music"),
    ("applications", "Applications"),
];

impl CommandRouter {
    /// Production constructor (real `$HOME`/`$USERPROFILE`). Used by
    /// `lib.rs` at startup; tests use `with_home` with fakes instead.
    pub fn new(discovery: Box<dyn AppDiscovery>) -> Self {
        Self::with_home(discovery, Box::new(RealHomeDir))
    }

    pub fn with_home(discovery: Box<dyn AppDiscovery>, home: Box<dyn HomeDirProvider>) -> Self {
        Self {
            discovery,
            home,
            cache: Mutex::new(None),
            cache_ttl: DEFAULT_CACHE_TTL,
        }
    }

    /// FR1: never panics on any input, including empty/whitespace,
    /// non-ASCII/unicode, and extremely long (10,000+ char) strings.
    pub fn route(&self, text: &str) -> RouteDecision {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return RouteDecision::NoMatch;
        }

        let lower = trimmed.to_lowercase();
        let lower = lower.strip_prefix("please ").unwrap_or(&lower);
        let Some(subject) = Self::strip_open_verb(lower) else {
            return RouteDecision::NoMatch;
        };
        if subject.is_empty() {
            return RouteDecision::NoMatch;
        }

        // Tier order mirrors vox's router exactly: a name that explicitly
        // says "folder"/"directory" disambiguates intent toward the path
        // registry even when an app of the same name exists (e.g. Apple's
        // own Music.app vs the ~/Music folder); otherwise app-launch wins
        // as the more common intent for a bare ambiguous name.
        let explicit_folder = subject.ends_with("folder") || subject.ends_with("directory");
        let explicit_website = subject.starts_with("website ");
        if explicit_folder {
            if let Some(action) = self.match_known_folder(subject) {
                return RouteDecision::Matched { action };
            }
            if let Some(action) = self.match_app(subject) {
                return RouteDecision::Matched { action };
            }
        } else if explicit_website {
            // Explicit "website" intent that fails domain validation must
            // return NoMatch outright, not fall back to guessing an
            // app/folder named after the (possibly garbage) remainder.
            return match Self::match_url(subject) {
                Some(action) => RouteDecision::Matched { action },
                None => RouteDecision::NoMatch,
            };
        } else {
            if let Some(action) = self.match_app(subject) {
                return RouteDecision::Matched { action };
            }
            if let Some(action) = self.match_known_folder(subject) {
                return RouteDecision::Matched { action };
            }
            if let Some(action) = Self::match_url(subject) {
                return RouteDecision::Matched { action };
            }
        }
        RouteDecision::NoMatch
    }

    /// NFR2: cached app list, refreshed only after `cache_ttl` elapses --
    /// never rescans the filesystem on every route() call.
    fn apps(&self) -> Vec<PathBuf> {
        let mut cache = self.cache.lock().expect("app cache mutex poisoned");
        let needs_refresh = match cache.as_ref() {
            Some(c) => c.fetched_at.elapsed() >= self.cache_ttl,
            None => true,
        };
        if needs_refresh {
            let apps = self.discovery.scan_installed_apps();
            *cache = Some(AppCache {
                apps: apps.clone(),
                fetched_at: Instant::now(),
            });
            apps
        } else {
            cache
                .as_ref()
                .expect("just checked Some above")
                .apps
                .clone()
        }
    }

    fn strip_open_verb(lower: &str) -> Option<&str> {
        for verb in OPEN_VERBS {
            if let Some(rest) = lower.strip_prefix(verb) {
                return Some(rest.trim());
            }
        }
        None
    }

    /// FR2/FR3: exact-normalized match first, then prefix tolerance for
    /// trailing version suffixes ("iterm" -> an app folder named
    /// "iterm2") -- ports vox's `AppRegistry.find` exactly rather than a
    /// fuzzy similarity score, so there's no magic threshold to tune and
    /// no risk of ever picking a merely-similar-but-wrong app.
    fn match_app(&self, subject: &str) -> Option<CommandAction> {
        let apps = self.apps();
        let stems: Vec<(String, &PathBuf)> = apps
            .iter()
            .filter_map(|p| {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| (s.to_lowercase(), p))
            })
            .collect();

        if let Some((name, path)) = stems.iter().find(|(name, _)| name == subject) {
            return Some(Self::open_app_action(name, path));
        }
        stems
            .iter()
            .find(|(name, _)| name.starts_with(subject))
            .map(|(name, path)| Self::open_app_action(name, path))
    }

    fn open_app_action(lower_name: &str, path: &PathBuf) -> CommandAction {
        // Recover the original-cased display name from the real file stem
        // rather than echoing back the lowercased match key.
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(lower_name)
            .to_string();
        CommandAction::OpenApp {
            name,
            resolved_path: path.clone(),
        }
    }

    /// Matches `CommandAction::OpenUrl` (backlog item: "open website X" /
    /// "go to x dot com"). Deliberately conservative: only bare domains are
    /// accepted (no scheme, no path, no port, no userinfo) -- anything else
    /// is treated as a non-match rather than guessed at, since this result
    /// gets handed straight to the OS URL opener.
    fn match_url(subject: &str) -> Option<CommandAction> {
        let (explicit, rest) = match subject.strip_prefix("website ") {
            Some(rest) => (true, rest.trim()),
            None => (false, subject.trim()),
        };
        if rest.is_empty() {
            return None;
        }
        // When speech is split into several words, require explicit "dot"
        // separators between domain labels so path words cannot be folded
        // into an otherwise-valid label by the normalization step below.
        let tokens: Vec<_> = rest.split_whitespace().collect();
        if tokens.len() > 1
            && tokens.iter().enumerate().any(|(index, token)| {
                (index % 2 == 0 && *token == "dot") || (index % 2 == 1 && *token != "dot")
            })
        {
            return None;
        }

        let mut normalized = Self::normalize_spoken_domain(rest);
        if explicit && !normalized.contains('.') {
            normalized.push_str(".com");
        }

        if Self::is_valid_domain(&normalized) {
            Some(CommandAction::OpenUrl {
                url: format!("https://{normalized}"),
            })
        } else {
            None
        }
    }

    /// Converts spoken-style domains ("google dot com") into literal ones
    /// ("google.com"). Tokens are joined with no separator since the
    /// spoken/literal "dot" is itself the separator; this also means any
    /// stray extra word (e.g. a trailing "slash search") gets smashed into
    /// the surrounding label and reliably fails `is_valid_domain` rather
    /// than being silently dropped -- this phase only supports bare
    /// domains, never paths.
    fn normalize_spoken_domain(rest: &str) -> String {
        rest.split_whitespace()
            .map(|token| if token == "dot" { "." } else { token })
            .collect::<String>()
    }

    /// Conservative bare-domain validator: lowercase alphanumeric labels
    /// (hyphens allowed mid-label, never leading/trailing), at least one
    /// literal dot, and a purely-alphabetic final TLD label 2-24 chars
    /// long. Rejects colons/slashes/@ by construction (not in the allowed
    /// character class), which blocks scheme-smuggling attempts like
    /// "javascript:alert(1)" before they ever reach the OS URL opener.
    fn is_valid_domain(candidate: &str) -> bool {
        static DOMAIN_RE: Lazy<Regex> = Lazy::new(|| {
            Regex::new(
                r"^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)*\.[a-z]{2,24}$",
            )
            .expect("static domain regex is valid")
        });
        candidate.len() <= 253 && DOMAIN_RE.is_match(candidate)
    }

    fn match_known_folder(&self, subject: &str) -> Option<CommandAction> {
        let cleaned = Self::canonicalize_folder_name(subject);
        let (_, relative) = KNOWN_FOLDERS.iter().find(|(key, _)| *key == cleaned)?;
        let home = self.home.home_dir()?;
        let path = if relative.is_empty() {
            home
        } else {
            home.join(relative)
        };
        path.exists().then_some(CommandAction::OpenPath { path })
    }

    /// Mirrors vox's `PathRegistry._canonicalize` exactly: strip a leading
    /// "the " (not "my " -- that's not something Python supports, and a
    /// prior draft of this phase's own test table incorrectly assumed it
    /// did), then strip one trailing "folder"/"directory" suffix.
    fn canonicalize_folder_name(subject: &str) -> String {
        let mut s = subject.trim();
        if let Some(rest) = s.strip_prefix("the ") {
            s = rest;
        }
        for suffix in [" folder", " directory"] {
            if let Some(rest) = s.strip_suffix(suffix) {
                s = rest;
                break;
            }
        }
        s.trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct FakeAppDiscovery(Vec<PathBuf>);

    impl AppDiscovery for FakeAppDiscovery {
        fn scan_installed_apps(&self) -> Vec<PathBuf> {
            self.0.clone()
        }
    }

    fn apps(names: &[&str]) -> Box<dyn AppDiscovery> {
        Box::new(FakeAppDiscovery(names.iter().map(PathBuf::from).collect()))
    }

    /// Real $HOME/$USERPROFILE, for tests that don't care about existence
    /// checks (most app-matching tests -- folders aren't involved).
    fn real_home() -> Box<dyn HomeDirProvider> {
        Box::new(RealHomeDir)
    }

    /// A temp directory standing in for $HOME, with the given standard
    /// folders actually created on disk -- needed because folder matching
    /// mirrors vox's existence check (a nonexistent "Downloads" must not
    /// match).
    struct FakeHome(PathBuf);
    impl HomeDirProvider for FakeHome {
        fn home_dir(&self) -> Option<PathBuf> {
            Some(self.0.clone())
        }
    }

    fn temp_home_with_folders(folders: &[&str]) -> (TempDir, Box<dyn HomeDirProvider>) {
        let dir = TempDir::new().expect("create temp home");
        for folder in folders {
            if folder.is_empty() {
                continue; // "home" itself, already exists
            }
            std::fs::create_dir(dir.path().join(folder)).expect("create fake folder");
        }
        let home: Box<dyn HomeDirProvider> = Box::new(FakeHome(dir.path().to_path_buf()));
        (dir, home)
    }

    fn router_with_apps(names: &[&str]) -> CommandRouter {
        CommandRouter::with_home(apps(names), real_home())
    }

    fn assert_opens_app(decision: RouteDecision, expected_name: &str) {
        match decision {
            RouteDecision::Matched {
                action: CommandAction::OpenApp { name, .. },
            } => assert_eq!(name, expected_name),
            other => panic!("expected Matched{{OpenApp}}, got {other:?}"),
        }
    }

    fn assert_opens_path(decision: RouteDecision, expected: &std::path::Path) {
        match decision {
            RouteDecision::Matched {
                action: CommandAction::OpenPath { path },
            } => assert_eq!(path, expected),
            other => panic!("expected Matched{{OpenPath}}, got {other:?}"),
        }
    }

    // ---- open app ----

    #[test]
    fn t1_matches_exact_app_name_case_insensitive_prefix() {
        let router = router_with_apps(&["iTerm.app", "Safari.app"]);
        assert_opens_app(router.route("open iterm"), "iTerm");
    }

    #[test]
    fn t2_matches_fully_uppercase_input() {
        let router = router_with_apps(&["iTerm.app"]);
        assert_opens_app(router.route("open ITERM"), "iTerm");
    }

    #[test]
    fn t3_matches_multiword_app_name_with_launch_verb() {
        let router = router_with_apps(&["Visual Studio Code.app"]);
        assert_opens_app(
            router.route("launch Visual Studio Code"),
            "Visual Studio Code",
        );
    }

    #[test]
    fn t4_abbreviation_without_alias_table_is_no_match() {
        // Ported from vox's own behavior, not just this phase's
        // assumption: AppRegistry.find has no entry for "vs code" (it only
        // indexes the real folder/bundle names), and "visual studio code"
        // does not start with the query "vs code" either, so this is
        // NoMatch via the same exact+prefix mechanism as everything else
        // -- no special-cased alias table needed to get this right.
        let router = router_with_apps(&["Visual Studio Code.app"]);
        assert_eq!(router.route("launch VS Code"), RouteDecision::NoMatch);
    }

    #[test]
    fn t5_no_installed_apps_is_no_match() {
        let router = router_with_apps(&[]);
        assert_eq!(router.route("open iterm"), RouteDecision::NoMatch);
    }

    #[test]
    fn t6_dissimilar_app_name_is_no_match() {
        let router = router_with_apps(&["iTerm.app"]);
        assert_eq!(router.route("open xcodebuilder"), RouteDecision::NoMatch);
    }

    #[test]
    fn t7_empty_string_is_no_match_no_panic() {
        let router = router_with_apps(&["iTerm.app"]);
        assert_eq!(router.route(""), RouteDecision::NoMatch);
    }

    #[test]
    fn t8_unicode_emoji_garbage_is_no_match_no_panic() {
        let router = router_with_apps(&["iTerm.app"]);
        assert_eq!(router.route("日本語テスト"), RouteDecision::NoMatch);
    }

    #[test]
    fn t9_extremely_long_input_does_not_hang() {
        let router = router_with_apps(&["iTerm.app"]);
        let long_text = "random word ".repeat(1000); // ~12,000 chars
        let start = Instant::now();
        let _ = router.route(&long_text);
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "route() took too long on a long input: {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn tolerates_trailing_version_suffix_ported_from_vox_app_registry_tests() {
        // vox's test_tolerates_trailing_version_suffix: real CFBundleName
        // "iTerm2" vs spoken "iterm". This phase indexes by folder stem
        // rather than CFBundleName (documented gap, see module docs), so
        // the equivalent case here is a folder literally named
        // "iTerm2.app" -- same prefix-tolerance mechanism, same intent.
        let router = router_with_apps(&["iTerm2.app"]);
        assert_opens_app(router.route("open iterm"), "iTerm2");
    }

    // ---- open path/folder ----

    #[test]
    fn t10_matches_known_folder() {
        let (dir, home) = temp_home_with_folders(&["Downloads"]);
        let router = CommandRouter::with_home(apps(&[]), home);
        assert_opens_path(
            router.route("open downloads"),
            &dir.path().join("Downloads"),
        );
    }

    #[test]
    fn t11_tolerates_the_prefix_and_folder_suffix() {
        // Ported from vox's test_folder_phrasing_variants_all_resolve.
        // NOTE: an earlier draft of this test used "open my downloads
        // folder" -- vox's PathRegistry only strips a leading "the ", not
        // "my ", so that input was never actually validated behavior.
        // Fixed to match what vox actually tests and ships.
        let (dir, home) = temp_home_with_folders(&["Downloads"]);
        let router = CommandRouter::with_home(apps(&[]), home);
        assert_opens_path(
            router.route("open the downloads folder"),
            &dir.path().join("Downloads"),
        );
    }

    #[test]
    fn folder_phrasing_directory_suffix_also_resolves() {
        let (dir, home) = temp_home_with_folders(&["Downloads"]);
        let router = CommandRouter::with_home(apps(&[]), home);
        assert_opens_path(
            router.route("open downloads directory"),
            &dir.path().join("Downloads"),
        );
    }

    #[test]
    fn t12_unknown_place_is_no_match() {
        let (_dir, home) = temp_home_with_folders(&["Downloads"]);
        let router = CommandRouter::with_home(apps(&[]), home);
        assert_eq!(router.route("open the moon"), RouteDecision::NoMatch);
    }

    #[test]
    fn known_folder_name_that_does_not_exist_on_disk_is_no_match() {
        // Ports vox's PathRegistry.find existence check -- a standard
        // folder name only resolves if it's actually there.
        let (_dir, home) = temp_home_with_folders(&[]); // no Downloads created
        let router = CommandRouter::with_home(apps(&[]), home);
        assert_eq!(router.route("open downloads"), RouteDecision::NoMatch);
    }

    #[test]
    fn applications_is_a_known_folder() {
        let (dir, home) = temp_home_with_folders(&["Applications"]);
        let router = CommandRouter::with_home(apps(&[]), home);
        assert_opens_path(
            router.route("open applications"),
            &dir.path().join("Applications"),
        );
    }

    // ---- additional verbs (ported from vox's dynamic-apps test suite) ----

    #[test]
    fn switch_to_and_go_to_verbs_also_dynamic_match() {
        let router = router_with_apps(&["Slack.app"]);
        assert_opens_app(router.route("switch to slack"), "Slack");
        assert_opens_app(router.route("go to slack"), "Slack");
    }

    #[test]
    fn please_prefix_is_tolerated() {
        let router = router_with_apps(&["Slack.app"]);
        assert_opens_app(router.route("please open slack"), "Slack");
    }

    // ---- app vs. folder name collisions (ported from vox's test suite) ----

    #[test]
    fn ambiguous_name_prefers_app_over_folder() {
        let (_dir, home) = temp_home_with_folders(&["Music"]);
        let router = CommandRouter::with_home(apps(&["Music.app"]), home);
        assert_opens_app(router.route("open music"), "Music");
    }

    #[test]
    fn explicit_folder_wording_overrides_app_collision() {
        let (dir, home) = temp_home_with_folders(&["Music"]);
        let router = CommandRouter::with_home(apps(&["Music.app"]), home);
        assert_opens_path(
            router.route("open the music folder"),
            &dir.path().join("Music"),
        );
    }

    #[test]
    fn explicit_directory_wording_also_overrides_app_collision() {
        let (dir, home) = temp_home_with_folders(&[]); // "home" needs no subfolder
        let router = CommandRouter::with_home(apps(&["Home.app"]), home);
        assert_opens_path(router.route("open home directory"), dir.path());
    }

    // ---- concurrency (T22) ----

    #[test]
    fn t22_concurrent_route_calls_do_not_panic_or_race() {
        use std::sync::Arc;
        use std::thread;

        let router = Arc::new(router_with_apps(&["iTerm.app", "Safari.app"]));
        let handles: Vec<_> = (0..16)
            .map(|i| {
                let router = Arc::clone(&router);
                thread::spawn(move || {
                    let text = if i % 2 == 0 {
                        "open iterm"
                    } else {
                        "open downloads"
                    };
                    router.route(text)
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("worker thread panicked");
        }
    }
    // ---- open website / URL (backlog: "open website X" / "go to x dot com") ----

    fn assert_opens_url(decision: RouteDecision, expected_url: &str) {
        match decision {
            RouteDecision::Matched {
                action: CommandAction::OpenUrl { url },
            } => assert_eq!(url, expected_url),
            other => panic!("expected Matched{{OpenUrl}}, got {other:?}"),
        }
    }

    #[test]
    fn u1_go_to_spoken_dot_domain_opens_url() {
        let router = router_with_apps(&[]);
        assert_opens_url(router.route("go to google dot com"), "https://google.com");
    }

    #[test]
    fn u2_open_website_with_bare_name_defaults_to_dot_com() {
        let router = router_with_apps(&[]);
        assert_opens_url(
            router.route("open website facebook"),
            "https://facebook.com",
        );
    }

    #[test]
    fn u3_open_literal_dotted_domain_opens_url() {
        let router = router_with_apps(&[]);
        assert_opens_url(router.route("open google.com"), "https://google.com");
    }

    #[test]
    fn u4_open_website_with_spoken_subdomain_dots() {
        let router = router_with_apps(&[]);
        assert_opens_url(
            router.route("open website www dot example dot com"),
            "https://www.example.com",
        );
    }

    #[test]
    fn u5_hyphenated_domain_label_is_valid() {
        let router = router_with_apps(&[]);
        assert_opens_url(router.route("go to my-site dot com"), "https://my-site.com");
    }

    #[test]
    fn u6_installed_app_still_wins_over_url_fallback_for_go_to() {
        // Regression: adding URL matching must not break the existing
        // app-launch behavior of the "go to" verb when there's no dot.
        let router = router_with_apps(&["Slack.app"]);
        assert_opens_app(router.route("go to slack"), "Slack");
    }

    #[test]
    fn u7_open_website_with_nothing_after_is_no_match() {
        let router = router_with_apps(&[]);
        assert_eq!(router.route("open website"), RouteDecision::NoMatch);
    }

    #[test]
    fn u8_bare_word_with_no_dot_and_no_website_keyword_is_no_match() {
        let router = router_with_apps(&[]);
        assert_eq!(router.route("go to facebook"), RouteDecision::NoMatch);
    }

    #[test]
    fn u9_single_letter_tld_is_rejected() {
        let router = router_with_apps(&[]);
        assert_eq!(router.route("go to example dot x"), RouteDecision::NoMatch);
    }

    #[test]
    fn u10_numeric_tld_is_rejected() {
        let router = router_with_apps(&[]);
        assert_eq!(router.route("go to 123 dot 456"), RouteDecision::NoMatch);
    }

    #[test]
    fn u11_colon_scheme_smuggling_is_rejected() {
        let router = router_with_apps(&[]);
        assert_eq!(
            router.route("go to javascript:alert(1)"),
            RouteDecision::NoMatch
        );
    }

    #[test]
    fn u12_trailing_extra_words_after_domain_is_rejected_not_truncated() {
        // Must not silently truncate to just the domain and drop a path --
        // this phase explicitly only supports bare domains, not paths.
        let router = router_with_apps(&[]);
        assert_eq!(
            router.route("go to google dot com slash search"),
            RouteDecision::NoMatch
        );
    }

    #[test]
    fn u13_explicit_website_keyword_does_not_fall_back_to_app_guessing_on_failure() {
        // Explicit "website" intent that fails validation must return
        // NoMatch outright, not silently try to match an app/folder named
        // after the garbage text.
        let router = router_with_apps(&["Javascript:alert(1).app"]);
        assert_eq!(
            router.route("open website javascript:alert(1)"),
            RouteDecision::NoMatch
        );
    }

    #[test]
    fn u14_multiple_spaces_are_tolerated() {
        let router = router_with_apps(&[]);
        assert_opens_url(
            router.route("go to   google   dot   com"),
            "https://google.com",
        );
    }

    #[test]
    fn u15_empty_and_whitespace_only_still_no_match_no_panic() {
        let router = router_with_apps(&[]);
        assert_eq!(router.route("go to"), RouteDecision::NoMatch);
        assert_eq!(router.route("open website   "), RouteDecision::NoMatch);
    }

    #[test]
    fn u16_long_dot_spam_does_not_hang() {
        let router = router_with_apps(&[]);
        let long_text = format!("go to {}", "dot ".repeat(2000));
        let start = Instant::now();
        let _ = router.route(&long_text);
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "route() took too long on url-shaped pathological input: {:?}",
            start.elapsed()
        );
    }
}
