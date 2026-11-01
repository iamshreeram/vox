//! Tamil speech recognition core: pack status, vocabulary, CTC decoding, chunking.
//! (Skeleton owned by the lead; step U2 fills it in. The backend trait below is the
//! fixed contract shared with `indic_stt_slot` and `indic_stt_ort`.)

// The engine is not consumed by the app until the slot and route steps land; keep the lib build quiet.
#![cfg_attr(not(test), allow(dead_code))]

use crate::managers::indic_stt_mel::{MelFeatures, MelFrontend, N_MELS};
use std::collections::BTreeMap;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// Log-probabilities produced by the encoder + CTC head for one chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct IndicLogits {
    /// Row-major `[steps, width]` log-probabilities over the full (unmasked) vocabulary.
    pub logprobs: Vec<f32>,
    /// Number of encoder output steps (about one per 80 ms of audio).
    pub steps: usize,
    /// Number of columns per step (5633 for the multilingual head).
    pub width: usize,
}

/// One loaded recognition session. Features are `[80, frames]` row-major (mel band major).
pub trait IndicBackend: Send {
    /// Run the encoder and the CTC head on one chunk of mel features.
    fn run(&mut self, features: &[f32], frames: usize) -> Result<IndicLogits, String>;
}

/// Directory name of the Indic STT pack under `aux_models`.
pub const PACK_DIR_NAME: &str = "indic_conformer_int8";

/// Pack files with their exact sizes in bytes, in the order they are reported.
const PACK_FILES: [(&str, u64); 4] = [
    ("encoder.onnx", 652_623_111),
    ("ctc_decoder.onnx", 23_095_918),
    ("vocab.json", 96_743),
    ("language_masks.json", 862_011),
];

/// Blank token id in the masked 257-entry space of the multilingual head.
pub const MULTILINGUAL_BLANK: usize = 256;

/// Sentencepiece word-boundary marker.
const WORD_BOUNDARY: char = '\u{2581}';

/// Minimum audio accepted by the engine (0.1 s at 16 kHz).
pub const MIN_AUDIO: usize = 1600;

/// Maximum samples per encoder call (30 s at 16 kHz).
pub const MAX_CHUNK: usize = 480_000;
/// Minimum length of the final chunk (1 s at 16 kHz); a cut never leaves less behind.
pub const MIN_TAIL: usize = 16_000;
/// How far before the upper cut bound the search for a quiet cut point reaches (6 s).
pub const SEARCH: usize = 96_000;
/// Length of the energy window scored for each candidate cut (100 ms).
pub const WIN: usize = 1_600;
/// Step between candidate cut points (50 ms).
pub const HOP: usize = 800;

/// Directory of the Indic STT pack inside the app data directory.
pub fn pack_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("aux_models").join(PACK_DIR_NAME)
}

/// Result of the stat-only pack check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndicPackStatus {
    /// All four files exist with the exact expected sizes.
    Ready,
    /// These files are absent (or a directory sits where a file belongs).
    Missing(Vec<String>),
    /// A file exists but its size differs from the pinned size.
    WrongSize {
        /// File name inside the pack directory.
        file: String,
        /// Pinned size in bytes.
        expected: u64,
        /// Size found on disk.
        actual: u64,
    },
}

/// Check the pack with metadata calls only (contents are never read).
/// Missing files are reported before wrong sizes.
pub fn indic_pack_status(dir: &Path) -> IndicPackStatus {
    let mut missing = Vec::new();
    let mut wrong_size = None;
    for (name, expected) in PACK_FILES {
        match fs::metadata(dir.join(name)) {
            Ok(meta) if meta.is_file() => {
                let actual = meta.len();
                if actual != expected && wrong_size.is_none() {
                    wrong_size = Some(IndicPackStatus::WrongSize {
                        file: name.to_string(),
                        expected,
                        actual,
                    });
                }
            }
            _ => missing.push(name.to_string()),
        }
    }
    if !missing.is_empty() {
        IndicPackStatus::Missing(missing)
    } else if let Some(status) = wrong_size {
        status
    } else {
        IndicPackStatus::Ready
    }
}

/// The vocabulary and column mask of one language head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LangHead {
    /// Token pieces, one per masked column.
    pub vocab: Vec<String>,
    /// Indices into the full output row, in increasing order (one per vocab entry).
    pub columns: Vec<usize>,
    /// Blank token id inside the masked space.
    pub blank: usize,
}

/// Parse a language head from the JSON texts of `vocab.json` and `language_masks.json`.
/// Pure; the blank id is set to the last vocab index (validated separately for the real pack).
pub fn parse_lang_head(vocab_json: &str, masks_json: &str, lang: &str) -> Result<LangHead, String> {
    let vocab_map: BTreeMap<String, Vec<String>> =
        serde_json::from_str(vocab_json).map_err(|e| format!("vocab.json is not valid: {e}"))?;
    let masks_map: BTreeMap<String, Vec<bool>> = serde_json::from_str(masks_json)
        .map_err(|e| format!("language_masks.json is not valid: {e}"))?;
    let vocab = vocab_map
        .get(lang)
        .ok_or_else(|| format!("language '{lang}' is not in vocab.json"))?
        .clone();
    let mask = masks_map
        .get(lang)
        .ok_or_else(|| format!("language '{lang}' is not in language_masks.json"))?;
    if vocab.is_empty() {
        return Err(format!("vocabulary for '{lang}' is empty"));
    }
    let columns: Vec<usize> = mask
        .iter()
        .enumerate()
        .filter(|(_, keep)| **keep)
        .map(|(index, _)| index)
        .collect();
    if columns.len() != vocab.len() {
        return Err(format!(
            "mask for '{lang}' selects {} columns but the vocabulary has {} entries",
            columns.len(),
            vocab.len()
        ));
    }
    let blank = vocab.len() - 1;
    Ok(LangHead {
        vocab,
        columns,
        blank,
    })
}

/// Check the invariants of the real multilingual head (blank id 256, inside the vocabulary).
pub fn validate_multilingual_head(head: &LangHead) -> Result<(), String> {
    if head.blank != MULTILINGUAL_BLANK {
        return Err(format!(
            "blank id is {} but the multilingual head requires {MULTILINGUAL_BLANK}",
            head.blank
        ));
    }
    if head.vocab.len() <= head.blank {
        return Err("vocabulary is shorter than the blank id".to_string());
    }
    Ok(())
}

/// Read `vocab.json` and `language_masks.json` from the pack and build the language head.
pub fn load_lang_head(dir: &Path, lang: &str) -> Result<LangHead, String> {
    let vocab_path = dir.join("vocab.json");
    let masks_path = dir.join("language_masks.json");
    let vocab_json = fs::read_to_string(&vocab_path)
        .map_err(|e| format!("cannot read {}: {e}", vocab_path.display()))?;
    let masks_json = fs::read_to_string(&masks_path)
        .map_err(|e| format!("cannot read {}: {e}", masks_path.display()))?;
    let head = parse_lang_head(&vocab_json, &masks_json, lang)?;
    validate_multilingual_head(&head)?;
    Ok(head)
}

/// Greedy CTC decode: argmax over the masked columns per step, collapse consecutive repeats,
/// drop the blank, join the pieces, map the word marker to a space and trim.
pub fn ctc_greedy(logits: &IndicLogits, head: &LangHead) -> Result<String, String> {
    if head.columns.is_empty() || head.columns.len() != head.vocab.len() {
        return Err("language head columns do not match its vocabulary".to_string());
    }
    if head.blank >= head.vocab.len() {
        return Err("blank id is outside the vocabulary".to_string());
    }
    let expected = logits
        .steps
        .checked_mul(logits.width)
        .ok_or_else(|| "logit shape overflows".to_string())?;
    if logits.logprobs.len() != expected {
        return Err(format!(
            "logprobs has {} values but {} steps x {} columns needs {expected}",
            logits.logprobs.len(),
            logits.steps,
            logits.width
        ));
    }
    if head.columns.iter().any(|&column| column >= logits.width) {
        return Err("language head column is outside the logit width".to_string());
    }
    if logits.logprobs.iter().any(|value| value.is_nan()) {
        return Err("logprobs contain NaN".to_string());
    }

    let mut text = String::new();
    let mut previous: Option<usize> = None;
    for step in 0..logits.steps {
        let row = &logits.logprobs[step * logits.width..(step + 1) * logits.width];
        let mut best = 0usize;
        let mut best_value = f32::NEG_INFINITY;
        for (index, &column) in head.columns.iter().enumerate() {
            let value = row[column];
            if index == 0 || value > best_value {
                best = index;
                best_value = value;
            }
        }
        if previous == Some(best) {
            continue;
        }
        previous = Some(best);
        if best == head.blank {
            continue;
        }
        if let Some(piece) = head.vocab.get(best) {
            text.push_str(piece);
        }
    }
    Ok(text.replace(WORD_BOUNDARY, " ").trim().to_string())
}

/// Mean square of `[end - WIN, end)`; `end` must be at least `WIN`.
fn window_energy(samples: &[f32], end: usize) -> f64 {
    let window = &samples[end - WIN..end];
    let sum: f64 = window.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    sum / WIN as f64
}

/// Pick the cut in `[lower, upper]` on the `HOP` grid that minimises the window energy.
/// Candidates are visited from `upper` downwards, so ties keep the latest cut.
fn choose_cut(buffer: &[f32], upper: usize, lower: usize) -> (usize, f64) {
    let mut best_cut = upper;
    let mut best_energy = f64::INFINITY;
    let mut cut = upper;
    loop {
        let energy = window_energy(buffer, cut);
        if energy < best_energy {
            best_energy = energy;
            best_cut = cut;
        }
        if cut < lower + HOP {
            break;
        }
        cut -= HOP;
    }
    (best_cut, best_energy)
}

/// Split 16 kHz audio into encoder-sized chunks that partition the input exactly.
/// Each cut is placed at the quietest candidate window in the last `SEARCH` samples before
/// the cap; the final chunk is at least `MIN_TAIL` samples whenever the input exceeds `MAX_CHUNK`.
pub fn split_chunks(samples: &[f32]) -> Vec<Range<usize>> {
    let total = samples.len();
    let mut chunks = Vec::new();
    if total == 0 {
        return chunks;
    }
    let mut start = 0usize;
    while total - start > MAX_CHUNK {
        let remaining = total - start;
        let upper = MAX_CHUNK.min(remaining - MIN_TAIL);
        let lower = upper.saturating_sub(SEARCH).max(MIN_TAIL);
        let buffer = &samples[start..];
        let (cut, energy) = choose_cut(buffer, upper, lower);
        let chunk = &buffer[..cut];
        let peak = chunk.iter().fold(0.0_f32, |acc, &s| acc.max(s.abs()));
        if f64::from(peak) > 0.0 && energy.sqrt() > 0.05 * f64::from(peak) {
            log::debug!(
                "Indic chunk cut at {} has RMS {:.4} above 5% of peak {:.4}; cut kept",
                start + cut,
                energy.sqrt(),
                peak
            );
        }
        chunks.push(start..start + cut);
        start += cut;
    }
    chunks.push(start..total);
    chunks
}

/// Features for one chunk, produced by a mel front end or an injected test source.
type FeatureFn = Box<dyn Fn(&[f32]) -> Result<MelFeatures, String> + Send>;

/// Tamil recognition engine: mel front end, encoder/CTC backend and the Tamil language head.
pub struct IndicEngine {
    backend: Box<dyn IndicBackend>,
    frontend: MelFrontend,
    head: LangHead,
    feature_fn: Option<FeatureFn>,
}

impl IndicEngine {
    /// Build the engine from its three parts.
    pub fn new(backend: Box<dyn IndicBackend>, frontend: MelFrontend, head: LangHead) -> Self {
        Self {
            backend,
            frontend,
            head,
            feature_fn: None,
        }
    }

    /// Replace the mel front end with an injected feature source (tests only).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn with_feature_fn(mut self, f: FeatureFn) -> Self {
        self.feature_fn = Some(f);
        self
    }

    /// Transcribe 16 kHz mono audio. Audio shorter than `MIN_AUDIO` gives an empty string.
    pub fn transcribe(&mut self, samples: &[f32]) -> Result<String, String> {
        if samples.len() < MIN_AUDIO {
            return Ok(String::new());
        }
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err("audio contains non-finite samples".to_string());
        }
        let mut pieces = Vec::new();
        for (index, range) in split_chunks(samples).into_iter().enumerate() {
            if range.is_empty() {
                continue;
            }
            let chunk = &samples[range];
            let number = index + 1;
            let features = match &self.feature_fn {
                Some(source) => source(chunk),
                None => self.frontend.features(chunk),
            }
            .map_err(|e| format!("Tamil speech features failed (chunk {number}): {e}"))?;
            if features.data.len() != N_MELS * features.frames {
                return Err(format!(
                    "Tamil speech features have the wrong shape (chunk {number})"
                ));
            }
            let logits = self
                .backend
                .run(&features.data, features.frames)
                .map_err(|e| format!("Tamil speech encoder failed (chunk {number}): {e}"))?;
            let text = ctc_greedy(&logits, &self.head)
                .map_err(|e| format!("Tamil speech decode failed (chunk {number}): {e}"))?;
            if !text.is_empty() {
                pieces.push(text);
            }
        }
        Ok(pieces.join(" "))
    }
}
