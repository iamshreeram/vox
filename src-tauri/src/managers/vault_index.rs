//! Per-vault search index contracts: identity, status, refresh report, query DTOs and the
//! `IndexManager` API (DESIGN Rev 3, sections 3.3, 3.4 and 3.5).
//!
//! Implemented: identity, cache file, refresh and the query methods. Nothing in the app calls
//! this module yet.
//! Index and query functions take the generation they started with and return
//! `RagError::GenerationMismatch` when the vault has changed since then.

// Not yet called by the app; the allowance below keeps the unused-code lint quiet until it is wired in.
#![allow(dead_code)]

use std::fmt::{self, Write as _};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::{Connection, ErrorCode, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use sha2::{Digest, Sha256};

use crate::managers::vault::NoteSource;
use crate::managers::vault_io::VaultIoWorker;

/// How long a SQLite call waits for a lock before it reports `busy`.
const BUSY_TIMEOUT: Duration = Duration::from_millis(250);

/// Schema version written by this build (`PRAGMA user_version`).
const SCHEMA_VERSION: i64 = 1;

/// Version 1 schema (DESIGN 3.4). `chunks` is an FTS5 table; `meta` holds identity and folders.
const SCHEMA_V1: &str = "
CREATE TABLE notes (
    rel TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    skip_reason TEXT,
    mtime_ms INTEGER NOT NULL,
    size INTEGER NOT NULL,
    title TEXT NOT NULL,
    folder TEXT NOT NULL,
    note_ts INTEGER,
    status TEXT,
    pending_since INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0
);
CREATE VIRTUAL TABLE chunks USING fts5(
    rel UNINDEXED, ord UNINDEXED, title, heading, body,
    tokenize = 'unicode61 remove_diacritics 2'
);
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

/// Stable identity of one vault: the first 16 hex characters of sha256(canonical root) (DESIGN 3.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultIdentity {
    /// 16 lowercase hex characters.
    pub id: String,
}

impl VaultIdentity {
    /// Derives the identity from the canonical root path (DESIGN 3.3).
    pub fn for_root(root: &Path) -> Result<VaultIdentity, RagError> {
        let canonical = fs::canonicalize(root).map_err(|_| RagError::Invalid)?;
        let digest = Sha256::digest(canonical.to_string_lossy().as_bytes());
        let mut id = String::with_capacity(16);
        for byte in digest.iter().take(8) {
            let _ = write!(id, "{byte:02x}");
        }
        Ok(VaultIdentity { id })
    }
}

/// Failure while opening the cache file, classified for `last_error_kind` (DESIGN 3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenError {
    /// Corrupt, not a database, or from a newer schema: the file is deleted and rebuilt.
    Rebuild,
    /// Another connection holds a lock. Never treated as corruption.
    Busy,
    /// Permission denied on the data directory or the file.
    Permission,
    /// The disk is full.
    Full,
    /// Any other I/O or SQLite failure.
    Io,
}

impl OpenError {
    /// Fixed kind word stored in `last_error_kind`. Never a message.
    fn kind(self) -> &'static str {
        match self {
            OpenError::Rebuild | OpenError::Io => "io",
            OpenError::Busy => "busy",
            OpenError::Permission => "permission",
            OpenError::Full => "full",
        }
    }
}

/// Maps a SQLite error to the open classification (DESIGN 3.3).
fn classify_sql(err: &rusqlite::Error) -> OpenError {
    match err {
        rusqlite::Error::SqliteFailure(e, _) => match e.code {
            ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase => OpenError::Rebuild,
            ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => OpenError::Busy,
            ErrorCode::PermissionDenied | ErrorCode::ReadOnly => OpenError::Permission,
            ErrorCode::DiskFull => OpenError::Full,
            _ => OpenError::Io,
        },
        _ => OpenError::Io,
    }
}

/// Maps a migration error to the open classification.
fn classify_migration(err: &rusqlite_migration::Error) -> OpenError {
    match err {
        rusqlite_migration::Error::RusqliteError { err, .. } => classify_sql(err),
        _ => OpenError::Io,
    }
}

/// Maps a filesystem error to the open classification.
fn classify_io(err: &io::Error) -> OpenError {
    match err.kind() {
        io::ErrorKind::PermissionDenied => OpenError::Permission,
        io::ErrorKind::StorageFull => OpenError::Full,
        _ => OpenError::Io,
    }
}

/// File name of the cache for one vault identity (DESIGN 3.3).
fn cache_name(id: &str) -> String {
    format!("vault_index-{id}.db")
}

/// Sets owner-only permissions on `path` (unix; no-op elsewhere).
fn set_private(path: &Path) -> Result<(), OpenError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|e| classify_io(&e))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Creates the cache file with mode 0600 before SQLite opens it (DESIGN 3.3).
fn create_private_file(path: &Path) -> Result<(), OpenError> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    match opts.open(path) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(classify_io(&e)),
    }
    set_private(path)
}

/// Restricts the WAL and shared-memory sidecars to owner-only (best effort, unix).
fn restrict_sidecars(path: &Path) {
    for suffix in ["-wal", "-shm"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        let side = PathBuf::from(name);
        if side.exists() {
            let _ = set_private(&side);
        }
    }
}

/// Deletes the cache file with its `-wal` and `-shm` sidecars together (best effort).
fn delete_db_files(path: &Path) {
    let _ = fs::remove_file(path);
    for suffix in ["-wal", "-shm"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        let _ = fs::remove_file(PathBuf::from(name));
    }
}

/// Deletes every other `vault_index-*.db` file and its sidecars in `dir` (DESIGN 3.3).
fn delete_other_caches(dir: &Path, keep: &str) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let base = name
            .strip_suffix("-wal")
            .or_else(|| name.strip_suffix("-shm"))
            .unwrap_or(&name);
        if base.starts_with("vault_index-") && base.ends_with(".db") && base != keep {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Reads one `meta` value, if present.
fn meta_value(conn: &Connection, key: &str) -> Result<Option<String>, OpenError> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| {
        r.get::<_, String>(0)
    })
    .optional()
    .map_err(|e| classify_sql(&e))
}

/// Checks identity and allowed folders against `meta`; a fresh file gets them written (DESIGN 3.4).
/// Any mismatch means the cache belongs to another vault or folder set and must be rebuilt.
fn check_meta(conn: &Connection, id: &str, folders: &str) -> Result<(), OpenError> {
    let stored_id = meta_value(conn, "vault_id")?;
    let stored_folders = meta_value(conn, "folders")?;
    match (stored_id, stored_folders) {
        (None, None) => conn
            .execute(
                "INSERT INTO meta(key, value) VALUES ('vault_id', ?1), ('folders', ?2)",
                [id, folders],
            )
            .map(|_| ())
            .map_err(|e| classify_sql(&e)),
        (Some(a), Some(b)) if a == id && b == folders => Ok(()),
        _ => Err(OpenError::Rebuild),
    }
}

/// Opens (or creates) the cache file, applies migrations and checks its meta (DESIGN 3.3, 3.4).
fn open_cache(path: &Path, id: &str, folders: &str) -> Result<Connection, OpenError> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| classify_io(&e))?;
    }
    create_private_file(path)?;
    let mut conn = Connection::open(path).map_err(|e| classify_sql(&e))?;
    conn.busy_timeout(BUSY_TIMEOUT)
        .map_err(|e| classify_sql(&e))?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| classify_sql(&e))?;
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| classify_sql(&e))?;
    if version > SCHEMA_VERSION {
        return Err(OpenError::Rebuild);
    }
    Migrations::new(vec![M::up(SCHEMA_V1)])
        .to_latest(&mut conn)
        .map_err(|e| classify_migration(&e))?;
    check_meta(&conn, id, folders)?;
    restrict_sidecars(path);
    Ok(conn)
}

/// Errors from index and query calls. `Display` prints fixed words only (DESIGN 3.3, 3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RagError {
    /// Index unavailable: worker busy, DB busy, or reader timeout.
    Unavailable,
    /// The vault or its allowed folders changed after the caller started (DESIGN 3.3).
    GenerationMismatch,
    /// The requested note or project does not exist.
    NotFound,
    /// The requested path is outside the allowed folders.
    Forbidden,
    /// The arguments were malformed.
    Invalid,
    /// Unexpected internal failure. Carries no detail.
    Internal,
}

impl fmt::Display for RagError {
    /// Fixed words only; never carries note text, queries or paths (DESIGN 3.3).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            RagError::Unavailable => "vault index unavailable",
            RagError::GenerationMismatch => "vault index generation changed",
            RagError::NotFound => "vault note not found",
            RagError::Forbidden => "vault path not allowed",
            RagError::Invalid => "invalid vault query",
            RagError::Internal => "vault index internal error",
        };
        f.write_str(text)
    }
}

impl std::error::Error for RagError {}

/// Snapshot of the index for the status command (DESIGN 3.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexStatus {
    /// Notes currently indexed.
    pub notes: u32,
    /// Chunks currently indexed.
    pub chunks: u32,
    /// Notes skipped (unreadable, oversized, unsafe, sensitive name, and so on).
    pub skipped: u32,
    /// Aggregate count of names rejected as sensitive. Names are never stored.
    pub sensitive_names: u32,
    /// Time of the last completed refresh, in ms since the Unix epoch.
    pub last_refresh_ms: Option<i64>,
    /// True while a refresh is running.
    pub refreshing: bool,
    /// Fixed kind of the last error (for example `busy`), never a message.
    pub last_error_kind: Option<String>,
}

/// Outcome of one refresh pass (DESIGN 3.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshReport {
    /// Notes newly indexed.
    pub added: u32,
    /// Notes re-indexed after a change.
    pub updated: u32,
    /// Notes removed because they are gone from the vault.
    pub removed: u32,
    /// Notes skipped during this pass.
    pub skipped: u32,
    /// True when the budget ended the pass before all pending notes were processed.
    pub timed_out: bool,
    /// True when another refresh was already running, so this call did nothing.
    pub already_running: bool,
    /// True when the walk completed, so deletions were allowed.
    pub walk_complete: bool,
}

/// Options for `IndexManager::search` (DESIGN 3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOpts {
    /// Restrict results to this top-level folder or sub-folder (component boundary).
    pub folder: Option<String>,
    /// Keep only notes whose recency timestamp is within this many days.
    pub since_days: Option<u32>,
    /// Maximum results (default 5, max 8).
    pub limit: usize,
}

/// Options for `IndexManager::recent` (DESIGN 3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentOpts {
    /// Restrict results to this top-level folder or sub-folder (component boundary).
    pub folder: Option<String>,
    /// Keep only notes whose recency timestamp is within this many days.
    pub since_days: Option<u32>,
    /// Maximum results (default 5, max 10).
    pub limit: usize,
}

/// One search hit: a chunk of a note (DESIGN 3.5).
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// Vault-relative path of the note.
    pub rel: String,
    /// Heading the chunk sits under.
    pub heading: String,
    /// Short redacted snippet of the chunk.
    pub snippet: String,
    /// Note date (from the file name or frontmatter) in ms since the Unix epoch, when known.
    pub note_ts: Option<i64>,
    /// Modification time in ms since the Unix epoch.
    pub mtime_ms: i64,
    /// Ranking score, larger is better.
    pub score: f64,
}

/// A project note eligible for project matching (DESIGN 3.4 F4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectKey {
    /// Title shown for the note (frontmatter, then first heading, then file stem).
    pub title: String,
    /// File name without the `.md` extension.
    pub stem: String,
    /// Vault-relative path of the note.
    pub rel: String,
}

/// One recent note (DESIGN 3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentNote {
    /// Vault-relative path of the note.
    pub rel: String,
    /// Note title.
    pub title: String,
    /// Note date in ms since the Unix epoch, when known.
    pub note_ts: Option<i64>,
    /// Modification time in ms since the Unix epoch.
    pub mtime_ms: i64,
    /// First lines of the note (at most 200 characters).
    pub first_lines: String,
}

/// Result of `IndexManager::read` (DESIGN 3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadResult {
    /// Vault-relative path of the note.
    pub rel: String,
    /// Redacted text, at most 6000 characters.
    pub text: String,
    /// True when the text was cut to the limit.
    pub truncated: bool,
}

/// Result of `IndexManager::project_status` (DESIGN 3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusResult {
    /// Vault-relative path of the project note.
    pub rel: String,
    /// Project title.
    pub title: String,
    /// Modification date in ms since the Unix epoch.
    pub modified_ms: i64,
    /// Frontmatter status, when present.
    pub status: Option<String>,
    /// True when the status came from an explicit frontmatter field.
    pub explicit_status: bool,
    /// Last Session Log entries, at most 5, each at most 400 characters.
    pub entries: Vec<String>,
    /// Text under Next/TODO/Status headings, at most 600 characters.
    pub next_text: Option<String>,
    /// Up to 3 snippets from recent Daily notes that mention the project.
    pub daily_snippets: Vec<String>,
    /// True when several notes matched the name.
    pub ambiguous: bool,
    /// Candidate notes when ambiguous, at most 4.
    pub candidates: Vec<String>,
    /// Rendered plain-text summary, at most 5000 characters.
    pub text: String,
}

/// One persisted `notes` row as read straight from the SQLite file (test seam, DESIGN 3.4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DebugNote {
    /// Stored vault-relative path.
    pub rel: String,
    /// `current`, `pending` or `skipped`.
    pub state: String,
    /// Skip reason when skipped.
    pub skip_reason: Option<String>,
    /// Stored title.
    pub title: String,
    /// Stored frontmatter status.
    pub status: Option<String>,
    /// Stored note date in ms, when known.
    pub note_ts: Option<i64>,
    /// Stored size in bytes.
    pub size: i64,
    /// Stored modification time in ms.
    pub mtime_ms: i64,
    /// Failed read attempts so far.
    pub attempts: i64,
}

/// One persisted `chunks` row as read straight from the SQLite file (test seam, DESIGN 3.4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DebugChunk {
    /// Vault-relative path the chunk belongs to.
    pub rel: String,
    /// Chunk ordinal within the note.
    pub ord: i64,
    /// Title stored on the chunk.
    pub title: String,
    /// Heading stored on the chunk.
    pub heading: String,
    /// Chunk body text.
    pub body: String,
}

/// Logical content of every index table, read from the SQLite file (test seam).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DebugDump {
    /// All `notes` rows.
    pub notes: Vec<DebugNote>,
    /// All `chunks` rows.
    pub chunks: Vec<DebugChunk>,
    /// All `meta` key/value pairs.
    pub meta: Vec<(String, String)>,
}

/// Mutable state guarded by one mutex: vault config, generation and the open connection.
#[derive(Default)]
struct Inner {
    /// Bumped on every `configure` call (DESIGN 3.3).
    generation: u64,
    /// Vault root, when configured.
    root: Option<PathBuf>,
    /// Sorted, de-duplicated allowed folders.
    folders: Vec<String>,
    /// Identity of the configured vault.
    id: Option<String>,
    /// Open writer connection to the cache file, when healthy.
    conn: Option<Connection>,
    /// Fixed kind word of the last failure, cleared on success.
    last_error_kind: Option<String>,
}

/// Owns the index connections, the generation counter and the refresh flag (DESIGN 3.3).
pub struct IndexManager {
    /// Application data directory where `vault_index-<id>.db` files live.
    data_dir: PathBuf,
    /// Shared bounded vault-IO worker.
    io: Arc<VaultIoWorker>,
    /// Configuration, generation and connection (shared with queued refresh jobs).
    inner: Arc<Mutex<Inner>>,
    /// True while a refresh pass is running (shared with queued refresh jobs).
    refreshing: Arc<AtomicBool>,
    /// Lock-free mirror of `Inner::generation`, bumped first by `configure` (DESIGN 3.0 F3).
    generation_atomic: AtomicU64,
}

#[cfg(test)]
impl IndexManager {
    /// T0 TEST SEAM (DESIGN 3.0 F3, hard_generation_readable_while_configure_is_busy).
    /// Behaves like `configure` and calls `fs_hook` once from inside configure's
    /// filesystem step (when a root is given), so a test can hold configure there.
    pub(crate) fn configure_hooked(
        &self,
        root: Option<PathBuf>,
        allowed_folders: Vec<String>,
        fs_hook: &mut dyn FnMut(),
    ) -> u64 {
        self.configure_locked(root, allowed_folders, fs_hook)
    }
}

/// Locks a mutex, recovering the value if a previous holder panicked.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Opens the cache for `path`; on a corrupt or newer file, deletes it and rebuilds once.
fn open_or_rebuild(path: &Path, id: &str, folders: &str) -> Result<Connection, OpenError> {
    match open_cache(path, id, folders) {
        Err(OpenError::Rebuild) => {
            delete_db_files(path);
            open_cache(path, id, folders)
        }
        other => other,
    }
}

/// Opens the cache for the configured vault into `inner` and records the outcome.
fn open_locked(inner: &mut Inner, data_dir: &Path) {
    inner.conn = None;
    let (Some(id), Some(_)) = (inner.id.clone(), inner.root.as_ref()) else {
        return;
    };
    let path = data_dir.join(cache_name(&id));
    let folders = inner.folders.join("\n");
    match open_or_rebuild(&path, &id, &folders) {
        Ok(conn) => {
            inner.conn = Some(conn);
            inner.last_error_kind = None;
        }
        Err(e) => inner.last_error_kind = Some(e.kind().to_string()),
    }
}

/// Keeps `\n` and `\t`, drops `\r`, NUL and every other control character (DESIGN 3.4).
pub(crate) fn normalise(text: &str) -> String {
    text.chars()
        .filter(|&c| match c {
            '\n' | '\t' => true,
            '\r' | '\0' => false,
            _ => !c.is_control(),
        })
        .collect()
}

/// Removes the quotes around a frontmatter value, if both ends match.
fn unquote(value: &str) -> String {
    let v = value.trim();
    for q in ['"', '\''] {
        if let Some(inner) = v.strip_prefix(q).and_then(|s| s.strip_suffix(q)) {
            return inner.to_string();
        }
    }
    v.to_string()
}

/// Splits a leading `---` frontmatter block into `key: value` pairs and the body (DESIGN 3.4).
/// Tolerant: an unterminated block, or one without a leading fence, means no frontmatter.
pub(crate) fn split_frontmatter(text: &str) -> (Vec<(String, String)>, &str) {
    let mut lines = text.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return (Vec::new(), text);
    };
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return (Vec::new(), text);
    }
    let mut pairs = Vec::new();
    let mut offset = first.len();
    for line in lines {
        offset += line.len();
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == "---" {
            return (pairs, &text[offset..]);
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            if !key.is_empty() && !key.starts_with(' ') {
                pairs.push((key.trim().to_ascii_lowercase(), unquote(value)));
            }
        }
    }
    (Vec::new(), text)
}

/// First non-empty frontmatter value for `key`.
fn fm_get<'a>(fm: &'a [(String, String)], key: &str) -> Option<&'a str> {
    fm.iter()
        .find(|(k, v)| k == key && !v.is_empty())
        .map(|(_, v)| v.as_str())
}

/// Text of a level-1 heading line, if the line is one.
fn h1_text(line: &str) -> Option<String> {
    line.trim_start()
        .strip_prefix("# ")
        .map(|rest| rest.trim().to_string())
}

/// Note title: frontmatter `name`, then `title`, then the first H1, then the file stem (DESIGN 3.4).
pub(crate) fn title_of(fm: &[(String, String)], body: &str, stem: &str) -> String {
    if let Some(v) = fm_get(fm, "name").or_else(|| fm_get(fm, "title")) {
        return v.to_string();
    }
    body.lines()
        .find_map(h1_text)
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| stem.to_string())
}

/// Parses a leading `YYYY-MM-DD` (or `YYYY-MM`, read as the first day) to UTC-midnight ms.
fn parse_date_prefix(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let digits =
        |r: std::ops::Range<usize>| b.get(r).is_some_and(|x| x.iter().all(u8::is_ascii_digit));
    if b.len() < 7 || !digits(0..4) || b.get(4) != Some(&b'-') || !digits(5..7) {
        return None;
    }
    let year: i32 = s.get(0..4)?.parse().ok()?;
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: u32 = if b.get(7) == Some(&b'-') {
        if !digits(8..10) {
            return None;
        }
        s.get(8..10)?.parse().ok()?
    } else {
        1
    };
    let midnight = chrono::NaiveDate::from_ymd_opt(year, month, day)?.and_hms_opt(0, 0, 0)?;
    Some(midnight.and_utc().timestamp_millis())
}

/// Note date from the file stem, else from frontmatter `date`, `day` or `last_interaction` (DESIGN 3.4).
pub(crate) fn note_ts_of(stem: &str, fm: &[(String, String)]) -> Option<i64> {
    parse_date_prefix(stem).or_else(|| {
        ["date", "day", "last_interaction"]
            .iter()
            .find_map(|k| fm_get(fm, k).and_then(parse_date_prefix))
    })
}

/// Largest chunk body, in characters (DESIGN 3.4).
const CHUNK_MAX_CHARS: usize = 1500;
/// Most chunks kept per note (DESIGN 3.4).
const CHUNK_MAX_PER_NOTE: usize = 200;

/// Rewrites `[[target|alias]]` to `alias` and `[[target]]` to `target` (DESIGN 3.4).
pub(crate) fn reduce_wikilinks(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find("[[") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("]]") {
            Some(end) => {
                let inner = &after[..end];
                let shown = inner.rsplit_once('|').map_or(inner, |(_, alias)| alias);
                out.push_str(shown);
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// True when a line opens or closes a fenced code block (``` or ~~~).
fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

/// Splits a body into `(heading, lines)` sections. Headings inside code fences do not split (DESIGN 3.4).
fn sections(body: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = vec![(String::new(), Vec::new())];
    let mut in_fence = false;
    for raw in body.lines() {
        let line = reduce_wikilinks(raw);
        if is_fence(&line) {
            in_fence = !in_fence;
        } else if !in_fence {
            if let Some(text) = heading_text(&line) {
                out.push((text, Vec::new()));
                continue;
            }
        }
        if let Some((_, lines)) = out.last_mut() {
            lines.push(line);
        }
    }
    out
}

/// Heading text for a `#`..`######` line followed by a space, if the line is a heading.
fn heading_text(line: &str) -> Option<String> {
    let t = line.trim_start();
    let hashes = t.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') {
        Some(t[hashes..].trim().to_string())
    } else {
        None
    }
}

/// Cuts `text` into pieces of at most `max` characters on char boundaries.
fn hard_split(text: &str, max: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut cur = String::new();
    let mut count = 0usize;
    for ch in text.chars() {
        if count == max {
            pieces.push(std::mem::take(&mut cur));
            count = 0;
        }
        cur.push(ch);
        count += 1;
    }
    if !cur.is_empty() {
        pieces.push(cur);
    }
    pieces
}

/// Splits a note body into `(heading, text)` chunks (DESIGN 3.4). Each chunk is at most
/// 1500 characters and at most 200 chunks are returned per note.
pub(crate) fn chunk_body(body: &str) -> Vec<(String, String)> {
    let mut chunks: Vec<(String, String)> = Vec::new();
    for (heading, lines) in sections(body) {
        let mut buf = String::new();
        for line in lines {
            for piece in hard_split(&line, CHUNK_MAX_CHARS - 1) {
                let needed = buf.chars().count() + piece.chars().count() + 1;
                if needed > CHUNK_MAX_CHARS && !buf.trim().is_empty() {
                    chunks.push((heading.clone(), std::mem::take(&mut buf)));
                }
                buf.push_str(&piece);
                buf.push('\n');
            }
        }
        if !buf.trim().is_empty() {
            chunks.push((heading.clone(), buf));
        }
    }
    chunks.retain(|(_, text)| !text.trim().is_empty());
    chunks.truncate(CHUNK_MAX_PER_NOTE);
    chunks
}

/// Stem (file name without `.md`) of a vault-relative path.
fn stem_of(rel: &str) -> &str {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    name.strip_suffix(".md").unwrap_or(name)
}

/// Top-level folder of a vault-relative path.
fn folder_of(rel: &str) -> &str {
    rel.split('/').next().unwrap_or(rel)
}

/// True when `rel` sits under one of the allowed folders at a component boundary.
fn allowed(rel: &str, folders: &[String]) -> bool {
    folders.iter().any(|f| {
        rel.strip_prefix(f.as_str())
            .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// True when a walked name is safe to store: it survives normalisation and redaction unchanged.
pub(crate) fn name_is_safe(rel: &str) -> bool {
    normalise(rel) == rel && crate::managers::redact::redact(rel) == rel
}

/// Counts one note's `pending` attempts and returns the new value.
fn bump_attempts(conn: &Connection, rel: &str) -> rusqlite::Result<i64> {
    conn.execute(
        "UPDATE notes SET attempts = attempts + 1 WHERE rel = ?1",
        [rel],
    )?;
    conn.query_row("SELECT attempts FROM notes WHERE rel = ?1", [rel], |r| {
        r.get(0)
    })
}

/// Deletes a note's rows (metadata and chunks) in one transaction.
fn drop_chunks(conn: &mut Connection, rel: &str) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM chunks WHERE rel = ?1", [rel])?;
    tx.execute("DELETE FROM notes WHERE rel = ?1", [rel])?;
    tx.commit()
}

/// Marks a note `skipped` with `reason` and removes its chunks, so stale content is never served.
fn mark_skipped(conn: &mut Connection, rel: &str, reason: &str) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM chunks WHERE rel = ?1", [rel])?;
    tx.execute(
        "UPDATE notes SET state = 'skipped', skip_reason = ?2, pending_since = NULL WHERE rel = ?1",
        rusqlite::params![rel, reason],
    )?;
    tx.commit()
}

/// Maps a vault read error to the skip reason stored for the note, or `None` when the read
/// should be retried (`Unstable`).
fn read_skip_reason(err: &crate::managers::vault::VaultError) -> Option<&'static str> {
    use crate::managers::vault::VaultError as V;
    match err {
        V::Unstable => None,
        V::TooLarge => Some("too_large"),
        V::Forbidden => Some("forbidden"),
        V::Unsafe => Some("unsafe"),
        V::Missing => Some("missing"),
        _ => Some("unreadable"),
    }
}

/// Builds the stored rows for one note from its text (DESIGN 3.4 pipeline).
/// Order: normalise, redact the whole note, then parse frontmatter, title, date and chunks.
/// Stored fields and chunks derived from one note (DESIGN 3.4 pipeline output).
struct NoteRows {
    title: String,
    status: Option<String>,
    folder: String,
    note_ts: Option<i64>,
    chunks: Vec<(String, String)>,
}

fn build_rows(rel: &str, raw: &str) -> NoteRows {
    let clean = crate::managers::redact::redact(&normalise(raw));
    let (fm, body) = split_frontmatter(&clean);
    let title = crate::managers::redact::redact(&title_of(&fm, body, stem_of(rel)));
    let status = fm_get(&fm, "status").map(crate::managers::redact::redact);
    NoteRows {
        title,
        status,
        folder: folder_of(rel).to_string(),
        note_ts: note_ts_of(stem_of(rel), &fm),
        chunks: chunk_body(body),
    }
}

/// Reads and stores one pending note atomically (DESIGN 3.4 step 4-5).
/// Returns true when the note is now `current`.
fn process_note(
    conn: &mut Connection,
    source: &dyn NoteSource,
    rel: &str,
    tally: &mut Tally,
) -> rusqlite::Result<bool> {
    use crate::managers::vault::VaultError;
    let read = match source.read(rel) {
        Ok(r) => r,
        Err(VaultError::Unstable) => {
            let attempts = bump_attempts(conn, rel)?;
            if attempts >= 3 {
                mark_skipped(conn, rel, "unstable")?;
                tally.skipped += 1;
            }
            return Ok(false);
        }
        Err(e) => {
            let reason = read_skip_reason(&e).unwrap_or("unreadable");
            mark_skipped(conn, rel, reason)?;
            tally.skipped += 1;
            return Ok(false);
        }
    };
    let NoteRows {
        title,
        status,
        folder,
        note_ts,
        chunks,
    } = build_rows(rel, &read.text);
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM chunks WHERE rel = ?1", [rel])?;
    tx.execute(
        "UPDATE notes SET state = 'current', skip_reason = NULL, pending_since = NULL,
             mtime_ms = ?2, size = ?3, title = ?4, folder = ?5, note_ts = ?6, status = ?7
         WHERE rel = ?1",
        rusqlite::params![
            rel,
            read.mtime_ms,
            i64::try_from(read.size).unwrap_or(i64::MAX),
            title,
            folder,
            note_ts,
            status,
        ],
    )?;
    for (ord, (heading, text)) in chunks.iter().enumerate() {
        tx.execute(
            "INSERT INTO chunks(rel, ord, title, heading, body) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![rel, ord as i64, title, heading, text],
        )?;
    }
    tx.commit()?;
    Ok(true)
}

/// Upserts `pending` rows for changed notes and removes sensitive rows, in one transaction.
/// Returns the list of notes to read (oldest pending first, then by rel).
fn stage_pending(
    conn: &mut Connection,
    entries: &[(String, u64, i64)],
    tally: &mut Tally,
    now: i64,
) -> rusqlite::Result<Vec<String>> {
    let tx = conn.transaction()?;
    let mut to_read = Vec::new();
    for (rel, size, mtime) in entries {
        let existing: Option<(i64, i64, String)> = tx
            .query_row(
                "SELECT mtime_ms, size, state FROM notes WHERE rel = ?1",
                [rel],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let size_i = i64::try_from(*size).unwrap_or(i64::MAX);
        let unchanged = matches!(&existing, Some((m, s, st)) if *m == *mtime && *s == size_i && (st == "current" || st == "pending"));
        if unchanged {
            if matches!(&existing, Some((_, _, st)) if st == "pending") {
                to_read.push(rel.clone());
            }
            continue;
        }
        match existing {
            None => tally.added += 1,
            Some(_) => tally.updated += 1,
        }
        tx.execute(
            "INSERT INTO notes(rel, state, mtime_ms, size, title, folder, pending_since, attempts)
             VALUES (?1, 'pending', ?2, ?3, '', ?4, ?5, 0)
             ON CONFLICT(rel) DO UPDATE SET state = 'pending', mtime_ms = ?2, size = ?3,
                 skip_reason = NULL, attempts = 0,
                 pending_since = COALESCE(notes.pending_since, ?5)",
            rusqlite::params![rel, mtime, size_i, folder_of(rel), now],
        )?;
        to_read.push(rel.clone());
    }
    tx.commit()?;
    Ok(to_read)
}

/// Current time in ms since the Unix epoch.
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Starts a refresh: sets the flag, or reports `already_running`. Returns false when busy.
fn begin_refresh(flag: &AtomicBool) -> bool {
    flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

/// True while the index is still at generation `gen` (the generation a job was bound to).
fn generation_is(inner: &Mutex<Inner>, gen: u64) -> bool {
    lock(inner).generation == gen
}

/// Runs one refresh pass for the bound `folders` and generation `gen` (DESIGN 3.0 F1, 3.4).
/// The caller holds the `refreshing` flag. A pass whose generation is no longer current returns
/// at once without any mutation, including `last_error_kind`.
fn run_refresh(
    inner: &Mutex<Inner>,
    source: &dyn NoteSource,
    budget: Duration,
    folders: Vec<String>,
    gen: u64,
    after_pending: &mut dyn FnMut(),
) -> RefreshReport {
    let started = std::time::Instant::now();
    let mut report = RefreshReport {
        added: 0,
        updated: 0,
        removed: 0,
        skipped: 0,
        timed_out: false,
        already_running: false,
        walk_complete: false,
    };
    if !generation_is(inner, gen) {
        return report;
    }
    let limits = crate::managers::vault::WalkLimits {
        allowed_roots: folders.clone(),
        ..Default::default()
    };
    let walk = match source.walk(&limits) {
        Ok(w) => w,
        Err(_) => {
            let mut g = lock(inner);
            if g.generation == gen {
                g.last_error_kind = Some("walk".to_string());
            }
            return report;
        }
    };
    report.walk_complete = walk.complete;
    let mut tally = Tally::default();
    let mut entries = Vec::new();
    let mut safe_rels: Vec<String> = Vec::new();
    for n in &walk.notes {
        if !allowed(&n.rel, &folders) {
            continue;
        }
        if !name_is_safe(&n.rel) {
            tally.sensitive += 1;
            continue;
        }
        safe_rels.push(n.rel.clone());
        entries.push((n.rel.clone(), n.size, n.mtime_ms));
    }
    let mut to_read = {
        let mut g = lock(inner);
        if g.generation != gen {
            return report;
        }
        let Some(conn) = g.conn.as_mut() else {
            return report;
        };
        match stage_pending(conn, &entries, &mut tally, now_ms()) {
            Ok(list) => list,
            Err(e) => {
                g.last_error_kind = Some(classify_sql(&e).kind().to_string());
                return report;
            }
        }
    };
    after_pending();
    {
        let mut g = lock(inner);
        if g.generation != gen {
            return report;
        }
        if let Some(conn) = g.conn.as_mut() {
            if walk.complete {
                tally.removed = reconcile_deletions(conn, &safe_rels, &folders);
            }
            purge_sensitive(conn, &walk.notes);
        }
    }
    to_read.sort_by(|a, b| {
        let g = lock(inner);
        g.conn
            .as_ref()
            .map_or(i64::MAX, |c| pending_since(c, a))
            .cmp(&g.conn.as_ref().map_or(i64::MAX, |c| pending_since(c, b)))
            .then_with(|| a.cmp(b))
    });
    for (done, rel) in to_read.into_iter().enumerate() {
        if started.elapsed() >= budget && done > 0 {
            report.timed_out = true;
            break;
        }
        let mut g = lock(inner);
        if g.generation != gen {
            return report;
        }
        let Some(conn) = g.conn.as_mut() else {
            return report;
        };
        let _ = process_note(conn, source, &rel, &mut tally);
    }
    report.added = tally.added;
    report.updated = tally.updated;
    report.removed = tally.removed;
    report.skipped = tally.skipped;
    let g = lock(inner);
    if g.generation == gen {
        if let Some(conn) = g.conn.as_ref() {
            record_refresh(conn, &tally);
        }
    }
    report
}

fn pending_since(conn: &Connection, rel: &str) -> i64 {
    conn.query_row(
        "SELECT COALESCE(pending_since, 9223372036854775807) FROM notes WHERE rel = ?1",
        [rel],
        |r| r.get(0),
    )
    .unwrap_or(i64::MAX)
}

/// Deletes rows for notes missing from a complete walk. Returns how many were removed.
fn reconcile_deletions(conn: &mut Connection, safe_rels: &[String], folders: &[String]) -> u32 {
    let present: std::collections::HashSet<&str> = safe_rels.iter().map(String::as_str).collect();
    let stored: Vec<String> = conn
        .prepare("SELECT rel FROM notes")
        .and_then(|mut st| st.query_map([], |r| r.get(0))?.collect())
        .unwrap_or_default();
    let mut removed = 0u32;
    for rel in stored {
        if present.contains(rel.as_str()) && allowed(&rel, folders) {
            continue;
        }
        if drop_chunks(conn, &rel).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Removes any row whose name is sensitive, so a credential-bearing name never persists (DESIGN 3.4).
fn purge_sensitive(conn: &mut Connection, walked: &[crate::managers::vault::NoteEntry]) {
    for n in walked {
        if !name_is_safe(&n.rel) {
            let _ = drop_chunks(conn, &n.rel);
        }
    }
}

/// Writes the refresh timestamp and sensitive-name count to `meta`.
fn record_refresh(conn: &Connection, tally: &Tally) {
    let _ = conn.execute(
        "INSERT INTO meta(key, value) VALUES ('last_refresh_ms', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [now_ms().to_string()],
    );
    let _ = conn.execute(
        "INSERT INTO meta(key, value) VALUES ('sensitive_names', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [tally.sensitive.to_string()],
    );
}

/// Count of `pending` rows still waiting (test and status helper).
fn pending_count(conn: &Connection) -> u32 {
    count_rows(conn, "SELECT COUNT(*) FROM notes WHERE state = 'pending'")
}

/// Reads every table into plain values for the test seam.
fn dump(conn: &Connection) -> DebugDump {
    let mut out = DebugDump::default();
    if let Ok(mut st) = conn.prepare(
        "SELECT rel, state, skip_reason, title, status, note_ts, size, mtime_ms, attempts FROM notes ORDER BY rel",
    ) {
        if let Ok(rows) = st.query_map([], |r| {
            Ok(DebugNote {
                rel: r.get(0)?,
                state: r.get(1)?,
                skip_reason: r.get(2)?,
                title: r.get(3)?,
                status: r.get(4)?,
                note_ts: r.get(5)?,
                size: r.get(6)?,
                mtime_ms: r.get(7)?,
                attempts: r.get(8)?,
            })
        }) {
            out.notes = rows.flatten().collect();
        }
    }
    if let Ok(mut st) = conn.prepare(
        "SELECT rel, ord, title, heading, body FROM chunks ORDER BY rel, CAST(ord AS INTEGER)",
    ) {
        if let Ok(rows) = st.query_map([], |r| {
            Ok(crate::managers::vault_index::DebugChunk {
                rel: r.get(0)?,
                ord: r.get::<_, i64>(1)?,
                title: r.get(2)?,
                heading: r.get(3)?,
                body: r.get(4)?,
            })
        }) {
            out.chunks = rows.flatten().collect();
        }
    }
    if let Ok(mut st) = conn.prepare("SELECT key, value FROM meta ORDER BY key") {
        if let Ok(rows) = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))) {
            out.meta = rows.flatten().collect();
        }
    }
    out
}

/// Counts of one refresh pass, kept separate from the public report.
#[derive(Default)]
struct Tally {
    added: u32,
    updated: u32,
    removed: u32,
    skipped: u32,
    sensitive: u32,
}

/// Counts rows with a query, returning 0 on failure.
fn count_rows(conn: &Connection, sql: &str) -> u32 {
    conn.query_row(sql, [], |r| r.get::<_, i64>(0))
        .ok()
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0)
}

/// Milliseconds in one day.
const DAY_MS: i64 = 86_400_000;
/// Longest query text read, in characters (DESIGN 3.5).
const QUERY_MAX_CHARS: usize = 200;
/// Longest single query term, in characters (DESIGN 3.5).
const TERM_MAX_CHARS: usize = 40;
/// Most query terms (DESIGN 3.5).
const TERMS_MAX: usize = 12;
/// Rows fetched from SQLite before per-note ranking (DESIGN 3.5).
const FETCH_ROWS: i64 = 400;
/// Most chunks returned per note (DESIGN 3.5).
const PER_NOTE_MAX: usize = 2;
/// Longest snippet returned in a hit, in characters (DESIGN 3.5).
const SNIPPET_MAX: usize = 400;
/// Longest note text returned by `read`, in characters (DESIGN 3.5).
const READ_MAX: usize = 6_000;
/// Deadline for one note read through the IO worker (DESIGN 3.5).
const READ_DEADLINE: Duration = Duration::from_secs(4);

/// Folder and recency filter shared by the query SQL. Parameters: ?2 exact folder (empty = any),
/// ?3 folder prefix pattern, ?4 cutoff in ms (the recency timestamp must be greater).
const SCOPE_SQL: &str = "(?2 = '' OR n.rel = ?2 OR n.rel LIKE ?3 ESCAPE '\\') AND MAX(COALESCE(n.note_ts, 0), n.mtime_ms) > ?4";

/// Most query terms for `search_any` (DESIGN 3.0 F4).
const ANY_TERMS_MAX: usize = 6;

/// Sanitised query terms: maximal alphanumeric runs, at most `max_terms` of them, each cut to
/// `TERM_MAX_CHARS`, read from the first `QUERY_MAX_CHARS` characters.
fn query_terms(query: &str, max_terms: usize) -> Vec<String> {
    let head: String = query.chars().take(QUERY_MAX_CHARS).collect();
    head.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .take(max_terms)
        .map(|t| t.chars().take(TERM_MAX_CHARS).collect())
        .collect()
}

/// Joins quoted terms with `joiner`. The last term gets the prefix star. With `column`, every
/// term is restricted to that FTS column. `None` when there are no terms.
fn match_expr(terms: &[String], column: Option<&str>, joiner: &str) -> Option<String> {
    let last = terms.len().checked_sub(1)?;
    let parts: Vec<String> = terms
        .iter()
        .enumerate()
        .map(|(i, term)| {
            let star = if i == last { "*" } else { "" };
            match column {
                Some(col) => format!("{col} : \"{term}\"{star}"),
                None => format!("\"{term}\"{star}"),
            }
        })
        .collect();
    Some(parts.join(joiner))
}

/// Builds the FTS5 MATCH expression for `query`, or `None` when it has no terms (DESIGN 3.5).
/// Terms are maximal runs of alphanumerics, so no quote, operator or column filter survives.
/// Each term is a quoted phrase; the last one also gets a prefix star. With `column`, every term
/// is restricted to that FTS column.
fn fts_match(query: &str, column: Option<&str>) -> Option<String> {
    match_expr(&query_terms(query, TERMS_MAX), column, " ")
}

/// Like `fts_match` with OR between terms and at most `ANY_TERMS_MAX` terms (DESIGN 3.0 F4).
fn fts_match_any(query: &str) -> Option<String> {
    match_expr(&query_terms(query, ANY_TERMS_MAX), None, " OR ")
}

/// Escapes `%`, `_` and `\` so a folder prefix matches literally in `LIKE ... ESCAPE '\'`.
fn escape_like(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Validates a folder filter and returns the `(exact, prefix)` SQL parameters, both empty when no
/// folder is given (DESIGN 3.5). Bad syntax is `Invalid`; a folder outside the allowed folders is
/// `Forbidden`. The folder must lie at a component boundary inside an allowed folder.
fn folder_scope(folder: Option<&str>, folders: &[String]) -> Result<(String, String), RagError> {
    let Some(f) = folder else {
        return Ok((String::new(), String::new()));
    };
    let well_formed = !f.is_empty()
        && !f.contains('\\')
        && !f.chars().any(char::is_control)
        && f.split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.'));
    if !well_formed {
        return Err(RagError::Invalid);
    }
    let inside = folders.iter().any(|a| {
        f == a
            || f.strip_prefix(a.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    });
    if !inside {
        return Err(RagError::Forbidden);
    }
    Ok((f.to_string(), format!("{}/%", escape_like(f))))
}

/// Cutoff in ms for `since_days`: a note is kept when its age in whole days is at most
/// `since_days` (DESIGN 3.5). `None` keeps everything.
fn since_cutoff(since_days: Option<u32>, now: i64) -> i64 {
    match since_days {
        Some(days) => now.saturating_sub((i64::from(days) + 1).saturating_mul(DAY_MS)),
        None => i64::MIN,
    }
}

/// Clamps a caller limit: zero means the default, anything else is capped at `max`.
fn clamp_limit(limit: usize, default: usize, max: usize) -> usize {
    match limit {
        0 => default,
        n => n.min(max),
    }
}

/// Maps a SQLite error from a query. Busy or locked is `Unavailable`; anything else is `Internal`.
fn map_sql(err: rusqlite::Error) -> RagError {
    match err {
        rusqlite::Error::SqliteFailure(e, _)
            if matches!(e.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) =>
        {
            RagError::Unavailable
        }
        _ => RagError::Internal,
    }
}

/// Maps a vault read error to the query error kinds (DESIGN 3.5).
fn map_vault(err: crate::managers::vault::VaultError) -> RagError {
    use crate::managers::vault::VaultError;
    match err {
        VaultError::Forbidden | VaultError::Unsafe => RagError::Forbidden,
        VaultError::Missing => RagError::NotFound,
        VaultError::Io(io::ErrorKind::NotFound) => RagError::NotFound,
        VaultError::Invalid | VaultError::TooLarge => RagError::Invalid,
        VaultError::Unstable => RagError::Unavailable,
        VaultError::Exists | VaultError::Io(_) => RagError::Internal,
    }
}

/// Redacts and control-strips `text`, then keeps at most `max` characters (DESIGN 3.5).
fn clean_text(text: &str, max: usize) -> String {
    crate::managers::redact::redact(&normalise(text))
        .chars()
        .take(max)
        .collect()
}

/// Runs `f` on the open connection at generation `gen`. Returns `Ok(None)` when no vault is
/// configured, `GenerationMismatch` for a stale generation and `Unavailable` when the index is
/// not open (DESIGN 3.3, 3.5).
fn with_conn<T>(
    inner: &Mutex<Inner>,
    gen: u64,
    f: impl FnOnce(&Connection, &[String]) -> Result<T, RagError>,
) -> Result<Option<T>, RagError> {
    let guard = lock(inner);
    if gen != guard.generation {
        return Err(RagError::GenerationMismatch);
    }
    if guard.root.is_none() {
        return Ok(None);
    }
    let Some(conn) = guard.conn.as_ref() else {
        return Err(RagError::Unavailable);
    };
    f(conn, &guard.folders).map(Some)
}

/// Age in whole-day fractions of a recency timestamp; future dates clamp to zero (DESIGN 3.5).
fn age_days(ts_ms: i64, now: i64) -> f64 {
    (now.saturating_sub(ts_ms).max(0) as f64) / DAY_MS as f64
}

/// Recency timestamp of a note: the later of its date and its modification time (DESIGN 3.5).
fn recency_of(note_ts: Option<i64>, mtime_ms: i64) -> i64 {
    note_ts.unwrap_or(i64::MIN).max(mtime_ms)
}

/// Builds one ranked hit from a raw FTS row. `bm25` is the SQLite score (smaller is better).
/// Relevance is `-bm25`, boosted by recency: `relevance * (1 + 0.5 * exp(-age/60))` (DESIGN 3.5).
#[allow(clippy::too_many_arguments)]
fn rank_hit(
    rel: String,
    heading: String,
    body: String,
    bm25: f64,
    note_ts: Option<i64>,
    mtime_ms: i64,
    now: i64,
) -> (Hit, f64, i64) {
    let recency = recency_of(note_ts, mtime_ms);
    let boost = 1.0 + 0.5 * (-age_days(recency, now) / 60.0).exp();
    let score = -bm25 * boost;
    let hit = Hit {
        rel,
        heading: clean_text(&heading, SNIPPET_MAX),
        snippet: clean_text(&body, SNIPPET_MAX),
        note_ts,
        mtime_ms,
        score,
    };
    (hit, score, recency)
}

/// Orders candidates by score, then recency (newer first), then path, keeps at most two per note
/// and at most `limit` hits (DESIGN 3.5).
fn keep_best(mut cands: Vec<(Hit, f64, i64)>, limit: usize) -> Vec<Hit> {
    cands.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.2.cmp(&a.2))
            .then(a.0.rel.cmp(&b.0.rel))
    });
    let mut per_note: Vec<(String, usize)> = Vec::new();
    let mut out = Vec::new();
    for (hit, _, _) in cands {
        if out.len() >= limit {
            break;
        }
        let count = match per_note.iter_mut().find(|(r, _)| *r == hit.rel) {
            Some((_, c)) => c,
            None => {
                per_note.push((hit.rel.clone(), 0));
                match per_note.last_mut() {
                    Some((_, c)) => c,
                    None => continue,
                }
            }
        };
        if *count < PER_NOTE_MAX {
            *count += 1;
            out.push(hit);
        }
    }
    out
}

/// One Markdown heading outside code fences (DESIGN 3.5).
struct Heading {
    /// Index of the heading line.
    line: usize,
    /// Heading level, 1 to 6.
    level: usize,
    /// Heading text, trimmed.
    text: String,
}

/// Finds the headings of `lines`, ignoring lines inside fenced code blocks.
fn headings_of(lines: &[&str]) -> Vec<Heading> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (line, raw) in lines.iter().copied().enumerate() {
        if is_fence(raw) {
            in_fence = !in_fence;
        } else if !in_fence {
            let t = raw.trim_start();
            let level = t.chars().take_while(|&c| c == '#').count();
            if (1..=6).contains(&level) && t[level..].starts_with(' ') {
                out.push(Heading {
                    line,
                    level,
                    text: t[level..].trim().to_string(),
                });
            }
        }
    }
    out
}

/// Index of the first heading whose text equals `title`, ignoring case.
fn find_heading(headings: &[Heading], title: &str) -> Option<usize> {
    let wanted = title.trim().to_lowercase();
    headings
        .iter()
        .position(|h| h.text.to_lowercase() == wanted)
}

/// Lines under heading `at`, up to the next heading of the same or higher level. Sub-headings
/// are included (DESIGN 3.5).
fn section_body(lines: &[&str], headings: &[Heading], at: usize) -> String {
    let Some(h) = headings.get(at) else {
        return String::new();
    };
    let end = headings[at + 1..]
        .iter()
        .find(|o| o.level <= h.level)
        .map_or(lines.len(), |o| o.line);
    lines.get(h.line + 1..end).unwrap_or(&[]).join("\n")
}

/// The heading line and its section, for an exact case-insensitive `title` (DESIGN 3.5).
fn extract_section(text: &str, title: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let headings = headings_of(&lines);
    let at = find_heading(&headings, title)?;
    let head = lines.get(headings[at].line)?;
    Some(format!("{head}\n{}", section_body(&lines, &headings, at)))
}

/// A project note chosen by name (DESIGN 3.5).
struct Named {
    /// Vault-relative path.
    rel: String,
    /// Stored title.
    title: String,
    /// Stored modification time in ms.
    mtime_ms: i64,
}

/// Outcome of resolving a project name (DESIGN 3.5).
enum Pick {
    /// Exactly one project note.
    One(Named),
    /// Several notes match; the candidate paths, at most four.
    Ambiguous(Vec<String>),
}

/// Resolves a project name: exact stems under `Projects/` first, then the title column.
fn resolve_project(conn: &Connection, name: &str) -> Result<Option<Pick>, RagError> {
    let wanted = name.to_lowercase();
    let mut stmt = conn
        .prepare(
            "SELECT rel, title, mtime_ms FROM notes
             WHERE state = 'current' AND folder = 'Projects'",
        )
        .map_err(map_sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Named {
                rel: row.get(0)?,
                title: row.get(1)?,
                mtime_ms: row.get(2)?,
            })
        })
        .map_err(map_sql)?;
    let mut exact = Vec::new();
    for row in rows {
        let named = row.map_err(map_sql)?;
        if stem_of(&named.rel).to_lowercase() == wanted {
            exact.push(named);
        }
    }
    match exact.len() {
        0 => fts_pick(conn, name),
        1 => Ok(exact.pop().map(Pick::One)),
        _ => Ok(Some(Pick::Ambiguous(
            exact.iter().take(4).map(|n| n.rel.clone()).collect(),
        ))),
    }
}

/// Title-column search for a project name (DESIGN 3.5): the best note wins unless the second
/// relevance is at least 0.9 of the first.
fn fts_pick(conn: &Connection, name: &str) -> Result<Option<Pick>, RagError> {
    let Some(matching) = fts_match(name, Some("title")) else {
        return Ok(None);
    };
    let mut stmt = conn
        .prepare(
            "SELECT chunks.rel, n.title, n.mtime_ms, bm25(chunks, 0.0, 0.0, 3.0, 2.0, 1.0)
             FROM chunks JOIN notes n ON n.rel = chunks.rel
             WHERE chunks MATCH ?1 AND n.state = 'current' AND n.folder = 'Projects'
             ORDER BY 4 LIMIT ?2",
        )
        .map_err(map_sql)?;
    let rows = stmt
        .query_map(rusqlite::params![matching, FETCH_ROWS], |row| {
            Ok((
                Named {
                    rel: row.get(0)?,
                    title: row.get(1)?,
                    mtime_ms: row.get(2)?,
                },
                row.get::<_, f64>(3)?,
            ))
        })
        .map_err(map_sql)?;
    let mut best: Vec<(Named, f64)> = Vec::new();
    for row in rows {
        let (named, bm25) = row.map_err(map_sql)?;
        let relevance = -bm25;
        if relevance <= 0.0 {
            continue;
        }
        match best.iter_mut().find(|(b, _)| b.rel == named.rel) {
            Some(entry) => entry.1 = entry.1.max(relevance),
            None => best.push((named, relevance)),
        }
    }
    best.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.rel.cmp(&b.0.rel))
    });
    let Some(top) = best.first().map(|(_, r)| *r) else {
        return Ok(None);
    };
    if best.get(1).is_some_and(|(_, r)| *r >= 0.9 * top) {
        let cands = best.iter().take(4).map(|(n, _)| n.rel.clone()).collect();
        return Ok(Some(Pick::Ambiguous(cands)));
    }
    Ok(best.into_iter().next().map(|(n, _)| Pick::One(n)))
}

/// Up to three snippets from Daily notes of the last 14 days that mention `name` (DESIGN 3.5).
fn daily_snippets(conn: &Connection, name: &str, now: i64) -> Result<Vec<String>, RagError> {
    let Some(matching) = fts_match(name, Some("body")) else {
        return Ok(Vec::new());
    };
    let cutoff = since_cutoff(Some(14), now);
    let mut stmt = conn
        .prepare(
            "SELECT chunks.body FROM chunks JOIN notes n ON n.rel = chunks.rel
             WHERE chunks MATCH ?1 AND n.state = 'current' AND n.folder = 'Daily'
               AND MAX(COALESCE(n.note_ts, 0), n.mtime_ms) > ?2
             ORDER BY MAX(COALESCE(n.note_ts, 0), n.mtime_ms) DESC LIMIT ?3",
        )
        .map_err(map_sql)?;
    let rows = stmt
        .query_map(rusqlite::params![matching, cutoff, FETCH_ROWS], |row| {
            row.get::<_, String>(0)
        })
        .map_err(map_sql)?;
    let needle = name.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    for row in rows {
        let body = row.map_err(map_sql)?;
        let line = body
            .lines()
            .find(|l| l.to_lowercase().contains(&needle))
            .or_else(|| body.lines().next())
            .unwrap_or("");
        let snippet = clean_text(line.trim(), 200);
        if !snippet.is_empty() && !out.contains(&snippet) && out.len() < 3 {
            out.push(snippet);
        }
    }
    Ok(out)
}

/// Session Log entries: a bullet, a sub-heading, or a block starting at one (DESIGN 3.5).
fn log_entries(lines: &[&str], headings: &[Heading]) -> Vec<String> {
    let Some(at) = find_heading(headings, "session log") else {
        return Vec::new();
    };
    let mut entries: Vec<String> = Vec::new();
    for line in section_body(lines, headings, at).lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(h) = heading_text(t) {
            entries.push(h);
        } else if let Some(rest) = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .or_else(|| t.strip_prefix("+ "))
        {
            entries.push(rest.to_string());
        } else if let Some(last) = entries.last_mut() {
            last.push(' ');
            last.push_str(t);
        }
    }
    entries
        .iter()
        .map(|e| clean_text(e, 400))
        .filter(|e| !e.is_empty())
        .collect()
}

/// Keeps the newest five entries: by parsed date when any entry has one, else the last five in
/// file order. Result is oldest first (DESIGN 3.5).
fn pick_recent_entries(entries: Vec<String>) -> Vec<String> {
    let mut dated: Vec<(i64, String)> = entries
        .iter()
        .filter_map(|e| parse_date_prefix(e).map(|ts| (ts, e.clone())))
        .collect();
    let mut chosen: Vec<String> = if dated.is_empty() {
        entries
    } else {
        dated.sort_by_key(|(ts, _)| *ts);
        dated.into_iter().map(|(_, e)| e).collect()
    };
    let skip = chosen.len().saturating_sub(5);
    chosen.split_off(skip)
}

/// Text under headings named Next, TODO or Status, at most 600 characters (DESIGN 3.5).
fn next_section(lines: &[&str], headings: &[Heading]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for (at, h) in headings.iter().enumerate() {
        if matches!(h.text.to_lowercase().as_str(), "next" | "todo" | "status") {
            let body = section_body(lines, headings, at);
            if !body.trim().is_empty() {
                parts.push(body.trim().to_string());
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(clean_text(&parts.join("\n"), 600))
    }
}

/// Calendar date (UTC, `YYYY-MM-DD`) of a timestamp in ms, or empty when out of range.
fn date_of(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// Builds the `read` result: redacted, control-stripped, optionally one section, at most 6000
/// characters (DESIGN 3.5). An unknown heading is `NotFound`.
fn read_result(rel: &str, raw: &str, heading: Option<&str>) -> Result<ReadResult, RagError> {
    let clean = crate::managers::redact::redact(&normalise(raw));
    let section = match heading {
        Some(h) => extract_section(&clean, h).ok_or(RagError::NotFound)?,
        None => clean,
    };
    let truncated = section.chars().count() > READ_MAX;
    Ok(ReadResult {
        rel: rel.to_string(),
        text: section.chars().take(READ_MAX).collect(),
        truncated,
    })
}

/// Status result for an ambiguous name: no entries, a fixed sentence and the candidates (DESIGN 3.5).
fn ambiguous_status(name: &str, candidates: Vec<String>) -> StatusResult {
    let title = clean_text(name, 200);
    StatusResult {
        rel: candidates.first().cloned().unwrap_or_default(),
        title,
        modified_ms: 0,
        status: None,
        explicit_status: false,
        entries: Vec::new(),
        next_text: None,
        daily_snippets: Vec::new(),
        ambiguous: true,
        candidates,
        text: "Several project notes match this name; ask the user which one they mean."
            .to_string(),
    }
}

/// Builds the status result for one project note from its raw text (DESIGN 3.5).
fn status_from_text(named: Named, raw: &str, daily: Vec<String>) -> StatusResult {
    let clean = crate::managers::redact::redact(&normalise(raw));
    let (fm, body) = split_frontmatter(&clean);
    let status = fm_get(&fm, "status").map(crate::managers::redact::redact);
    let lines: Vec<&str> = body.lines().collect();
    let headings = headings_of(&lines);
    let entries = pick_recent_entries(log_entries(&lines, &headings));
    let next_text = next_section(&lines, &headings);
    let title = clean_text(&named.title, 200);
    let mut text = format!("Project: {title}\nModified: {}\n", date_of(named.mtime_ms));
    match &status {
        Some(s) => text.push_str(&format!("Status: {s}\n")),
        None => text.push_str(&format!("{NO_STATUS_SENTENCE}\n")),
    }
    if let Some(next) = &next_text {
        text.push_str(&format!("Next:\n{next}\n"));
    }
    for entry in &entries {
        text.push_str(&format!("- {entry}\n"));
    }
    for snippet in &daily {
        text.push_str(&format!("Daily: {snippet}\n"));
    }
    StatusResult {
        rel: named.rel,
        title,
        modified_ms: named.mtime_ms,
        explicit_status: status.is_some(),
        status,
        entries,
        next_text,
        daily_snippets: daily,
        ambiguous: false,
        candidates: Vec::new(),
        text: clean_text(&text, 5_000),
    }
}

/// The fixed sentence used when a project has no explicit status (DESIGN 3.5).
const NO_STATUS_SENTENCE: &str =
    "No explicit status is recorded; these are the most recent log entries.";

impl IndexManager {
    /// Creates an empty manager with no vault configured (DESIGN 3.3).
    pub fn new(data_dir: PathBuf, io: Arc<VaultIoWorker>) -> Self {
        IndexManager {
            data_dir,
            io,
            inner: Arc::new(Mutex::new(Inner::default())),
            refreshing: Arc::new(AtomicBool::new(false)),
            generation_atomic: AtomicU64::new(0),
        }
    }

    /// Sets the vault root and allowed folders, bumps the generation and returns the new
    /// generation (DESIGN 3.3).
    pub fn configure(&self, root: Option<PathBuf>, allowed_folders: Vec<String>) -> u64 {
        self.configure_locked(root, allowed_folders, &mut || {})
    }

    /// Shared body of `configure`. The generation is bumped first, before any filesystem or
    /// SQLite work, so lock-free readers see the new generation at once. `fs_hook` runs inside
    /// the filesystem step (a no-op outside tests).
    fn configure_locked(
        &self,
        root: Option<PathBuf>,
        allowed_folders: Vec<String>,
        fs_hook: &mut dyn FnMut(),
    ) -> u64 {
        let mut inner = lock(&self.inner);
        inner.generation = inner.generation.wrapping_add(1);
        self.generation_atomic
            .store(inner.generation, Ordering::SeqCst);
        inner.conn = None;
        inner.id = None;
        inner.last_error_kind = None;
        let mut folders = allowed_folders;
        folders.sort();
        folders.dedup();
        inner.folders = folders;
        inner.root = root.clone();
        if let Some(root) = root {
            fs_hook();
            match VaultIdentity::for_root(&root) {
                Ok(identity) => {
                    delete_other_caches(&self.data_dir, &cache_name(&identity.id));
                    inner.id = Some(identity.id);
                    open_locked(&mut inner, &self.data_dir);
                }
                Err(_) => inner.last_error_kind = Some("invalid".to_string()),
            }
        }
        inner.generation
    }

    /// The current generation (DESIGN 3.3). Reads a lock-free mirror, so it never waits for a
    /// `configure` that is still running.
    pub fn generation(&self) -> u64 {
        self.generation_atomic.load(Ordering::SeqCst)
    }

    /// Snapshot of the index counters and last refresh state (DESIGN 3.8).
    pub fn status(&self) -> IndexStatus {
        let inner = lock(&self.inner);
        let mut st = IndexStatus {
            notes: 0,
            chunks: 0,
            skipped: 0,
            sensitive_names: 0,
            last_refresh_ms: None,
            refreshing: self.refreshing.load(Ordering::SeqCst),
            last_error_kind: inner.last_error_kind.clone(),
        };
        if let Some(conn) = inner.conn.as_ref() {
            st.notes = count_rows(conn, "SELECT COUNT(*) FROM notes WHERE state = 'current'");
            st.chunks = count_rows(conn, "SELECT COUNT(*) FROM chunks");
            st.skipped = count_rows(conn, "SELECT COUNT(*) FROM notes WHERE state = 'skipped'");
            st.sensitive_names = meta_value(conn, "sensitive_names")
                .ok()
                .flatten()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            st.last_refresh_ms = meta_value(conn, "last_refresh_ms")
                .ok()
                .flatten()
                .and_then(|v| v.parse().ok());
        }
        st
    }

    /// One refresh pass against `source`, bounded by `budget` (DESIGN 3.4). This is the test
    /// seam: tests pass a fake `NoteSource`.
    pub fn refresh_with(&self, source: &dyn NoteSource, budget: Duration) -> RefreshReport {
        self.refresh_with_hook(source, budget, &mut || {})
    }

    /// Same as `refresh_with`, but calls `after_pending` right after the pending-rows
    /// transaction commits and before any note body is read (test seam, DESIGN 3.4).
    pub(crate) fn refresh_with_hook(
        &self,
        source: &dyn NoteSource,
        budget: Duration,
        after_pending: &mut dyn FnMut(),
    ) -> RefreshReport {
        if !begin_refresh(&self.refreshing) {
            return RefreshReport {
                added: 0,
                updated: 0,
                removed: 0,
                skipped: 0,
                timed_out: false,
                already_running: true,
                walk_complete: false,
            };
        }
        let (folders, gen) = {
            let g = lock(&self.inner);
            (g.folders.clone(), g.generation)
        };
        let report = run_refresh(&self.inner, source, budget, folders, gen, after_pending);
        self.refreshing.store(false, Ordering::SeqCst);
        report
    }

    /// Enqueues a refresh on the vault-IO worker with a `VaultSource` (DESIGN 3.3, 3.0 F1).
    ///
    /// The root, allowed folders and generation are captured now, at submit, and the job runs
    /// against them. If the index is reconfigured before the job runs, the job does nothing.
    /// This call blocks the calling thread for up to `budget` while the job runs on the worker,
    /// so it is not "fire and forget": do not call it from async or UI code without moving it
    /// onto a blocking task. If the worker queue is full the refresh is dropped without an error.
    pub fn request_refresh(&self, budget: Duration) {
        let (root, folders, gen) = {
            let g = lock(&self.inner);
            let Some(root) = g.root.clone() else {
                return;
            };
            (root, g.folders.clone(), g.generation)
        };
        let inner = Arc::clone(&self.inner);
        let refreshing = Arc::clone(&self.refreshing);
        let job = Box::new(move || {
            if !begin_refresh(&refreshing) {
                return;
            }
            let source = crate::managers::vault::VaultSource::new(root, folders.clone());
            let _ = run_refresh(&inner, &source, budget, folders, gen, &mut || {});
            refreshing.store(false, Ordering::SeqCst);
        });
        let _ = self.io.submit(job, budget);
    }

    /// Search at generation `gen` (DESIGN 3.5). Returns at most `opts.limit` hits (default 5,
    /// max 8), at most two per note, best first.
    pub fn search(&self, gen: u64, q: &str, opts: &SearchOpts) -> Result<Vec<Hit>, RagError> {
        self.search_matching(gen, fts_match(q, None), opts)
    }

    /// Search like `search`, but the terms are joined with OR, at most six of them (DESIGN 3.0
    /// F4). Ranking, limits, folder scope and generation checks are the same as `search`.
    pub fn search_any(
        &self,
        gen: u64,
        query: &str,
        opts: &SearchOpts,
    ) -> Result<Vec<Hit>, RagError> {
        self.search_matching(gen, fts_match_any(query), opts)
    }

    /// Runs a prepared FTS expression at generation `gen`. `None` means no usable terms.
    fn search_matching(
        &self,
        gen: u64,
        matching: Option<String>,
        opts: &SearchOpts,
    ) -> Result<Vec<Hit>, RagError> {
        let found = with_conn(&self.inner, gen, |conn, folders| {
            let Some(matching) = matching else {
                return Ok(Vec::new());
            };
            let (exact, prefix) = folder_scope(opts.folder.as_deref(), folders)?;
            let cutoff = since_cutoff(opts.since_days, now_ms());
            let limit = clamp_limit(opts.limit, 5, 8);
            let sql = format!(
                "SELECT chunks.rel, chunks.heading, chunks.body,
                        bm25(chunks, 0.0, 0.0, 3.0, 2.0, 1.0), n.note_ts, n.mtime_ms
                 FROM chunks JOIN notes n ON n.rel = chunks.rel
                 WHERE chunks MATCH ?1 AND n.state IN ('current', 'pending') AND {SCOPE_SQL}
                 ORDER BY 4 LIMIT ?5"
            );
            let mut stmt = conn.prepare(&sql).map_err(map_sql)?;
            let rows = stmt
                .query_map(
                    rusqlite::params![matching, exact, prefix, cutoff, FETCH_ROWS],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, f64>(3)?,
                            row.get::<_, Option<i64>>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    },
                )
                .map_err(map_sql)?;
            let mut cands = Vec::new();
            for row in rows {
                let (rel, heading, body, bm25, note_ts, mtime_ms) = row.map_err(map_sql)?;
                cands.push(rank_hit(
                    rel,
                    heading,
                    body,
                    bm25,
                    note_ts,
                    mtime_ms,
                    now_ms(),
                ));
            }
            Ok(keep_best(cands, limit))
        });
        Ok(found?.unwrap_or_default())
    }

    /// Recent notes at generation `gen` (DESIGN 3.5). Newest first; default 5, max 10.
    pub fn recent(&self, gen: u64, opts: &RecentOpts) -> Result<Vec<RecentNote>, RagError> {
        let found = with_conn(&self.inner, gen, |conn, folders| {
            let (exact, prefix) = folder_scope(opts.folder.as_deref(), folders)?;
            let cutoff = since_cutoff(opts.since_days, now_ms());
            let limit = clamp_limit(opts.limit, 5, 10);
            let sql = format!(
                "SELECT n.rel, n.title, n.note_ts, n.mtime_ms,
                        (SELECT c.body FROM chunks c WHERE c.rel = n.rel AND c.ord = 0)
                 FROM notes n
                 WHERE n.state = 'current' AND {SCOPE_SQL}
                 ORDER BY MAX(COALESCE(n.note_ts, 0), n.mtime_ms) DESC, n.rel ASC
                 LIMIT ?5"
            );
            let mut stmt = conn.prepare(&sql).map_err(map_sql)?;
            let rows = stmt
                .query_map(
                    rusqlite::params![exact, exact, prefix, cutoff, limit as i64],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<i64>>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, Option<String>>(4)?,
                        ))
                    },
                )
                .map_err(map_sql)?;
            let mut out = Vec::new();
            for row in rows {
                let (rel, title, note_ts, mtime_ms, body) = row.map_err(map_sql)?;
                out.push(RecentNote {
                    rel,
                    title: clean_text(&title, 200),
                    note_ts,
                    mtime_ms,
                    first_lines: clean_text(body.as_deref().unwrap_or(""), 200),
                });
            }
            Ok(out)
        });
        Ok(found?.unwrap_or_default())
    }

    /// Project note titles at generation `gen` (DESIGN 3.5). Sorted, de-duplicated, at least three
    /// characters each, at most 500. Only notes under `Projects/` count.
    pub fn project_names(&self, gen: u64) -> Result<Vec<String>, RagError> {
        let found = with_conn(&self.inner, gen, |conn, _| {
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT title FROM notes
                     WHERE state = 'current' AND folder = 'Projects'
                     ORDER BY title LIMIT 500",
                )
                .map_err(map_sql)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(map_sql)?;
            let mut names = Vec::new();
            for row in rows {
                let title = clean_text(&row.map_err(map_sql)?, 200);
                if title.chars().count() >= 3 && !names.contains(&title) {
                    names.push(title);
                }
            }
            Ok(names)
        });
        Ok(found?.unwrap_or_default())
    }

    /// Eligible `Projects/` notes at generation `gen` (DESIGN 3.0 F4), ordered by path. Only
    /// current notes under an allowed `Projects/` folder count; People/ and other folders never do.
    pub fn project_keys(&self, gen: u64) -> Result<Vec<ProjectKey>, RagError> {
        let found = with_conn(&self.inner, gen, |conn, folders| {
            let mut stmt = conn
                .prepare(
                    "SELECT rel, title FROM notes
                     WHERE state = 'current' AND folder = 'Projects'
                     ORDER BY rel LIMIT 500",
                )
                .map_err(map_sql)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(map_sql)?;
            let mut keys = Vec::new();
            for row in rows {
                let (rel, title) = row.map_err(map_sql)?;
                if !rel.starts_with("Projects/") || !allowed(&rel, folders) {
                    continue;
                }
                keys.push(ProjectKey {
                    title: clean_text(&title, 200),
                    stem: stem_of(&rel).to_string(),
                    rel,
                });
            }
            Ok(keys)
        });
        Ok(found?.unwrap_or_default())
    }

    /// Reads one note through the IO worker at generation `gen` (DESIGN 3.5). A wedged worker
    /// or a missing vault is `Unavailable`; `heading` selects one section, case-insensitively.
    pub fn read(&self, gen: u64, rel: &str, heading: Option<&str>) -> Result<ReadResult, RagError> {
        if self.io.wedged() {
            return Err(RagError::Unavailable);
        }
        let note = self.fetch_note(gen, rel)?;
        read_result(rel, &note.text, heading)
    }

    /// Project status for `name` at generation `gen` (DESIGN 3.5). Resolves the name in the index,
    /// then reads the note through the IO worker. Ambiguous names carry candidates and no entries.
    pub fn project_status(&self, gen: u64, name: &str) -> Result<StatusResult, RagError> {
        let found = with_conn(&self.inner, gen, |conn, _| {
            let Some(pick) = resolve_project(conn, name)? else {
                return Err(RagError::NotFound);
            };
            let snippets = match &pick {
                Pick::One(named) => daily_snippets(conn, &named.title, now_ms())?,
                Pick::Ambiguous(_) => Vec::new(),
            };
            Ok((pick, snippets))
        });
        let Some((pick, snippets)) = found? else {
            return Err(RagError::Unavailable);
        };
        match pick {
            Pick::Ambiguous(cands) => Ok(ambiguous_status(name, cands)),
            Pick::One(named) => {
                let note = self.fetch_note(gen, &named.rel)?;
                Ok(status_from_text(named, &note.text, snippets))
            }
        }
    }

    /// Reads one allowed note through the IO worker at generation `gen` (DESIGN 3.5). Validates
    /// the generation and the folder first, then reads with the 4 s deadline.
    fn fetch_note(
        &self,
        gen: u64,
        rel: &str,
    ) -> Result<crate::managers::vault::NoteRead, RagError> {
        let (root, folders) = {
            let guard = lock(&self.inner);
            // A never-configured index is Unavailable (DESIGN 3.6: never built), checked first.
            let Some(root) = guard.root.clone() else {
                return Err(RagError::Unavailable);
            };
            if gen != guard.generation {
                return Err(RagError::GenerationMismatch);
            }
            (root, guard.folders.clone())
        };
        // The vault layer enforces the allowed folders and the path rules (Forbidden / Invalid).
        let owned = rel.to_string();
        let job = Box::new(move || {
            let source = crate::managers::vault::VaultSource::new(root, folders);
            crate::managers::vault::NoteSource::read(&source, &owned)
        });
        match self.io.submit(job, READ_DEADLINE) {
            Ok(Ok(note)) => {
                // DESIGN 3.0 F2: a reconfigure while the read was in flight makes the text stale.
                if gen != self.generation() {
                    return Err(RagError::GenerationMismatch);
                }
                Ok(note)
            }
            Ok(Err(e)) => Err(map_vault(e)),
            Err(_) => Err(RagError::Unavailable),
        }
    }

    /// SQLite file so tests can assert what was persisted.
    pub(crate) fn debug_rows(&self) -> DebugDump {
        let inner = lock(&self.inner);
        match inner.conn.as_ref() {
            Some(conn) => dump(conn),
            None => DebugDump::default(),
        }
    }
}
