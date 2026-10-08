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
