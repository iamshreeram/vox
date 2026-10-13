//! Contract tests for `conversation_store` (design section 4, T8).
//!
//! Red step: the store is stubbed with `todo!()`, so these tests compile and
//! fail only through `todo!()` panics until the implementation lands.
//! All fixtures are synthetic.

use crate::managers::conversation_record::{
    DeliveryLabel, EndLabel, SessionRecord, TurnOutcomeLabel, TurnRecord,
};
use crate::managers::conversation_store::{ConversationStore, MAX_LIST, RETENTION};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

const STARTED_MS: i64 = 1_700_000_000_000;
const ENDED_MS: i64 = 1_700_000_060_000;

fn turn(i: usize) -> TurnRecord {
    TurnRecord {
        user: format!("synthetic question {i}"),
        assistant: Some(format!("synthetic answer {i}")),
        outcome: TurnOutcomeLabel::Answered,
        delivery: DeliveryLabel::Submitted,
        detail: None,
    }
}

fn session(turns: Vec<TurnRecord>, summary: Option<&str>) -> SessionRecord {
    SessionRecord {
        started_at_ms: STARTED_MS,
        ended_at_ms: ENDED_MS,
        end: EndLabel::UserEnded,
        turns,
        summary: summary.map(str::to_owned),
    }
}

fn db_path(dir: &Path) -> PathBuf {
    dir.join("conversations.db")
}

/// Opens an independent connection, bypassing the store API.
fn raw(path: &Path) -> Connection {
    Connection::open(path).expect("raw connection")
}

fn sha256_of(path: &Path) -> Vec<u8> {
    Sha256::digest(std::fs::read(path).expect("read file")).to_vec()
}

#[test]
fn fresh_db_has_exact_schema_and_user_version_1() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(dir.path());
    let _store = ConversationStore::open(&path).expect("open fresh db");

    let conn = raw(&path);
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1, "user_version must be 1 after migration");

    // (name, declared type, notnull, pk)
    let mut stmt = conn
        .prepare("PRAGMA table_info(conversation_records)")
        .unwrap();
    let cols: Vec<(String, String, i64, i64)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(5)?,
            ))
        })
        .unwrap()
        .map(|x| x.unwrap())
        .collect();
    let expected: Vec<(String, String, i64, i64)> = vec![
        ("id", "INTEGER", 0, 1),
        ("started_at", "INTEGER", 1, 0),
        ("ended_at", "INTEGER", 1, 0),
        ("end_reason", "TEXT", 1, 0),
        ("summary", "TEXT", 1, 0),
        ("transcript_json", "TEXT", 1, 0),
        ("vault_note", "TEXT", 0, 0),
    ]
    .into_iter()
    .map(|(a, b, c, d)| (a.to_string(), b.to_string(), c, d))
    .collect();
    assert_eq!(cols, expected, "exact column set of conversation_records");
}

#[test]
fn insert_returns_increasing_ids_and_list_roundtrips_every_field() {
    let dir = tempfile::tempdir().unwrap();
    let store = ConversationStore::open(&db_path(dir.path())).unwrap();

    let id1 = store
        .insert(
            &session(vec![turn(1), turn(2)], Some("two turns")),
            Some("notes/a.md"),
        )
        .unwrap();
    let id2 = store.insert(&session(vec![turn(3)], None), None).unwrap();
    assert!(id2 > id1, "ids must increase: {id1} then {id2}");

    let rows = store.list(10).unwrap();
    assert_eq!(rows.len(), 2);
    // newest first
    let newest = &rows[0];
    let oldest = &rows[1];
    assert_eq!(newest.id, id2);
    assert_eq!(oldest.id, id1);

    assert_eq!(oldest.started_at, STARTED_MS);
    assert_eq!(oldest.ended_at, ENDED_MS);
    assert_eq!(oldest.end_reason, EndLabel::UserEnded.as_word());
    assert_eq!(oldest.summary, "two turns");
    assert_eq!(oldest.turns, 2);
    assert_eq!(oldest.vault_note.as_deref(), Some("notes/a.md"));

    // summary None stored as empty string; vault_note None preserved as None
    assert_eq!(newest.started_at, STARTED_MS);
    assert_eq!(newest.ended_at, ENDED_MS);
    assert_eq!(newest.end_reason, EndLabel::UserEnded.as_word());
    assert_eq!(newest.summary, "");
    assert_eq!(newest.turns, 1);
    assert_eq!(newest.vault_note, None);
}

#[test]
fn transcript_json_deserializes_to_input_turns() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(dir.path());
    let store = ConversationStore::open(&path).unwrap();

    let turns = vec![
        turn(1),
        TurnRecord {
            user: "synthetic silent turn".into(),
            assistant: None,
            outcome: TurnOutcomeLabel::NoReply,
            delivery: DeliveryLabel::NotSpoken,
            detail: Some("synthetic detail".into()),
        },
        TurnRecord {
            user: "synthetic interrupted".into(),
            assistant: Some("partial answer".into()),
            outcome: TurnOutcomeLabel::Interrupted,
            delivery: DeliveryLabel::Interrupted,
            detail: None,
        },
    ];
    let id = store
        .insert(&session(turns.clone(), Some("mixed")), None)
        .unwrap();

    let json: String = raw(&path)
        .query_row(
            "SELECT transcript_json FROM conversation_records WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    let decoded: Vec<TurnRecord> = serde_json::from_str(&json).expect("valid transcript json");
    assert_eq!(decoded, turns);
}

#[test]
fn list_limit_is_clamped_to_one_and_max_list() {
    let dir = tempfile::tempdir().unwrap();
    let store = ConversationStore::open(&db_path(dir.path())).unwrap();

    for _ in 0..250 {
        store.insert(&session(vec![turn(0)], None), None).unwrap();
    }
    assert_eq!(store.list(0).unwrap().len(), 1, "limit 0 clamps to 1");
    assert_eq!(
        store.list(1000).unwrap().len(),
        MAX_LIST,
        "limit 1000 clamps to MAX_LIST"
    );
    assert_eq!(MAX_LIST, 200);
    assert_eq!(store.list(5).unwrap().len(), 5);
}

#[test]
fn retention_keeps_exactly_newest_rows_and_deletes_oldest() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(dir.path());
    let store = ConversationStore::open(&path).unwrap();
    assert_eq!(RETENTION, 500);

    let total = RETENTION + 5;
    let mut ids = Vec::with_capacity(total);
    for _ in 0..total {
        ids.push(store.insert(&session(vec![turn(0)], None), None).unwrap());
    }

    let conn = raw(&path);
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM conversation_records", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count as usize, RETENTION);

    let min_id: i64 = conn
        .query_row("SELECT MIN(id) FROM conversation_records", [], |r| r.get(0))
        .unwrap();
    let max_id: i64 = conn
        .query_row("SELECT MAX(id) FROM conversation_records", [], |r| r.get(0))
        .unwrap();
    assert_eq!(min_id, ids[5], "the five oldest rows must be deleted");
    assert_eq!(max_id, *ids.last().unwrap(), "the newest row must be kept");
}

#[test]
fn reopen_keeps_rows_and_does_not_rerun_migrations() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(dir.path());
    {
        let store = ConversationStore::open(&path).unwrap();
        for i in 0..3 {
            store
                .insert(&session(vec![turn(i)], Some("kept")), None)
                .unwrap();
        }
    }

    let reopened = ConversationStore::open(&path).expect("reopen existing db");
    let rows = reopened.list(10).unwrap();
    assert_eq!(rows.len(), 3, "rows must survive reopen");
    assert!(rows.iter().all(|r| r.summary == "kept"));

    let conn = raw(&path);
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1);
    let tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'conversation_records'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 1, "table must exist exactly once");
}

#[test]
fn corrupt_file_at_path_returns_err_without_panic() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(dir.path());
    std::fs::write(
        &path,
        b"this is not a sqlite database, synthetic garbage bytes",
    )
    .unwrap();

    let result = ConversationStore::open(&path);
    assert!(result.is_err(), "garbage file must yield Err, not Ok");
}

#[test]
fn missing_parent_directory_is_created_and_db_opens() {
    // Decision (documented in the stub): open creates missing parent directories,
    // matching memory.rs. The directory must then hold the database file.
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("a").join("b");
    let path = db_path(&nested);
    assert!(!nested.exists());

    let store = ConversationStore::open(&path).expect("open with missing parents");
    assert!(path.is_file(), "conversations.db must exist after open");
    assert_eq!(store.list(5).unwrap().len(), 0);
}

#[test]
fn two_stores_on_same_path_insert_concurrently_without_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(dir.path());
    // Create the schema once up front so both threads race only on inserts.
    drop(ConversationStore::open(&path).unwrap());

    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for t in 0..2 {
        let path = path.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            let store = ConversationStore::open(&path).expect("open in thread");
            barrier.wait();
            for i in 0..20 {
                store
                    .insert(&session(vec![turn(t * 100 + i)], None), None)
                    .expect("concurrent insert must not error");
            }
        }));
    }
    for h in handles {
        h.join().expect("thread must not panic");
    }

    let count: i64 = raw(&path)
        .query_row("SELECT COUNT(*) FROM conversation_records", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 40);
}

#[test]
fn history_db_is_never_opened_or_modified() {
    let dir = tempfile::tempdir().unwrap();
    let history = dir.path().join("history.db");
    let known_bytes: &[u8] = b"synthetic history file, not sqlite, must stay byte-identical";
    std::fs::write(&history, known_bytes).unwrap();
    let before = sha256_of(&history);

    let conv = db_path(dir.path());
    let store = ConversationStore::open(&conv).unwrap();
    store
        .insert(&session(vec![turn(1)], Some("x")), Some("n.md"))
        .unwrap();
    store.list(10).unwrap();
    drop(store);

    assert_eq!(
        sha256_of(&history),
        before,
        "history.db bytes must not change"
    );
    assert_eq!(std::fs::read(&history).unwrap(), known_bytes);
    assert_ne!(conv, history);
    let conv_bytes = std::fs::read(&conv).unwrap();
    assert!(
        conv_bytes.starts_with(b"SQLite format 3\0"),
        "conversations.db must be its own SQLite file"
    );
    assert_ne!(conv_bytes, known_bytes);
}

#[test]
fn max_size_record_with_200_turns_of_1200_chars_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(dir.path());
    let store = ConversationStore::open(&path).unwrap();

    let turns: Vec<TurnRecord> = (0..200)
        .map(|i| TurnRecord {
            user: format!("{i}{}", "u".repeat(1200 - i.to_string().len())),
            assistant: Some("a".repeat(1200)),
            outcome: TurnOutcomeLabel::Answered,
            delivery: DeliveryLabel::Submitted,
            detail: None,
        })
        .collect();
    assert!(turns.iter().all(|t| t.user.chars().count() == 1200));

    let id = store
        .insert(&session(turns.clone(), Some("big")), Some("big.md"))
        .unwrap();

    let rows = store.list(1).unwrap();
    assert_eq!(rows[0].id, id);
    assert_eq!(rows[0].turns, 200);

    let json: String = raw(&path)
        .query_row(
            "SELECT transcript_json FROM conversation_records WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    let decoded: Vec<TurnRecord> = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, turns);
}
