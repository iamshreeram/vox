//! SQLite store for finished conversations.
//!
//! The store lives in its own `conversations.db` file. It never touches
//! `history.db`. Each [`ConversationStore`] owns one connection guarded by a
//! mutex, so its methods take `&self` and can be called from several threads.

use super::conversation_record::SessionRecord;
use rusqlite::{params, Connection, TransactionBehavior};
use rusqlite_migration::{Migrations, M};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

/// Maximum number of rows `list` returns. Larger limits are clamped to this.
pub const MAX_LIST: usize = 200;

/// Number of newest rows kept after each insert. Older rows are deleted.
pub const RETENTION: usize = 500;

/// How long a connection waits for a competing writer before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Schema migrations. `PRAGMA user_version` tracks how many have run.
static MIGRATIONS: &[M] = &[M::up(
    "CREATE TABLE conversation_records (
        id INTEGER PRIMARY KEY,
        started_at INTEGER NOT NULL,
        ended_at INTEGER NOT NULL,
        end_reason TEXT NOT NULL,
        summary TEXT NOT NULL,
        transcript_json TEXT NOT NULL,
        vault_note TEXT
    );",
)];

/// Errors from the conversation store. Messages carry no transcript text.
#[derive(Debug)]
pub enum StoreError {
    /// SQLite open, migration, or query failure.
    Db(String),
    /// Transcript serialization failure.
    Json(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Db(msg) => write!(f, "conversation store database error: {msg}"),
            StoreError::Json(msg) => write!(f, "conversation store serialization error: {msg}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// One listed conversation. The transcript is intentionally not included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationRow {
    /// Row id (`INTEGER PRIMARY KEY`).
    pub id: i64,
    /// Session start, milliseconds since the Unix epoch (from `started_at_ms`).
    pub started_at: i64,
    /// Session end, milliseconds since the Unix epoch (from `ended_at_ms`).
    pub ended_at: i64,
    /// `EndLabel::as_word()` of the session end.
    pub end_reason: String,
    /// Session summary. `None` in the record is stored and returned as `""`.
    pub summary: String,
    /// Number of turns in the session (`turns.len()`).
    pub turns: usize,
    /// Relative vault note path, if one was written.
    pub vault_note: Option<String>,
}

/// Handle to the `conversations.db` file.
///
/// Contract (defined by tests):
/// - Table `conversation_records` with exactly the columns
///   `id, started_at, ended_at, end_reason, summary, transcript_json, vault_note`.
/// - Schema is managed with `rusqlite_migration`; `PRAGMA user_version` is 1.
/// - `history.db` is never opened or modified.
pub struct ConversationStore {
    conn: Mutex<Connection>,
}

impl ConversationStore {
    /// Opens the store at `conversations.db` in the app data directory.
    ///
    /// Separate from `history.db`, so rolling back the app never touches history.
    pub fn new(app: &tauri::AppHandle) -> Result<ConversationStore, StoreError> {
        let dir = crate::portable::app_data_dir(app)
            .map_err(|e| StoreError::Db(format!("app data dir: {e}")))?;
        ConversationStore::open(&dir.join("conversations.db"))
    }

    /// Opens or creates the store at `path`, creating missing parent
    /// directories first (same as `memory.rs`). Migrations run on open, so a
    /// corrupt or non-database file returns `Err` (never a panic). Reopening an
    /// existing database does not re-run migrations.
    ///
    /// The database uses WAL journaling and a busy timeout, so several stores
    /// on the same path can write concurrently.
    pub fn open(path: &Path) -> Result<ConversationStore, StoreError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| StoreError::Db(format!("create directory: {e}")))?;
            }
        }
        let mut conn = Connection::open(path).map_err(|e| StoreError::Db(format!("open: {e}")))?;
        conn.busy_timeout(BUSY_TIMEOUT)
            .map_err(|e| StoreError::Db(format!("busy timeout: {e}")))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| StoreError::Db(format!("journal mode: {e}")))?;
        Migrations::new(MIGRATIONS.to_vec())
            .to_latest(&mut conn)
            .map_err(|e| StoreError::Db(format!("migrate: {e}")))?;
        Ok(ConversationStore {
            conn: Mutex::new(conn),
        })
    }

    /// Inserts one finished session and returns its new row id.
    ///
    /// `summary: None` is stored as `""`. The transcript is stored as JSON
    /// (`Vec<TurnRecord>`). After the insert, only the newest [`RETENTION`]
    /// rows are kept. Failures return `Err`.
    pub fn insert(&self, rec: &SessionRecord, vault_note: Option<&str>) -> Result<i64, StoreError> {
        let transcript = serde_json::to_string(&rec.turns)
            .map_err(|e| StoreError::Json(format!("transcript: {e}")))?;
        let summary = rec.summary.as_deref().unwrap_or("");
        let end_reason = rec.end.as_word();

        let mut conn = self.lock()?;
        // IMMEDIATE takes the write lock up front, so a competing writer waits
        // on the busy timeout instead of failing a read-to-write upgrade.
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| StoreError::Db(format!("begin insert: {e}")))?;
        tx.execute(
            "INSERT INTO conversation_records (
                started_at, ended_at, end_reason, summary, transcript_json, vault_note
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                rec.started_at_ms,
                rec.ended_at_ms,
                end_reason,
                summary,
                transcript,
                vault_note,
            ],
        )
        .map_err(|e| StoreError::Db(format!("insert: {e}")))?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "DELETE FROM conversation_records
             WHERE id NOT IN (
                SELECT id FROM conversation_records ORDER BY id DESC LIMIT ?1
             )",
            [retention_param()],
        )
        .map_err(|e| StoreError::Db(format!("retention: {e}")))?;
        tx.commit()
            .map_err(|e| StoreError::Db(format!("commit insert: {e}")))?;
        Ok(id)
    }

    /// Lists conversations newest first (by id, descending), without transcripts.
    /// `limit` is clamped to `1..=MAX_LIST`, so `0` yields one row and any value
    /// above `MAX_LIST` yields at most `MAX_LIST` rows.
    pub fn list(&self, limit: usize) -> Result<Vec<ConversationRow>, StoreError> {
        let limit = limit.clamp(1, MAX_LIST);
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, started_at, ended_at, end_reason, summary,
                        json_array_length(transcript_json), vault_note
                 FROM conversation_records
                 ORDER BY id DESC
                 LIMIT ?1",
            )
            .map_err(|e| StoreError::Db(format!("prepare list: {e}")))?;
        let raw_rows = stmt
            .query_map([limit as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })
            .map_err(|e| StoreError::Db(format!("query list: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Db(format!("read list: {e}")))?;

        raw_rows
            .into_iter()
            .map(
                |(id, started_at, ended_at, end_reason, summary, turns, vault_note)| {
                    // NULL means the stored transcript is not a JSON array.
                    let turns = turns.ok_or_else(|| {
                        StoreError::Json("stored transcript is not an array".into())
                    })?;
                    let turns = usize::try_from(turns)
                        .map_err(|_| StoreError::Json("turn count out of range".into()))?;
                    Ok(ConversationRow {
                        id,
                        started_at,
                        ended_at,
                        end_reason,
                        summary,
                        turns,
                        vault_note,
                    })
                },
            )
            .collect()
    }

    /// Locks the connection. A poisoned lock becomes an error, not a panic.
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StoreError> {
        self.conn
            .lock()
            .map_err(|_| StoreError::Db("connection lock poisoned".into()))
    }
}

/// [`RETENTION`] as the `i64` SQLite binds expect.
fn retention_param() -> i64 {
    i64::try_from(RETENTION).unwrap_or(i64::MAX)
}
