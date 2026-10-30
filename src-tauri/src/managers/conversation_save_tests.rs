//! Contract tests for `conversation_save` (M1 B3a, red-first).
//!
//! Every test here compiles against the stubs and FAILS until B3b implements them (a `todo!()`
//! panic or an assertion). All fixtures are synthetic; no real vault or home directory is read.

use crate::managers::conversation_record::{
    clean_summary, fallback_summary, note_rel_path, render_markdown, session_id8, DeliveryLabel,
    EndLabel, SessionRecord, TurnOutcomeLabel, TurnRecord, MAX_TURNS,
};
use crate::managers::conversation_save::{
    answer_only_for, load_vault_block, save_session, save_session_with_deadline, summary_options,
    summary_route, vault_context_active, CollectedSession, NoteSink, RecordStoreSink, SaveDeps,
    SaveSettings, SkipReason, Summarizer, SummaryRoute, TranscriptCollector,
};
use crate::managers::converse::{
    Delivery, SessionEnd, TranscriptEvent, TranscriptSink, TurnOutcome,
};
use crate::managers::vault::VaultError;
use crate::settings::{get_default_settings, AppSettings};
use futures_util::future::BoxFuture;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Synthetic GitHub-style token (not a real credential).
const TOKEN: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
const START_MS: i64 = 1_700_000_000_000;
const NOW_MS: i64 = 1_700_000_060_000;
const VAULT: &str = "/tmp/vox-synthetic-vault";

// ---------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum SumMode {
    Reply(&'static str),
    Fail,
    Hang,
    Sleep(Duration, &'static str),
}

struct FakeSummarizer {
    mode: SumMode,
    calls: AtomicUsize,
}

impl FakeSummarizer {
    fn new(mode: SumMode) -> Self {
        FakeSummarizer {
            mode,
            calls: AtomicUsize::new(0),
        }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Summarizer for FakeSummarizer {
    fn summarize<'a>(
        &'a self,
        _system: String,
        _user: String,
    ) -> BoxFuture<'a, Result<String, String>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.mode {
            SumMode::Reply(text) => Box::pin(async move { Ok(text.to_string()) }),
            SumMode::Fail => Box::pin(async move { Err("backend refused".to_string()) }),
            SumMode::Hang => Box::pin(std::future::pending()),
            SumMode::Sleep(d, text) => Box::pin(async move {
                tokio::time::sleep(d).await;
                Ok(text.to_string())
            }),
        }
    }
}

struct FakeStore {
    fail: bool,
    attempts: AtomicUsize,
    rows: Mutex<Vec<(SessionRecord, Option<String>)>>,
}

impl FakeStore {
    fn new(fail: bool) -> Self {
        FakeStore {
            fail,
            attempts: AtomicUsize::new(0),
            rows: Mutex::new(Vec::new()),
        }
    }
    fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }
    fn rows(&self) -> Vec<(SessionRecord, Option<String>)> {
        self.rows.lock().map(|r| r.clone()).unwrap_or_default()
    }
}

impl RecordStoreSink for FakeStore {
    fn insert(&self, rec: &SessionRecord, vault_note: Option<&str>) -> Result<i64, String> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err("store unavailable".to_string());
        }
        let mut rows = self.rows.lock().map_err(|_| "poisoned".to_string())?;
        rows.push((rec.clone(), vault_note.map(str::to_string)));
        Ok(rows.len() as i64)
    }
}

/// One scripted `write_note` result.
#[derive(Clone, Copy)]
enum Step {
    Ok,
    Exists,
    Unsafe,
    TooLarge,
    Invalid,
    Io,
}

fn step_result(step: Step, vault_path: &str, rel: &str) -> Result<PathBuf, VaultError> {
    match step {
        Step::Ok => Ok(PathBuf::from(format!("{vault_path}/{rel}"))),
        Step::Exists => Err(VaultError::Exists),
        Step::Unsafe => Err(VaultError::Unsafe),
        Step::TooLarge => Err(VaultError::TooLarge),
        Step::Invalid => Err(VaultError::Invalid),
        Step::Io => Err(VaultError::Io(std::io::ErrorKind::Other)),
    }
}

struct FakeNotes {
    script: Mutex<VecDeque<Step>>,
    default: Step,
    /// (vault_path, rel, contents) per call.
    calls: Mutex<Vec<(String, String, String)>>,
}

impl FakeNotes {
    fn new(default: Step) -> Self {
        FakeNotes {
            script: Mutex::new(VecDeque::new()),
            default,
            calls: Mutex::new(Vec::new()),
        }
    }
    fn scripted(steps: Vec<Step>, default: Step) -> Self {
        FakeNotes {
            script: Mutex::new(steps.into_iter().collect()),
            default,
            calls: Mutex::new(Vec::new()),
        }
    }
    fn calls(&self) -> Vec<(String, String, String)> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }
}

impl NoteSink for FakeNotes {
    fn write_note(
        &self,
        vault_path: &str,
        rel: &str,
        contents: &str,
    ) -> Result<PathBuf, VaultError> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push((
                vault_path.to_string(),
                rel.to_string(),
                contents.to_string(),
            ));
        }
        let step = self
            .script
            .lock()
            .ok()
            .and_then(|mut s| s.pop_front())
            .unwrap_or(self.default);
        step_result(step, vault_path, rel)
    }
}

fn turn(i: usize) -> TurnRecord {
    TurnRecord {
        user: format!("question number {i}"),
        assistant: Some(format!("answer number {i}")),
        outcome: TurnOutcomeLabel::Answered,
        delivery: DeliveryLabel::Submitted,
        detail: None,
    }
}

fn session(turns: usize) -> CollectedSession {
    CollectedSession {
        untrusted: false,
        turns: (0..turns).map(turn).collect(),
        dropped: 0,
        started_at_ms: START_MS,
    }
}

fn enabled(to_vault: bool, vault_path: &str) -> SaveSettings {
    SaveSettings {
        records_enabled: true,
        to_vault,
        vault_path: vault_path.to_string(),
    }
}

fn fixed_now() -> i64 {
    NOW_MS
}

// ---------------------------------------------------------------------------
// a) TranscriptCollector
// ---------------------------------------------------------------------------

fn event(user: &str, assistant: Option<&str>, outcome: TurnOutcome) -> TranscriptEvent {
    TranscriptEvent {
        user: user.to_string(),
        assistant: assistant.map(str::to_string),
        outcome,
        delivery: Delivery::Submitted,
        detail: None,
    }
}

#[test]
fn collector_maps_labels_and_passes_detail_through() {
    let c = TranscriptCollector::new();
    c.record(TranscriptEvent {
        user: "open the notes app".to_string(),
        assistant: Some("Opening it.".to_string()),
        outcome: TurnOutcome::Acted,
        delivery: Delivery::Interrupted,
        detail: Some("action: open_app".to_string()),
    });
    c.record(event("are you there", None, TurnOutcome::NoReply));
    let got = c.take();
    assert_eq!(got.turns.len(), 2);
    assert_eq!(got.turns[0].outcome, TurnOutcomeLabel::Acted);
    assert_eq!(got.turns[0].delivery, DeliveryLabel::Interrupted);
    assert_eq!(got.turns[0].assistant.as_deref(), Some("Opening it."));
    assert_eq!(got.turns[0].detail.as_deref(), Some("action: open_app"));
    assert_eq!(got.turns[1].outcome, TurnOutcomeLabel::NoReply);
    assert_eq!(got.turns[1].delivery, DeliveryLabel::Submitted);
    assert_eq!(got.turns[1].assistant, None);
    assert_eq!(got.dropped, 0);
}

#[test]
fn collector_sanitizes_user_text_and_redacts_secrets() {
    let c = TranscriptCollector::new();
    c.record(event(
        &format!("bell\u{7} my token is {TOKEN}"),
        Some("noted"),
        TurnOutcome::Answered,
    ));
    let got = c.take();
    let user = &got.turns[0].user;
    assert!(!user.contains('\u{7}'), "control chars must be stripped");
    assert!(!user.contains(TOKEN), "secrets must be redacted");
    assert!(user.contains("bell"));
}

#[test]
fn collector_is_bounded_at_max_turns_and_counts_dropped() {
    let c = TranscriptCollector::new();
    for i in 0..(MAX_TURNS + 5) {
        c.record(event(&format!("turn {i}"), None, TurnOutcome::NoReply));
    }
    let got = c.take();
    assert_eq!(got.turns.len(), MAX_TURNS);
    assert_eq!(got.dropped, 5);
    assert_eq!(
        got.turns[0].user, "turn 0",
        "the first turns are kept in order"
    );
}

#[test]
fn collector_take_empties_the_collector() {
    let c = TranscriptCollector::new();
    c.record(event("one", None, TurnOutcome::NoReply));
    assert_eq!(c.take().turns.len(), 1);
    let second = c.take();
    assert!(second.turns.is_empty());
    assert_eq!(second.dropped, 0);
}

#[test]
fn collector_instances_are_independent() {
    let a = TranscriptCollector::new();
    let b = TranscriptCollector::new();
    a.record(event("from a", None, TurnOutcome::NoReply));
    b.record(event("from b", None, TurnOutcome::NoReply));
    b.record(event("from b again", None, TurnOutcome::NoReply));
    let got_a = a.take();
    let got_b = b.take();
    assert_eq!(got_a.turns.len(), 1);
    assert_eq!(got_a.turns[0].user, "from a");
    assert_eq!(got_b.turns.len(), 2);
    assert!(got_b.turns.iter().all(|t| t.user.starts_with("from b")));
}

#[test]
fn collector_stamps_started_at_from_the_injected_clock() {
    let c = TranscriptCollector::with_clock(Box::new(|| START_MS));
    assert_eq!(c.take().started_at_ms, START_MS);
}

// ---------------------------------------------------------------------------
// b) save_session (T7)
// ---------------------------------------------------------------------------

fn deps<'a>(
    sum: &'a FakeSummarizer,
    store: &'a FakeStore,
    notes: &'a FakeNotes,
    now: &'a (dyn Fn() -> i64 + Send + Sync),
) -> SaveDeps<'a> {
    SaveDeps {
        summarizer: sum,
        store,
        notes,
        now_ms: now,
        utc_offset_secs: 0,
    }
}

#[tokio::test]
async fn save_disabled_makes_no_calls_even_with_turns() {
    let sum = FakeSummarizer::new(SumMode::Reply("should not run"));
    let store = FakeStore::new(false);
    let notes = FakeNotes::new(Step::Ok);
    let now = fixed_now;
    let settings = SaveSettings {
        records_enabled: false,
        to_vault: true,
        vault_path: VAULT.to_string(),
    };
    let report = save_session(
        &settings,
        &deps(&sum, &store, &notes, &now),
        session(3),
        SessionEnd::UserEnded,
    )
    .await;
    assert_eq!(report.skipped, Some(SkipReason::Disabled));
    assert!(!report.db_saved && !report.vault_saved);
    assert_eq!(sum.calls(), 0);
    assert_eq!(store.attempts(), 0);
    assert!(notes.calls().is_empty());
}

#[tokio::test]
async fn save_zero_turns_is_skipped_without_calls() {
    let sum = FakeSummarizer::new(SumMode::Reply("unused"));
    let store = FakeStore::new(false);
    let notes = FakeNotes::new(Step::Ok);
    let now = fixed_now;
    let report = save_session(
        &enabled(true, VAULT),
        &deps(&sum, &store, &notes, &now),
        session(0),
        SessionEnd::Idle,
    )
    .await;
    assert_eq!(report.skipped, Some(SkipReason::NoTurns));
    assert_eq!(store.attempts(), 0);
    assert!(notes.calls().is_empty());
    assert_eq!(sum.calls(), 0);
}

#[tokio::test]
async fn save_not_configured_and_audio_unavailable_are_not_recorded() {
    for end in [SessionEnd::NotConfigured, SessionEnd::AudioUnavailable] {
        let sum = FakeSummarizer::new(SumMode::Reply("unused"));
        let store = FakeStore::new(false);
        let notes = FakeNotes::new(Step::Ok);
        let now = fixed_now;
        let report = save_session(
            &enabled(true, VAULT),
            &deps(&sum, &store, &notes, &now),
            session(2),
            end,
        )
        .await;
        assert_eq!(report.skipped, Some(SkipReason::NotRecordable), "{end:?}");
        assert_eq!(store.attempts(), 0, "{end:?}");
        assert!(notes.calls().is_empty(), "{end:?}");
        assert_eq!(sum.calls(), 0, "{end:?}");
    }
}

#[tokio::test]
async fn save_enabled_writes_one_row_and_one_note_with_rendered_contents() {
    let sum = FakeSummarizer::new(SumMode::Reply("The user asked about questions."));
    let store = FakeStore::new(false);
    let notes = FakeNotes::new(Step::Ok);
    let now = fixed_now;
    let report = save_session(
        &enabled(true, VAULT),
        &deps(&sum, &store, &notes, &now),
        session(2),
        SessionEnd::UserEnded,
    )
    .await;
    assert!(report.db_saved);
    assert!(report.vault_saved);
    assert_eq!(report.skipped, None);
    assert_eq!(store.attempts(), 1);
    assert_eq!(notes.calls().len(), 1);

    let rows = store.rows();
    let (stored, vault_note) = &rows[0];
    assert_eq!(stored.started_at_ms, START_MS);
    assert_eq!(stored.ended_at_ms, NOW_MS);
    assert_eq!(stored.end, EndLabel::UserEnded);
    assert_eq!(stored.turns.len(), 2);
    assert!(vault_note.is_some(), "the row records the note path");

    let (vault, rel, contents) = &notes.calls()[0];
    assert_eq!(vault, VAULT);
    let sid8 = session_id8(stored.started_at_ms, &stored.turns);
    assert_eq!(rel, &note_rel_path(stored, &sid8, 0));
    assert_eq!(contents, &render_markdown(stored, &sid8));
}

#[tokio::test]
async fn save_without_to_vault_writes_the_row_only() {
    let sum = FakeSummarizer::new(SumMode::Reply("A summary."));
    let store = FakeStore::new(false);
    let notes = FakeNotes::new(Step::Ok);
    let now = fixed_now;
    let report = save_session(
        &enabled(false, VAULT),
        &deps(&sum, &store, &notes, &now),
        session(1),
        SessionEnd::Idle,
    )
    .await;
    assert!(report.db_saved);
    assert!(!report.vault_saved);
    assert_eq!(store.attempts(), 1);
    assert!(notes.calls().is_empty());
}

#[tokio::test]
async fn save_model_summary_is_stored_after_cleaning() {
    let sum = FakeSummarizer::new(SumMode::Reply("A short summary of the talk."));
    let store = FakeStore::new(false);
    let notes = FakeNotes::new(Step::Ok);
    let now = fixed_now;
    let report = save_session(
        &enabled(false, VAULT),
        &deps(&sum, &store, &notes, &now),
        session(2),
        SessionEnd::UserEnded,
    )
    .await;
    assert!(report.summary_from_model);
    assert_eq!(sum.calls(), 1);
    let rows = store.rows();
    assert_eq!(
        rows[0].0.summary.as_deref(),
        clean_summary("A short summary of the talk.").as_deref()
    );
}

#[tokio::test]
async fn save_summary_error_uses_the_fallback() {
    let sess = session(3);
    let expected = fallback_summary(&sess.turns);
    let sum = FakeSummarizer::new(SumMode::Fail);
    let store = FakeStore::new(false);
    let notes = FakeNotes::new(Step::Ok);
    let now = fixed_now;
    let report = save_session(
        &enabled(false, VAULT),
        &deps(&sum, &store, &notes, &now),
        sess,
        SessionEnd::UserEnded,
    )
    .await;
    assert!(!report.summary_from_model);
    assert!(report.db_saved);
    assert_eq!(
        store.rows()[0].0.summary.as_deref(),
        Some(expected.as_str())
    );
}

#[tokio::test]
async fn save_empty_or_unusable_summary_uses_the_fallback() {
    for raw in ["", "   \n  ", "```\n```"] {
        let sess = session(1);
        let expected = fallback_summary(&sess.turns);
        let sum = FakeSummarizer::new(SumMode::Reply(raw));
        let store = FakeStore::new(false);
        let notes = FakeNotes::new(Step::Ok);
        let now = fixed_now;
        let report = save_session(
            &enabled(false, VAULT),
            &deps(&sum, &store, &notes, &now),
            sess,
            SessionEnd::Idle,
        )
        .await;
        assert!(!report.summary_from_model, "raw={raw:?}");
        assert_eq!(
            store.rows()[0].0.summary.as_deref(),
            Some(expected.as_str()),
            "raw={raw:?}"
        );
    }
}

#[tokio::test]
async fn save_summary_timeout_uses_the_fallback_and_respects_the_deadline() {
    let sess = session(2);
    let expected = fallback_summary(&sess.turns);
    let sum = FakeSummarizer::new(SumMode::Sleep(Duration::from_secs(5), "too late"));
    let store = FakeStore::new(false);
    let notes = FakeNotes::new(Step::Ok);
    let now = fixed_now;
    let started = Instant::now();
    let report = save_session_with_deadline(
        &enabled(false, VAULT),
        &deps(&sum, &store, &notes, &now),
        sess,
        SessionEnd::UserEnded,
        Duration::from_millis(100),
    )
    .await;
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(!report.summary_from_model);
    assert_eq!(
        store.rows()[0].0.summary.as_deref(),
        Some(expected.as_str())
    );
}

#[tokio::test]
async fn save_hanging_summarizer_does_not_block_beyond_the_deadline() {
    let sum = FakeSummarizer::new(SumMode::Hang);
    let store = FakeStore::new(false);
    let notes = FakeNotes::new(Step::Ok);
    let now = fixed_now;
    let started = Instant::now();
    let report = save_session_with_deadline(
        &enabled(true, VAULT),
        &deps(&sum, &store, &notes, &now),
        session(2),
        SessionEnd::Idle,
        Duration::from_millis(100),
    )
    .await;
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(report.db_saved, "the row is still written after a hang");
    assert!(!report.summary_from_model);
}

#[tokio::test]
async fn save_db_failure_still_writes_the_vault_note() {
    let sum = FakeSummarizer::new(SumMode::Reply("A summary."));
    let store = FakeStore::new(true);
    let notes = FakeNotes::new(Step::Ok);
    let now = fixed_now;
    let report = save_session(
        &enabled(true, VAULT),
        &deps(&sum, &store, &notes, &now),
        session(2),
        SessionEnd::UserEnded,
    )
    .await;
    assert!(!report.db_saved);
    assert!(report.vault_saved);
    assert_eq!(notes.calls().len(), 1);
}

#[tokio::test]
async fn save_vault_failures_keep_the_db_row() {
    for step in [Step::Unsafe, Step::TooLarge, Step::Invalid, Step::Io] {
        let sum = FakeSummarizer::new(SumMode::Reply("A summary."));
        let store = FakeStore::new(false);
        let notes = FakeNotes::new(step);
        let now = fixed_now;
        let report = save_session(
            &enabled(true, VAULT),
            &deps(&sum, &store, &notes, &now),
            session(2),
            SessionEnd::UserEnded,
        )
        .await;
        assert!(report.db_saved, "db row must survive a vault failure");
        assert!(!report.vault_saved);
        assert_eq!(store.attempts(), 1);
    }
}

#[tokio::test]
async fn save_exists_retries_once_with_a_12_hex_id() {
    let sum = FakeSummarizer::new(SumMode::Reply("A summary."));
    let store = FakeStore::new(false);
    let notes = FakeNotes::scripted(vec![Step::Exists, Step::Ok], Step::Ok);
    let now = fixed_now;
    let report = save_session(
        &enabled(true, VAULT),
        &deps(&sum, &store, &notes, &now),
        session(2),
        SessionEnd::UserEnded,
    )
    .await;
    let calls = notes.calls();
    assert_eq!(calls.len(), 2, "exactly one retry after Exists");
    assert!(report.vault_saved);
    assert!(report.db_saved);

    let sid_of = |rel: &str| {
        rel.rsplit('_')
            .next()
            .unwrap_or_default()
            .trim_end_matches(".md")
            .to_string()
    };
    let first = sid_of(&calls[0].1);
    let second = sid_of(&calls[1].1);
    assert_eq!(first.len(), 8);
    assert_eq!(second.len(), 12, "the retry uses a 12-hex id");
    assert!(second.chars().all(|c| c.is_ascii_hexdigit()));
}

#[tokio::test]
async fn save_exists_twice_gives_up_silently_and_keeps_the_row() {
    let sum = FakeSummarizer::new(SumMode::Reply("A summary."));
    let store = FakeStore::new(false);
    let notes = FakeNotes::new(Step::Exists);
    let now = fixed_now;
    let report = save_session(
        &enabled(true, VAULT),
        &deps(&sum, &store, &notes, &now),
        session(2),
        SessionEnd::UserEnded,
    )
    .await;
    assert_eq!(notes.calls().len(), 2, "one attempt plus exactly one retry");
    assert!(!report.vault_saved);
    assert!(report.db_saved);
}

#[tokio::test]
async fn save_blank_vault_path_writes_the_db_only() {
    for path in ["", "   "] {
        let sum = FakeSummarizer::new(SumMode::Reply("A summary."));
        let store = FakeStore::new(false);
        let notes = FakeNotes::new(Step::Ok);
        let now = fixed_now;
        let report = save_session(
            &enabled(true, path),
            &deps(&sum, &store, &notes, &now),
            session(2),
            SessionEnd::UserEnded,
        )
        .await;
        assert!(report.db_saved, "path={path:?}");
        assert!(!report.vault_saved, "path={path:?}");
        assert!(notes.calls().is_empty(), "path={path:?}");
    }
}

#[tokio::test]
async fn save_invalid_relative_vault_path_writes_the_db_only_without_panic() {
    let sum = FakeSummarizer::new(SumMode::Reply("A summary."));
    let store = FakeStore::new(false);
    let notes = FakeNotes::new(Step::Invalid);
    let now = fixed_now;
    let report = save_session(
        &enabled(true, "relative/vault"),
        &deps(&sum, &store, &notes, &now),
        session(2),
        SessionEnd::UserEnded,
    )
    .await;
    assert!(report.db_saved);
    assert!(!report.vault_saved);
}

#[tokio::test]
async fn save_report_carries_no_transcript_summary_or_path_text() {
    let sum = FakeSummarizer::new(SumMode::Reply("A summary."));
    let store = FakeStore::new(false);
    let notes = FakeNotes::new(Step::Unsafe);
    let now = fixed_now;
    let report = save_session(
        &enabled(true, VAULT),
        &deps(&sum, &store, &notes, &now),
        session(2),
        SessionEnd::UserEnded,
    )
    .await;
    let debug = format!("{report:?}");
    assert!(!debug.contains("question number"));
    assert!(!debug.contains("answer number"));
    assert!(!debug.contains(VAULT));
}

// ---------------------------------------------------------------------------
// c) load_vault_block
// ---------------------------------------------------------------------------

const HEADER_START: &str = "REFERENCE NOTES FROM THE USER'S VAULT";

fn write_pack(root: &std::path::Path, body: &str) {
    let dir = root.join("Memory").join("Code-Puppy");
    std::fs::create_dir_all(&dir).unwrap_or_default();
    std::fs::write(dir.join("CONTEXT-PACK.md"), body).unwrap_or_default();
}

#[tokio::test]
async fn load_vault_block_reads_a_normal_pack_with_the_untrusted_header() {
    let dir = tempfile::tempdir().unwrap_or_else(|_| panic!("tempdir"));
    write_pack(
        dir.path(),
        "# Projects\n- alpha: a synthetic demo project\n",
    );
    let root = dir.path().to_string_lossy().to_string();
    let started = Instant::now();
    let block = load_vault_block(&root, Duration::from_secs(2)).await;
    assert!(started.elapsed() < Duration::from_secs(2));
    let block = block.unwrap_or_default();
    assert!(block.starts_with(HEADER_START), "got: {block:?}");
    assert!(block.contains("alpha"));
}

#[tokio::test]
async fn load_vault_block_missing_pack_is_none() {
    let dir = tempfile::tempdir().unwrap_or_else(|_| panic!("tempdir"));
    let root = dir.path().to_string_lossy().to_string();
    assert_eq!(load_vault_block(&root, Duration::from_secs(2)).await, None);
}

#[tokio::test]
async fn load_vault_block_rejects_an_oversize_pack() {
    let dir = tempfile::tempdir().unwrap_or_else(|_| panic!("tempdir"));
    let body = format!("{}\n", "x".repeat(64 * 1024 + 8));
    write_pack(dir.path(), &body);
    let root = dir.path().to_string_lossy().to_string();
    assert_eq!(load_vault_block(&root, Duration::from_secs(2)).await, None);
}

#[cfg(unix)]
#[tokio::test]
async fn load_vault_block_rejects_a_symlinked_pack() {
    let dir = tempfile::tempdir().unwrap_or_else(|_| panic!("tempdir"));
    let outside = tempfile::tempdir().unwrap_or_else(|_| panic!("tempdir"));
    let target = outside.path().join("secret-pack.md");
    std::fs::write(&target, "# outside\n").unwrap_or_default();
    let dir_path = dir.path().join("Memory").join("Code-Puppy");
    std::fs::create_dir_all(&dir_path).unwrap_or_default();
    let _ = std::os::unix::fs::symlink(&target, dir_path.join("CONTEXT-PACK.md"));
    let root = dir.path().to_string_lossy().to_string();
    assert_eq!(load_vault_block(&root, Duration::from_secs(2)).await, None);
}

#[tokio::test]
async fn load_vault_block_empty_path_is_none() {
    assert_eq!(load_vault_block("", Duration::from_secs(2)).await, None);
    assert_eq!(load_vault_block("   ", Duration::from_secs(2)).await, None);
}

#[tokio::test]
async fn load_vault_block_returns_promptly_with_a_zero_timeout() {
    let dir = tempfile::tempdir().unwrap_or_else(|_| panic!("tempdir"));
    write_pack(dir.path(), "# Projects\n- beta\n");
    let root = dir.path().to_string_lossy().to_string();
    let started = Instant::now();
    let block = load_vault_block(&root, Duration::ZERO).await;
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(block.is_none() || block.is_some_and(|b| b.starts_with(HEADER_START)));
}

// ---------------------------------------------------------------------------
// d) answer_only_for / vault_context_active truth tables
// ---------------------------------------------------------------------------

fn settings_with(vault_context: bool, vault_path: &str, realtime: bool) -> AppSettings {
    let mut s = get_default_settings();
    s.vault_context_enabled = vault_context;
    s.vault_path = vault_path.to_string();
    s.exp_rt_enabled = realtime;
    s
}

#[test]
fn answer_only_truth_table() {
    for vault_context in [false, true] {
        for path in ["", "  ", "/x"] {
            for realtime in [false, true] {
                let s = settings_with(vault_context, path, realtime);
                let expected = vault_context && !path.trim().is_empty();
                assert_eq!(
                    answer_only_for(&s),
                    expected,
                    "vault_context={vault_context} path={path:?} realtime={realtime}"
                );
            }
        }
    }
}

#[test]
fn vault_context_active_truth_table_adds_the_realtime_master() {
    for vault_context in [false, true] {
        for path in ["", "  ", "/x"] {
            for realtime in [false, true] {
                let s = settings_with(vault_context, path, realtime);
                let expected = vault_context && !path.trim().is_empty() && realtime;
                assert_eq!(
                    vault_context_active(&s),
                    expected,
                    "vault_context={vault_context} path={path:?} realtime={realtime}"
                );
            }
        }
    }
}

#[test]
fn rag_only_config_is_answer_only_but_not_m1_context() {
    // The RAG switch alone makes the session answer-only; it never turns on the M1 block.
    let mut s = settings_with(false, "/x", true);
    s.vault_rag_enabled = true;
    assert!(answer_only_for(&s));
    assert!(!vault_context_active(&s));
}

#[test]
fn m1_config_is_answer_only_and_m1_context_active() {
    let s = settings_with(true, "/x", true);
    assert!(answer_only_for(&s));
    assert!(vault_context_active(&s));
}

#[test]
fn m1_config_with_realtime_off_is_not_m1_context() {
    let s = settings_with(true, "/x", false);
    assert!(!vault_context_active(&s));
}

// ---------------------------------------------------------------------------
// e) summary_route / summary_options
// ---------------------------------------------------------------------------

fn model_settings() -> AppSettings {
    let mut s = get_default_settings();
    s.tts_composer_mode = crate::managers::composer_config::TtsComposerMode::Model;
    s.tts_composer_provider_id = "openai".to_string();
    s.tts_composer_model = "synthetic-model".to_string();
    s
}

#[test]
fn summary_route_direct_anthropic_endpoint() {
    let mut s = model_settings();
    s.exp_rt_enabled = true;
    s.exp_rt_direct_endpoint = true;
    s.exp_rt_endpoint_url = "https://anthropic.example.invalid/v1".to_string();
    s.exp_rt_model = "synthetic-model".to_string();
    assert_eq!(summary_route(&s), SummaryRoute::DirectAnthropic);
}

#[test]
fn summary_route_openai_compatible_when_the_stream_route_is_on() {
    let mut s = model_settings();
    s.exp_rt_enabled = true;
    s.exp_rt_stream_reply = true;
    assert_eq!(summary_route(&s), SummaryRoute::OpenAiCompatible);
}

#[test]
fn summary_route_legacy_model_when_the_stream_route_is_off() {
    let mut s = model_settings();
    s.exp_rt_enabled = false;
    assert_eq!(summary_route(&s), SummaryRoute::LegacyModel);
}

#[test]
fn summary_route_command_backend_never_calls_a_model() {
    let mut s = model_settings();
    s.tts_composer_mode = crate::managers::composer_config::TtsComposerMode::Command;
    s.tts_composer_command_path = "/usr/bin/synthetic-agent".to_string();
    assert_eq!(summary_route(&s), SummaryRoute::NoModel);
}

#[test]
fn summary_route_off_never_calls_a_model() {
    let mut s = model_settings();
    s.tts_composer_mode = crate::managers::composer_config::TtsComposerMode::Off;
    assert_eq!(summary_route(&s), SummaryRoute::NoModel);
}

#[test]
fn summary_route_misconfigured_direct_endpoint_never_falls_back() {
    let mut s = model_settings();
    s.exp_rt_enabled = true;
    s.exp_rt_direct_endpoint = true;
    s.exp_rt_endpoint_url = String::new();
    assert_eq!(summary_route(&s), SummaryRoute::DirectAnthropic);
}

#[test]
fn summary_options_for_every_model_route() {
    for route in [
        SummaryRoute::DirectAnthropic,
        SummaryRoute::OpenAiCompatible,
        SummaryRoute::LegacyModel,
    ] {
        let o = summary_options(route);
        assert!(!o.json_mode, "{route:?}");
        assert_eq!(o.max_tokens, 300, "{route:?}");
        assert_eq!(o.deadline_secs, 20, "{route:?}");
    }
}
