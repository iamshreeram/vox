//! Contract tests for the vault index (DESIGN 3.3, 3.4; CRITIC C2, C3, C7).
//!
//! Every test is named `idx_*` and runs against the real index over synthetic fixtures; a failure
//! means an index contract regressed. Credential-shaped values are built with `concat!`
//! so no contiguous token-shaped text exists in source.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::managers::vault::{NoteEntry, NoteRead, NoteSource, VaultError, WalkLimits, WalkResult};
use crate::managers::vault_index::{
    DebugDump, DebugNote, IndexManager, RagError, RecentOpts, VaultIdentity,
};
use crate::managers::vault_io::VaultIoWorker;

const BUDGET: Duration = Duration::from_secs(30);
const PAUSE_WAIT: Duration = Duration::from_secs(10);

const PEM_HEAD: &str = concat!("-----BEGIN ", "PRIVATE KEY-----");
const PEM_TAIL: &str = concat!("-----END ", "PRIVATE KEY-----");
const TOKEN_A: &str = concat!("sk-ant-", "api03TESTFIXTUREAAAAAAAAAA");
const TOKEN_B: &str = concat!("glpat-", "TESTFIXTUREBBBBBBBBBBB");
const TOKEN_C: &str = concat!("xoxb-", "123456789012-TESTFIXTURECCCCCCCC");
const PEM_BODY: &str = concat!("MIIEvQIBADANBgkqhkiG9w0BAQEFAAS", "CBKcwggSjAgEAAoIBAQC");
const KV_SECRET: &str = concat!("api", "_key=TESTFIXTUREDDDDDDDDDDDD");

/// Scripted failure for one note path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fail {
    Unstable,
    TooLarge,
    Forbidden,
    Missing,
    Io,
}

/// A pause hook: the fake sends on `.0` when reached, then waits up to `PAUSE_WAIT` for `.1`.
type Pause = (Sender<()>, Receiver<()>);

/// Scripted fake vault used by all index tests.
#[derive(Default)]
struct FakeNoteSource {
    notes: Mutex<BTreeMap<String, (String, i64)>>,
    failures: Mutex<BTreeMap<String, Fail>>,
    complete: Mutex<bool>,
    walk_error: Mutex<bool>,
    walk_calls: Mutex<usize>,
    read_calls: Mutex<usize>,
    /// Pause inside `walk`.
    walk_pause: Mutex<Option<Pause>>,
    /// Pause inside `read`.
    read_pause: Mutex<Option<Pause>>,
    /// Read order, for fairness checks.
    read_order: Mutex<VecDeque<String>>,
}

impl FakeNoteSource {
    fn new() -> Self {
        let f = FakeNoteSource::default();
        *f.complete.lock().expect("fixture lock") = true;
        f
    }

    fn put(&self, rel: &str, text: &str) {
        self.notes
            .lock()
            .expect("fixture lock")
            .insert(rel.to_string(), (text.to_string(), 1_700_000_000_000));
    }

    fn remove(&self, rel: &str) {
        self.notes.lock().expect("fixture lock").remove(rel);
    }

    fn fail(&self, rel: &str, kind: Fail) {
        self.failures
            .lock()
            .expect("fixture lock")
            .insert(rel.to_string(), kind);
    }

    fn set_complete(&self, complete: bool) {
        *self.complete.lock().expect("fixture lock") = complete;
    }
}

impl NoteSource for FakeNoteSource {
    fn walk(&self, _limits: &WalkLimits) -> Result<WalkResult, VaultError> {
        *self.walk_calls.lock().expect("fixture lock") += 1;
        if let Some((tx, rx)) = self.walk_pause.lock().expect("fixture lock").as_ref() {
            let _ = tx.send(());
            let _ = rx.recv_timeout(PAUSE_WAIT);
        }
        if *self.walk_error.lock().expect("fixture lock") {
            return Err(VaultError::Io(std::io::ErrorKind::PermissionDenied));
        }
        let notes = self
            .notes
            .lock()
            .expect("fixture lock")
            .iter()
            .map(|(rel, (text, mtime))| NoteEntry {
                rel: rel.clone(),
                size: text.len() as u64,
                mtime_ms: *mtime,
            })
            .collect();
        Ok(WalkResult {
            notes,
            complete: *self.complete.lock().expect("fixture lock"),
            skipped_large: 0,
            skipped_other: 0,
            skipped_unreadable: 0,
        })
    }

    fn read(&self, rel: &str) -> Result<NoteRead, VaultError> {
        *self.read_calls.lock().expect("fixture lock") += 1;
        self.read_order
            .lock()
            .expect("fixture lock")
            .push_back(rel.to_string());
        if let Some((tx, rx)) = self.read_pause.lock().expect("fixture lock").as_ref() {
            let _ = tx.send(());
            let _ = rx.recv_timeout(PAUSE_WAIT);
        }
        if let Some(kind) = self
            .failures
            .lock()
            .expect("fixture lock")
            .get(rel)
            .copied()
        {
            return Err(match kind {
                Fail::Unstable => VaultError::Unstable,
                Fail::TooLarge => VaultError::TooLarge,
                Fail::Forbidden => VaultError::Forbidden,
                Fail::Missing => VaultError::Missing,
                Fail::Io => VaultError::Io(std::io::ErrorKind::Other),
            });
        }
        let notes = self.notes.lock().expect("fixture lock");
        match notes.get(rel) {
            Some((text, mtime)) => Ok(NoteRead {
                text: text.clone(),
                size: text.len() as u64,
                mtime_ms: *mtime,
            }),
            None => Err(VaultError::Missing),
        }
    }
}

fn manager(dir: &Path) -> IndexManager {
    IndexManager::new(dir.to_path_buf(), Arc::new(VaultIoWorker::start()))
}

fn configured(dir: &Path, root: &Path) -> (IndexManager, u64) {
    let m = manager(dir);
    let gen = m.configure(
        Some(root.to_path_buf()),
        vec!["Projects".into(), "Daily".into()],
    );
    (m, gen)
}

/// Every persisted string, concatenated, for "must not appear anywhere" checks.
fn all_text(d: &DebugDump) -> String {
    let mut s = String::new();
    for n in &d.notes {
        s.push_str(&n.rel);
        s.push('\n');
        s.push_str(&n.title);
        s.push('\n');
        s.push_str(n.status.as_deref().unwrap_or(""));
        s.push('\n');
    }
    for c in &d.chunks {
        s.push_str(&c.rel);
        s.push('\n');
        s.push_str(&c.title);
        s.push('\n');
        s.push_str(&c.heading);
        s.push('\n');
        s.push_str(&c.body);
        s.push('\n');
    }
    for (k, v) in &d.meta {
        s.push_str(k);
        s.push('=');
        s.push_str(v);
        s.push('\n');
    }
    s
}

fn chunk_bodies(d: &DebugDump, rel: &str) -> Vec<String> {
    d.chunks
        .iter()
        .filter(|c| c.rel == rel)
        .map(|c| c.body.clone())
        .collect()
}

fn note_row<'a>(d: &'a DebugDump, rel: &str) -> Option<&'a DebugNote> {
    d.notes.iter().find(|n| n.rel == rel)
}

// ---------------------------------------------------------------- pipeline (3.4)

#[test]
fn idx_pipeline_pem_spanning_chunks_is_redacted_before_chunking() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    let mut body = String::from("# Keys\n");
    body.push_str(PEM_HEAD);
    body.push('\n');
    for _ in 0..80 {
        body.push_str(PEM_BODY);
        body.push('\n');
    }
    body.push_str(PEM_TAIL);
    src.put("Projects/pem.md", &body);
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let text = all_text(&m.debug_rows());
    assert!(!text.contains(PEM_BODY));
    assert!(!text.contains(PEM_HEAD));
    assert!(text.contains("<redacted:pem>"));
}

#[test]
fn idx_pipeline_secret_straddling_1500_char_split_is_redacted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    let mut body = "a ".repeat(740);
    body.push_str(KV_SECRET);
    body.push_str(" tail words");
    src.put("Projects/straddle.md", &body);
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let text = all_text(&m.debug_rows());
    assert!(!text.contains("TESTFIXTUREDDDDDDDDDDDD"));
    assert!(text.contains("<redacted:"));
}

#[test]
fn idx_pipeline_secret_in_title_frontmatter_status_and_heading_is_not_persisted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    let body = format!(
        "---\nname: Atlas {TOKEN_A}\nstatus: {TOKEN_B}\n---\n# Heading {TOKEN_C}\nbody text\n"
    );
    src.put("Projects/atlas.md", &body);
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let text = all_text(&m.debug_rows());
    for secret in [TOKEN_A, TOKEN_B, TOKEN_C] {
        assert!(!text.contains(secret), "secret persisted");
    }
    assert!(text.contains("<redacted:"));
}

#[test]
fn idx_pipeline_control_char_inside_token_is_still_redacted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    let (head, tail) = TOKEN_A.split_at(10);
    let body = format!("note start {head}\u{7}{tail} end\n");
    src.put("Projects/ctrl.md", &body);
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let text = all_text(&m.debug_rows());
    assert!(!text.contains(TOKEN_A));
    assert!(!text.contains(tail));
}

#[test]
fn idx_pipeline_crlf_and_nul_are_normalised() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put(
        "Projects/crlf.md",
        "# Title\r\nline one\r\n\u{0}line two\r\n",
    );
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let d = m.debug_rows();
    let bodies = chunk_bodies(&d, "Projects/crlf.md");
    assert!(!bodies.is_empty());
    for b in &bodies {
        assert!(!b.contains('\r'));
        assert!(!b.contains('\u{0}'));
    }
    assert!(bodies.iter().any(|b| b.contains("line two")));
}

#[test]
fn idx_frontmatter_valid_none_malformed_unterminated_are_tolerated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put(
        "Projects/valid.md",
        "---\nstatus: active\ntags: [a, b]\n---\n# V\nbody\n",
    );
    src.put("Projects/none.md", "# N\nplain body\n");
    src.put("Projects/malformed.md", "---\n: : :\n\u{1}---\n# M\nbody\n");
    src.put(
        "Projects/unterminated.md",
        "---\nstatus: open\n# U\nbody still here\n",
    );
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let d = m.debug_rows();
    assert_eq!(
        note_row(&d, "Projects/valid.md").and_then(|n| n.status.clone()),
        Some("active".to_string())
    );
    assert!(note_row(&d, "Projects/none.md").is_some());
    assert!(note_row(&d, "Projects/malformed.md").is_some());
    assert!(note_row(&d, "Projects/unterminated.md").is_some());
    assert!(chunk_bodies(&d, "Projects/unterminated.md")
        .iter()
        .any(|b| b.contains("body still here")));
}

#[test]
fn idx_frontmatter_status_key_is_stored_and_tags_list_not_in_status() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put(
        "Projects/st.md",
        "---\ntags:\n  - alpha\nstatus: blocked\n---\n# St\nx\n",
    );
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let d = m.debug_rows();
    assert_eq!(
        note_row(&d, "Projects/st.md").and_then(|n| n.status.clone()),
        Some("blocked".to_string())
    );
}

#[test]
fn idx_title_order_frontmatter_then_h1_then_stem() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put(
        "Projects/fm-name.md",
        "---\nname: From Name\n---\n# Other H1\nx\n",
    );
    src.put(
        "Projects/fm-title.md",
        "---\ntitle: From Title\n---\n# Other H1\nx\n",
    );
    src.put("Projects/h1.md", "intro line\n# Heading One\nx\n");
    src.put("Projects/stem-only.md", "no heading at all\n");
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let d = m.debug_rows();
    assert_eq!(
        note_row(&d, "Projects/fm-name.md").map(|n| n.title.as_str()),
        Some("From Name")
    );
    assert_eq!(
        note_row(&d, "Projects/fm-title.md").map(|n| n.title.as_str()),
        Some("From Title")
    );
    assert_eq!(
        note_row(&d, "Projects/h1.md").map(|n| n.title.as_str()),
        Some("Heading One")
    );
    assert_eq!(
        note_row(&d, "Projects/stem-only.md").map(|n| n.title.as_str()),
        Some("stem-only")
    );
}

#[test]
fn idx_note_ts_from_file_name_frontmatter_and_invalid() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put("Daily/2026-03-05.md", "# Day\nx\n");
    src.put("Daily/2026-04.md", "# Month\nx\n");
    src.put(
        "Daily/fm.md",
        "---\ndate: 2026-02-10T08:00:00Z\n---\n# Fm\nx\n",
    );
    src.put("Daily/bad.md", "---\nday: 2026-13-45\n---\n# Bad\nx\n");
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let d = m.debug_rows();
    let utc_midnight = |y: i32, mo: u32, da: u32| {
        chrono::NaiveDate::from_ymd_opt(y, mo, da)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|t| t.and_utc().timestamp_millis())
    };
    assert_eq!(
        note_row(&d, "Daily/2026-03-05.md").and_then(|n| n.note_ts),
        utc_midnight(2026, 3, 5)
    );
    assert_eq!(
        note_row(&d, "Daily/2026-04.md").and_then(|n| n.note_ts),
        utc_midnight(2026, 4, 1)
    );
    assert_eq!(
        note_row(&d, "Daily/fm.md").and_then(|n| n.note_ts),
        utc_midnight(2026, 2, 10)
    );
    assert_eq!(note_row(&d, "Daily/bad.md").and_then(|n| n.note_ts), None);
}

#[test]
fn idx_note_ts_future_date_is_stored_as_is() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put("Daily/2999-01-01.md", "# Future\nx\n");
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let d = m.debug_rows();
    let expected = chrono::NaiveDate::from_ymd_opt(2999, 1, 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|t| t.and_utc().timestamp_millis());
    assert_eq!(
        note_row(&d, "Daily/2999-01-01.md").and_then(|n| n.note_ts),
        expected
    );
}

#[test]
fn idx_chunking_splits_outside_fences_not_inside() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    let body = "# Top\nintro\n## Real Section\nreal body\n```\n# not a heading\n## also not\n```\n## After\nafter body\n";
    src.put("Projects/fence.md", body);
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let d = m.debug_rows();
    let headings: Vec<String> = d
        .chunks
        .iter()
        .filter(|c| c.rel == "Projects/fence.md")
        .map(|c| c.heading.clone())
        .collect();
    assert!(headings.iter().any(|h| h.contains("Real Section")));
    assert!(headings.iter().any(|h| h.contains("After")));
    assert!(!headings.iter().any(|h| h.contains("not a heading")));
    assert!(!headings.iter().any(|h| h.contains("also not")));
}

#[test]
fn idx_chunking_zero_heading_note_yields_chunks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put("Projects/plain.md", "just words without any heading\n");
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let d = m.debug_rows();
    assert!(!chunk_bodies(&d, "Projects/plain.md").is_empty());
}

#[test]
fn idx_chunking_each_chunk_at_most_1500_chars_on_char_boundaries() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    let body = "héllo wörld ünïcode 日本語テキスト ".repeat(400);
    src.put("Projects/long.md", &body);
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let d = m.debug_rows();
    let bodies = chunk_bodies(&d, "Projects/long.md");
    assert!(bodies.len() > 1);
    for b in &bodies {
        assert!(b.chars().count() <= 1500, "chunk too long");
    }
}

#[test]
fn idx_chunking_at_most_200_chunks_per_note() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    let mut body = String::new();
    for i in 0..600 {
        body.push_str(&format!("## Section {i}\n{}\n", "word ".repeat(120)));
    }
    src.put("Projects/many.md", &body);
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let d = m.debug_rows();
    assert!(chunk_bodies(&d, "Projects/many.md").len() <= 200);
}

#[test]
fn idx_chunking_title_repeated_on_every_chunk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    let body = format!(
        "---\nname: Repeated Title\n---\n{}",
        "## S\nfiller text here\n".repeat(200)
    );
    src.put("Projects/rep.md", &body);
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let d = m.debug_rows();
    let rows: Vec<_> = d
        .chunks
        .iter()
        .filter(|c| c.rel == "Projects/rep.md")
        .collect();
    assert!(rows.len() > 1);
    assert!(rows.iter().all(|c| c.title == "Repeated Title"));
}

#[test]
fn idx_wikilinks_alias_and_plain_are_reduced() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put(
        "Projects/links.md",
        "# L\nsee [[Target Note|shown name]] and [[Plain]] here\n",
    );
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let d = m.debug_rows();
    let text = chunk_bodies(&d, "Projects/links.md").join("\n");
    assert!(text.contains("shown name"));
    assert!(text.contains("Plain"));
    assert!(!text.contains("[["));
    assert!(!text.contains("Target Note"));
}

// ---------------------------------------------------------------- sensitive names (C3)

#[test]
fn idx_sensitive_name_absent_from_all_tables_right_after_pending_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    let rel = format!("Projects/{TOKEN_A}.md");
    src.put(&rel, "# Secret named\nbody\n");
    src.put("Projects/ok.md", "# Ok\nbody\n");
    let (m, _) = configured(dir.path(), root.path());
    let mut seen_pending = false;
    let mut hook = || {
        seen_pending = true;
        let d = m.debug_rows();
        let text = all_text(&d);
        assert!(
            !text.contains(TOKEN_A),
            "credential persisted after pending rows"
        );
        assert!(
            !text.contains(&rel),
            "sensitive rel persisted after pending rows"
        );
        assert!(
            d.notes.iter().any(|n| n.rel == "Projects/ok.md"),
            "hook ran before pending rows were committed"
        );
    };
    m.refresh_with_hook(&src, BUDGET, &mut hook);
    assert!(seen_pending, "after_pending hook never ran");
    let d = m.debug_rows();
    assert!(!all_text(&d).contains(TOKEN_A));
    assert!(!all_text(&d).contains(&rel));
}

#[test]
fn idx_sensitive_name_increments_aggregate_count_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put(&format!("Projects/{TOKEN_B}.md"), "# x\nbody\n");
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    assert!(m.status().sensitive_names >= 1);
    assert!(!all_text(&m.debug_rows()).contains(TOKEN_B));
}

#[test]
fn idx_sensitive_name_preexisting_row_deleted_and_not_reinserted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    let rel = format!("Projects/{TOKEN_C}.md");
    src.put(&rel, "# Before\nbody\n");
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    src.put(&rel, "# After\nbody\n");
    m.refresh_with(&src, BUDGET);
    m.refresh_with(&src, BUDGET);
    let d = m.debug_rows();
    assert!(note_row(&d, &rel).is_none());
    assert!(!all_text(&d).contains(TOKEN_C));
}

// ---------------------------------------------------------------- refresh semantics (C7)

#[test]
fn idx_refresh_first_pass_indexes_everything_with_added_counts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put("Projects/a.md", "# A\nalpha\n");
    src.put("Daily/2026-01-02.md", "# B\nbeta\n");
    let (m, _) = configured(dir.path(), root.path());
    let r = m.refresh_with(&src, BUDGET);
    assert_eq!(r.added, 2);
    assert_eq!(r.updated, 0);
    assert_eq!(r.removed, 0);
    assert!(r.walk_complete);
    assert!(!r.timed_out);
    assert!(!r.already_running);
    assert_eq!(m.status().notes, 2);
}

#[test]
fn idx_refresh_unchanged_second_pass_does_no_reads() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put("Projects/a.md", "# A\nalpha\n");
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let before = *src.read_calls.lock().expect("fixture lock");
    let r = m.refresh_with(&src, BUDGET);
    let after = *src.read_calls.lock().expect("fixture lock");
    assert_eq!(after, before, "unchanged note was read again");
    assert_eq!(r.added + r.updated, 0);
}

#[test]
fn idx_refresh_changed_note_updated_and_old_chunks_replaced() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put("Projects/c.md", "# C\nOLDMARKER text\n");
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    src.put("Projects/c.md", "# C\nNEWMARKER text that is different\n");
    let r = m.refresh_with(&src, BUDGET);
    assert_eq!(r.updated, 1);
    let text = chunk_bodies(&m.debug_rows(), "Projects/c.md").join("\n");
    assert!(text.contains("NEWMARKER"));
    assert!(!text.contains("OLDMARKER"));
}

#[test]
fn idx_refresh_removed_note_deleted_only_when_walk_complete() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put("Projects/gone.md", "# G\nbody\n");
    src.put("Projects/stay.md", "# S\nbody\n");
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    src.remove("Projects/gone.md");
    src.set_complete(false);
    m.refresh_with(&src, BUDGET);
    assert!(
        note_row(&m.debug_rows(), "Projects/gone.md").is_some(),
        "deleted on incomplete walk"
    );
    src.set_complete(true);
    let r = m.refresh_with(&src, BUDGET);
    assert_eq!(r.removed, 1);
    let d = m.debug_rows();
    assert!(note_row(&d, "Projects/gone.md").is_none());
    assert!(chunk_bodies(&d, "Projects/gone.md").is_empty());
}

#[test]
fn idx_refresh_incomplete_walk_never_deletes_anything() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put("Projects/keep.md", "# K\nbody\n");
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    src.remove("Projects/keep.md");
    src.set_complete(false);
    let r = m.refresh_with(&src, BUDGET);
    assert_eq!(r.removed, 0);
    assert!(!r.walk_complete);
    assert!(note_row(&m.debug_rows(), "Projects/keep.md").is_some());
}

#[test]
fn idx_refresh_fair_pending_with_tiny_budget_eventually_indexes_all() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    for i in 0..5 {
        src.put(&format!("Projects/n{i}.md"), &format!("# N{i}\nbody {i}\n"));
    }
    let (m, _) = configured(dir.path(), root.path());
    for _ in 0..20 {
        m.refresh_with(&src, Duration::from_millis(0));
    }
    let d = m.debug_rows();
    for i in 0..5 {
        assert!(
            note_row(&d, &format!("Projects/n{i}.md")).is_some(),
            "note {i} never indexed"
        );
    }
    let order: Vec<String> = src
        .read_order
        .lock()
        .expect("fixture lock")
        .iter()
        .cloned()
        .collect();
    assert!(!order.is_empty());
    // Oldest-pending-first: with equal pending age, the first read must be the first walked
    // note (BTreeMap order), and the reads must never go backwards to an earlier note.
    let expected_first = "Projects/n0.md";
    assert_eq!(order.first().map(String::as_str), Some(expected_first));
    let mut seen: Vec<&str> = Vec::new();
    for rel in &order {
        if !seen.contains(&rel.as_str()) {
            seen.push(rel.as_str());
        }
    }
    let mut sorted = seen.clone();
    sorted.sort();
    assert_eq!(seen, sorted, "notes were not read oldest-pending-first");
}

#[test]
fn idx_refresh_unstable_read_stays_pending_then_skipped_after_three() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put("Projects/flaky.md", "# F\nbody\n");
    src.fail("Projects/flaky.md", Fail::Unstable);
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let d = m.debug_rows();
    let row = note_row(&d, "Projects/flaky.md").expect("row exists");
    assert_eq!(row.state, "pending");
    assert_eq!(row.attempts, 1);
    m.refresh_with(&src, BUDGET);
    m.refresh_with(&src, BUDGET);
    let d = m.debug_rows();
    let row = note_row(&d, "Projects/flaky.md").expect("row exists");
    assert_eq!(row.state, "skipped");
    assert_eq!(row.skip_reason.as_deref(), Some("unstable"));
}

#[test]
fn idx_transition_readable_to_unreadable_deletes_chunks_immediately() {
    for kind in [Fail::TooLarge, Fail::Forbidden, Fail::Io] {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = tempfile::tempdir().expect("root");
        let src = FakeNoteSource::new();
        src.put("Projects/t.md", "# T\nSTALEMARKER body\n");
        let (m, _) = configured(dir.path(), root.path());
        m.refresh_with(&src, BUDGET);
        assert!(!chunk_bodies(&m.debug_rows(), "Projects/t.md").is_empty());
        src.put("Projects/t.md", "# T\nchanged body\n");
        src.fail("Projects/t.md", kind);
        m.refresh_with(&src, BUDGET);
        let d = m.debug_rows();
        assert!(
            chunk_bodies(&d, "Projects/t.md").is_empty(),
            "stale chunks kept for {kind:?}"
        );
        let row = note_row(&d, "Projects/t.md").expect("row exists");
        assert_eq!(row.state, "skipped");
        assert!(row.skip_reason.is_some());
    }
}

#[test]
fn idx_failed_read_of_one_note_does_not_abort_others() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put("Projects/a-bad.md", "# Bad\nbody\n");
    src.put("Projects/b-good.md", "# Good\nbody\n");
    src.fail("Projects/a-bad.md", Fail::Io);
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    assert!(note_row(&m.debug_rows(), "Projects/b-good.md").is_some_and(|n| n.state == "current"));
}

#[test]
fn idx_budget_timeout_sets_timed_out_and_next_refresh_resumes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    for i in 0..6 {
        src.put(&format!("Projects/t{i}.md"), &format!("# T{i}\nbody\n"));
    }
    let (m, _) = configured(dir.path(), root.path());
    let first = m.refresh_with(&src, Duration::from_nanos(1));
    assert!(first.timed_out);
    let mut guard = 0;
    while guard < 20 {
        guard += 1;
        if !m.refresh_with(&src, BUDGET).timed_out {
            break;
        }
    }
    let d = m.debug_rows();
    for i in 0..6 {
        assert!(note_row(&d, &format!("Projects/t{i}.md")).is_some());
    }
}

#[test]
fn idx_concurrent_refresh_second_call_reports_already_running() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put("Projects/busy.md", "# B\nbody\n");
    let (tx_walk, rx_walk) = channel::<()>();
    let (tx_go, rx_go) = channel::<()>();
    *src.walk_pause.lock().expect("fixture lock") = Some((tx_walk, rx_go));
    let (m, _) = configured(dir.path(), root.path());
    let m = Arc::new(m);
    let src = Arc::new(src);
    let (m2, src2) = (Arc::clone(&m), Arc::clone(&src));
    let handle = std::thread::spawn(move || m2.refresh_with(src2.as_ref(), BUDGET));
    rx_walk
        .recv_timeout(PAUSE_WAIT)
        .expect("first refresh paused");
    let second = m.refresh_with(src.as_ref(), BUDGET);
    assert!(second.already_running);
    tx_go.send(()).expect("release");
    let first = handle.join().expect("first refresh thread");
    assert!(!first.already_running);
}

#[test]
fn idx_walk_error_surfaces_in_status_without_wiping_index() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put("Projects/kept.md", "# K\nbody\n");
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    *src.walk_error.lock().expect("fixture lock") = true;
    m.refresh_with(&src, BUDGET);
    assert!(m.status().last_error_kind.is_some());
    assert!(note_row(&m.debug_rows(), "Projects/kept.md").is_some());
}

// ---------------------------------------------------------------- generation and lifecycle (C2)

#[test]
fn idx_generation_discards_old_refresh_and_writes_nothing_for_old_vault() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root_a = tempfile::tempdir().expect("root a");
    let root_b = tempfile::tempdir().expect("root b");
    let src_a = FakeNoteSource::new();
    src_a.put("Projects/only-a.md", "# A\nALPHAROW body\n");
    let (tx_walk, rx_walk) = channel::<()>();
    let (tx_go, rx_go) = channel::<()>();
    *src_a.walk_pause.lock().expect("fixture lock") = Some((tx_walk, rx_go));
    let m = manager(dir.path());
    let gen_a = m.configure(Some(root_a.path().to_path_buf()), vec!["Projects".into()]);
    let m = Arc::new(m);
    let src_a = Arc::new(src_a);
    let (m2, src_a2) = (Arc::clone(&m), Arc::clone(&src_a));
    let handle = std::thread::spawn(move || m2.refresh_with(src_a2.as_ref(), BUDGET));
    rx_walk.recv_timeout(PAUSE_WAIT).expect("A refresh paused");
    let gen_b = m.configure(Some(root_b.path().to_path_buf()), vec!["Projects".into()]);
    assert_ne!(gen_a, gen_b);
    tx_go.send(()).expect("release A");
    let report = handle.join().expect("A refresh thread");
    let d = m.debug_rows();
    assert!(
        !all_text(&d).contains("ALPHAROW"),
        "A rows leaked into B cache"
    );
    assert!(!d.notes.iter().any(|n| n.rel == "Projects/only-a.md"));
    assert_eq!(
        report.added + report.updated,
        0,
        "discarded work was counted"
    );
}

#[test]
fn idx_generation_mismatch_is_returned_to_old_generation_callers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root_a = tempfile::tempdir().expect("root a");
    let root_b = tempfile::tempdir().expect("root b");
    let m = manager(dir.path());
    let gen_a = m.configure(Some(root_a.path().to_path_buf()), vec!["Projects".into()]);
    let _gen_b = m.configure(Some(root_b.path().to_path_buf()), vec!["Projects".into()]);
    let opts = RecentOpts {
        folder: None,
        since_days: None,
        limit: 5,
    };
    let res = m.recent(gen_a, &opts);
    assert!(matches!(
        res,
        Err(RagError::GenerationMismatch) | Err(RagError::Unavailable)
    ));
}

#[test]
fn idx_cache_file_name_is_per_vault_identity() {
    let root = tempfile::tempdir().expect("root");
    let id = VaultIdentity::for_root(root.path()).expect("identity");
    assert_eq!(id.id.len(), 16);
    assert!(id
        .id
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    let dir = tempfile::tempdir().expect("tempdir");
    let src = FakeNoteSource::new();
    src.put("Projects/p.md", "# P\nbody\n");
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let expected: PathBuf = dir.path().join(format!("vault_index-{}.db", id.id));
    assert!(expected.exists(), "cache file not named per vault");
}

#[test]
fn idx_switching_vault_deletes_other_cache_files_with_sidecars() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root_a = tempfile::tempdir().expect("root a");
    let root_b = tempfile::tempdir().expect("root b");
    let src = FakeNoteSource::new();
    src.put("Projects/p.md", "# P\nbody\n");
    let (m, _) = configured(dir.path(), root_a.path());
    m.refresh_with(&src, BUDGET);
    let stale = dir.path().join("vault_index-0000000000000000.db");
    std::fs::write(&stale, b"old").expect("write stale");
    std::fs::write(dir.path().join("vault_index-0000000000000000.db-wal"), b"w").expect("wal");
    std::fs::write(dir.path().join("vault_index-0000000000000000.db-shm"), b"s").expect("shm");
    m.configure(Some(root_b.path().to_path_buf()), vec!["Projects".into()]);
    assert!(!stale.exists());
    assert!(!dir
        .path()
        .join("vault_index-0000000000000000.db-wal")
        .exists());
    assert!(!dir
        .path()
        .join("vault_index-0000000000000000.db-shm")
        .exists());
}

#[test]
fn idx_same_vault_different_folders_triggers_rebuild() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put("Projects/p.md", "# P\nbody\n");
    let m = manager(dir.path());
    m.configure(Some(root.path().to_path_buf()), vec!["Projects".into()]);
    m.refresh_with(&src, BUDGET);
    m.configure(
        Some(root.path().to_path_buf()),
        vec!["Projects".into(), "Daily".into()],
    );
    assert!(
        m.debug_rows().notes.is_empty(),
        "meta mismatch did not rebuild"
    );
}

#[cfg(unix)]
#[test]
fn idx_cache_file_mode_0600_before_sqlite_writes() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put("Projects/p.md", "# P\nbody\n");
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let id = VaultIdentity::for_root(root.path()).expect("identity");
    let db = dir.path().join(format!("vault_index-{}.db", id.id));
    let mode = std::fs::metadata(&db)
        .expect("db exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    let wal = dir.path().join(format!("vault_index-{}.db-wal", id.id));
    if wal.exists() {
        let wmode = std::fs::metadata(&wal).expect("wal").permissions().mode() & 0o777;
        assert_eq!(wmode, 0o600);
    }
}

#[test]
fn idx_corrupt_file_is_rebuilt_and_refresh_works() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let id = VaultIdentity::for_root(root.path()).expect("identity");
    std::fs::write(
        dir.path().join(format!("vault_index-{}.db", id.id)),
        b"this is not a sqlite database at all, just garbage bytes",
    )
    .expect("write garbage");
    let src = FakeNoteSource::new();
    src.put("Projects/p.md", "# P\nbody\n");
    let (m, _) = configured(dir.path(), root.path());
    let r = m.refresh_with(&src, BUDGET);
    assert_eq!(r.added, 1);
    assert!(note_row(&m.debug_rows(), "Projects/p.md").is_some());
}

#[test]
fn idx_newer_user_version_triggers_rebuild() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let id = VaultIdentity::for_root(root.path()).expect("identity");
    let db = dir.path().join(format!("vault_index-{}.db", id.id));
    {
        let conn = rusqlite::Connection::open(&db).expect("open");
        conn.pragma_update(None, "user_version", 9999)
            .expect("set version");
    }
    let src = FakeNoteSource::new();
    src.put("Projects/p.md", "# P\nbody\n");
    let (m, _) = configured(dir.path(), root.path());
    let r = m.refresh_with(&src, BUDGET);
    assert_eq!(r.added, 1);
}

#[test]
fn idx_busy_lock_is_not_corruption_and_file_is_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let id = VaultIdentity::for_root(root.path()).expect("identity");
    let db = dir.path().join(format!("vault_index-{}.db", id.id));
    let src = FakeNoteSource::new();
    src.put("Projects/p.md", "# P\nbody\n");
    let (m, _) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let before = std::fs::read(&db).expect("read db");
    let locker = rusqlite::Connection::open(&db).expect("second connection");
    locker
        .execute_batch("BEGIN EXCLUSIVE;")
        .expect("exclusive lock");
    src.put("Projects/p.md", "# P\nchanged\n");
    let r = m.refresh_with(&src, BUDGET);
    assert_eq!(m.status().last_error_kind.as_deref(), Some("busy"));
    assert_eq!(r.updated, 0);
    locker.execute_batch("ROLLBACK;").expect("release");
    let after = std::fs::read(&db).expect("read db after");
    assert_eq!(before, after, "busy lock treated as corruption");
}

#[cfg(unix)]
#[test]
fn idx_permission_denied_data_dir_reports_error_kind_and_deletes_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let base = tempfile::tempdir().expect("tempdir");
    let data = base.path().join("data");
    std::fs::create_dir(&data).expect("data dir");
    let root = tempfile::tempdir().expect("root");
    let sentinel = data.join("keep-me.txt");
    std::fs::write(&sentinel, b"keep").expect("sentinel");
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o500)).expect("chmod");
    let src = FakeNoteSource::new();
    src.put("Projects/p.md", "# P\nbody\n");
    let (m, _) = configured(&data, root.path());
    m.refresh_with(&src, BUDGET);
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).expect("restore");
    assert!(m.status().last_error_kind.is_some());
    assert!(sentinel.exists(), "file deleted on permission error");
}

#[test]
fn idx_data_dir_missing_is_created() {
    let base = tempfile::tempdir().expect("tempdir");
    let data = base.path().join("does").join("not").join("exist");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put("Projects/p.md", "# P\nbody\n");
    let (m, _) = configured(&data, root.path());
    m.refresh_with(&src, BUDGET);
    assert!(data.is_dir());
}

#[test]
fn idx_index_manager_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<IndexManager>();
}

// ---------------------------------------------------------------- T0 foundation hardening (DESIGN 3.0 F1-F4)

use crate::managers::vault_index::{ProjectKey, SearchOpts};
use std::thread;
use std::time::Instant;

/// Pause after a background call is started, so the call has passed its generation check and
/// queued its job before the test reconfigures. Timing assumption, not a synchronisation point.
const SETTLE: Duration = Duration::from_millis(200);

/// Writes a real note under `root` (for tests that go through the real `VaultSource`).
fn write_real(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("note has a parent")).expect("mkdir");
    std::fs::write(path, text).expect("write note");
}

/// Holds the single IO worker on a barrier job. Returns once the job has started, plus the
/// sender that releases it and the thread that owns the submission.
fn hold_worker(io: &Arc<VaultIoWorker>) -> (Sender<()>, thread::JoinHandle<()>) {
    let (started_tx, started_rx) = channel::<()>();
    let (release_tx, release_rx) = channel::<()>();
    let io = Arc::clone(io);
    let handle = thread::spawn(move || {
        let job: Box<dyn FnOnce() -> u8 + Send> = Box::new(move || {
            let _ = started_tx.send(());
            let _ = release_rx.recv_timeout(PAUSE_WAIT);
            0u8
        });
        let _ = io.submit(job, Duration::from_secs(60));
    });
    started_rx
        .recv_timeout(PAUSE_WAIT)
        .expect("worker is held by the barrier job");
    (release_tx, handle)
}

fn plain_opts(limit: usize) -> SearchOpts {
    SearchOpts {
        folder: None,
        since_days: None,
        limit,
    }
}

#[test]
fn hard_stale_refresh_is_bound_to_submit_time_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root_a = tempfile::tempdir().expect("root a");
    let root_b = tempfile::tempdir().expect("root b");
    write_real(
        root_a.path(),
        "Projects/alpha-a.md",
        "# Alpha A\nSTALEROW body\n",
    );
    let io = Arc::new(VaultIoWorker::start());
    let m = IndexManager::new(dir.path().to_path_buf(), Arc::clone(&io));
    m.configure(Some(root_a.path().to_path_buf()), vec!["Projects".into()]);
    let (release, holder) = hold_worker(&io);
    let b_src = FakeNoteSource::new();
    b_src.put("Projects/beta-b.md", "# Beta B\nBRANDB body\n");
    thread::scope(|s| {
        // Queues a refresh bound to vault A (root captured at submit), behind the barrier job.
        let queued = s.spawn(|| m.request_refresh(BUDGET));
        thread::sleep(SETTLE);
        m.configure(Some(root_b.path().to_path_buf()), vec!["Projects".into()]);
        m.refresh_with(&b_src, BUDGET);
        release.send(()).expect("release worker");
        queued.join().expect("request_refresh thread");
    });
    holder.join().expect("holder thread");
    let d = m.debug_rows();
    assert!(
        !all_text(&d).contains("STALEROW"),
        "stale refresh of vault A wrote rows into vault B"
    );
    assert!(note_row(&d, "Projects/alpha-a.md").is_none());
    assert!(note_row(&d, "Projects/beta-b.md").is_some());
    assert_eq!(
        m.status().last_error_kind,
        None,
        "stale job must not touch last_error_kind"
    );
}

#[test]
fn hard_stale_refresh_does_not_set_last_error_kind() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root_a = tempfile::tempdir().expect("root a");
    let root_b = tempfile::tempdir().expect("root b");
    write_real(root_a.path(), "Projects/alpha-a.md", "# Alpha A\nbody\n");
    let io = Arc::new(VaultIoWorker::start());
    let m = IndexManager::new(dir.path().to_path_buf(), Arc::clone(&io));
    m.configure(Some(root_a.path().to_path_buf()), vec!["Projects".into()]);
    let (release, holder) = hold_worker(&io);
    let b_src = FakeNoteSource::new();
    b_src.put("Projects/beta-b.md", "# Beta B\nbody\n");
    thread::scope(|s| {
        let queued = s.spawn(|| m.request_refresh(BUDGET));
        thread::sleep(SETTLE);
        m.configure(Some(root_b.path().to_path_buf()), vec!["Projects".into()]);
        m.refresh_with(&b_src, BUDGET);
        // Vault A disappears before the stale job runs: its walk would fail.
        let _ = std::fs::remove_dir_all(root_a.path());
        release.send(()).expect("release worker");
        queued.join().expect("request_refresh thread");
    });
    holder.join().expect("holder thread");
    assert_eq!(
        m.status().last_error_kind,
        None,
        "a stale job for vault A must not set last_error_kind on vault B"
    );
}

#[test]
fn hard_read_finishing_after_reconfigure_is_discarded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root_a = tempfile::tempdir().expect("root a");
    let root_b = tempfile::tempdir().expect("root b");
    write_real(root_a.path(), "Projects/note.md", "# Note\nREADBODY text\n");
    let io = Arc::new(VaultIoWorker::start());
    let m = IndexManager::new(dir.path().to_path_buf(), Arc::clone(&io));
    let gen_a = m.configure(Some(root_a.path().to_path_buf()), vec!["Projects".into()]);
    let (release, holder) = hold_worker(&io);
    let outcome = thread::scope(|s| {
        // The read passes its generation check, then waits behind the barrier job.
        let reader = s.spawn(|| m.read(gen_a, "Projects/note.md", None));
        thread::sleep(SETTLE);
        let gen_b = m.configure(Some(root_b.path().to_path_buf()), vec!["Projects".into()]);
        assert_ne!(gen_a, gen_b, "configure bumps the generation");
        release.send(()).expect("release worker");
        reader.join().expect("read thread")
    });
    holder.join().expect("holder thread");
    match outcome {
        // DESIGN F2: discarded as Unavailable (GenerationMismatch is accepted as Unavailable-style).
        Err(RagError::Unavailable) | Err(RagError::GenerationMismatch) => {}
        Ok(_) => panic!("a read that finished after configure must not return note text"),
        Err(other) => panic!("expected an Unavailable-style error, got {other:?}"),
    }
}

#[test]
fn hard_generation_readable_while_configure_is_busy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let m = manager(dir.path());
    let gen_before = m.generation();
    let (reached_tx, reached_rx) = channel::<()>();
    let (release_tx, release_rx) = channel::<()>();
    let root_path = root.path().to_path_buf();
    let m_ref = &m;
    thread::scope(|s| {
        // configure is held inside its filesystem step by the test hook.
        let busy = s.spawn(move || {
            let mut hook = move || {
                let _ = reached_tx.send(());
                let _ = release_rx.recv_timeout(PAUSE_WAIT);
            };
            m_ref.configure_hooked(Some(root_path), vec!["Projects".into()], &mut hook)
        });
        reached_rx
            .recv_timeout(PAUSE_WAIT)
            .expect("configure reached its filesystem step");
        let (gen_tx, gen_rx) = channel::<u64>();
        let reader = s.spawn(move || {
            let _ = gen_tx.send(m_ref.generation());
        });
        let began = Instant::now();
        let during = gen_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("generation() returned while configure was busy");
        let took = began.elapsed();
        assert!(
            took < Duration::from_millis(200),
            "generation() waited {took:?} behind configure"
        );
        assert!(
            during > gen_before,
            "configure bumps the generation before its filesystem work"
        );
        release_tx.send(()).expect("release configure");
        let after = busy.join().expect("configure thread");
        assert_eq!(after, during);
        reader.join().expect("generation reader");
    });
}

#[test]
fn hard_search_any_relaxed_or() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put("Projects/alpha.md", "# Alpha\nalpha notes about the plan\n");
    let (m, gen) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let query = "notes about alpha and zebra";
    let strict = m
        .search(gen, query, &plain_opts(5))
        .expect("AND search runs");
    assert!(
        strict.is_empty(),
        "AND search needs every term; zebra is missing"
    );
    let relaxed = m
        .search_any(gen, query, &plain_opts(5))
        .expect("OR search runs");
    assert!(
        relaxed.iter().any(|h| h.rel == "Projects/alpha.md"),
        "OR search finds the alpha note"
    );
}

#[test]
fn hard_search_any_sanitises_operators_and_caps_terms() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put("Projects/alpha.md", "# Alpha\nalpha body text\n");
    src.put("Projects/seventh.md", "# Seventh\nseventhword body text\n");
    let (m, gen) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let opts = plain_opts(5);
    for hostile in [
        "\"alpha\" OR NOT (zebra) * ^ NEAR(alpha, 2) \"",
        "AND OR NOT",
        "\"\"\"",
        "*** ((( ",
        "alpha\"; DROP TABLE chunks; --",
        "",
        "   ",
    ] {
        assert!(
            m.search_any(gen, hostile, &opts).is_ok(),
            "hostile query {hostile:?} must not error"
        );
    }
    let last_prefix = m.search_any(gen, "zebra alp", &opts).expect("search runs");
    assert!(
        last_prefix.iter().any(|h| h.rel == "Projects/alpha.md"),
        "the last term gets a prefix star"
    );
    let not_last = m.search_any(gen, "alp zebra", &opts).expect("search runs");
    assert!(not_last.is_empty(), "a non-last term is exact only");
    let six = m
        .search_any(gen, "t1 t2 t3 t4 t5 seventhword", &opts)
        .expect("search runs");
    assert!(
        six.iter().any(|h| h.rel == "Projects/seventh.md"),
        "six terms are all used"
    );
    let seven = m
        .search_any(gen, "t1 t2 t3 t4 t5 t6 seventhword", &opts)
        .expect("search runs");
    assert!(
        seven.is_empty(),
        "terms beyond six are dropped, so the seventh term cannot match"
    );
}

#[test]
fn hard_project_keys() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("root");
    let src = FakeNoteSource::new();
    src.put("Projects/Atlas.md", "# Migration\nAtlas body text\n");
    src.put("People/Sam.md", "# Sam Person\nbody\n");
    src.put("Notes/Loose.md", "# Loose Idea\nbody\n");
    let (m, gen) = configured(dir.path(), root.path());
    m.refresh_with(&src, BUDGET);
    let keys = m.project_keys(gen).expect("project keys");
    let atlas = ProjectKey {
        title: "Migration".into(),
        stem: "Atlas".into(),
        rel: "Projects/Atlas.md".into(),
    };
    assert!(keys.contains(&atlas), "Projects/Atlas.md is a project key");
    assert!(
        keys.iter().all(|k| k.rel.starts_with("Projects/")),
        "only Projects/ notes are keys"
    );
    assert!(
        !keys
            .iter()
            .any(|k| k.rel.starts_with("People/") || k.rel.starts_with("Notes/")),
        "People/ and outside-folder notes are absent"
    );
}

// ---------------------------------------------------------------- S3 direct checks (DESIGN A6 m2, m5b)

/// A configured index refreshed over one allowed note, so queries reach the database.
fn refreshed_index(dir: &Path) -> (IndexManager, u64) {
    let root = dir.join("vault");
    std::fs::create_dir_all(root.join("Projects")).expect("mkdir");
    std::fs::write(root.join("Projects/a.md"), "# A\n\nalpha body\n").expect("write");
    let (m, gen) = configured(dir, &root);
    let folders = vec!["Projects".to_string(), "Daily".to_string()];
    let source = crate::managers::vault::VaultSource::new(root, folders);
    let _report = m.refresh_with(&source, Duration::from_secs(5));
    (m, gen)
}

#[test]
fn index_search_rejects_disallowed_folder_query() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (m, gen) = refreshed_index(dir.path());
    assert!(
        m.search(gen, "alpha", &plain_opts(5)).is_ok(),
        "precondition: an allowed query at the current generation succeeds"
    );
    // People is not an allowed folder: the index itself refuses the scope, whatever the vault holds.
    let refused = m.search(
        gen,
        "alpha",
        &SearchOpts {
            folder: Some("People".into()),
            since_days: None,
            limit: 5,
        },
    );
    assert_eq!(refused, Err(RagError::Forbidden));
}

#[test]
fn index_rejects_stale_generation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (m, gen) = refreshed_index(dir.path());
    assert!(
        m.search(gen, "alpha", &plain_opts(5)).is_ok(),
        "precondition: the current generation is served"
    );
    let stale = gen.wrapping_sub(1);
    assert_eq!(
        m.search(stale, "alpha", &plain_opts(5)),
        Err(RagError::GenerationMismatch),
        "a stale search is refused by the index"
    );
    assert_eq!(
        m.read(stale, "Projects/a.md", None),
        Err(RagError::GenerationMismatch),
        "a stale read is refused by the index"
    );
}
