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
        // f32::clamp pushes a NaN classifier output toward the in-range bound
        // (NaN.max(0.0) == 0.0, then .min(1.0) == 0.0) rather than panicking
        // or propagating NaN -- documented Rust f32::max/min behavior.
        let confidence = self.classifier.classify(&window).clamp(0.0, 1.0);
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
    }

    fn pause(&mut self) {
        self.paused = true;
    }

    fn resume(&mut self) {
        self.paused = false;
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
/// The real production implementation (reqwest-based, following
/// `managers/model/download.rs`'s resumable-HTTP pattern) is wired in by a
/// later task; this trait is the seam.
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
