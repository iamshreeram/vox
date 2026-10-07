//! Deterministic voice-command routing -- no LLM, no fuzzy ML intent
//! classification. Mirrors vox's `src/vox/commands/router.py`: a small,
//! fixed action set (open app / open path / open URL) matched via bounded
//! string similarity, never "closest guess above zero" when nothing is
//! confidently similar enough.
//!
//! Real `/Applications` scanning is macOS-only (see [`MacAppDiscovery`]);
//! other platforms get [`NullAppDiscovery`], which always returns no apps
//! (app-open commands correctly become `NoMatch` there, per the phase's
//! non-goals). The router itself and its matching logic are fully
//! cross-platform and exercised everywhere via [`AppDiscovery`] fakes.

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
pub enum CommandAction {
    OpenApp { name: String, resolved_path: PathBuf },
    OpenPath { path: PathBuf },
    #[allow(dead_code)] // no voice command produces this yet in this phase
    OpenUrl { url: String },
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

/// Scans `/Applications` and `~/Applications` for `.app` bundles.
#[cfg(target_os = "macos")]
pub struct MacAppDiscovery;

#[cfg(target_os = "macos")]
impl AppDiscovery for MacAppDiscovery {
    fn scan_installed_apps(&self) -> Vec<PathBuf> {
        let mut apps = Vec::new();
        let mut dirs = vec![PathBuf::from("/Applications")];
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join("Applications"));
        }
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

struct AppCache {
    apps: Vec<PathBuf>,
    fetched_at: Instant,
}

/// Deterministic command router. Holds an [`AppDiscovery`] (real scan or
/// fake) plus a TTL cache over it (NFR2 -- must not rescan the filesystem
/// on every [`CommandRouter::route`] call).
pub struct CommandRouter {
    discovery: Box<dyn AppDiscovery>,
    cache: Mutex<Option<AppCache>>,
    cache_ttl: Duration,
    /// `strsim::normalized_levenshtein` threshold above which an app-name
    /// match is accepted (range 0.0-1.0, 1.0 = identical). Chosen against
    /// this phase's test table: every exact-after-normalization match
    /// (e.g. "iterm" vs "iterm") lands at 1.0, while "vs code" vs "visual
    /// studio code" (T4, deliberately NoMatch -- no alias table yet) lands
    /// well under 0.5. 0.85 leaves comfortable margin on both sides
    /// without needing per-case tuning.
    similarity_threshold: f64,
}

const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(30);
const DEFAULT_SIMILARITY_THRESHOLD: f64 = 0.85;

const OPEN_VERBS: &[&str] = &["open ", "launch ", "start "];

const KNOWN_FOLDERS: &[&str] = &[
    "downloads",
    "documents",
    "desktop",
    "home",
    "pictures",
    "movies",
    "music",
];

impl CommandRouter {
    pub fn new(discovery: Box<dyn AppDiscovery>) -> Self {
        Self::with_ttl(discovery, DEFAULT_CACHE_TTL)
    }

    pub fn with_ttl(discovery: Box<dyn AppDiscovery>, cache_ttl: Duration) -> Self {
        let _ = (discovery, cache_ttl);
        todo!("RED: implemented in the next commit")
    }

    /// FR1: never panics on any input, including empty/whitespace,
    /// non-ASCII/unicode, and extremely long (10,000+ char) strings.
    pub fn route(&self, text: &str) -> RouteDecision {
        let _ = text;
        todo!("RED: implemented in the next commit")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeAppDiscovery(Vec<&'static str>);

    impl AppDiscovery for FakeAppDiscovery {
        fn scan_installed_apps(&self) -> Vec<PathBuf> {
            self.0.iter().map(PathBuf::from).collect()
        }
    }

    fn router_with_apps(apps: &[&'static str]) -> CommandRouter {
        CommandRouter::new(Box::new(FakeAppDiscovery(apps.to_vec())))
    }

    fn assert_opens_app(decision: RouteDecision, expected_name: &str) {
        match decision {
            RouteDecision::Matched {
                action: CommandAction::OpenApp { name, .. },
            } => assert_eq!(name, expected_name),
            other => panic!("expected Matched{{OpenApp}}, got {other:?}"),
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
        assert_opens_app(router.route("launch Visual Studio Code"), "Visual Studio Code");
    }

    #[test]
    fn t4_abbreviation_without_alias_table_is_no_match() {
        // Deliberately NoMatch for this phase -- no alias table yet. A
        // future phase adding aliases makes this a visible, intentional
        // test change, not a silent regression.
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

    // ---- open path/folder ----

    #[test]
    fn t10_matches_known_folder() {
        let router = router_with_apps(&[]);
        let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap();
        match router.route("open downloads") {
            RouteDecision::Matched {
                action: CommandAction::OpenPath { path },
            } => assert_eq!(path, PathBuf::from(home).join("Downloads")),
            other => panic!("expected Matched{{OpenPath}}, got {other:?}"),
        }
    }

    #[test]
    fn t11_tolerates_filler_words_around_folder_name() {
        let router = router_with_apps(&[]);
        let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap();
        match router.route("open my downloads folder") {
            RouteDecision::Matched {
                action: CommandAction::OpenPath { path },
            } => assert_eq!(path, PathBuf::from(home).join("Downloads")),
            other => panic!("expected Matched{{OpenPath}}, got {other:?}"),
        }
    }

    #[test]
    fn t12_unknown_place_is_no_match() {
        let router = router_with_apps(&[]);
        assert_eq!(router.route("open the moon"), RouteDecision::NoMatch);
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
                    let text = if i % 2 == 0 { "open iterm" } else { "open downloads" };
                    router.route(text)
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("worker thread panicked");
        }
    }
}
