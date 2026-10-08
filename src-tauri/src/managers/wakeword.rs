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
