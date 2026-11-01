//! Tests for the ONNX Runtime Indic backend (`indic_stt_ort.rs`).
//!
//! Offline tests use the tiny fake pack in `tests/fixtures/tamil_stt/fake_pack` (built by
//! `scripts/tamil_stt_fake_pack.py`). They need no real model and run in CI.
//!
//! Live tests are `#[ignore]`d. They read `VOX_INDIC_DIR` (the real pack, four files),
//! `VOX_INDIC_REF` (the reference JSON written by `scripts/tamil_stt_ref.py`) and
//! `VOX_INDIC_TTS_WAVS` (optional folder of 16 kHz wavs with `.txt` prompts).
//! Run them with:
//! `VOX_INDIC_DIR=<pack> VOX_INDIC_REF=<ref.json> cargo test --lib indic_stt_ort -- --ignored --test-threads=1 --nocapture`
//!
//! Known limit of the live comparison: `IndicLogits` does not expose the encoder output, so the
//! encoder values recorded in the reference JSON are not compared. The comparison is the CTC
//! log-prob argmax path (exact, in the masked index space) and the decoded text (exact).

use super::indic_stt_core::{ctc_greedy, load_lang_head, IndicBackend, IndicLogits};
use super::indic_stt_mel::{MelFrontend, N_MELS};
use super::indic_stt_ort::{load_engine, OrtIndicBackend};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

/// Full CTC output width of the multilingual head.
const WIDTH: usize = 5633;
/// Tamil vocabulary columns in the full output row (256 consecutive plus the blank).
const TAMIL_FIRST: usize = 4864;
const TAMIL_LAST: usize = 5119;
const TAMIL_BLANK: usize = 5632;
/// Columns the fake pack's decoder emits for a positive and a non-positive encoder step.
const FAKE_POSITIVE: usize = 4865;
const FAKE_NON_POSITIVE: usize = 4869;

/// Directory of the committed fake pack.
fn fake_pack_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tamil_stt/fake_pack")
}

/// Encoder and CTC files of the fake pack.
fn fake_files() -> (PathBuf, PathBuf) {
    let dir = fake_pack_dir();
    (dir.join("encoder.onnx"), dir.join("ctc_decoder.onnx"))
}

/// Loads the fake pack's two sessions (no pack-status gate).
fn fake_backend() -> OrtIndicBackend {
    let (encoder, ctc) = fake_files();
    OrtIndicBackend::load_files(&encoder, &ctc).expect("fake pack must load")
}

/// Tamil columns of the full output row, in increasing order.
fn tamil_columns() -> Vec<usize> {
    (TAMIL_FIRST..=TAMIL_LAST)
        .chain(std::iter::once(TAMIL_BLANK))
        .collect()
}

/// Band-major `[80, frames]` features where band 0 holds `blocks[t / per_block]` and the other
/// bands are constant. The fake encoder only reads band 0.
fn block_features(blocks: &[f32], per_block: usize) -> (Vec<f32>, usize) {
    let frames = blocks.len() * per_block;
    let mut data = vec![0.5_f32; N_MELS * frames];
    for t in 0..frames {
        data[t] = blocks[t / per_block];
    }
    (data, frames)
}

/// Per step, the full-width column with the largest log-prob among `columns` (first wins).
fn argmax_path(logits: &IndicLogits, columns: &[usize]) -> Vec<usize> {
    (0..logits.steps)
        .map(|step| {
            let row = &logits.logprobs[step * logits.width..(step + 1) * logits.width];
            let mut best = columns[0];
            for &column in columns {
                if row[column] > row[best] {
                    best = column;
                }
            }
            best
        })
        .collect()
}

/// Per step, the position inside `columns` with the largest log-prob (first wins). This is the
/// masked index space used by the Python reference.
fn masked_argmax_path(logits: &IndicLogits, columns: &[usize]) -> Vec<usize> {
    (0..logits.steps)
        .map(|step| {
            let row = &logits.logprobs[step * logits.width..(step + 1) * logits.width];
            let mut best = 0usize;
            for (position, &column) in columns.iter().enumerate() {
                if row[column] > row[columns[best]] {
                    best = position;
                }
            }
            best
        })
        .collect()
}

/// A fresh directory under the system temp dir (removed by the caller).
fn scratch_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!(
        "vox-indic-ort-{tag}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

// ---------------------------------------------------------------------------
// Character error rate (pure; used by the round-trip live test)
// ---------------------------------------------------------------------------

/// Characters kept for scoring: everything except whitespace, ASCII punctuation and the
/// Devanagari danda marks (U+0964, U+0965), which Tamil text also uses.
fn scored_chars(text: &str) -> Vec<char> {
    text.chars()
        .filter(|c| {
            !(c.is_whitespace() || c.is_ascii_punctuation() || matches!(c, '\u{0964}' | '\u{0965}'))
        })
        .collect()
}

/// Levenshtein distance over Unicode scalar values (single-row dynamic programme).
fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut current = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            let deletion = previous[j + 1] + 1;
            let insertion = current[j] + 1;
            current[j + 1] = substitution.min(deletion).min(insertion);
        }
        previous = current;
    }
    previous[b.len()]
}

/// Character error rate of `hyp` against `reference`, after removing spaces and punctuation.
/// An empty reference gives 0 when the hypothesis is also empty and 1 otherwise.
pub(crate) fn cer(reference: &str, hyp: &str) -> f32 {
    let r = scored_chars(reference);
    let h = scored_chars(hyp);
    if r.is_empty() {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    edit_distance(&r, &h) as f32 / r.len() as f32
}

#[test]
fn cer_identical_text_is_zero() {
    assert_eq!(cer("இன்று வானிலை", "இன்று வானிலை"), 0.0);
}

#[test]
fn cer_ignores_spaces_and_punctuation() {
    assert_eq!(cer("வணக்கம், நீங்கள்!", "வணக்கம்நீங்கள்"), 0.0);
    assert_eq!(cer("abc.", "a b c"), 0.0);
}

#[test]
fn cer_empty_reference_counts_any_hypothesis_as_one() {
    assert_eq!(cer("", ""), 0.0);
    assert_eq!(cer(" ,", "abc"), 1.0);
}

#[test]
fn cer_substitution_is_one_edit() {
    assert_eq!(cer("abcd", "abxd"), 0.25);
}

#[test]
fn cer_deletion_is_one_edit() {
    assert_eq!(cer("abcd", "abd"), 0.25);
    assert_eq!(cer("abc", ""), 1.0);
}

#[test]
fn cer_insertion_is_one_edit_over_reference_length() {
    assert!((cer("abd", "abcd") - 1.0 / 3.0).abs() < 1e-6);
}

#[test]
fn cer_counts_unicode_scalars_not_bytes() {
    // One Tamil letter replaced by another: one substitution out of three scalars.
    assert!((cer("கசப", "கதப") - 1.0 / 3.0).abs() < 1e-6);
}

// ---------------------------------------------------------------------------
// Offline tests on the fake pack
// ---------------------------------------------------------------------------

#[test]
fn load_files_accepts_the_fake_pack() {
    let (encoder, ctc) = fake_files();
    let result = OrtIndicBackend::load_files(&encoder, &ctc);
    assert!(result.is_ok(), "fake pack must load: {:?}", result.err());
}

#[test]
fn run_returns_expected_steps_width_and_argmax_path() {
    let mut backend = fake_backend();
    let (features, frames) = block_features(&[1.0, -1.0, 0.0], 8);
    let logits = backend.run(&features, frames).expect("run");
    assert_eq!(logits.steps, 3);
    assert_eq!(logits.width, WIDTH);
    assert_eq!(logits.logprobs.len(), 3 * WIDTH);
    assert_eq!(
        argmax_path(&logits, &tamil_columns()),
        vec![FAKE_POSITIVE, FAKE_NON_POSITIVE, FAKE_NON_POSITIVE]
    );
}

#[test]
fn run_output_follows_the_input() {
    let mut backend = fake_backend();
    let (features, frames) = block_features(&[-1.0, 1.0], 8);
    let logits = backend.run(&features, frames).expect("run");
    assert_eq!(logits.steps, 2);
    assert_eq!(
        argmax_path(&logits, &tamil_columns()),
        vec![FAKE_NON_POSITIVE, FAKE_POSITIVE]
    );
}

#[test]
fn run_twice_gives_identical_output() {
    let mut backend = fake_backend();
    let (features, frames) = block_features(&[1.0, -1.0, 1.0, -1.0], 8);
    let first = backend.run(&features, frames).expect("first run");
    let second = backend.run(&features, frames).expect("second run");
    assert_eq!(first, second);
}

#[test]
fn run_with_zero_frames_is_an_error_not_a_panic() {
    let mut backend = fake_backend();
    let result = backend.run(&[], 0);
    assert!(result.is_err());
}

#[test]
fn run_with_wrong_feature_length_is_an_error() {
    let mut backend = fake_backend();
    let result = backend.run(&vec![0.0_f32; N_MELS * 8 - 1], 8);
    let message = result.err().unwrap_or_default();
    assert!(message.contains("features"), "message was: {message}");
}

#[test]
fn run_with_too_few_frames_returns_without_panic() {
    // Fewer frames than the encoder's subsampling window: the session may refuse the input.
    // Either way the result must be consistent or an error.
    let mut backend = fake_backend();
    let (features, frames) = block_features(&[1.0], 5);
    match backend.run(&features, frames) {
        Ok(logits) => assert_eq!(logits.logprobs.len(), logits.steps * logits.width),
        Err(message) => assert!(!message.is_empty()),
    }
}

#[test]
fn load_files_missing_encoder_names_the_file() {
    let dir = scratch_dir("missing-enc");
    let (_, ctc) = fake_files();
    let result = OrtIndicBackend::load_files(&dir.join("encoder.onnx"), &ctc);
    let _ = std::fs::remove_dir_all(&dir);
    let message = result.err().unwrap_or_default();
    assert!(message.contains("encoder.onnx"), "message was: {message}");
}

#[test]
fn load_files_missing_ctc_names_the_file() {
    let dir = scratch_dir("missing-ctc");
    let (encoder, _) = fake_files();
    let result = OrtIndicBackend::load_files(&encoder, &dir.join("ctc_decoder.onnx"));
    let _ = std::fs::remove_dir_all(&dir);
    let message = result.err().unwrap_or_default();
    assert!(
        message.contains("ctc_decoder.onnx"),
        "message was: {message}"
    );
}

#[test]
fn load_on_fake_pack_dir_is_gated_by_pack_status() {
    // The fake pack has no vocab.json or language_masks.json, so the status gate fails first.
    let result = OrtIndicBackend::load(&fake_pack_dir());
    let message = result.err().unwrap_or_default();
    assert!(message.contains("vocab.json"), "message was: {message}");
}

#[test]
fn load_with_wrong_encoder_size_is_rejected_before_sessions() {
    let dir = scratch_dir("wrong-size");
    let (encoder, ctc) = fake_files();
    std::fs::copy(&encoder, dir.join("encoder.onnx")).expect("copy encoder");
    std::fs::copy(&ctc, dir.join("ctc_decoder.onnx")).expect("copy ctc");
    std::fs::write(dir.join("vocab.json"), "{}").expect("vocab");
    std::fs::write(dir.join("language_masks.json"), "{}").expect("masks");
    let result = OrtIndicBackend::load(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    let message = result.err().unwrap_or_default();
    assert!(message.contains("wrong size"), "message was: {message}");
    assert!(message.contains("encoder.onnx"), "message was: {message}");
}

#[test]
fn load_engine_on_fake_pack_dir_is_an_error_not_a_panic() {
    let result = load_engine(&fake_pack_dir(), "ta");
    assert!(result.is_err());
}

#[test]
fn tamil_column_layout_constants_are_consistent() {
    // 256 consecutive Tamil columns plus the blank, all inside the full width.
    let columns = tamil_columns();
    assert_eq!(columns.len(), 257);
    assert_eq!(columns[0], TAMIL_FIRST);
    assert_eq!(columns[255], TAMIL_LAST);
    assert_eq!(columns[256], TAMIL_BLANK);
    assert!(columns.iter().all(|&c| c < WIDTH));
}

// ---------------------------------------------------------------------------
// LIVE tests (need the real pack: VOX_INDIC_DIR; reference: VOX_INDIC_REF)
// ---------------------------------------------------------------------------

/// Path from an environment variable; panics with a clear message when unset (live tests only).
fn env_path(var: &str) -> PathBuf {
    PathBuf::from(std::env::var(var).unwrap_or_else(|_| panic!("set {var} to run this test")))
}

/// Reads a 16 kHz mono WAV as f32 in [-1, 1). 16-bit PCM is divided by 32768 (the same scale
/// soundfile uses for `dtype='float32'`). Other rates and layouts are errors (no resampling).
fn read_wav_16k_mono(path: &Path) -> Result<Vec<f32>, String> {
    let mut reader =
        hound::WavReader::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let spec = reader.spec();
    if spec.sample_rate != 16_000 || spec.channels != 1 {
        return Err(format!(
            "{}: need 16 kHz mono, found {} Hz {} channel(s)",
            path.display(),
            spec.sample_rate,
            spec.channels
        ));
    }
    let samples = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|s| s.map(|v| f32::from(v) / 32_768.0))
            .collect::<Result<Vec<f32>, hound::Error>>(),
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<Result<Vec<f32>, hound::Error>>(),
        (format, bits) => {
            return Err(format!(
                "{}: unsupported WAV format {format:?} {bits}-bit",
                path.display()
            ))
        }
    };
    samples.map_err(|e| format!("{}: cannot read samples: {e}", path.display()))
}

/// Resident set size of this process in MiB, from `ps` (macOS and Linux both report KiB).
fn rss_mib() -> Option<f64> {
    let pid = std::process::id().to_string();
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .ok()?;
    let kib: f64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .ok()?;
    Some(kib / 1024.0)
}

/// Loads the reference JSON written by `scripts/tamil_stt_ref.py`.
fn load_reference() -> serde_json::Value {
    let path = env_path("VOX_INDIC_REF");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&text).expect("reference JSON must parse")
}

#[test]
#[ignore = "needs the Indic STT pack (VOX_INDIC_DIR)"]
fn live_pack_loads_in_isolation_and_reports_load_time() {
    let dir = env_path("VOX_INDIC_DIR");
    let started = Instant::now();
    let backend = OrtIndicBackend::load(&dir);
    let seconds = started.elapsed().as_secs_f64();
    println!("indic pack load: {seconds:.3} s from {}", dir.display());
    assert!(backend.is_ok(), "pack must load: {:?}", backend.err());
}

#[test]
#[ignore = "needs the Indic STT pack (VOX_INDIC_DIR) and the reference JSON (VOX_INDIC_REF)"]
fn live_argmax_path_and_text_match_python_reference() {
    let dir = env_path("VOX_INDIC_DIR");
    let reference = load_reference();
    let items = reference["items"]
        .as_array()
        .expect("reference JSON needs an items array");
    assert!(!items.is_empty(), "reference JSON has no items");

    let head = load_lang_head(&dir, "ta").expect("Tamil head");
    let mut backend = OrtIndicBackend::load(&dir).expect("pack must load");
    let frontend = MelFrontend::new();

    for item in items {
        let wav = item["wav"].as_str().expect("item.wav");
        let samples = read_wav_16k_mono(Path::new(wav)).expect("wav");
        assert_eq!(
            samples.len() as u64,
            item["samples"].as_u64().expect("item.samples"),
            "sample count differs for {wav}"
        );
        let features = frontend.features(&samples).expect("features");
        assert_eq!(
            features.frames as u64,
            item["mel_frames"].as_u64().expect("item.mel_frames"),
            "mel frames differ for {wav}"
        );
        let logits = backend.run(&features.data, features.frames).expect("run");

        let expected_path: Vec<usize> = item["ctc_argmax_path"]
            .as_array()
            .expect("item.ctc_argmax_path")
            .iter()
            .map(|v| v.as_u64().expect("path entry") as usize)
            .collect();
        let rust_path = masked_argmax_path(&logits, &head.columns);
        assert_eq!(
            rust_path, expected_path,
            "CTC argmax path differs for {wav}"
        );

        let text = ctc_greedy(&logits, &head).expect("decode");
        let expected_text = item["text"].as_str().expect("item.text");
        println!("{wav}: {} steps, text: {text}", logits.steps);
        assert!(!text.is_empty(), "decoded text is empty for {wav}");
        assert_eq!(text, expected_text, "decoded text differs for {wav}");
    }
}

/// Concatenates the parts cyclically until `target` samples are collected.
fn build_clip(parts: &[Vec<f32>], target: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(target);
    if parts.iter().all(|p| p.is_empty()) {
        return out;
    }
    while out.len() < target {
        for part in parts {
            for &sample in part {
                if out.len() == target {
                    return out;
                }
                out.push(sample);
            }
        }
    }
    out
}

#[test]
#[ignore = "needs the Indic STT pack (VOX_INDIC_DIR) and the reference JSON (VOX_INDIC_REF)"]
fn live_latency_cold_warm_and_rss_gate() {
    let dir = env_path("VOX_INDIC_DIR");
    let reference = load_reference();
    let parts: Vec<Vec<f32>> = reference["items"]
        .as_array()
        .expect("reference JSON needs an items array")
        .iter()
        .map(|item| {
            let wav = item["wav"].as_str().expect("item.wav");
            read_wav_16k_mono(Path::new(wav)).expect("wav")
        })
        .collect();
    let clip = build_clip(&parts, 5 * 16_000);
    assert_eq!(
        clip.len(),
        5 * 16_000,
        "reference wavs must total at least 5 s"
    );

    let rss_before = rss_mib();
    let started = Instant::now();
    let mut engine = load_engine(&dir, "ta").expect("engine");
    let cold = started.elapsed().as_secs_f64();
    let mut peak = rss_mib();
    println!(
        "cold load (engine, sessions + Tamil head): {cold:.3} s; RSS before {:?} MiB",
        rss_before.map(|m| (m * 10.0).round() / 10.0)
    );

    let mut warm_times = Vec::new();
    for run in 1..=3 {
        let started = Instant::now();
        let text = engine.transcribe(&clip).expect("transcribe");
        let seconds = started.elapsed().as_secs_f64();
        peak = match (peak, rss_mib()) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        println!(
            "warm run {run}: {seconds:.3} s for 5.0 s audio (RTF {:.3}); text: {text}",
            seconds / 5.0
        );
        assert!(!text.is_empty(), "warm run {run} returned empty text");
        assert!(
            seconds < 3.0,
            "warm 5 s decode took {seconds:.3} s (gate 3 s)"
        );
        assert!(seconds / 5.0 < 0.6, "RTF {:.3} (gate 0.6)", seconds / 5.0);
        warm_times.push(seconds);
    }
    let mean = warm_times.iter().sum::<f64>() / warm_times.len() as f64;
    println!(
        "warm mean {mean:.3} s (RTF {:.3}); peak RSS sampled at checkpoints: {:?} MiB",
        mean / 5.0,
        peak.map(|m| (m * 10.0).round() / 10.0)
    );
}

#[test]
#[ignore = "needs the Indic STT pack (VOX_INDIC_DIR); optional VOX_INDIC_TTS_WAVS"]
fn live_round_trip_cer_on_tts_output() {
    let Ok(wav_dir) = std::env::var("VOX_INDIC_TTS_WAVS") else {
        println!("skipped: set VOX_INDIC_TTS_WAVS to a folder of 16 kHz wavs with .txt prompts");
        return;
    };
    let dir = env_path("VOX_INDIC_DIR");
    let mut wavs: Vec<PathBuf> = std::fs::read_dir(&wav_dir)
        .expect("VOX_INDIC_TTS_WAVS must be a folder")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "wav") && p.with_extension("txt").is_file())
        .collect();
    wavs.sort();
    assert!(!wavs.is_empty(), "no wav/.txt pairs in {wav_dir}");

    let mut engine = load_engine(&dir, "ta").expect("engine");
    let mut scores = Vec::new();
    for wav in &wavs {
        let samples = read_wav_16k_mono(wav).expect("wav");
        let prompt = std::fs::read_to_string(wav.with_extension("txt")).expect("prompt");
        let text = engine.transcribe(&samples).expect("transcribe");
        let score = cer(&prompt, &text);
        println!("{}: CER {score:.3}; text: {text}", wav.display());
        assert!(score < 0.5, "CER {score:.3} >= 0.5 for {}", wav.display());
        scores.push(score);
    }
    let mean = scores.iter().sum::<f32>() / scores.len() as f32;
    println!("mean CER over {} files: {mean:.3}", scores.len());
    assert!(mean < 0.35, "mean CER {mean:.3} >= 0.35");
}
