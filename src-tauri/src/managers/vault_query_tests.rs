//! Index query contracts for the vault search layer, run through the real `IndexManager`,
//! `VaultSource` and `VaultIoWorker` over synthetic on-disk vaults.
//!
//! Covers query sanitation, search ranking and filters, recent listings, note reads (including
//! a wedged IO worker), project status, project-name listing and generation mismatch. The
//! retrieval layer (intent, pre-retrieval, render block, tool calls) is not covered here.
//!
//! Fixtures are synthetic notes only.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::managers::vault::VaultSource;
use crate::managers::vault_index::{IndexManager, RagError, RecentOpts, SearchOpts};
use crate::managers::vault_io::VaultIoWorker;

/// The folders the vault exposes to retrieval. `People` is deliberately absent.
const ALLOWED: &[&str] = &["Projects", "Daily", "Notes"];

/// Fixed sentence used when a project has no explicit status (DESIGN 3.5).
const NO_STATUS_SENTENCE: &str =
    "No explicit status is recorded; these are the most recent log entries.";

/// A built vault: the temp directory must stay alive while the index is used.
struct Fixture {
    /// Keeps the vault and data directories on disk.
    _tmp: tempfile::TempDir,
    /// Index built by a real refresh over the fixture notes.
    index: IndexManager,
    /// Generation after the refresh.
    gen: u64,
}

/// Writes one note with its modification time set `days_ago` days in the past.
fn write_note(root: &Path, rel: &str, body: &str, days_ago: u64) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, body).unwrap();
    let when = SystemTime::now() - Duration::from_secs(days_ago * 86_400 + 3_600);
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

/// Builds a real vault from `(rel, body, days_ago)` triples and refreshes the index over it.
fn build(notes: &[(&str, &str, u64)]) -> Fixture {
    let folders: Vec<String> = ALLOWED.iter().map(|s| s.to_string()).collect();
    build_with(notes, folders)
}

/// Like `build`, but with an explicit allowed-folder list.
fn build_with(notes: &[(&str, &str, u64)], folders: Vec<String>) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("vault");
    fs::create_dir_all(&root).unwrap();
    for (rel, body, days) in notes {
        write_note(&root, rel, body, *days);
    }
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let index = IndexManager::new(data, Arc::new(VaultIoWorker::start()));
    index.configure(Some(root.clone()), folders.clone());
    let source = VaultSource::new(root, folders);
    let _report = index.refresh_with(&source, Duration::from_secs(5));
    let gen = index.generation();
    Fixture {
        _tmp: tmp,
        index,
        gen,
    }
}

/// A standard Atlas project note with a Session Log whose entries are not in file order.
fn atlas_note() -> &'static str {
    "---\ntitle: Atlas\n---\n# Atlas\n\n## Next\nFinish the export pipeline.\n\n## Session Log\n\
     - 2026-09-20 Wrote the parser draft\n- 2026-09-02 Kickoff with the team\n\
     - 2026-09-11 Fixed the export bug\n- 2026-09-05 Reviewed the schema\n\
     - 2026-09-15 Planned the rollout\n- 2026-09-01 Created the repo\n- 2026-09-18 Benchmarked\n"
}

/// Default search options with no filters.
fn plain_search(limit: usize) -> SearchOpts {
    SearchOpts {
        folder: None,
        since_days: None,
        limit,
    }
}

/// Default recent options with no filters.
fn plain_recent(limit: usize) -> RecentOpts {
    RecentOpts {
        folder: None,
        since_days: None,
        limit,
    }
}

// ---------------------------------------------------------------------------------------------
// Query sanitation (3.5)
// ---------------------------------------------------------------------------------------------

#[test]
fn rag_query_operators_and_quotes_never_error_or_inject() {
    let f = build(&[("Notes/plain.md", "# Plain\n\nThe quick brown fox.\n", 1)]);
    let hostile = [
        "NEAR(a b)",
        "a AND b OR c NOT d",
        "\"quoted phrase\"",
        "body:secret",
        "title:x*",
        "*",
        "a\" OR \"b",
        " emoji query ",
        "...!!!???",
        "   ",
        "",
        " launch  plan",
        "Атлас 图表 café  NEAR(x) \"quoted\"",
        " ... ;;; --",
        "\u{1F600} launch \u{1F680}",
        "\u{1F468}\u{200D}\u{1F469} \u{0627}\u{0644} \u{4E2D}\u{6587} \"x\" NEAR(y)",
    ];
    for q in hostile {
        let out = f.index.search(f.gen, q, &plain_search(5));
        assert!(out.is_ok(), "hostile query {q:?} must not error");
    }
    // Only punctuation or nothing yields zero terms, hence an empty result.
    let empty = f
        .index
        .search(f.gen, "...!!!???", &plain_search(5))
        .unwrap();
    assert!(empty.is_empty(), "zero terms must return an empty result");
}

#[test]
fn rag_query_ten_kilobytes_is_capped_and_safe() {
    let f = build(&[("Notes/plain.md", "# Plain\n\nalpha beta gamma.\n", 1)]);
    let huge = "word ".repeat(2_000);
    let out = f.index.search(f.gen, &huge, &plain_search(5));
    assert!(out.is_ok(), "a 10 KB query must not error");
}

#[test]
fn rag_query_max_twelve_terms_and_implicit_and() {
    let f = build(&[
        ("Notes/a.md", "# A\n\nalpha beta\n", 1),
        (
            "Notes/b.md",
            "# B\n\nalpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi\n",
            1,
        ),
    ]);
    // Terms are joined with implicit AND: only the note holding both words matches.
    let both = f
        .index
        .search(f.gen, "alpha gamma", &plain_search(5))
        .unwrap();
    assert!(!both.is_empty(), "AND of two present terms must match");
    assert!(both.iter().all(|h| h.rel == "Notes/b.md"));
}

/// Added in this PR: the test above is named for the twelve-term cap but only checks the AND of
/// two terms, so the cap itself was never pinned. A search sabotaged to keep every term passed.
#[test]
fn rag_query_thirteenth_term_is_ignored_and_twelfth_is_required() {
    let f = build(&[(
        "Notes/b.md",
        "# B\n\nalpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi\n",
        1,
    )]);
    // Twelve words in the note, then a word that is not in it: the cap drops the 13th term, so
    // the note still matches. Without the cap the missing word would be ANDed in and nothing hits.
    let capped = f
        .index
        .search(
            f.gen,
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu zzzabsent",
            &plain_search(5),
        )
        .unwrap();
    assert!(
        !capped.is_empty(),
        "a 13th term beyond the cap must be ignored"
    );
    // The 12th term still counts: an absent word in position twelve means no note matches.
    let strict = f
        .index
        .search(
            f.gen,
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda zzzabsent nu",
            &plain_search(5),
        )
        .unwrap();
    assert!(strict.is_empty(), "the 12th term must still be required");
}

#[test]
fn rag_query_last_term_is_prefix_match() {
    let f = build(&[("Projects/Atlas.md", atlas_note(), 1)]);
    let hits = f.index.search(f.gen, "atl", &plain_search(5)).unwrap();
    assert!(
        hits.iter().any(|h| h.rel == "Projects/Atlas.md"),
        "'atl' must prefix-match Atlas"
    );
}

#[test]
fn rag_query_unicode_terms_match() {
    let f = build(&[
        (
            "Notes/uber.md",
            "# Übersicht\n\nÜbersicht über das Projekt Straße.\n",
            1,
        ),
        (
            "Notes/cyrillic.md",
            "# Атлас\n\nОбзор проекта Атлас для команды.\n",
            1,
        ),
        ("Notes/cjk.md", "# 图表\n\n项目图表与进度说明。\n", 1),
        (
            "Notes/uni.md",
            "# Notizen\n\nÜbersicht über das Café und Straße.\n",
            1,
        ),
    ]);
    let hits = f
        .index
        .search(f.gen, "Übersicht", &plain_search(5))
        .unwrap();
    assert!(
        hits.iter().any(|h| h.rel == "Notes/uber.md"),
        "accented term must match"
    );
    let cyr = f.index.search(f.gen, "Атлас", &plain_search(5)).unwrap();
    assert!(
        cyr.iter().any(|h| h.rel == "Notes/cyrillic.md"),
        "Cyrillic term must match"
    );
    let cjk = f.index.search(f.gen, "图表", &plain_search(5)).unwrap();
    assert!(
        cjk.iter().any(|h| h.rel == "Notes/cjk.md"),
        "CJK term must match"
    );
    let uni = f.index.search(f.gen, "Straße", &plain_search(5)).unwrap();
    assert!(
        uni.iter().any(|h| h.rel == "Notes/uni.md"),
        "unicode term must match"
    );
}

// ---------------------------------------------------------------------------------------------
// Search ranking (3.5)
// ---------------------------------------------------------------------------------------------

#[test]
fn rag_search_title_match_outranks_body_only_match() {
    let f = build(&[
        (
            "Notes/title.md",
            "# Quasar\n\nsome unrelated body text here.\n",
            5,
        ),
        (
            "Notes/body.md",
            "# Other\n\nquasar appears once in the body.\n",
            5,
        ),
    ]);
    let hits = f.index.search(f.gen, "quasar", &plain_search(5)).unwrap();
    assert_eq!(hits.first().map(|h| h.rel.as_str()), Some("Notes/title.md"));
}

#[test]
fn rag_search_newer_wins_on_equal_relevance() {
    let f = build(&[
        ("Notes/old.md", "# Nebula\n\nnebula notes.\n", 300),
        ("Notes/new.md", "# Nebula\n\nnebula notes.\n", 1),
    ]);
    let hits = f.index.search(f.gen, "nebula", &plain_search(5)).unwrap();
    assert_eq!(hits.first().map(|h| h.rel.as_str()), Some("Notes/new.md"));
}

#[test]
fn rag_search_much_more_relevant_old_note_beats_slightly_relevant_new_note() {
    let f = build(&[
        (
            "Notes/old.md",
            "# Pulsar\n\npulsar pulsar pulsar pulsar pulsar pulsar.\n",
            400,
        ),
        (
            "Notes/new.md",
            "# Misc\n\nmentions pulsar in passing among many other words here.\n",
            1,
        ),
    ]);
    let hits = f.index.search(f.gen, "pulsar", &plain_search(5)).unwrap();
    assert_eq!(hits.first().map(|h| h.rel.as_str()), Some("Notes/old.md"));
}

#[test]
fn rag_search_max_two_chunks_per_note() {
    let body =
        "# Comet\n\n## One\ncomet a\n\n## Two\ncomet b\n\n## Three\ncomet c\n\n## Four\ncomet d\n";
    let f = build(&[("Notes/comet.md", body, 1)]);
    let hits = f.index.search(f.gen, "comet", &plain_search(8)).unwrap();
    let for_note = hits.iter().filter(|h| h.rel == "Notes/comet.md").count();
    assert!(for_note <= 2, "at most two chunks per note, got {for_note}");
}

#[test]
fn rag_search_default_k_is_five_and_max_is_eight() {
    let bodies: Vec<String> = (0..12)
        .map(|i| format!("# Doc {i}\n\nzeta token {i}\n"))
        .collect();
    let rels: Vec<String> = (0..12).map(|i| format!("Notes/doc{i:02}.md")).collect();
    let notes: Vec<(&str, &str, u64)> = rels
        .iter()
        .zip(bodies.iter())
        .map(|(rel, body)| (rel.as_str(), body.as_str(), 1))
        .collect();
    let f = build(&notes);
    let defaulted = f.index.search(f.gen, "zeta", &plain_search(0)).unwrap();
    assert!(
        !defaulted.is_empty() && defaulted.len() <= 5,
        "limit 0 must fall back to the default (5) or 1, got {}",
        defaulted.len()
    );
    let capped = f.index.search(f.gen, "zeta", &plain_search(100)).unwrap();
    assert!(
        capped.len() <= 8,
        "limit 100 must clamp to 8, got {}",
        capped.len()
    );
}

#[test]
fn rag_search_future_note_date_clamps_age_to_zero() {
    let f = build(&[("Notes/2099-01-01.md", "# Future\n\nhorizon item.\n", 1)]);
    let hits = f.index.search(f.gen, "horizon", &plain_search(5)).unwrap();
    assert!(!hits.is_empty(), "the future-dated note must be found");
    assert!(
        hits.iter().all(|h| h.score > 0.0),
        "future dates must keep a positive score"
    );
}

#[test]
fn rag_search_month_only_date_is_parsed() {
    let f = build(&[("Notes/2026-08.md", "# August\n\nmonthly review.\n", 1)]);
    let hits = f.index.search(f.gen, "monthly", &plain_search(5)).unwrap();
    assert!(hits
        .iter()
        .any(|h| h.rel == "Notes/2026-08.md" && h.note_ts.is_some()));
}

#[test]
fn rag_search_hit_fields_are_populated() {
    let long = "x".repeat(2_000);
    let body = format!("# Quartz\n\nquartz {long}\n");
    let f = build(&[
        ("Notes/quartz.md", body.as_str(), 2),
        ("Notes/2026-08-15.md", "# Dated\n\nquartz dated entry\n", 2),
    ]);
    let hits = f.index.search(f.gen, "quartz", &plain_search(5)).unwrap();
    let hit = hits
        .iter()
        .find(|h| h.rel == "Notes/quartz.md")
        .expect("a hit for quartz");
    assert!(!hit.heading.is_empty());
    assert!(
        hit.snippet.chars().count() <= 400,
        "snippet must be capped at 400 chars"
    );
    // note_ts comes only from the file name or frontmatter date (design 3.4); this note has neither.
    assert!(
        hit.note_ts.is_none(),
        "an undated note must have no note_ts"
    );
    assert!(hit.mtime_ms > 0);
    assert!(hit.score > 0.0, "score must be positive");
    let dated = hits
        .iter()
        .find(|h| h.rel == "Notes/2026-08-15.md")
        .expect("a hit for the dated note");
    assert!(
        dated.note_ts.is_some(),
        "a note dated by file name must carry note_ts"
    );
}

// ---------------------------------------------------------------------------------------------
// Filters (3.5)
// ---------------------------------------------------------------------------------------------

#[test]
fn rag_search_is_diacritic_insensitive() {
    let f = build(&[("Notes/cafe.md", "# Café\n\nAt the café we planned it.\n", 1)]);
    let hits = f.index.search(f.gen, "cafe", &plain_search(5)).unwrap();
    assert!(
        hits.iter().any(|h| h.rel == "Notes/cafe.md"),
        "unaccented 'cafe' must find 'café' (unicode61 remove_diacritics 2)"
    );
}

#[test]
fn rag_search_folder_filter_is_component_boundary() {
    let f = build(&[
        ("Projects/a.md", "# A\n\nfilterterm here\n", 1),
        ("Projects/x/y.md", "# Y\n\nfilterterm here\n", 1),
        ("Projects-old/a.md", "# Old\n\nfilterterm here\n", 1),
        ("ProjectsX/a.md", "# X\n\nfilterterm here\n", 1),
    ]);
    let opts = SearchOpts {
        folder: Some("Projects".into()),
        since_days: None,
        limit: 8,
    };
    let hits = f.index.search(f.gen, "filterterm", &opts).unwrap();
    let rels: Vec<&str> = hits.iter().map(|h| h.rel.as_str()).collect();
    assert!(rels.contains(&"Projects/a.md"));
    assert!(rels.contains(&"Projects/x/y.md"));
    assert!(
        !rels.iter().any(|r| r.starts_with("Projects-old")),
        "Projects-old must not match"
    );
    assert!(
        !rels.iter().any(|r| r.starts_with("ProjectsX")),
        "ProjectsX must not match"
    );
}

#[test]
fn rag_search_like_metacharacters_in_folder_are_literal() {
    let folders: Vec<String> = ["Projects", "My_Notes", "MyXNotes", "Daily"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let f = build_with(
        &[
            ("My_Notes/a.md", "# A\n\nliketerm here\n", 1),
            ("MyXNotes/b.md", "# B\n\nliketerm here\n", 1),
        ],
        folders,
    );
    // An unescaped `_` would also match `MyXNotes`; only `My_Notes` hits may come back.
    let opts = SearchOpts {
        folder: Some("My_Notes".into()),
        since_days: None,
        limit: 8,
    };
    let hits = f.index.search(f.gen, "liketerm", &opts).unwrap();
    assert!(!hits.is_empty(), "My_Notes must match its own note");
    assert!(
        hits.iter().all(|h| h.rel.starts_with("My_Notes/")),
        "_ must be literal, not a wildcard"
    );
    // `%` is not an allowed folder, so it must be rejected as Forbidden, never as a wildcard.
    let pct = SearchOpts {
        folder: Some("Pro%".into()),
        since_days: None,
        limit: 8,
    };
    assert_eq!(
        f.index.search(f.gen, "liketerm", &pct),
        Err(RagError::Forbidden)
    );
}

#[test]
fn rag_search_folder_outside_allowed_is_forbidden() {
    let f = build(&[("People/alice.md", "# Alice\n\nsecretterm here\n", 1)]);
    let opts = SearchOpts {
        folder: Some("People".into()),
        since_days: None,
        limit: 5,
    };
    assert_eq!(
        f.index.search(f.gen, "secretterm", &opts),
        Err(RagError::Forbidden)
    );
}

#[test]
fn rag_search_invalid_folder_is_invalid() {
    let f = build(&[("Projects/a.md", "# A\n\nbody\n", 1)]);
    for bad in ["../etc", ".hidden", "/absolute/path", "Projects/../People"] {
        let opts = SearchOpts {
            folder: Some(bad.into()),
            since_days: None,
            limit: 5,
        };
        assert_eq!(
            f.index.search(f.gen, "body", &opts),
            Err(RagError::Invalid),
            "folder {bad:?}"
        );
    }
}

#[test]
fn rag_search_since_days_boundary() {
    let f = build(&[
        ("Notes/exact.md", "# Exact\n\nboundaryterm one\n", 10),
        ("Notes/over.md", "# Over\n\nboundaryterm two\n", 11),
    ]);
    let opts = SearchOpts {
        folder: None,
        since_days: Some(10),
        limit: 8,
    };
    let hits = f.index.search(f.gen, "boundaryterm", &opts).unwrap();
    let rels: Vec<&str> = hits.iter().map(|h| h.rel.as_str()).collect();
    assert!(
        rels.contains(&"Notes/exact.md"),
        "a note exactly 10 days old is included"
    );
    assert!(
        !rels.contains(&"Notes/over.md"),
        "a note 11 days old is excluded"
    );
}

// ---------------------------------------------------------------------------------------------
// Recent (3.5)
// ---------------------------------------------------------------------------------------------

#[test]
fn rag_recent_orders_by_recency_and_clamps_limit() {
    let bodies: Vec<String> = (0..12)
        .map(|i| format!("# Entry {i}\n\nbody {i}\n"))
        .collect();
    let rels: Vec<String> = (0..12).map(|i| format!("Notes/e{i:02}.md")).collect();
    let notes: Vec<(&str, &str, u64)> = rels
        .iter()
        .zip(bodies.iter())
        .enumerate()
        .map(|(i, (rel, body))| (rel.as_str(), body.as_str(), i as u64 + 1))
        .collect();
    let f = build(&notes);
    let out = f.index.recent(f.gen, &plain_recent(0)).unwrap();
    assert!(
        !out.is_empty() && out.len() <= 5,
        "limit 0 falls back to the default of 5"
    );
    let capped = f.index.recent(f.gen, &plain_recent(50)).unwrap();
    assert!(capped.len() <= 10, "limit must clamp to 10");
    for pair in capped.windows(2) {
        assert!(
            pair[0].mtime_ms >= pair[1].mtime_ms,
            "recent must be newest first"
        );
    }
}

#[test]
fn rag_recent_title_and_first_lines_are_capped() {
    let long = "y".repeat(1_000);
    let body = format!("# Long\n\n{long}\n");
    let f = build(&[("Notes/long.md", body.as_str(), 1)]);
    let out = f.index.recent(f.gen, &plain_recent(5)).unwrap();
    let note = out
        .iter()
        .find(|n| n.rel == "Notes/long.md")
        .expect("long note listed");
    assert!(note.first_lines.chars().count() <= 200);
    assert!(note.title.chars().count() <= 200);
}

#[test]
fn rag_recent_skipped_sensitive_and_people_never_appear() {
    let key = concat!("xox", "b-", "1234567890-abcdefghij");
    let secret_body = format!("# Keys\n\nthe token is {key}\n");
    let f = build(&[
        ("Notes/keys.md", secret_body.as_str(), 1),
        ("Notes/ok.md", "# Ok\n\nnormal body\n", 1),
        ("People/bob.md", "# Bob\n\nhidden person note\n", 1),
    ]);
    let out = f.index.recent(f.gen, &plain_recent(10)).unwrap();
    let rels: Vec<&str> = out.iter().map(|n| n.rel.as_str()).collect();
    assert!(
        !rels.iter().any(|r| r.starts_with("People/")),
        "People must never appear"
    );
    assert!(
        out.iter()
            .all(|n| !n.first_lines.contains(key) && !n.title.contains(key)),
        "a sensitive note must never leak its credential"
    );
}

#[test]
fn rag_recent_folder_and_since_filters() {
    let f = build(&[
        ("Projects/p.md", "# P\n\nbody\n", 1),
        ("Notes/n.md", "# N\n\nbody\n", 40),
    ]);
    let by_folder = f
        .index
        .recent(
            f.gen,
            &RecentOpts {
                folder: Some("Notes".into()),
                since_days: None,
                limit: 10,
            },
        )
        .unwrap();
    assert!(by_folder.iter().all(|n| n.rel.starts_with("Notes/")));
    let by_since = f
        .index
        .recent(
            f.gen,
            &RecentOpts {
                folder: None,
                since_days: Some(30),
                limit: 10,
            },
        )
        .unwrap();
    assert!(
        !by_since.iter().any(|n| n.rel == "Notes/n.md"),
        "a 40-day-old note is outside 30 days"
    );
}

// ---------------------------------------------------------------------------------------------
// Read (3.5)
// ---------------------------------------------------------------------------------------------

#[test]
fn rag_read_returns_redacted_text_for_allowed_note() {
    let key = concat!("gh", "p_", "abcdefghijklmnopqrstuvwxyz0123456789");
    let body = format!("# Plan\n\nkeep this {key} out\n");
    let f = build(&[("Notes/plan.md", body.as_str(), 1)]);
    let out = f.index.read(f.gen, "Notes/plan.md", None).unwrap();
    assert!(
        !out.text.contains(key),
        "secrets must be redacted from read output"
    );
    assert!(!out.truncated);
}

#[test]
fn rag_read_errors_for_forbidden_missing_and_invalid() {
    let f = build(&[("People/alice.md", "# Alice\n\nbody\n", 1)]);
    assert_eq!(
        f.index.read(f.gen, "People/alice.md", None).err(),
        Some(RagError::Forbidden)
    );
    assert_eq!(
        f.index.read(f.gen, "Notes/missing.md", None).err(),
        Some(RagError::NotFound)
    );
    assert_eq!(
        f.index.read(f.gen, "../escape.md", None).err(),
        Some(RagError::Invalid)
    );
    assert_eq!(
        f.index.read(f.gen, ".hidden/x.md", None).err(),
        Some(RagError::Invalid)
    );
}

#[test]
fn rag_read_heading_selects_section_case_insensitively() {
    let body = "# Doc\n\n## Alpha\nalpha body\n\n### Sub\nsub body\n\n## Beta\nbeta body\n";
    let f = build(&[("Notes/sections.md", body, 1)]);
    let out = f
        .index
        .read(f.gen, "Notes/sections.md", Some("alpha"))
        .unwrap();
    assert!(out.text.contains("alpha body"));
    assert!(
        out.text.contains("sub body"),
        "the section includes deeper sub-headings"
    );
    assert!(
        !out.text.contains("beta body"),
        "the section ends at the next same-level heading"
    );
}

#[test]
fn rag_read_unknown_heading_is_not_found() {
    let f = build(&[("Notes/sections.md", "# Doc\n\n## Alpha\nbody\n", 1)]);
    assert_eq!(
        f.index
            .read(f.gen, "Notes/sections.md", Some("No Such Heading"))
            .err(),
        Some(RagError::NotFound)
    );
}

#[test]
fn rag_read_output_capped_at_6000_chars_and_truncated() {
    let long = "z".repeat(20_000);
    let body = format!("# Big\n\n{long}\n");
    let f = build(&[("Notes/big.md", body.as_str(), 1)]);
    let out = f.index.read(f.gen, "Notes/big.md", None).unwrap();
    assert!(
        out.text.chars().count() <= 6_000,
        "read output must be capped at 6000 chars"
    );
    assert!(out.truncated, "a capped read must set truncated=true");
}

#[test]
fn rag_read_removes_control_chars() {
    let body = "# Ctl\n\nvisible\u{7}bell\u{1b}[31m escape\n";
    let f = build(&[("Notes/ctl.md", body, 1)]);
    let out = f.index.read(f.gen, "Notes/ctl.md", None).unwrap();
    assert!(!out
        .text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t'));
}

#[test]
fn rag_read_wedged_worker_is_unavailable_quickly() {
    let f = build(&[("Notes/plan.md", "# Plan\n\nbody\n", 1)]);
    let worker = Arc::new(VaultIoWorker::start());
    // Hold the worker with a job that runs past the 10 s wedge threshold.
    let held = Arc::clone(&worker);
    let _holder = std::thread::spawn(move || {
        let _ = held.submit(
            Box::new(|| std::thread::sleep(Duration::from_secs(12))),
            Duration::from_secs(15),
        );
    });
    std::thread::sleep(Duration::from_millis(200));
    let index = IndexManager::new(f._tmp.path().join("data2"), worker);
    let began = std::time::Instant::now();
    let out = index.read(f.gen, "Notes/plan.md", None);
    assert_eq!(out.err(), Some(RagError::Unavailable));
    assert!(
        began.elapsed() < Duration::from_secs(2),
        "a wedged worker must fail quickly"
    );
}

// ---------------------------------------------------------------------------------------------
// Project status (3.5)
// ---------------------------------------------------------------------------------------------

#[test]
fn rag_project_status_exact_stem_match_is_case_insensitive() {
    let f = build(&[("Projects/Work/atlas.md", atlas_note(), 2)]);
    let st = f.index.project_status(f.gen, "ATLAS").unwrap();
    assert_eq!(st.rel, "Projects/Work/atlas.md");
    assert!(!st.ambiguous);
}

#[test]
fn rag_project_status_same_stem_in_two_folders_is_ambiguous() {
    let f = build(&[
        ("Projects/One/atlas.md", atlas_note(), 2),
        ("Projects/Two/atlas.md", atlas_note(), 2),
    ]);
    let st = f.index.project_status(f.gen, "atlas").unwrap();
    assert!(st.ambiguous);
    assert!(st.candidates.len() >= 2 && st.candidates.len() <= 4);
    assert!(
        st.entries.is_empty(),
        "an ambiguous status carries no entries"
    );
}

#[test]
fn rag_project_status_title_fts_unique_best_is_chosen() {
    let f = build(&[
        (
            "Projects/Helios.md",
            "# Helios Program\n\nHelios helios helios rollout.\n",
            2,
        ),
        (
            "Projects/Other.md",
            "# Unrelated\n\nnothing to see here.\n",
            2,
        ),
    ]);
    let st = f.index.project_status(f.gen, "helios program").unwrap();
    assert!(
        !st.ambiguous,
        "a clearly dominant title must not be ambiguous"
    );
    assert_eq!(st.rel, "Projects/Helios.md");
}

#[test]
fn rag_project_status_near_identical_titles_are_ambiguous() {
    let f = build(&[
        (
            "Projects/Gemini Alpha.md",
            "# Gemini Shared Work\n\nshared shared shared.\n",
            2,
        ),
        (
            "Projects/Gemini Beta.md",
            "# Gemini Shared Work\n\nshared shared shared.\n",
            2,
        ),
    ]);
    let st = f.index.project_status(f.gen, "gemini shared work").unwrap();
    assert!(st.ambiguous, "second >= 0.9 * first must be ambiguous");
    assert!(st.candidates.len() <= 4);
}

#[test]
fn rag_project_status_unknown_project_is_not_found() {
    let f = build(&[("Projects/Atlas.md", atlas_note(), 2)]);
    assert_eq!(
        f.index.project_status(f.gen, "Nonexistent Quark").err(),
        Some(RagError::NotFound)
    );
}

#[test]
fn rag_project_status_last_five_entries_newest_by_date() {
    let f = build(&[("Projects/Atlas.md", atlas_note(), 2)]);
    let st = f.index.project_status(f.gen, "Atlas").unwrap();
    assert_eq!(st.entries.len(), 5, "exactly the last five entries");
    // Newest by parsed date, not file order: 09-20 is newest and must be present.
    assert!(st.entries.iter().any(|e| e.contains("2026-09-20")));
    assert!(
        !st.entries.iter().any(|e| e.contains("2026-09-01")),
        "the oldest entry is dropped"
    );
    assert!(st.entries.iter().all(|e| e.chars().count() <= 400));
}

#[test]
fn rag_project_status_next_text_is_capped() {
    let long = "n".repeat(2_000);
    let body = format!("# Atlas\n\n## Next\n{long}\n\n## Session Log\n- 2026-09-01 start\n");
    let f = build(&[("Projects/Atlas.md", body.as_str(), 2)]);
    let st = f.index.project_status(f.gen, "Atlas").unwrap();
    let next = st.next_text.expect("Next section present");
    assert!(next.chars().count() <= 600);
}

#[test]
fn rag_project_status_daily_snippets_last_fourteen_days_only() {
    let f = build(&[
        ("Projects/Atlas.md", atlas_note(), 2),
        ("Daily/recent.md", "# Day\n\nworked on Atlas today\n", 3),
        ("Daily/old.md", "# Day\n\nworked on Atlas long ago\n", 15),
    ]);
    let st = f.index.project_status(f.gen, "Atlas").unwrap();
    assert!(st.daily_snippets.len() <= 3);
    assert!(
        st.daily_snippets.iter().all(|s| !s.contains("long ago")),
        "a 15-day-old note is excluded"
    );
}

#[test]
fn rag_project_status_explicit_frontmatter_status_is_used() {
    let body = "---\nstatus: on track\n---\n# Atlas\n\n## Session Log\n- 2026-09-01 start\n";
    let f = build(&[("Projects/Atlas.md", body, 2)]);
    let st = f.index.project_status(f.gen, "Atlas").unwrap();
    assert!(st.explicit_status);
    assert_eq!(st.status.as_deref(), Some("on track"));
}

#[test]
fn rag_project_status_without_status_uses_fixed_sentence() {
    let f = build(&[("Projects/Atlas.md", atlas_note(), 2)]);
    let st = f.index.project_status(f.gen, "Atlas").unwrap();
    assert!(!st.explicit_status);
    assert!(
        st.text.contains(NO_STATUS_SENTENCE),
        "missing status must use the exact fixed sentence"
    );
}

#[test]
fn rag_project_status_text_is_capped_at_5000() {
    let entries: String = (1..=5)
        .map(|i| format!("- 2026-09-0{i} {}\n", "e".repeat(380)))
        .collect();
    let body = format!(
        "# Atlas\n\n## Next\n{}\n\n## Session Log\n{entries}",
        "n".repeat(600)
    );
    let f = build(&[("Projects/Atlas.md", body.as_str(), 2)]);
    let st = f.index.project_status(f.gen, "Atlas").unwrap();
    assert!(st.text.chars().count() <= 5_000);
}

#[test]
fn rag_project_status_without_session_log_still_returns_title() {
    let f = build(&[("Projects/Bare.md", "# Bare Project\n\nno log here\n", 2)]);
    let st = f.index.project_status(f.gen, "Bare").unwrap();
    assert!(st.entries.is_empty());
    assert!(!st.title.is_empty());
    assert!(st.modified_ms > 0);
}

#[test]
fn rag_project_status_redacts_secrets() {
    let key = concat!("AK", "IA", "ABCDEFGHIJKLMNOP");
    let body = format!("# Atlas\n\n## Session Log\n- 2026-09-01 used {key} by mistake\n");
    let f = build(&[("Projects/Atlas.md", body.as_str(), 2)]);
    let st = f.index.project_status(f.gen, "Atlas").unwrap();
    assert!(!st.text.contains(key));
    assert!(st.entries.iter().all(|e| !e.contains(key)));
}

// ---------------------------------------------------------------------------------------------
// Project names (3.5)
// ---------------------------------------------------------------------------------------------

#[test]
fn rag_project_names_titles_deduped_sorted_and_min_length() {
    let f = build(&[
        ("Projects/Zeta.md", "# Zeta\n\nbody\n", 2),
        ("Projects/Alpha.md", "# Alpha\n\nbody\n", 2),
        ("Projects/Alpha copy.md", "# Alpha\n\nbody\n", 2),
        ("Projects/Go.md", "# Go\n\nbody\n", 2),
        ("Notes/Ignored.md", "# Ignored\n\nbody\n", 2),
    ]);
    let names = f.index.project_names(f.gen).unwrap();
    assert!(names.len() <= 500);
    assert!(
        !names.iter().any(|n| n.chars().count() < 3),
        "names shorter than 3 chars are dropped"
    );
    assert!(
        !names.contains(&"Ignored".to_string()),
        "only Projects/ notes count"
    );
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "names must be sorted");
    let mut dedup = names.clone();
    dedup.dedup();
    assert_eq!(names, dedup, "names must be deduplicated");
}

// ---------------------------------------------------------------------------------------------
// Generation (3.3)
// ---------------------------------------------------------------------------------------------

#[test]
fn rag_stale_generation_is_generation_mismatch() {
    let f = build(&[("Notes/a.md", "# A\n\nbody\n", 1)]);
    let stale = f.gen.wrapping_sub(1);
    assert_eq!(
        f.index.search(stale, "body", &plain_search(5)),
        Err(RagError::GenerationMismatch)
    );
    assert_eq!(
        f.index.recent(stale, &plain_recent(5)),
        Err(RagError::GenerationMismatch)
    );
    assert_eq!(
        f.index.project_names(stale),
        Err(RagError::GenerationMismatch)
    );
    assert_eq!(
        f.index.read(stale, "Notes/a.md", None).err(),
        Some(RagError::GenerationMismatch)
    );
    assert_eq!(
        f.index.project_status(stale, "Atlas").err(),
        Some(RagError::GenerationMismatch)
    );
}

#[test]
fn rag_queries_before_any_refresh_return_empty_not_error() {
    let tmp = tempfile::tempdir().unwrap();
    let index = IndexManager::new(tmp.path().to_path_buf(), Arc::new(VaultIoWorker::start()));
    let gen = index.generation();
    assert_eq!(
        index.search(gen, "anything", &plain_search(5)),
        Ok(Vec::new())
    );
    assert_eq!(index.recent(gen, &plain_recent(5)), Ok(Vec::new()));
}
