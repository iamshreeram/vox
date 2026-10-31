//! Tests for the pure Tamil STT core: pack status, language head, CTC greedy decode,
//! chunking and the engine (with a fake backend and a fake feature source).

use crate::managers::indic_stt_core::*;
use crate::managers::indic_stt_mel::{MelFeatures, MelFrontend, N_MELS};
use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const SIZES: [(&str, u64); 4] = [
    ("encoder.onnx", 652_623_111),
    ("ctc_decoder.onnx", 23_095_918),
    ("vocab.json", 96_743),
    ("language_masks.json", 862_011),
];

fn sparse(dir: &Path, name: &str, size: u64) {
    let file = fs::File::create(dir.join(name)).expect("create sparse file");
    file.set_len(size).expect("set_len");
}

fn all_ready(dir: &Path) {
    for (name, size) in SIZES {
        sparse(dir, name, size);
    }
}

// ---------- pack layout and status ----------

#[test]
fn pack_dir_is_under_aux_models() {
    let dir = pack_dir(Path::new("/data/app"));
    assert_eq!(dir, Path::new("/data/app/aux_models/indic_conformer_int8"));
    assert_eq!(PACK_DIR_NAME, "indic_conformer_int8");
}

#[test]
fn pack_status_ready_with_exact_sizes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    all_ready(tmp.path());
    assert_eq!(indic_pack_status(tmp.path()), IndicPackStatus::Ready);
}

#[test]
fn pack_status_missing_lists_all_in_stable_order() {
    let tmp = tempfile::tempdir().expect("tempdir");
    assert_eq!(
        indic_pack_status(tmp.path()),
        IndicPackStatus::Missing(vec![
            "encoder.onnx".to_string(),
            "ctc_decoder.onnx".to_string(),
            "vocab.json".to_string(),
            "language_masks.json".to_string(),
        ])
    );
}

#[test]
fn pack_status_missing_dir_itself_counts_as_missing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let absent = tmp.path().join("not-there");
    match indic_pack_status(&absent) {
        IndicPackStatus::Missing(names) => assert_eq!(names.len(), 4),
        other => panic!("expected Missing, got {other:?}"),
    }
}

#[test]
fn pack_status_partial_reports_only_absent_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    sparse(tmp.path(), "encoder.onnx", 652_623_111);
    sparse(tmp.path(), "vocab.json", 96_743);
    assert_eq!(
        indic_pack_status(tmp.path()),
        IndicPackStatus::Missing(vec![
            "ctc_decoder.onnx".to_string(),
            "language_masks.json".to_string(),
        ])
    );
}

#[test]
fn pack_status_wrong_size_names_file_and_sizes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    all_ready(tmp.path());
    sparse(tmp.path(), "vocab.json", 96_742);
    assert_eq!(
        indic_pack_status(tmp.path()),
        IndicPackStatus::WrongSize {
            file: "vocab.json".to_string(),
            expected: 96_743,
            actual: 96_742,
        }
    );
}

#[test]
fn pack_status_directory_in_place_of_file_is_missing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    all_ready(tmp.path());
    fs::remove_file(tmp.path().join("language_masks.json")).expect("remove");
    fs::create_dir(tmp.path().join("language_masks.json")).expect("mkdir");
    assert_eq!(
        indic_pack_status(tmp.path()),
        IndicPackStatus::Missing(vec!["language_masks.json".to_string()])
    );
}

#[test]
fn pack_status_missing_is_reported_before_wrong_size() {
    let tmp = tempfile::tempdir().expect("tempdir");
    sparse(tmp.path(), "encoder.onnx", 1);
    sparse(tmp.path(), "vocab.json", 96_743);
    sparse(tmp.path(), "language_masks.json", 862_011);
    assert_eq!(
        indic_pack_status(tmp.path()),
        IndicPackStatus::Missing(vec!["ctc_decoder.onnx".to_string()])
    );
}

// ---------- language head ----------

const VOCAB_JSON: &str = r#"{"ta": ["a", "b", "<blk>"], "hi": ["x"]}"#;
const MASKS_JSON: &str = r#"{"ta": [true, false, true, false, true], "hi": [false, true]}"#;

#[test]
fn parse_lang_head_picks_masked_columns_in_order() {
    let head = parse_lang_head(VOCAB_JSON, MASKS_JSON, "ta").expect("head");
    assert_eq!(head.vocab, vec!["a", "b", "<blk>"]);
    assert_eq!(head.columns, vec![0, 2, 4]);
    assert_eq!(head.blank, 2);
}

#[test]
fn parse_lang_head_mask_length_mismatch_is_err() {
    let masks = r#"{"ta": [true, true, false]}"#;
    assert!(parse_lang_head(VOCAB_JSON, masks, "ta").is_err());
}

#[test]
fn parse_lang_head_unknown_lang_names_the_lang() {
    let err = parse_lang_head(VOCAB_JSON, MASKS_JSON, "xx").expect_err("unknown");
    assert!(err.contains("xx"), "error should name the lang: {err}");
}

#[test]
fn parse_lang_head_rejects_empty_vocab_and_bad_json() {
    assert!(parse_lang_head(r#"{"ta": []}"#, r#"{"ta": []}"#, "ta").is_err());
    assert!(parse_lang_head("not json", MASKS_JSON, "ta").is_err());
}

#[test]
fn multilingual_validation_requires_blank_256() {
    let small = parse_lang_head(VOCAB_JSON, MASKS_JSON, "ta").expect("head");
    assert!(validate_multilingual_head(&small).is_err());

    let vocab: Vec<String> = (0..257).map(|i| format!("p{i}")).collect();
    let head = LangHead {
        vocab,
        columns: (0..257).collect(),
        blank: 256,
    };
    assert!(validate_multilingual_head(&head).is_ok());
}

// ---------- CTC greedy decode ----------

/// Masked head: columns [1,3,4,6] over a width-7 row. Masked index 3 is the blank.
fn test_head() -> LangHead {
    LangHead {
        vocab: vec![
            "a".to_string(),
            "b".to_string(),
            "\u{2581}c".to_string(),
            "<blk>".to_string(),
        ],
        columns: vec![1, 3, 4, 6],
        blank: 3,
    }
}

const WIDTH: usize = 7;

/// Build logits whose argmax over the masked head is `picks[step]`. Column 0 is outside the
/// mask and is set very high so a decoder that ignores the mask would fail.
fn logits_for(head: &LangHead, picks: &[usize]) -> IndicLogits {
    let mut logprobs = Vec::with_capacity(picks.len() * WIDTH);
    for &pick in picks {
        let mut row = vec![-5.0_f32; WIDTH];
        row[0] = 9.0;
        row[head.columns[pick]] = -0.1;
        logprobs.extend(row);
    }
    IndicLogits {
        logprobs,
        steps: picks.len(),
        width: WIDTH,
    }
}

#[test]
fn ctc_collapses_only_consecutive_identical_frames() {
    let head = test_head();
    let logits = logits_for(&head, &[0, 0, 1]);
    assert_eq!(ctc_greedy(&logits, &head).expect("decode"), "ab");
}

#[test]
fn ctc_blank_between_identical_tokens_keeps_both() {
    let head = test_head();
    let logits = logits_for(&head, &[0, 3, 0]);
    assert_eq!(ctc_greedy(&logits, &head).expect("decode"), "aa");
}

#[test]
fn ctc_leading_and_trailing_blanks_are_dropped() {
    let head = test_head();
    let logits = logits_for(&head, &[3, 3, 0, 0, 3, 3]);
    assert_eq!(ctc_greedy(&logits, &head).expect("decode"), "a");
}

#[test]
fn ctc_maps_sentencepiece_space_and_trims() {
    let head = test_head();
    // "\u{2581}c" at the start is trimmed; in the middle it becomes a space.
    let leading = logits_for(&head, &[2, 2, 1, 3]);
    assert_eq!(ctc_greedy(&leading, &head).expect("decode"), "cb");
    let middle = logits_for(&head, &[0, 2, 1]);
    assert_eq!(ctc_greedy(&middle, &head).expect("decode"), "a cb");
}

#[test]
fn ctc_all_blank_is_empty() {
    let head = test_head();
    let logits = logits_for(&head, &[3, 3, 3]);
    assert_eq!(ctc_greedy(&logits, &head).expect("decode"), "");
    let none = IndicLogits {
        logprobs: vec![],
        steps: 0,
        width: WIDTH,
    };
    assert_eq!(ctc_greedy(&none, &head).expect("decode"), "");
}

#[test]
fn ctc_ties_pick_the_first_masked_column() {
    let head = test_head();
    // Masked 0 (column 1) and masked 1 (column 3) tie; the first one wins.
    let mut row = vec![-5.0_f32; WIDTH];
    row[1] = -0.5;
    row[3] = -0.5;
    let logits = IndicLogits {
        logprobs: row,
        steps: 1,
        width: WIDTH,
    };
    assert_eq!(ctc_greedy(&logits, &head).expect("decode"), "a");
}

#[test]
fn ctc_bad_shapes_are_err_not_panic() {
    let head = test_head();
    let short = IndicLogits {
        logprobs: vec![0.0; WIDTH - 1],
        steps: 1,
        width: WIDTH,
    };
    assert!(ctc_greedy(&short, &head).is_err());

    let narrow = IndicLogits {
        logprobs: vec![0.0; 6],
        steps: 1,
        width: 6,
    };
    assert!(ctc_greedy(&narrow, &head).is_err(), "column 6 >= width 6");

    let overflow = IndicLogits {
        logprobs: vec![],
        steps: usize::MAX,
        width: 2,
    };
    assert!(ctc_greedy(&overflow, &head).is_err());
}

#[test]
fn ctc_nan_log_prob_is_err() {
    let head = test_head();
    let mut logits = logits_for(&head, &[0, 1]);
    logits.logprobs[WIDTH + 3] = f32::NAN;
    assert!(ctc_greedy(&logits, &head).is_err());
}

#[test]
fn ctc_unmasked_columns_are_ignored() {
    let head = test_head();
    // Column 0 (outside the mask) is 9.0 in every row; the decode still uses the mask.
    let logits = logits_for(&head, &[1]);
    assert_eq!(ctc_greedy(&logits, &head).expect("decode"), "b");
}

// ---------- chunking ----------

/// Deterministic LCG noise in [-1, 1).
struct Lcg(u64);

impl Lcg {
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }

    fn unit(&mut self) -> f32 {
        (self.next_u32() as f32 / (1u32 << 31) as f32) - 1.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next_u32() as usize) % bound
    }
}

fn noise(len: usize, seed: u64) -> Vec<f32> {
    let mut rng = Lcg(seed);
    (0..len).map(|_| rng.unit() * 0.5).collect()
}

fn assert_partition(len: usize, chunks: &[std::ops::Range<usize>]) {
    if len == 0 {
        assert!(chunks.is_empty());
        return;
    }
    assert_eq!(chunks.first().expect("first").start, 0);
    assert_eq!(chunks.last().expect("last").end, len);
    for pair in chunks.windows(2) {
        assert_eq!(pair[0].end, pair[1].start, "chunks must be contiguous");
    }
    for chunk in chunks {
        assert!(!chunk.is_empty(), "chunk must be non-empty");
        assert!(chunk.len() <= MAX_CHUNK, "chunk over the cap: {chunk:?}");
    }
    if len > MAX_CHUNK {
        assert!(
            chunks.last().expect("last").len() >= MIN_TAIL,
            "final chunk too short for len {len}"
        );
    }
}

#[test]
fn chunk_constants_match_the_design() {
    assert_eq!(MAX_CHUNK, 480_000);
    assert_eq!(MIN_TAIL, 16_000);
    assert_eq!(SEARCH, 96_000);
    assert_eq!(WIN, 1_600);
    assert_eq!(HOP, 800);
}

#[test]
fn chunk_boundary_lengths_partition_exactly() {
    for len in [
        0usize, 1, 1599, 1600, 480_000, 480_001, 496_000, 496_001, 960_000, 960_001, 1_000_000,
    ] {
        let samples = noise(len, len as u64 + 7);
        let chunks = split_chunks(&samples);
        assert_partition(len, &chunks);
    }
}

#[test]
fn chunk_zero_length_is_empty() {
    assert!(split_chunks(&[]).is_empty());
}

#[test]
fn chunk_thirty_seconds_exactly_is_one_chunk() {
    let samples = noise(MAX_CHUNK, 3);
    assert_eq!(split_chunks(&samples), vec![0..MAX_CHUNK]);
    let samples = noise(MAX_CHUNK + 1, 3);
    assert_eq!(split_chunks(&samples).len(), 2);
}

#[test]
fn chunk_random_lengths_partition_exactly() {
    let mut rng = Lcg(0x5eed);
    for round in 0..40u64 {
        let len = rng.below(1_100_000);
        let samples = noise(len, round + 11);
        let chunks = split_chunks(&samples);
        assert_partition(len, &chunks);
    }
}

#[test]
fn chunk_cut_lands_on_planted_silence() {
    let len = 1_000_000;
    let mut samples = noise(len, 99);
    for sample in &mut samples[400_000..402_000] {
        *sample = 0.0;
    }
    let chunks = split_chunks(&samples);
    assert_partition(len, &chunks);
    // Candidates step down from 480000 by 800; 401600 is the silent window's on-grid cut.
    assert_eq!(chunks[0].end, 401_600);
}

#[test]
fn chunk_quietest_at_upper_bound_is_chosen() {
    let len = 1_000_000;
    let mut samples = noise(len, 42);
    for sample in &mut samples[MAX_CHUNK - WIN..MAX_CHUNK] {
        *sample = 0.0;
    }
    let chunks = split_chunks(&samples);
    assert_partition(len, &chunks);
    assert_eq!(chunks[0].end, MAX_CHUNK);
}

#[test]
fn chunk_all_silent_ties_pick_the_latest_cut() {
    let samples = vec![0.0_f32; 1_000_000];
    let chunks = split_chunks(&samples);
    assert_partition(1_000_000, &chunks);
    assert_eq!(chunks[0].end, MAX_CHUNK);
}

#[test]
fn chunk_loud_everywhere_still_partitions() {
    let len = 1_000_000;
    let samples: Vec<f32> = (0..len)
        .map(|i| if i % 2 == 0 { 0.9 } else { -0.9 })
        .collect();
    let chunks = split_chunks(&samples);
    assert_partition(len, &chunks);
}

// ---------- engine (fake backend, fake features) ----------

enum Reply {
    Picks(Vec<usize>),
    Fail(String),
}

struct FakeBackend {
    replies: Arc<Mutex<VecDeque<Reply>>>,
    head: LangHead,
    calls: Arc<AtomicUsize>,
}

impl IndicBackend for FakeBackend {
    fn run(&mut self, features: &[f32], frames: usize) -> Result<IndicLogits, String> {
        assert_eq!(features.len(), N_MELS * frames, "feature shape");
        self.calls.fetch_add(1, Ordering::SeqCst);
        let reply = self
            .replies
            .lock()
            .map_err(|_| "poisoned".to_string())?
            .pop_front()
            .ok_or_else(|| "no reply scripted".to_string())?;
        match reply {
            Reply::Picks(picks) => Ok(logits_for(&self.head, &picks)),
            Reply::Fail(message) => Err(message),
        }
    }
}

fn fake_features(chunk: &[f32]) -> Result<MelFeatures, String> {
    let frames = chunk.len() / 160;
    Ok(MelFeatures {
        data: vec![0.0; N_MELS * frames],
        frames,
    })
}

fn engine_with(replies: Vec<Reply>) -> (IndicEngine, Arc<AtomicUsize>) {
    let head = test_head();
    let calls = Arc::new(AtomicUsize::new(0));
    let backend = FakeBackend {
        replies: Arc::new(Mutex::new(replies.into_iter().collect())),
        head: head.clone(),
        calls: Arc::clone(&calls),
    };
    let engine = IndicEngine::new(Box::new(backend), MelFrontend::new(), head)
        .with_feature_fn(Box::new(fake_features));
    (engine, calls)
}

#[test]
fn engine_joins_three_chunk_texts_with_single_spaces() {
    // 1,000,000 silent samples split into 480000 + 480000 + 40000 (three chunks).
    let (mut engine, calls) = engine_with(vec![
        Reply::Picks(vec![0]),
        Reply::Picks(vec![1]),
        Reply::Picks(vec![2]),
    ]);
    let text = engine
        .transcribe(&vec![0.0; 1_000_000])
        .expect("transcribe");
    assert_eq!(text, "a b c");
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[test]
fn engine_empty_chunk_text_adds_no_double_space() {
    let (mut engine, _calls) = engine_with(vec![
        Reply::Picks(vec![0]),
        Reply::Picks(vec![3, 3]),
        Reply::Picks(vec![1]),
    ]);
    let text = engine
        .transcribe(&vec![0.0; 1_000_000])
        .expect("transcribe");
    assert_eq!(text, "a b");
}

#[test]
fn engine_short_audio_never_calls_backend() {
    let (mut engine, calls) = engine_with(vec![]);
    assert_eq!(engine.transcribe(&[]).expect("empty"), "");
    assert_eq!(engine.transcribe(&vec![0.1; 1599]).expect("short"), "");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn engine_exactly_one_tenth_second_reaches_backend() {
    let (mut engine, calls) = engine_with(vec![Reply::Picks(vec![0])]);
    assert_eq!(engine.transcribe(&vec![0.1; 1600]).expect("ok"), "a");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn engine_non_finite_samples_are_err_without_backend_call() {
    let (mut engine, calls) = engine_with(vec![]);
    let mut nan = vec![0.1_f32; 4000];
    nan[2000] = f32::NAN;
    assert!(engine.transcribe(&nan).is_err());
    let mut inf = vec![0.1_f32; 4000];
    inf[10] = f32::INFINITY;
    assert!(engine.transcribe(&inf).is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn engine_backend_error_propagates_with_context() {
    let (mut engine, _calls) = engine_with(vec![Reply::Fail("boom".to_string())]);
    let err = engine.transcribe(&vec![0.1; 4000]).expect_err("must fail");
    assert!(err.contains("boom"), "cause kept: {err}");
    assert!(err.contains("chunk 1"), "context added: {err}");
}

// ---------- real pack (ignored) ----------

#[test]
#[ignore = "needs the Indic STT pack (VOX_INDIC_DIR)"]
fn real_tamil_head_matches_the_pack() {
    let dir = std::env::var("VOX_INDIC_DIR").expect("set VOX_INDIC_DIR to the pack directory");
    let head = load_lang_head(Path::new(&dir), "ta").expect("load ta head");
    assert_eq!(head.vocab.len(), 257);
    assert_eq!(head.columns.len(), 257);
    assert_eq!(head.blank, 256);
    assert_eq!(head.columns[0], 4864);
    assert_eq!(head.columns[256], 5632);
    assert!(validate_multilingual_head(&head).is_ok());
}
