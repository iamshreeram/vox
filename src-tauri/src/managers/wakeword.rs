//! Wake-word detection interfaces and the safe no-op fallback.

#![allow(dead_code)]

use crate::settings::AppSettings;
use serde::{Deserialize, Serialize};
use specta::Type;

/// A wake-word detection emitted by an engine. Must be Serialize/Type so it
/// can be sent as a Tauri event payload (see `commands/wakeword.rs`'s
/// `wakeword://detected` event, task W6) and exported to the TypeScript
/// bindings via specta.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct WakeWordDetection {
    pub model_name: String,
    pub confidence: f32,
    pub timestamp_ms: u64,
}

/// Processes mono audio frames and reports wake-word detections.
pub trait WakeWordEngine: Send + Sync {
    /// Feed one frame of mono audio and return a detection if the confidence
    /// threshold is crossed.
    fn process_frame(&mut self, samples: &[f32]) -> Option<WakeWordDetection>;

    /// Reset the engine's buffered audio and detection state.
    fn reset(&mut self);

    /// Pause detection while another audio operation owns the microphone.
    fn pause(&mut self);

    /// Resume detection after the microphone becomes available.
    fn resume(&mut self);

    /// EXPERIMENTAL (not yet wired into production call sites): prime any
    /// internal warm-up state before real audio starts flowing. Default
    /// no-op.
    fn warm_up(&mut self) {}
}

/// A cross-platform fallback that never reports a detection.
#[derive(Debug, Default)]
pub struct NullWakeWordEngine;

impl WakeWordEngine for NullWakeWordEngine {
    fn process_frame(&mut self, _samples: &[f32]) -> Option<WakeWordDetection> {
        None
    }

    fn reset(&mut self) {}

    fn pause(&mut self) {}

    fn resume(&mut self) {}
}

/// Whether wake-word processing is enabled in the current settings.
/// Future detection integrations must gate microphone processing on this flag.
pub fn wakeword_enabled(settings: &AppSettings) -> bool {
    settings.wake_word_enabled
}

#[cfg(test)]
mod tests {
    use super::{wakeword_enabled, NullWakeWordEngine, WakeWordEngine};
    use crate::settings::get_default_settings;

    #[test]
    fn wakeword_disabled_setting_disables_processing() {
        let settings = get_default_settings();
        assert!(!wakeword_enabled(&settings));
    }

    #[test]
    fn null_engine_never_reports_detection() {
        let mut engine = NullWakeWordEngine;
        assert!(engine.process_frame(&[0.0; 1280]).is_none());
        engine.reset();
        engine.pause();
        engine.resume();
    }
}

/// Fixed window size openWakeWord's melspectrogram/embedding pipeline
/// expects: 80ms @ 16kHz = 1280 samples. See `WakeWordEngine::process_frame`'s
/// doc comment above and `docs/vox-phases/PHASE-5-wakeword.md` section 3.
pub const WINDOW_SAMPLES: usize = 1280;

/// Classifies one full 1280-sample window into a raw (not yet
/// threshold-compared or clamped) confidence score. The real ONNX-backed
/// melspectrogram -> embedding -> classifier cascade implements this in
/// task W4; tests inject a stub/spy implementation.
pub trait FrameClassifier: Send + Sync {
    fn classify(&mut self, window: &[f32]) -> f32;

    /// Clears any internal rolling audio/feature buffers. Default no-op for
    /// stateless/stub classifiers (existing tests rely on this). A stateful
    /// classifier (e.g. `OpenWakeWordClassifier`, task W4) MUST override
    /// this -- its melspectrogram/embedding/feature rolling buffers carry
    /// audio context across `classify` calls, so without a real `reset()`
    /// here, `WakeWordEngine::reset()` (T7) would only clear the outer
    /// `BufferingWakeWordEngine`'s pre-window sample buffer while leaving
    /// the classifier's own internal history intact -- letting pre-reset
    /// audio silently leak into post-reset detections.
    fn reset(&mut self) {}
}

/// Buffers arbitrary-sized input chunks into exactly `WINDOW_SAMPLES`-sample
/// windows before invoking the injected classifier, per the
/// `WakeWordEngine::process_frame` contract. Assumes the caller already
/// supplies 16kHz mono audio -- W7's mic integration is responsible for
/// resampling if needed before calling this.
#[allow(dead_code)]
pub struct BufferingWakeWordEngine<C: FrameClassifier> {
    model_name: String,
    confidence_threshold: f32,
    classifier: C,
    buffer: Vec<f32>,
    paused: bool,
}

#[allow(dead_code)]
impl<C: FrameClassifier> BufferingWakeWordEngine<C> {
    pub fn new(model_name: String, confidence_threshold: f32, classifier: C) -> Self {
        Self {
            model_name,
            confidence_threshold,
            classifier,
            buffer: Vec::with_capacity(WINDOW_SAMPLES),
            paused: false,
        }
    }
}

impl<C: FrameClassifier> WakeWordEngine for BufferingWakeWordEngine<C> {
    fn process_frame(&mut self, samples: &[f32]) -> Option<WakeWordDetection> {
        if self.paused || samples.is_empty() {
            return None;
        }
        self.buffer.extend_from_slice(samples);
        if self.buffer.len() < WINDOW_SAMPLES {
            return None;
        }
        let window: Vec<f32> = self.buffer.drain(..WINDOW_SAMPLES).collect();
        let raw_score = self.classifier.classify(&window);
        // A classifier must never be allowed to propagate NaN/Inf as a
        // confidence score (FR4/T5) -- f32::clamp does NOT sanitize NaN
        // (`NaN.clamp(0.0, 1.0)` returns NaN unchanged, since NaN compares
        // false against both the min and max bounds inside clamp's own
        // implementation), so a classifier bug or a corrupted ONNX output
        // could otherwise slip a NaN confidence into a real `Some(detection)`
        // despite this function's own f32::clamp call below. Sanitize first.
        let raw_score = if raw_score.is_finite() {
            raw_score
        } else {
            0.0
        };
        let confidence = raw_score.clamp(0.0, 1.0);
        if confidence < self.confidence_threshold {
            return None;
        }
        Some(WakeWordDetection {
            model_name: self.model_name.clone(),
            confidence,
            timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
        })
    }

    fn reset(&mut self) {
        self.buffer.clear();
        self.classifier.reset();
    }

    fn pause(&mut self) {
        self.paused = true;
    }

    fn resume(&mut self) {
        self.paused = false;
    }

    fn warm_up(&mut self) {
        let silent_window = vec![0.0_f32; WINDOW_SAMPLES];
        for _ in 0..ZERO_GUARD_FRAMES {
            let _ = self.classifier.classify(&silent_window);
        }
    }
}

#[cfg(test)]
mod buffering_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct SpyClassifier {
        calls: Arc<Mutex<Vec<Vec<f32>>>>,
        fixed_confidence: f32,
    }

    impl FrameClassifier for SpyClassifier {
        fn classify(&mut self, window: &[f32]) -> f32 {
            self.calls.lock().unwrap().push(window.to_vec());
            self.fixed_confidence
        }
    }

    fn make_engine(
        confidence: f32,
        threshold: f32,
    ) -> (
        BufferingWakeWordEngine<SpyClassifier>,
        Arc<Mutex<Vec<Vec<f32>>>>,
    ) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let classifier = SpyClassifier {
            calls: calls.clone(),
            fixed_confidence: confidence,
        };
        let engine = BufferingWakeWordEngine::new("test_model".to_string(), threshold, classifier);
        (engine, calls)
    }

    #[test]
    fn empty_slice_returns_none_and_does_not_classify() {
        let (mut engine, calls) = make_engine(0.9, 0.5);
        assert!(engine.process_frame(&[]).is_none());
        assert_eq!(calls.lock().unwrap().len(), 0);
    }

    #[test]
    fn nan_and_infinite_samples_do_not_panic_and_confidence_is_never_nan() {
        let (mut engine, _calls) = make_engine(0.9, 0.5);
        let mut samples = vec![f32::NAN; 1280];
        samples[0] = f32::INFINITY;
        samples[1] = f32::NEG_INFINITY;
        if let Some(detection) = engine.process_frame(&samples) {
            assert!(!detection.confidence.is_nan());
        }
    }

    #[test]
    fn short_buffer_is_accumulated_until_a_full_window_then_classifier_invoked_once() {
        let (mut engine, calls) = make_engine(0.9, 0.5);
        assert!(engine.process_frame(&vec![0.1_f32; 600]).is_none());
        assert_eq!(calls.lock().unwrap().len(), 0);

        let result = engine.process_frame(&vec![0.2_f32; 680]);
        assert!(result.is_some());

        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].len(), 1280);
        assert!(recorded[0][..600].iter().all(|&s| s == 0.1));
        assert!(recorded[0][600..].iter().all(|&s| s == 0.2));
    }

    #[test]
    fn reset_mid_window_prevents_mixing_pre_and_post_reset_samples() {
        let (mut engine, calls) = make_engine(0.9, 0.5);
        assert!(engine.process_frame(&vec![0.1_f32; 600]).is_none());
        engine.reset();
        assert!(engine.process_frame(&vec![0.2_f32; 600]).is_none());
        assert!(engine.process_frame(&vec![0.3_f32; 680]).is_some());

        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert!(recorded[0][..600].iter().all(|&s| s == 0.2));
        assert!(recorded[0][600..].iter().all(|&s| s == 0.3));
    }

    #[test]
    fn classifier_confidence_above_one_is_clamped_to_one() {
        let (mut engine, _calls) = make_engine(1.5, 0.5);
        let detection = engine.process_frame(&vec![0.0_f32; 1280]).unwrap();
        assert_eq!(detection.confidence, 1.0);
    }

    #[test]
    fn classifier_confidence_below_zero_is_clamped_to_zero() {
        let (mut engine, _calls) = make_engine(-0.2, 0.0);
        let detection = engine.process_frame(&vec![0.0_f32; 1280]).unwrap();
        assert_eq!(detection.confidence, 0.0);
    }

    #[test]
    fn below_threshold_confidence_returns_none() {
        let (mut engine, _calls) = make_engine(0.1, 0.5);
        assert!(engine.process_frame(&vec![0.0_f32; 1280]).is_none());
    }
}

/// openWakeWord's pipeline needs 3 ONNX model files per model: a
/// melspectrogram featurizer, a shared embedding model, and a
/// model-specific classifier. Not bundled -- downloaded on first use and
/// cached locally, matching the existing STT model manager's pattern
/// (`managers/model.rs` + `managers/model/download.rs`).
pub const WAKEWORD_MODEL_FILES: [&str; 3] = [
    "melspectrogram.onnx",
    "embedding_model.onnx",
    "classifier.onnx",
];

mod downloader;
pub use downloader::HttpModelDownloader;

/// Whether a wake-word model's files are present locally. Distinct from a
/// download failure -- querying readiness never attempts a download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelReadiness {
    NotDownloaded,
    Ready,
}

/// A distinct, specific error for a failed model download -- never let a
/// missing file surface as a raw ONNX runtime "file not found" error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelDownloadError {
    Network(String),
    Io(String),
}

/// Abstracts the actual file transfer so tests never hit the network.
pub trait ModelDownloader: Send + Sync {
    /// Downloads `file_name` for `model_name` to `destination`.
    fn download_file(
        &self,
        model_name: &str,
        file_name: &str,
        destination: &std::path::Path,
    ) -> Result<(), ModelDownloadError>;
}

/// Resolves the wake-word model directory under the app's data dir.
pub fn wakeword_model_dir(app_data_dir: &std::path::Path, model_name: &str) -> std::path::PathBuf {
    app_data_dir.join("wakeword_models").join(model_name)
}

/// Checks whether all of a model's files are already present locally.
/// Never attempts a download -- purely a filesystem check.
pub fn model_readiness(app_data_dir: &std::path::Path, model_name: &str) -> ModelReadiness {
    let dir = wakeword_model_dir(app_data_dir, model_name);
    let all_present = WAKEWORD_MODEL_FILES
        .iter()
        .all(|file| dir.join(file).is_file());
    if all_present {
        ModelReadiness::Ready
    } else {
        ModelReadiness::NotDownloaded
    }
}

/// Ensures all of a model's files are present locally, downloading any
/// missing ones. Idempotent: a file already present is never re-downloaded.
/// Atomic per-file: downloads to a `.partial` sibling first and only renames
/// it into place on success; on failure the partial is deleted so no
/// corrupt/incomplete file is ever left at the final path.
pub fn ensure_model_downloaded(
    app_data_dir: &std::path::Path,
    model_name: &str,
    downloader: &dyn ModelDownloader,
) -> Result<(), ModelDownloadError> {
    let dir = wakeword_model_dir(app_data_dir, model_name);
    std::fs::create_dir_all(&dir).map_err(|e| ModelDownloadError::Io(e.to_string()))?;
    for file in WAKEWORD_MODEL_FILES {
        let destination = dir.join(file);
        if destination.is_file() {
            continue;
        }
        let tmp_destination = dir.join(format!("{file}.partial"));
        match downloader.download_file(model_name, file, &tmp_destination) {
            Ok(()) => {
                std::fs::rename(&tmp_destination, &destination)
                    .map_err(|e| ModelDownloadError::Io(e.to_string()))?;
            }
            Err(error) => {
                let _ = std::fs::remove_file(&tmp_destination);
                return Err(error);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod download_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct FakeDownloader {
        calls: Arc<Mutex<Vec<String>>>,
        should_fail: bool,
    }

    impl ModelDownloader for FakeDownloader {
        fn download_file(
            &self,
            _model_name: &str,
            file_name: &str,
            destination: &std::path::Path,
        ) -> Result<(), ModelDownloadError> {
            self.calls.lock().unwrap().push(file_name.to_string());
            if self.should_fail {
                // Simulate a transfer that wrote some bytes before failing.
                let _ = std::fs::write(destination, b"partial-garbage");
                return Err(ModelDownloadError::Network("connection reset".to_string()));
            }
            std::fs::write(destination, b"fake-model-bytes").unwrap();
            Ok(())
        }
    }

    #[test]
    fn fresh_cache_dir_downloads_all_three_files() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let downloader = FakeDownloader {
            calls: calls.clone(),
            should_fail: false,
        };
        let result = ensure_model_downloaded(dir.path(), "hey_jarvis", &downloader);
        assert!(result.is_ok());
        assert_eq!(calls.lock().unwrap().len(), 3);
        assert_eq!(
            model_readiness(dir.path(), "hey_jarvis"),
            ModelReadiness::Ready
        );
    }

    #[test]
    fn already_present_model_is_not_redownloaded() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let downloader = FakeDownloader {
            calls: calls.clone(),
            should_fail: false,
        };
        ensure_model_downloaded(dir.path(), "hey_jarvis", &downloader).unwrap();
        assert_eq!(calls.lock().unwrap().len(), 3);

        ensure_model_downloaded(dir.path(), "hey_jarvis", &downloader).unwrap();
        assert_eq!(calls.lock().unwrap().len(), 3);
    }

    #[test]
    fn failed_download_returns_distinct_error_and_leaves_no_partial_file_at_final_path() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let downloader = FakeDownloader {
            calls,
            should_fail: true,
        };
        let result = ensure_model_downloaded(dir.path(), "hey_jarvis", &downloader);
        assert!(matches!(result, Err(ModelDownloadError::Network(_))));

        let model_dir = wakeword_model_dir(dir.path(), "hey_jarvis");
        let final_path = model_dir.join(WAKEWORD_MODEL_FILES[0]);
        assert!(!final_path.exists());
        let tmp_path = model_dir.join(format!("{}.partial", WAKEWORD_MODEL_FILES[0]));
        assert!(!tmp_path.exists());
    }

    #[test]
    fn readiness_before_any_download_is_not_downloaded() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            model_readiness(dir.path(), "hey_jarvis"),
            ModelReadiness::NotDownloaded
        );
    }

    #[test]
    fn readiness_after_a_failed_download_is_still_not_downloaded_not_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let downloader = FakeDownloader {
            calls,
            should_fail: true,
        };
        let result = ensure_model_downloaded(dir.path(), "hey_jarvis", &downloader);
        assert!(result.is_err());
        assert_eq!(
            model_readiness(dir.path(), "hey_jarvis"),
            ModelReadiness::NotDownloaded
        );
    }
}

#[cfg(test)]
mod pause_resume_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct SpyClassifier {
        calls: Arc<Mutex<usize>>,
        fixed_confidence: f32,
    }

    impl FrameClassifier for SpyClassifier {
        fn classify(&mut self, _window: &[f32]) -> f32 {
            *self.calls.lock().unwrap() += 1;
            self.fixed_confidence
        }
    }

    #[test]
    fn paused_engine_returns_none_and_never_invokes_the_classifier() {
        let calls = Arc::new(Mutex::new(0usize));
        let classifier = SpyClassifier {
            calls: calls.clone(),
            fixed_confidence: 0.9,
        };
        let mut engine = BufferingWakeWordEngine::new("model_a".to_string(), 0.5, classifier);
        engine.pause();
        assert!(engine.process_frame(&vec![0.1_f32; 1280]).is_none());
        assert_eq!(*calls.lock().unwrap(), 0);
    }

    #[test]
    fn resume_after_pause_restores_normal_detection() {
        let calls = Arc::new(Mutex::new(0usize));
        let classifier = SpyClassifier {
            calls: calls.clone(),
            fixed_confidence: 0.9,
        };
        let mut engine = BufferingWakeWordEngine::new("model_a".to_string(), 0.5, classifier);
        engine.pause();
        assert!(engine.process_frame(&vec![0.1_f32; 1280]).is_none());
        engine.resume();
        assert!(engine.process_frame(&vec![0.1_f32; 1280]).is_some());
        assert_eq!(*calls.lock().unwrap(), 1);
    }

    #[test]
    fn t8_two_instances_with_different_models_do_not_share_state() {
        let calls_a = Arc::new(Mutex::new(0usize));
        let calls_b = Arc::new(Mutex::new(0usize));
        let classifier_a = SpyClassifier {
            calls: calls_a.clone(),
            fixed_confidence: 0.9,
        };
        let classifier_b = SpyClassifier {
            calls: calls_b.clone(),
            fixed_confidence: 0.1,
        };
        let mut engine_a = BufferingWakeWordEngine::new("model_a".to_string(), 0.5, classifier_a);
        let mut engine_b = BufferingWakeWordEngine::new("model_b".to_string(), 0.5, classifier_b);

        let detection_a = engine_a.process_frame(&vec![0.1_f32; 1280]);
        assert!(detection_a.is_some());
        assert_eq!(detection_a.unwrap().model_name, "model_a");
        assert_eq!(*calls_a.lock().unwrap(), 1);
        assert_eq!(*calls_b.lock().unwrap(), 0);

        assert!(engine_b.process_frame(&vec![0.2_f32; 1280]).is_none());
        assert_eq!(*calls_b.lock().unwrap(), 1);
        assert_eq!(*calls_a.lock().unwrap(), 1);
    }
}

// ===========================================================================
// W4 -- real ONNX melspectrogram -> embedding -> classifier cascade.
// ===========================================================================
//
// PLACEHOLDER_MARKER_FOR_W4_IMPLEMENTATION

/// Real, ONNX-backed `FrameClassifier` implementing openWakeWord's own
/// melspectrogram -> embedding -> classifier 3-model cascade, reproduced
/// from the upstream `dscripka/openWakeWord` Python package (v0.5.1,
/// Apache-2.0) `openwakeword/model.py` + `openwakeword/utils.py`
/// (`AudioFeatures` class) -- NOT from any private/unavailable reference;
/// verified directly against the real pretrained ONNX files' I/O shapes
/// and against the real Python pipeline's own output scores (see
/// `tests/fixtures/wakeword/` and the `real_classifier_tests` module
/// below for the exact golden-reference methodology).
pub struct OpenWakeWordClassifier {
    melspec_session: ort::session::Session,
    embedding_session: ort::session::Session,
    classifier_session: ort::session::Session,

    /// Rolling raw-audio history, in int16-magnitude float32 units (i.e.
    /// `sample * 32767.0`, NOT normalized to [-1, 1] -- openWakeWord's own
    /// melspectrogram model expects this exact scale; see `classify`'s
    /// doc comment). Only the most recent ~1760 samples
    /// (`WINDOW_SAMPLES + MELSPEC_LOOKBACK_SAMPLES`) are ever read per
    /// call, but a slightly larger cap is kept for safety margin.
    raw_history: Vec<f32>,

    /// Rolling mel-spectrogram frames, each a `MEL_BINS`-wide row.
    /// Initialized to `MELSPEC_WINDOW_FRAMES` rows of `1.0` (NOT zero --
    /// matches upstream's own `np.ones((76, 32))` initial buffer exactly,
    /// see `AudioFeatures.__init__`). Capped at `MELSPEC_BUFFER_MAX_FRAMES`.
    melspec_buffer: Vec<[f32; MEL_BINS]>,

    /// Rolling embedding vectors, each `EMBEDDING_DIM`-wide. Deliberately
    /// primed with `EMBEDDING_CONTEXT_FRAMES` rows of `0.0` rather than
    /// upstream's own random-noise-through-the-real-pipeline priming
    /// (`np.random.randint(-1000, 1000, 16000*4)` fed through
    /// `_get_embeddings`). This is an intentional, documented deviation --
    /// NOT a correctness shortcut -- justified as follows: (1) upstream's
    /// own priming is itself non-deterministic (unseeded `np.random`), so
    /// two real Python runs already produce different early-frame scores
    /// from each other (empirically confirmed: 0.9949 vs 0.9963 peak
    /// confidence across two real runs on identical "hey jarvis" audio --
    /// see the golden-reference generation notes in
    /// `tests/fixtures/wakeword/`); (2) upstream's own `predict()` forcibly
    /// zeroes every model's first `ZERO_GUARD_FRAMES` returned predictions
    /// regardless of buffer content (`if len(self.prediction_buffer[cls])
    /// < 5: predictions[cls] = 0.0`), so whatever the priming buffer
    /// contains during that window is provably never observable by a
    /// caller; (3) once `EMBEDDING_CONTEXT_FRAMES` real frames have been
    /// appended, the classifier's input window is mathematically
    /// guaranteed to contain zero contribution from the initial priming
    /// rows, regardless of what they were -- this is replicated by this
    /// implementation's own `ZERO_GUARD_FRAMES` guard in `classify` below.
    /// Capped at `FEATURE_BUFFER_MAX_FRAMES`.
    feature_buffer: Vec<[f32; EMBEDDING_DIM]>,

    /// Count of `classify` calls since construction or the last `reset()`.
    /// The first `ZERO_GUARD_FRAMES` calls always return `0.0` regardless
    /// of the real computed score -- matches upstream's own model-
    /// initialization guard (see `feature_buffer`'s doc comment above).
    calls_since_reset: u32,
}

/// 80ms @ 16kHz. Matches `WINDOW_SAMPLES` -- see that constant's own doc
/// comment. Duplicated as a distinct name here only for local clarity
/// inside this cascade's own doc comments; must always equal
/// `WINDOW_SAMPLES`.
const MELSPEC_LOOKBACK_SAMPLES: usize = 480; // 160 * 3, per upstream `_streaming_melspectrogram`
const MEL_BINS: usize = 32;
const MELSPEC_WINDOW_FRAMES: usize = 76;
const MELSPEC_BUFFER_MAX_FRAMES: usize = 970; // 10 * 97, matches upstream `melspectrogram_max_len`
const EMBEDDING_DIM: usize = 96;
const EMBEDDING_CONTEXT_FRAMES: usize = 16; // classifier's own context window
const FEATURE_BUFFER_MAX_FRAMES: usize = 120; // matches upstream `feature_buffer_max_len`
const ZERO_GUARD_FRAMES: u32 = 5; // matches upstream's prediction_buffer-length guard
const RAW_HISTORY_MAX_SAMPLES: usize = 8000; // generous cap; only last ~1760 ever read

impl OpenWakeWordClassifier {
    /// Loads the 3-model cascade from `dir`, expecting exactly the 3 files
    /// named in `WAKEWORD_MODEL_FILES` ("melspectrogram.onnx",
    /// "embedding_model.onnx", "classifier.onnx") to already be present
    /// (the caller is responsible for having called
    /// `ensure_model_downloaded`/checked `model_readiness` first -- this
    /// function does not download anything itself).
    ///
    /// IMPLEMENTATION NOTES (verified directly against the real v0.5.1
    /// pretrained ONNX files -- do not re-derive these from guesswork):
    /// - melspectrogram.onnx: input name "input", shape (1, N) f32,
    ///   dynamic N. Output name "output" -- its declared ONNX shape has
    ///   extra size-1 dims (`[time, 1, X, 32]`); DO NOT try to match its
    ///   literal rank via ort's shape API. Instead take the output's raw
    ///   flat `f32` buffer, assert `flat.len() % MEL_BINS == 0`, and treat
    ///   it as `flat.len() / MEL_BINS` rows of `MEL_BINS` each
    ///   (row-major) -- this sidesteps any ambiguity in exactly which
    ///   dims are size-1 at runtime.
    /// - embedding_model.onnx: input name "input_1", shape
    ///   (1, 76, 32, 1) f32. Output name "conv2d_19" -- same flattening
    ///   approach; a single (1,76,32,1) input call always produces
    ///   exactly `EMBEDDING_DIM` (96) output values.
    /// - classifier.onnx: input name "x.1", shape (1, 16, 96) f32. Output
    ///   name "53", a single scalar (already post-sigmoid -- do not apply
    ///   an extra sigmoid).
    /// - Set `inter_op_num_threads = 1` and `intra_op_num_threads = 1` on
    ///   every session (matches upstream's own `SessionOptions`, keeps
    ///   per-prediction CPU cost small and predictable per NFR1). Use
    ///   whatever exact `ort = "=2.0.0-rc.12"` builder method names compile
    ///   -- grep the vendored source under `~/.cargo/registry/src/` for
    ///   `ort-2.0.0-rc.12` if the exact method name isn't obvious; do not
    ///   guess and leave it uncompiled.
    pub fn load_from_dir(dir: &std::path::Path) -> anyhow::Result<Self> {
        use ort::session::Session;

        let build_session = |file_name: &str| -> anyhow::Result<Session> {
            let builder = Session::builder().map_err(|error| anyhow::anyhow!(error.to_string()))?;
            let builder = builder
                .with_inter_threads(1)
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            let mut builder = builder
                .with_intra_threads(1)
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            builder
                .commit_from_file(dir.join(file_name))
                .map_err(|error| anyhow::anyhow!(error.to_string()))
        };

        Ok(Self {
            melspec_session: build_session("melspectrogram.onnx")?,
            embedding_session: build_session("embedding_model.onnx")?,
            classifier_session: build_session("classifier.onnx")?,
            raw_history: Vec::with_capacity(RAW_HISTORY_MAX_SAMPLES),
            melspec_buffer: vec![[1.0; MEL_BINS]; MELSPEC_WINDOW_FRAMES],
            feature_buffer: vec![[0.0; EMBEDDING_DIM]; EMBEDDING_CONTEXT_FRAMES],
            calls_since_reset: 0,
        })
    }
}

impl FrameClassifier for OpenWakeWordClassifier {
    /// `window` is always exactly `WINDOW_SAMPLES` (1280) f32 samples in
    /// `[-1.0, 1.0]` -- guaranteed by `BufferingWakeWordEngine`, which is
    /// this trait's only production caller. NaN/Inf samples must never
    /// reach the ONNX runtime or propagate into the returned score (FR2,
    /// FR4, T5).
    ///
    /// EXACT ALGORITHM (reproduced from upstream `AudioFeatures.__call__`
    /// -> `_streaming_features` -> `_streaming_melspectrogram`, specialized
    /// for the fixed case this trait is always called in: exactly
    /// `WINDOW_SAMPLES` new samples per call, never more, never less --
    /// `BufferingWakeWordEngine` guarantees this, so the general-purpose
    /// multi-chunk-batch branches of upstream's own code never apply here
    /// and can be ignored):
    ///
    /// 1. Convert each input sample to int16-magnitude float32: for each
    ///    `s` in `window`, compute `let scaled = s * 32767.0;` then
    ///    `let sanitized = (scaled as i16) as f32;`. Rust's float->int `as`
    ///    cast is a *saturating* cast (stable since Rust 1.45): NaN -> 0,
    ///    +inf -> i16::MAX, -inf -> i16::MIN, out-of-range values clamp to
    ///    the nearest bound. This is the ONLY sanitization point needed --
    ///    do not additionally special-case NaN/Inf before this, and do not
    ///    use `f32::clamp` here (it does not remove NaN; see the
    ///    `BufferingWakeWordEngine::process_frame` fix above for why).
    ///    Append all 1280 sanitized values to `self.raw_history`; if
    ///    `self.raw_history.len() > RAW_HISTORY_MAX_SAMPLES`, drain the
    ///    oldest excess from the front.
    /// 2. Take the last `WINDOW_SAMPLES + MELSPEC_LOOKBACK_SAMPLES` (1760)
    ///    samples of `self.raw_history` (or all of it, if shorter --
    ///    matches upstream's own Python negative-slice-past-start
    ///    semantics, which silently returns fewer elements rather than
    ///    erroring; e.g. `Vec`'s `len().saturating_sub(1760)..` start index
    ///    achieves the same thing in Rust).
    /// 3. Run `melspec_session` with input "input" = that slice reshaped
    ///    to (1, slice.len()). Apply `spec = spec / 10.0 + 2.0` elementwise
    ///    to the flattened output (see `load_from_dir`'s doc comment for
    ///    the flattening approach) -- this rescaling constant is upstream
    ///    openWakeWord's own documented transform
    ///    (`melspec_transform: Callable = lambda x: x/10 + 2` in
    ///    `AudioFeatures._get_melspectrogram`), NOT an invented value.
    /// 4. Append the resulting `(n_frames, MEL_BINS)` rows to
    ///    `self.melspec_buffer` (push each row individually in order). If
    ///    `self.melspec_buffer.len() > MELSPEC_BUFFER_MAX_FRAMES`, drain
    ///    the oldest excess from the front.
    /// 5. Take the LAST `MELSPEC_WINDOW_FRAMES` (76) rows of
    ///    `self.melspec_buffer` (guaranteed to always have >= 76 rows,
    ///    since it starts with 76 and only grows). Reshape to
    ///    (1, 76, 32, 1) and run `embedding_session` input "input_1" ->
    ///    output "conv2d_19", flattened to exactly `EMBEDDING_DIM` (96)
    ///    values.
    /// 6. Push that 96-value row onto `self.feature_buffer`. If
    ///    `self.feature_buffer.len() > FEATURE_BUFFER_MAX_FRAMES`, drain
    ///    the oldest excess from the front.
    /// 7. Take the LAST `EMBEDDING_CONTEXT_FRAMES` (16) rows of
    ///    `self.feature_buffer` (guaranteed to always have >= 16, since it
    ///    starts with 16 and only grows). Reshape to (1, 16, 96) and run
    ///    `classifier_session` input "x.1" -> output "53", a single scalar.
    /// 8. `self.calls_since_reset += 1;`. If the PRE-INCREMENT count was
    ///    `< ZERO_GUARD_FRAMES` (i.e. this is one of the first 5 calls
    ///    since construction/reset), return `0.0` regardless of the real
    ///    computed scalar (still do all of steps 1-7 first -- the buffers
    ///    must stay warmed/in-sync; only the RETURNED value is overridden).
    ///    Otherwise return the real scalar, but first replace it with
    ///    `0.0` if it is NaN or infinite (belt-and-suspenders on top of the
    ///    caller's own sanitization in `BufferingWakeWordEngine`).
    fn classify(&mut self, window: &[f32]) -> f32 {
        use ort::value::Tensor;

        self.raw_history.extend(window.iter().map(|sample| {
            let scaled = sample * 32767.0;
            (scaled as i16) as f32
        }));
        if self.raw_history.len() > RAW_HISTORY_MAX_SAMPLES {
            let excess = self.raw_history.len() - RAW_HISTORY_MAX_SAMPLES;
            self.raw_history.drain(..excess);
        }

        let history_start = self
            .raw_history
            .len()
            .saturating_sub(WINDOW_SAMPLES + MELSPEC_LOOKBACK_SAMPLES);
        let audio = &self.raw_history[history_start..];
        let audio_tensor = match Tensor::<f32>::from_array(([1, audio.len()], audio.to_vec())) {
            Ok(tensor) => tensor,
            Err(_) => return 0.0,
        };
        let spec_output = match self
            .melspec_session
            .run(ort::inputs!["input" => audio_tensor])
        {
            Ok(outputs) => outputs,
            Err(_) => return 0.0,
        };
        let spec = match spec_output["output"].try_extract_tensor::<f32>() {
            Ok((_, values)) if values.len() % MEL_BINS == 0 => values
                .iter()
                .map(|value| value / 10.0 + 2.0)
                .collect::<Vec<_>>(),
            _ => return 0.0,
        };
        for row in spec.chunks_exact(MEL_BINS) {
            let mut mel_row = [0.0; MEL_BINS];
            mel_row.copy_from_slice(row);
            self.melspec_buffer.push(mel_row);
        }
        if self.melspec_buffer.len() > MELSPEC_BUFFER_MAX_FRAMES {
            let excess = self.melspec_buffer.len() - MELSPEC_BUFFER_MAX_FRAMES;
            self.melspec_buffer.drain(..excess);
        }

        let mel_start = self.melspec_buffer.len() - MELSPEC_WINDOW_FRAMES;
        let mel_values = self.melspec_buffer[mel_start..]
            .iter()
            .flat_map(|row| row.iter().copied())
            .collect::<Vec<_>>();
        let mel_tensor = match Tensor::<f32>::from_array(([1, 76, 32, 1], mel_values)) {
            Ok(tensor) => tensor,
            Err(_) => return 0.0,
        };
        let embedding_output = match self
            .embedding_session
            .run(ort::inputs!["input_1" => mel_tensor])
        {
            Ok(outputs) => outputs,
            Err(_) => return 0.0,
        };
        let embedding = match embedding_output["conv2d_19"].try_extract_tensor::<f32>() {
            Ok((_, values)) if values.len() == EMBEDDING_DIM => {
                let mut row = [0.0; EMBEDDING_DIM];
                row.copy_from_slice(values);
                row
            }
            _ => return 0.0,
        };
        self.feature_buffer.push(embedding);
        if self.feature_buffer.len() > FEATURE_BUFFER_MAX_FRAMES {
            let excess = self.feature_buffer.len() - FEATURE_BUFFER_MAX_FRAMES;
            self.feature_buffer.drain(..excess);
        }

        let feature_start = self.feature_buffer.len() - EMBEDDING_CONTEXT_FRAMES;
        let feature_values = self.feature_buffer[feature_start..]
            .iter()
            .flat_map(|row| row.iter().copied())
            .collect::<Vec<_>>();
        let feature_tensor = match Tensor::<f32>::from_array(([1, 16, 96], feature_values)) {
            Ok(tensor) => tensor,
            Err(_) => return 0.0,
        };
        let classifier_output = match self
            .classifier_session
            .run(ort::inputs!["x.1" => feature_tensor])
        {
            Ok(outputs) => outputs,
            Err(_) => return 0.0,
        };
        let score = match classifier_output["53"].try_extract_tensor::<f32>() {
            Ok((_, values)) if values.len() == 1 => values[0],
            _ => return 0.0,
        };

        let previous_calls = self.calls_since_reset;
        self.calls_since_reset += 1;
        if previous_calls < ZERO_GUARD_FRAMES {
            0.0
        } else if score.is_finite() {
            score
        } else {
            0.0
        }
    }

    /// Clears all rolling state back to the exact construction-time
    /// initial values: `raw_history` empty, `melspec_buffer` reset to
    /// `MELSPEC_WINDOW_FRAMES` rows of `1.0`, `feature_buffer` reset to
    /// `EMBEDDING_CONTEXT_FRAMES` rows of `0.0`, `calls_since_reset = 0`.
    /// Required for T7 -- see the `FrameClassifier::reset` trait doc
    /// comment for why this must exist and be wired up.
    fn reset(&mut self) {
        self.raw_history.clear();
        self.melspec_buffer = vec![[1.0; MEL_BINS]; MELSPEC_WINDOW_FRAMES];
        self.feature_buffer = vec![[0.0; EMBEDDING_DIM]; EMBEDDING_CONTEXT_FRAMES];
        self.calls_since_reset = 0;
    }
}

#[cfg(test)]
mod nan_sanitization_regression_tests {
    use super::*;

    /// Regression test for a real bug found while designing W4: a
    /// classifier that returns NaN must never let NaN escape as a
    /// `WakeWordDetection.confidence` (FR4/T5). `f32::clamp` alone does
    /// NOT fix this -- `NaN.clamp(0.0, 1.0)` returns `NaN` unchanged, since
    /// clamp's own `<`/`>` comparisons against NaN are always false. This
    /// test exercises `BufferingWakeWordEngine::process_frame`'s own fix
    /// directly (sanitizing before clamping), independent of whichever
    /// real `FrameClassifier` is plugged in.
    struct NanClassifier;
    impl FrameClassifier for NanClassifier {
        fn classify(&mut self, _window: &[f32]) -> f32 {
            f32::NAN
        }
    }

    #[test]
    fn nan_classifier_output_never_propagates_as_detection_confidence() {
        let mut engine = BufferingWakeWordEngine::new("m".to_string(), 0.0, NanClassifier);
        let result = engine.process_frame(&vec![0.0_f32; 1280]);
        // threshold 0.0: a sanitized confidence of 0.0 is NOT < 0.0, so a
        // detection IS returned -- but its confidence must be the
        // sanitized 0.0, never NaN.
        let detection = result.expect("sanitized 0.0 confidence should still pass a 0.0 threshold");
        assert_eq!(detection.confidence, 0.0);
        assert!(!detection.confidence.is_nan());
    }

    struct InfClassifier;
    impl FrameClassifier for InfClassifier {
        fn classify(&mut self, _window: &[f32]) -> f32 {
            f32::INFINITY
        }
    }

    #[test]
    fn infinite_classifier_output_is_sanitized_not_propagated() {
        let mut engine = BufferingWakeWordEngine::new("m".to_string(), 0.0, InfClassifier);
        let detection = engine
            .process_frame(&vec![0.0_f32; 1280])
            .expect("sanitized 0.0 confidence should still pass a 0.0 threshold");
        assert_eq!(detection.confidence, 0.0);
        assert!(detection.confidence.is_finite());
    }
}

#[cfg(test)]
mod reset_propagation_regression_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Regression test proving `WakeWordEngine::reset()` actually reaches
    /// a stateful classifier's own `reset()` -- without this wiring, a
    /// stateful classifier like `OpenWakeWordClassifier` would keep its
    /// internal rolling buffers across a `reset()` call, letting pre-reset
    /// audio silently influence post-reset detections (T7).
    struct ResetSpyClassifier {
        reset_calls: Arc<Mutex<usize>>,
    }

    impl FrameClassifier for ResetSpyClassifier {
        fn classify(&mut self, _window: &[f32]) -> f32 {
            0.0
        }
        fn reset(&mut self) {
            *self.reset_calls.lock().unwrap() += 1;
        }
    }

    #[test]
    fn engine_reset_propagates_to_the_classifiers_own_reset() {
        let reset_calls = Arc::new(Mutex::new(0usize));
        let classifier = ResetSpyClassifier {
            reset_calls: reset_calls.clone(),
        };
        let mut engine = BufferingWakeWordEngine::new("m".to_string(), 0.5, classifier);
        assert_eq!(*reset_calls.lock().unwrap(), 0);
        engine.reset();
        assert_eq!(*reset_calls.lock().unwrap(), 1);
        engine.reset();
        assert_eq!(*reset_calls.lock().unwrap(), 2);
    }
}

/// Real, end-to-end tests against the actual pretrained openWakeWord ONNX
/// files (redistributed as test fixtures -- see
/// `tests/fixtures/wakeword/models/NOTICE.md`) and real synthesized speech
/// audio (see `tests/fixtures/wakeword/audio/`, generated via macOS `say`,
/// exactly mirroring the phase doc's own suggested methodology). These are
/// the authoritative T1-T11 tests from `docs/vox-phases/PHASE-5-wakeword.md`
/// SS6. Deliberately offline/deterministic -- no network access, no
/// dependency on any particular Python environment being installed.
///
/// NOTE on tolerances: these tests use wide, empirically-justified safety
/// margins (e.g. "> 0.9" for real wake-word audio, "< 0.01" for
/// silence/unrelated speech) rather than brittle tight-tolerance numeric
/// matching against one specific Python reference run. This is a
/// deliberate choice: the real upstream Python pipeline's own early-frame
/// scores are themselves non-deterministic (unseeded `np.random` buffer
/// priming -- two independent real runs on identical audio produced 0.9949
/// and 0.9963 peak confidence respectively), so tight exact-matching would
/// be testing an artifact of one particular priming draw, not a real
/// correctness property. The wide-margin bound checks below are exactly as
/// rigorous for the property that actually matters -- does the cascade
/// correctly discriminate real wake-word audio from silence/other speech --
/// and are not flaky.
#[cfg(test)]
mod real_classifier_tests {
    use super::*;

    const HEY_JARVIS_PCM: &[u8] =
        include_bytes!("../../tests/fixtures/wakeword/audio/hey_jarvis.pcm");
    const UNRELATED_PCM: &[u8] =
        include_bytes!("../../tests/fixtures/wakeword/audio/unrelated.pcm");
    const BORDERLINE_PCM: &[u8] =
        include_bytes!("../../tests/fixtures/wakeword/audio/borderline.pcm");
    const RESET_PRE_5FRAMES_PCM: &[u8] =
        include_bytes!("../../tests/fixtures/wakeword/audio/reset_pre_5frames.pcm");

    fn fixture_models_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wakeword/models")
    }

    fn new_classifier() -> OpenWakeWordClassifier {
        OpenWakeWordClassifier::load_from_dir(&fixture_models_dir())
            .expect("real wake-word test fixture models must load")
    }

    /// Converts little-endian 16-bit PCM bytes to f32 samples in
    /// [-1.0, 1.0] -- the exact inverse of the int16-magnitude conversion
    /// documented on `OpenWakeWordClassifier::classify` (both sides use
    /// 32767.0, not 32768.0).
    fn pcm_bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
        assert_eq!(
            bytes.len() % 2,
            0,
            "PCM byte length must be even (16-bit samples)"
        );
        bytes
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32767.0)
            .collect()
    }

    fn feed_all_frames<C: FrameClassifier>(
        engine: &mut BufferingWakeWordEngine<C>,
        samples: &[f32],
    ) -> Vec<WakeWordDetection> {
        samples
            .chunks(WINDOW_SAMPLES)
            .filter_map(|chunk| engine.process_frame(chunk))
            .collect()
    }

    fn assert_valid_confidence(c: f32) {
        assert!(!c.is_nan(), "confidence must never be NaN");
        assert!(
            (0.0..=1.0).contains(&c),
            "confidence {c} out of [0,1] range"
        );
    }

    #[test]
    fn t1_real_hey_jarvis_audio_triggers_a_high_confidence_detection() {
        let mut engine =
            BufferingWakeWordEngine::new("hey_jarvis".to_string(), 0.5, new_classifier());
        let samples = pcm_bytes_to_f32(HEY_JARVIS_PCM);
        let detections = feed_all_frames(&mut engine, &samples);

        assert!(
            !detections.is_empty(),
            "expected at least one detection on real 'hey jarvis' audio"
        );
        for d in &detections {
            assert_valid_confidence(d.confidence);
            assert_eq!(d.model_name, "hey_jarvis");
        }
        let max_confidence = detections
            .iter()
            .map(|d| d.confidence)
            .fold(0.0_f32, f32::max);
        // Both independent real-Python reference runs landed at >= 0.99
        // peak confidence on this exact audio (0.9949 and 0.9963) -- 0.9 is
        // a safe, generously-margined floor, not a tight fit to one run.
        assert!(
            max_confidence > 0.9,
            "expected a high-confidence detection on real wake-word audio, got max {max_confidence}"
        );
    }

    #[test]
    fn t2_real_ten_seconds_of_silence_never_detects() {
        let mut engine =
            BufferingWakeWordEngine::new("hey_jarvis".to_string(), 0.5, new_classifier());
        let silence = vec![0.0_f32; 1280 * 125]; // 10 seconds @ 16kHz/1280-sample frames
        let detections = feed_all_frames(&mut engine, &silence);
        assert!(
            detections.is_empty(),
            "expected no detections on silence at the default threshold, got {detections:?}"
        );
    }

    #[test]
    fn t2b_raw_classifier_score_on_silence_stays_far_below_any_sane_threshold() {
        // Stronger than T2: checks the RAW score (bypassing any particular
        // threshold choice) directly, per FR3's "regardless of threshold"
        // wording. The real reference pipeline's own max observed score on
        // 10s of silence was ~0.00007 -- 0.01 is a two-orders-of-magnitude
        // safety margin, not a tight fit.
        let mut classifier = new_classifier();
        let silence = vec![0.0_f32; 1280];
        let mut max_score = 0.0_f32;
        for _ in 0..125 {
            let score = classifier.classify(&silence);
            assert_valid_confidence(score.clamp(0.0, 1.0));
            max_score = max_score.max(score);
        }
        assert!(max_score < 0.01, "raw silence score spiked to {max_score}");
    }

    #[test]
    fn t3_real_unrelated_speech_does_not_detect() {
        let mut engine =
            BufferingWakeWordEngine::new("hey_jarvis".to_string(), 0.5, new_classifier());
        let samples = pcm_bytes_to_f32(UNRELATED_PCM);
        let detections = feed_all_frames(&mut engine, &samples);
        assert!(
            detections.is_empty(),
            "expected no detections on unrelated speech, got {detections:?}"
        );
    }

    #[test]
    fn t4_empty_slice_with_real_classifier_returns_none_without_panicking() {
        let mut engine =
            BufferingWakeWordEngine::new("hey_jarvis".to_string(), 0.5, new_classifier());
        assert!(engine.process_frame(&[]).is_none());
    }

    #[test]
    fn t5_nan_and_inf_samples_with_real_classifier_never_panic_or_return_nan() {
        let mut engine =
            BufferingWakeWordEngine::new("hey_jarvis".to_string(), 0.5, new_classifier());
        let mut samples = vec![f32::NAN; 1280];
        samples[0] = f32::INFINITY;
        samples[1] = f32::NEG_INFINITY;
        samples[2] = 0.3;
        if let Some(detection) = engine.process_frame(&samples) {
            assert_valid_confidence(detection.confidence);
        }
        // Must also not have wedged the engine for subsequent real calls.
        let follow_up = engine.process_frame(&vec![0.0_f32; 1280]);
        if let Some(detection) = follow_up {
            assert_valid_confidence(detection.confidence);
        }
    }

    #[test]
    fn t6_split_window_with_real_classifier_eventually_classifies() {
        let mut engine =
            BufferingWakeWordEngine::new("hey_jarvis".to_string(), 0.0, new_classifier());
        let samples = pcm_bytes_to_f32(HEY_JARVIS_PCM);
        assert!(samples.len() >= 1280);
        let first = &samples[..600];
        assert!(
            engine.process_frame(first).is_none(),
            "a partial window must not classify yet"
        );
        let remaining = &samples[600..1280];
        let result = engine.process_frame(remaining);
        assert!(
            result.is_some(),
            "completing the window across two calls must invoke the real classifier \
             (threshold 0.0 means any valid score detects)"
        );
    }

    #[test]
    fn t7_reset_mid_utterance_prevents_pre_reset_audio_from_leaking() {
        let mut engine =
            BufferingWakeWordEngine::new("hey_jarvis".to_string(), 0.5, new_classifier());
        let pre_reset_samples = pcm_bytes_to_f32(RESET_PRE_5FRAMES_PCM);
        assert_eq!(pre_reset_samples.len(), 1280 * 5);
        let pre_reset_detections = feed_all_frames(&mut engine, &pre_reset_samples);
        assert!(
            pre_reset_detections.is_empty(),
            "the first 5 frames are always zero-guarded regardless of audio content"
        );

        engine.reset();

        let silence = vec![0.0_f32; 1280 * 125];
        let post_reset_detections = feed_all_frames(&mut engine, &silence);
        assert!(
            post_reset_detections.is_empty(),
            "pre-reset wake-word-shaped audio must not leak into post-reset silence \
             detections, got {post_reset_detections:?}"
        );
    }

    #[test]
    fn t9_high_threshold_rejects_borderline_audio() {
        let mut engine =
            BufferingWakeWordEngine::new("hey_jarvis".to_string(), 0.99, new_classifier());
        let samples = pcm_bytes_to_f32(BORDERLINE_PCM);
        let detections = feed_all_frames(&mut engine, &samples);
        assert!(
            detections.is_empty(),
            "a 0.99 threshold must reject borderline-confidence audio, got {detections:?}"
        );
    }

    #[test]
    fn t10_low_threshold_accepts_the_same_borderline_audio() {
        let mut engine =
            BufferingWakeWordEngine::new("hey_jarvis".to_string(), 0.01, new_classifier());
        let samples = pcm_bytes_to_f32(BORDERLINE_PCM);
        let detections = feed_all_frames(&mut engine, &samples);
        assert!(
            !detections.is_empty(),
            "a 0.01 threshold must accept the same borderline-confidence audio that a 0.99 \
             threshold rejects (proves the threshold is actually applied both directions)"
        );
    }

    #[test]
    fn t11_confidence_is_always_in_range_across_every_fixture() {
        for (threshold, pcm) in [
            (0.0_f32, HEY_JARVIS_PCM),
            (0.0_f32, UNRELATED_PCM),
            (0.0_f32, BORDERLINE_PCM),
        ] {
            let mut engine =
                BufferingWakeWordEngine::new("hey_jarvis".to_string(), threshold, new_classifier());
            let samples = pcm_bytes_to_f32(pcm);
            for d in feed_all_frames(&mut engine, &samples) {
                assert_valid_confidence(d.confidence);
            }
        }
    }

    #[test]
    fn experiment_truncation_vs_zero_guard() {
        let samples = pcm_bytes_to_f32(HEY_JARVIS_PCM);
        eprintln!(
            "hey_jarvis.pcm total samples = {}, full windows = {}",
            samples.len(),
            samples.len() / WINDOW_SAMPLES
        );
        for cut_frames in 0..=8usize {
            let cut = cut_frames * WINDOW_SAMPLES;
            if cut >= samples.len() {
                continue;
            }
            let truncated = &samples[cut..];

            let mut engine =
                BufferingWakeWordEngine::new("hey_jarvis".to_string(), 0.5, new_classifier());
            let detections = feed_all_frames(&mut engine, truncated);
            let max_conf = detections.iter().map(|d| d.confidence).fold(0.0_f32, f32::max);
            eprintln!(
                "cut_frames={cut_frames} (removed {}ms leading audio) -> no-warmup: detections={} max_conf={max_conf:.4}",
                cut_frames * 80,
                detections.len()
            );

            let mut engine2 =
                BufferingWakeWordEngine::new("hey_jarvis".to_string(), 0.5, new_classifier());
            engine2.warm_up();
            let detections2 = feed_all_frames(&mut engine2, truncated);
            let max_conf2 = detections2.iter().map(|d| d.confidence).fold(0.0_f32, f32::max);
            eprintln!(
                "  with warm_up -> detections={} max_conf={max_conf2:.4}",
                detections2.len()
            );
        }
    }

    /// Locks in the measured, evidence-based fix for the real production bug
    /// (user reports needing to repeat "Hey Jarvis" 3-4 times): the
    /// dedicated wake-word microphone stream is closed between dictation
    /// sessions (see `close_recorder`/`open_recorder` in
    /// `wakeword_listener.rs`) and must be reopened before listening can
    /// resume. Real audio hardware does not deliver samples the instant a
    /// stream (re)opens (CoreAudio/WASAPI startup latency is commonly
    /// 100-300ms), so a user who starts speaking right when they expect the
    /// mic to already be "live" can lose the first ~1-2 classification
    /// windows (80-160ms) of their own wake phrase -- reproduced here by
    /// simply never feeding those leading samples to the engine.
    ///
    /// Measured against the real `hey_jarvis.pcm` fixture (11 full windows,
    /// 880ms total) via `experiment_truncation_vs_zero_guard` above: losing
    /// 160ms (2 windows) of leading audio takes confidence from 0.9966
    /// (nothing lost) to a flat 0.0 (zero detections) without warm-up, but
    /// recovers to 0.5259 (a real, if borderline, pass at the default 0.5
    /// threshold) once `warm_up()` is called on the engine before replaying
    /// the truncated audio. Beyond ~240ms lost, neither configuration
    /// recovers -- `warm_up()` narrows the failure window, it does not
    /// eliminate every possible timing case. This test locks in exactly
    /// that measured boundary so a future change to the classifier/guard
    /// cannot silently regress the fix without failing a test.
    #[test]
    fn warm_up_recovers_detection_when_leading_audio_is_lost_to_stream_reopen_latency() {
        let samples = pcm_bytes_to_f32(HEY_JARVIS_PCM);
        let cut = 2 * WINDOW_SAMPLES; // 160ms lost -- the empirically worst-case still-recoverable amount
        assert!(
            cut < samples.len(),
            "fixture must be long enough to survive this truncation"
        );
        let truncated = &samples[cut..];

        let mut without_warm_up =
            BufferingWakeWordEngine::new("hey_jarvis".to_string(), 0.5, new_classifier());
        let baseline = feed_all_frames(&mut without_warm_up, truncated);
        assert!(
            baseline.is_empty(),
            "sanity check failed: without warm_up, 160ms of lost leading audio was expected \
             to fail detection (this locks in the documented bug/baseline) but got {baseline:?}"
        );

        let mut with_warm_up =
            BufferingWakeWordEngine::new("hey_jarvis".to_string(), 0.5, new_classifier());
        with_warm_up.warm_up();
        let fixed = feed_all_frames(&mut with_warm_up, truncated);
        assert!(
            !fixed.is_empty(),
            "warm_up() must recover detection once the ZERO_GUARD_FRAMES window is \
             pre-consumed on synthetic silence instead of the user's real (partially-lost) \
             speech, but got no detections"
        );
    }

    /// Corner case the above fix must not trade away: calling `warm_up()`
    /// before real audio starts flowing must never manufacture a false
    /// detection on silence or on unrelated speech. `warm_up()` only feeds
    /// silent windows, so it must be indistinguishable from the no-warm-up
    /// case for anything that isn't the wake phrase itself.
    #[test]
    fn warm_up_does_not_cause_false_positives_on_silence_or_unrelated_speech() {
        let mut silence_engine =
            BufferingWakeWordEngine::new("hey_jarvis".to_string(), 0.5, new_classifier());
        silence_engine.warm_up();
        let silence = vec![0.0_f32; 1280 * 50];
        assert!(
            feed_all_frames(&mut silence_engine, &silence).is_empty(),
            "warm_up() followed by silence must never detect"
        );

        let mut unrelated_engine =
            BufferingWakeWordEngine::new("hey_jarvis".to_string(), 0.5, new_classifier());
        unrelated_engine.warm_up();
        let unrelated = pcm_bytes_to_f32(UNRELATED_PCM);
        assert!(
            feed_all_frames(&mut unrelated_engine, &unrelated).is_empty(),
            "warm_up() followed by unrelated real speech must never detect"
        );
    }
}
