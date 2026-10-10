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
//! App matching: each installed `.app` is indexed by its folder (file
//! stem) plus its Info.plist names (`CFBundleDisplayName`, then
//! `CFBundleName`, so Visual Studio Code also answers to "Code"). Matches
//! are ranked: exact stem, exact alias, stem prefix, alias prefix. Ties
//! inside a rank go to the smallest path, so results never depend on
//! directory scan order. Only when none of those match does a small curated
//! table map common speech-to-text mishearings (e.g. "eater" -> iTerm,
//! "vs code" -> Visual Studio Code) onto a target, which must itself resolve
//! against the installed apps -- the router never invents an app that is not
//! installed. The index (including plist aliases) is cached with the app list
//! for the TTL, so plists are read once per refresh, not per route call.
//!
//! Not ported from Python: the non-app entries of Python's YAML phrase
//! allowlist (e.g. `open_url`). URLs are handled by the bare-domain matcher
//! in this module instead, and command *execution* wiring is a later phase.
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

    /// Extra names an installed app answers to (its bundle display name,
    /// bundle name, ...). Called ONCE per app per cache refresh -- the parsed
    /// index is cached with the app list for the TTL. Default: none.
    fn aliases_for(&self, _app: &std::path::Path) -> Vec<String> {
        Vec::new()
    }
}

/// Aliases shorter than this (after normalization) are ignored.
pub(crate) const MIN_ALIAS_CHARS: usize = 2;
/// Aliases longer than this are ignored.
pub(crate) const MAX_ALIAS_CHARS: usize = 100;
/// `Info.plist` files larger than this are not read.
pub(crate) const MAX_PLIST_BYTES: u64 = 1024 * 1024;

/// `CFBundleDisplayName` then `CFBundleName` from an `Info.plist` (XML or
/// binary). Non-string values, empty/whitespace values are skipped, values are
/// trimmed, and duplicates (case-insensitive) keep the first. Malformed or
/// non-dictionary input yields an empty list; never panics.
pub(crate) fn bundle_names_from_plist_bytes(bytes: &[u8]) -> Vec<String> {
    let Ok(value) = plist::Value::from_reader(std::io::Cursor::new(bytes)) else {
        return Vec::new();
    };
    let Some(dict) = value.as_dictionary() else {
        return Vec::new();
    };
    let mut names: Vec<String> = Vec::new();
    for key in ["CFBundleDisplayName", "CFBundleName"] {
        let Some(raw) = dict.get(key).and_then(plist::Value::as_string) else {
            continue;
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let folded = trimmed.to_lowercase();
        if names
            .iter()
            .any(|existing| existing.to_lowercase() == folded)
        {
            continue;
        }
        names.push(trimmed.to_string());
    }
    names
}

/// Reads `<app>/Contents/Info.plist` (only if it is at most
/// [`MAX_PLIST_BYTES`]) and returns [`bundle_names_from_plist_bytes`]; a
/// missing/unreadable/oversized file yields an empty list.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // only macOS discovery calls this in production
pub(crate) fn read_bundle_names(app_path: &std::path::Path) -> Vec<String> {
    let plist_path = app_path.join("Contents").join("Info.plist");
    let Ok(metadata) = std::fs::metadata(&plist_path) else {
        return Vec::new();
    };
    if metadata.len() > MAX_PLIST_BYTES {
        return Vec::new();
    }
    match std::fs::read(&plist_path) {
        Ok(bytes) => bundle_names_from_plist_bytes(&bytes),
        Err(_) => Vec::new(),
    }
}

/// Curated speech-to-text mishearings, consulted only when no installed
/// app matches the subject directly. Each left-hand phrase maps to one
/// target name, which is matched against the installed index (never
/// invented). Lookups are single-step: a target never re-enters this table.
const STT_ALIASES: &[(&str, &str)] = &[
    ("vs code", "visual studio code"),
    ("vscode", "visual studio code"),
    ("eater", "iterm"),
    ("i term", "iterm"),
    ("chrome", "google chrome"),
];

/// Always reports no installed apps -- the production discovery for
/// non-macOS targets, where app-open commands are out of scope for this
/// phase and must deterministically return `NoMatch`.
#[cfg_attr(target_os = "macos", allow(dead_code))] // macOS builds use MacAppDiscovery
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

    fn aliases_for(&self, app: &std::path::Path) -> Vec<String> {
        read_bundle_names(app)
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

/// One installed app in the cached index. `stem_lower` and `aliases_lower`
/// are pre-normalized (lowercased, length-filtered, deduplicated) at refresh
/// time so `route()` never touches the filesystem.
#[derive(Debug, Clone)]
struct AppEntry {
    path: PathBuf,
    stem_lower: String,
    aliases_lower: Vec<String>,
}

/// Cached app index plus the time it was built (drives the TTL).
struct AppCache {
    entries: Vec<AppEntry>,
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
    fn entries(&self) -> Vec<AppEntry> {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let needs_refresh = match cache.as_ref() {
            Some(c) => c.fetched_at.elapsed() >= self.cache_ttl,
            None => true,
        };
        if needs_refresh {
            let entries = self.build_index();
            *cache = Some(AppCache {
                entries: entries.clone(),
                fetched_at: Instant::now(),
            });
            entries
        } else {
            cache
                .as_ref()
                .map(|c| c.entries.clone())
                .unwrap_or_default()
        }
    }

    /// Scans installed apps once and reads each app's aliases once, producing
    /// the normalized index that `route()` searches.
    fn build_index(&self) -> Vec<AppEntry> {
        self.discovery
            .scan_installed_apps()
            .into_iter()
            .filter_map(|path| {
                let stem_lower = path.file_stem()?.to_str()?.to_lowercase();
                let mut aliases_lower: Vec<String> = Vec::new();
                for raw in self.discovery.aliases_for(&path) {
                    let alias = raw.trim().to_lowercase();
                    let len = alias.chars().count();
                    if !(MIN_ALIAS_CHARS..=MAX_ALIAS_CHARS).contains(&len) {
                        continue;
                    }
                    if !aliases_lower.contains(&alias) {
                        aliases_lower.push(alias);
                    }
                }
                Some(AppEntry {
                    path,
                    stem_lower,
                    aliases_lower,
                })
            })
            .collect()
    }

    fn strip_open_verb(lower: &str) -> Option<&str> {
        for verb in OPEN_VERBS {
            if let Some(rest) = lower.strip_prefix(verb) {
                return Some(rest.trim());
            }
        }
        None
    }

    /// FR2/FR3 + F6: ranked matching over the cached index. Ranks, first
    /// non-empty wins: (1) stem == subject, (2) an alias == subject, (3) stem
    /// starts with subject, (4) an alias starts with subject. Within a rank the
    /// smallest path wins, so the result never depends on scan order. Only if
    /// all four miss does the curated STT table apply: the heard phrase is
    /// re-matched with ranks 1-4 against its target, once, and the target must
    /// resolve against installed apps (an app is never invented).
    fn match_app(&self, subject: &str) -> Option<CommandAction> {
        let entries = self.entries();
        if let Some(action) = Self::rank_match(&entries, subject) {
            return Some(action);
        }
        let target = STT_ALIASES
            .iter()
            .find(|(heard, _)| *heard == subject)
            .map(|(_, target)| *target)?;
        Self::rank_match(&entries, target)
    }

    /// Applies ranks 1-4 of [`CommandRouter::match_app`] for one key.
    fn rank_match(entries: &[AppEntry], key: &str) -> Option<CommandAction> {
        type Rank = fn(&AppEntry, &str) -> bool;
        let ranks: [Rank; 4] = [
            |e, k| e.stem_lower == k,
            |e, k| e.aliases_lower.iter().any(|a| a == k),
            |e, k| e.stem_lower.starts_with(k),
            |e, k| e.aliases_lower.iter().any(|a| a.starts_with(k)),
        ];
        for matches in ranks {
            let best = entries
                .iter()
                .filter(|e| matches(e, key))
                .min_by(|a, b| a.path.cmp(&b.path));
            if let Some(entry) = best {
                return Some(Self::open_app_action(&entry.stem_lower, &entry.path));
            }
        }
        None
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
    fn t4_vs_code_abbreviation_resolves_through_the_curated_table() {
        // REPLACES the old t4 ("launch VS Code" == NoMatch): the curated STT
        // table now maps the common abbreviations onto the real installed app.
        let router = router_with_apps(&["Visual Studio Code.app"]);
        assert_opens_app(router.route("launch VS Code"), "Visual Studio Code");
        assert_opens_app(router.route("open vscode"), "Visual Studio Code");
    }

    #[test]
    fn t4b_an_abbreviation_in_no_table_is_still_no_match() {
        let router = router_with_apps(&["Visual Studio Code.app"]);
        assert_eq!(router.route("launch vsc"), RouteDecision::NoMatch);
        assert_eq!(router.route("launch xyz code"), RouteDecision::NoMatch);
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

    // =====================================================================
    // F6: bundle-name aliases, ranking, curated STT table, plist parsing
    // =====================================================================

    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct AliasedDiscovery {
        apps: Vec<PathBuf>,
        aliases: HashMap<PathBuf, Vec<String>>,
        scans: Arc<AtomicUsize>,
        alias_calls: Arc<AtomicUsize>,
    }

    impl AppDiscovery for AliasedDiscovery {
        fn scan_installed_apps(&self) -> Vec<PathBuf> {
            self.scans.fetch_add(1, Ordering::SeqCst);
            self.apps.clone()
        }
        fn aliases_for(&self, app: &std::path::Path) -> Vec<String> {
            self.alias_calls.fetch_add(1, Ordering::SeqCst);
            self.aliases.get(app).cloned().unwrap_or_default()
        }
    }

    struct Aliased {
        router: CommandRouter,
        scans: Arc<AtomicUsize>,
        alias_calls: Arc<AtomicUsize>,
    }

    fn aliased(entries: &[(&str, &[&str])]) -> Aliased {
        let scans = Arc::new(AtomicUsize::new(0));
        let alias_calls = Arc::new(AtomicUsize::new(0));
        let apps: Vec<PathBuf> = entries
            .iter()
            .map(|(path, _)| PathBuf::from(path))
            .collect();
        let aliases = entries
            .iter()
            .map(|(path, names)| {
                (
                    PathBuf::from(path),
                    names.iter().map(|n| n.to_string()).collect(),
                )
            })
            .collect();
        let discovery = AliasedDiscovery {
            apps,
            aliases,
            scans: scans.clone(),
            alias_calls: alias_calls.clone(),
        };
        Aliased {
            router: CommandRouter::with_home(Box::new(discovery), real_home()),
            scans,
            alias_calls,
        }
    }

    fn opened_path(decision: RouteDecision) -> PathBuf {
        match decision {
            RouteDecision::Matched {
                action: CommandAction::OpenApp { resolved_path, .. },
            } => resolved_path,
            other => panic!("expected Matched{{OpenApp}}, got {other:?}"),
        }
    }

    #[test]
    fn alias_matches_the_bundle_display_name() {
        let a = aliased(&[("/Applications/Visual Studio Code.app", &["Code"])]);
        assert_opens_app(a.router.route("open code"), "Visual Studio Code");
    }

    #[test]
    fn exact_stem_beats_an_alias_of_another_app() {
        let a = aliased(&[
            ("/Applications/Code.app", &[]),
            ("/Applications/Visual Studio Code.app", &["Code"]),
        ]);
        assert_opens_app(a.router.route("open code"), "Code");
    }

    #[test]
    fn exact_alias_beats_a_prefix_stem() {
        let a = aliased(&[
            ("/Applications/Codex.app", &[]),
            ("/Applications/Visual Studio Code.app", &["Code"]),
        ]);
        assert_opens_app(a.router.route("open code"), "Visual Studio Code");
    }

    #[test]
    fn prefix_stem_beats_prefix_alias() {
        let a = aliased(&[
            ("/Applications/Stickies.app", &["Notes Board"]),
            ("/Applications/Notes Plus.app", &[]),
        ]);
        assert_opens_app(a.router.route("open notes"), "Notes Plus");
    }

    #[test]
    fn prefix_alias_matches_when_nothing_ranks_higher() {
        let a = aliased(&[("/Applications/Stickies.app", &["Notes Board"])]);
        assert_opens_app(a.router.route("open notes"), "Stickies");
    }

    #[test]
    fn alias_collisions_resolve_to_the_smallest_path_in_any_scan_order() {
        let forward = aliased(&[("/a/Alpha.app", &["Editor"]), ("/b/Beta.app", &["Editor"])]);
        let reverse = aliased(&[("/b/Beta.app", &["Editor"]), ("/a/Alpha.app", &["Editor"])]);
        assert_eq!(
            opened_path(forward.router.route("open editor")),
            PathBuf::from("/a/Alpha.app")
        );
        assert_eq!(
            opened_path(reverse.router.route("open editor")),
            PathBuf::from("/a/Alpha.app")
        );
    }

    #[test]
    fn duplicate_stems_and_prefix_ties_resolve_to_the_smallest_path_in_any_scan_order() {
        let dup_forward = aliased(&[("/a/Same.app", &[]), ("/b/Same.app", &[])]);
        let dup_reverse = aliased(&[("/b/Same.app", &[]), ("/a/Same.app", &[])]);
        assert_eq!(
            opened_path(dup_forward.router.route("open same")),
            PathBuf::from("/a/Same.app")
        );
        assert_eq!(
            opened_path(dup_reverse.router.route("open same")),
            PathBuf::from("/a/Same.app")
        );

        let tie_forward = aliased(&[
            ("/Applications/Notes.app", &[]),
            ("/Applications/Notebook.app", &[]),
        ]);
        let tie_reverse = aliased(&[
            ("/Applications/Notebook.app", &[]),
            ("/Applications/Notes.app", &[]),
        ]);
        assert_eq!(
            opened_path(tie_forward.router.route("open note")),
            PathBuf::from("/Applications/Notebook.app")
        );
        assert_eq!(
            opened_path(tie_reverse.router.route("open note")),
            PathBuf::from("/Applications/Notebook.app")
        );
    }

    #[test]
    fn too_short_empty_whitespace_and_oversized_aliases_are_ignored() {
        let long = "x".repeat(101);
        let a = aliased(&[("/Applications/Thing.app", &["X", "", "   ", long.as_str()])]);
        assert_eq!(a.router.route("open x"), RouteDecision::NoMatch);
        assert_eq!(
            a.router.route(&format!("open {long}")),
            RouteDecision::NoMatch
        );
        // The exact boundary lengths are accepted.
        let two = aliased(&[("/Applications/Thing.app", &["ab"])]);
        assert_opens_app(two.router.route("open ab"), "Thing");
        let hundred = "y".repeat(100);
        let max = aliased(&[("/Applications/Thing.app", &[hundred.as_str()])]);
        assert_opens_app(max.router.route(&format!("open {hundred}")), "Thing");
    }

    #[test]
    fn alias_matching_is_case_insensitive() {
        let a = aliased(&[("/Applications/Visual Studio Code.app", &["Code"])]);
        assert_opens_app(a.router.route("open CODE"), "Visual Studio Code");
    }

    // ---- curated STT-mishearing table ----

    #[test]
    fn curated_table_maps_common_mishearings_onto_installed_apps() {
        let code = router_with_apps(&["Visual Studio Code.app"]);
        assert_opens_app(code.route("open vs code"), "Visual Studio Code");
        assert_opens_app(code.route("open vscode"), "Visual Studio Code");
        let term = router_with_apps(&["iTerm.app"]);
        assert_opens_app(term.route("open eater"), "iTerm");
        let term2 = router_with_apps(&["iTerm2.app"]);
        assert_opens_app(term2.route("open eater"), "iTerm2");
        let chrome = router_with_apps(&["Google Chrome.app"]);
        assert_opens_app(chrome.route("open chrome"), "Google Chrome");
    }

    #[test]
    fn curated_table_never_invents_an_app_that_is_not_installed() {
        let none = router_with_apps(&["Safari.app"]);
        assert_eq!(none.route("open eater"), RouteDecision::NoMatch);
        assert_eq!(none.route("open vs code"), RouteDecision::NoMatch);
        assert_eq!(none.route("open chrome"), RouteDecision::NoMatch);
    }

    #[test]
    fn direct_matches_always_beat_the_curated_table() {
        // An app literally called Eater wins over mapping "eater" -> iTerm.
        let both = router_with_apps(&["Eater.app", "iTerm.app"]);
        assert_opens_app(both.route("open eater"), "Eater");
        // "chrome" is a direct prefix of "Chrome Canary" -- direct beats curated.
        let canary = router_with_apps(&["Chrome Canary.app", "Google Chrome.app"]);
        assert_opens_app(canary.route("open chrome"), "Chrome Canary");
    }

    #[test]
    fn curated_table_can_also_hit_an_alias() {
        let a = aliased(&[("/Applications/Visual Studio Code.app", &["Code"])]);
        assert_opens_app(a.router.route("open vs code"), "Visual Studio Code");
    }

    // ---- index caching ----

    #[test]
    fn aliases_are_read_once_per_cache_refresh_not_per_route() {
        let a = aliased(&[
            ("/Applications/One.app", &["Uno"]),
            ("/Applications/Two.app", &["Dos"]),
        ]);
        for _ in 0..5 {
            let _ = a.router.route("open uno");
        }
        assert_eq!(a.scans.load(Ordering::SeqCst), 1);
        assert_eq!(
            a.alias_calls.load(Ordering::SeqCst),
            2,
            "once per app, not per route"
        );
    }

    #[test]
    fn a_ttl_refresh_rescans_and_rereads_aliases() {
        let mut a = aliased(&[
            ("/Applications/One.app", &["Uno"]),
            ("/Applications/Two.app", &["Dos"]),
        ]);
        a.router.cache_ttl = Duration::ZERO;
        for _ in 0..3 {
            let _ = a.router.route("open uno");
        }
        assert_eq!(a.scans.load(Ordering::SeqCst), 3);
        assert_eq!(a.alias_calls.load(Ordering::SeqCst), 6);
    }

    // ---- plist parsing ----

    fn xml_plist(pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut body = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>",
        );
        for (key, value) in pairs {
            body.push_str(&format!("<key>{key}</key><string>{value}</string>"));
        }
        body.push_str("</dict></plist>");
        body.into_bytes()
    }

    #[test]
    fn plist_returns_display_name_then_bundle_name() {
        let bytes = xml_plist(&[
            ("CFBundleName", "Short"),
            ("CFBundleDisplayName", "Pretty Name"),
        ]);
        assert_eq!(
            bundle_names_from_plist_bytes(&bytes),
            vec!["Pretty Name", "Short"]
        );
    }

    #[test]
    fn plist_dedups_case_insensitively_keeping_the_first() {
        let bytes = xml_plist(&[("CFBundleDisplayName", "Code"), ("CFBundleName", "code")]);
        assert_eq!(bundle_names_from_plist_bytes(&bytes), vec!["Code"]);
    }

    #[test]
    fn plist_with_only_one_name_key() {
        assert_eq!(
            bundle_names_from_plist_bytes(&xml_plist(&[("CFBundleName", "Only")])),
            vec!["Only"]
        );
        assert_eq!(
            bundle_names_from_plist_bytes(&xml_plist(&[("CFBundleDisplayName", "OnlyDisplay")])),
            vec!["OnlyDisplay"]
        );
    }

    #[test]
    fn plist_trims_and_drops_empty_names() {
        let bytes = xml_plist(&[
            ("CFBundleDisplayName", "  Padded  "),
            ("CFBundleName", "   "),
        ]);
        assert_eq!(bundle_names_from_plist_bytes(&bytes), vec!["Padded"]);
        assert!(bundle_names_from_plist_bytes(&xml_plist(&[("CFBundleName", "")])).is_empty());
    }

    #[test]
    fn plist_ignores_non_string_values_and_unrelated_keys() {
        let bytes = b"<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>CFBundleName</key><integer>5</integer><key>CFBundleDisplayName</key><array><string>x</string></array><key>Other</key><string>nope</string></dict></plist>";
        assert!(bundle_names_from_plist_bytes(bytes).is_empty());
    }

    #[test]
    fn plist_reads_binary_format() {
        let mut dict = plist::Dictionary::new();
        dict.insert(
            "CFBundleDisplayName".into(),
            plist::Value::String("Binary Name".into()),
        );
        let mut buffer = Vec::new();
        plist::to_writer_binary(&mut buffer, &plist::Value::Dictionary(dict)).unwrap();
        assert_eq!(bundle_names_from_plist_bytes(&buffer), vec!["Binary Name"]);
    }

    #[test]
    fn malformed_empty_and_non_dictionary_plists_yield_nothing_without_panicking() {
        assert!(bundle_names_from_plist_bytes(b"").is_empty());
        assert!(bundle_names_from_plist_bytes(b"not a plist at all").is_empty());
        assert!(bundle_names_from_plist_bytes(&[0xff, 0xfe, 0x00, 0x01]).is_empty());
        assert!(bundle_names_from_plist_bytes(b"bplist00\x00\x01").is_empty());
        assert!(bundle_names_from_plist_bytes(b"<?xml version=\"1.0\"?><plist version=\"1.0\"><array><string>a</string></array></plist>").is_empty());
        assert!(bundle_names_from_plist_bytes(b"<plist><dict><key>CFBundleName</key>").is_empty());
    }

    #[test]
    fn read_bundle_names_reads_the_app_info_plist() {
        let dir = TempDir::new().unwrap();
        let contents = dir.path().join("Thing.app/Contents");
        std::fs::create_dir_all(&contents).unwrap();
        std::fs::write(
            contents.join("Info.plist"),
            xml_plist(&[("CFBundleDisplayName", "Thing Pro")]),
        )
        .unwrap();
        assert_eq!(
            read_bundle_names(&dir.path().join("Thing.app")),
            vec!["Thing Pro"]
        );
    }

    #[test]
    fn read_bundle_names_tolerates_missing_files() {
        let dir = TempDir::new().unwrap();
        assert!(read_bundle_names(&dir.path().join("Nope.app")).is_empty());
        std::fs::create_dir_all(dir.path().join("Empty.app/Contents")).unwrap();
        assert!(read_bundle_names(&dir.path().join("Empty.app")).is_empty());
    }

    #[test]
    fn read_bundle_names_refuses_oversized_files() {
        let dir = TempDir::new().unwrap();
        let contents = dir.path().join("Huge.app/Contents");
        std::fs::create_dir_all(&contents).unwrap();
        let mut body = String::from(
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>CFBundleName</key><string>Huge</string></dict></plist><!--",
        );
        body.push_str(&"a".repeat(MAX_PLIST_BYTES as usize));
        body.push_str("-->");
        std::fs::write(contents.join("Info.plist"), body).unwrap();
        assert!(read_bundle_names(&dir.path().join("Huge.app")).is_empty());
    }
}
