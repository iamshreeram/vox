//! Per-turn latency report and rolling statistics (design D4, A7).
//! STUB (step T7): signatures only, bodies are `todo!()`. Implementation is a later step.

use serde::{Deserialize, Serialize};

/// Endpointing mode used for a listen. Serialized as "fixed" | "smart_turn" | "smart_turn_fallback".
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, specta::Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum EndpointMode {
    #[default]
    Fixed,
    SmartTurn,
    SmartTurnFallback,
}

/// Timing captured by the capture worker for one listen. Numbers and enums only.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq, specta::Type, Default)]
pub struct ListenTiming {
    pub endpoint_wait_ms: Option<u32>,
    pub transcribe_ms: Option<u32>,
    pub verify_ms: Option<u32>,
    pub mode: EndpointMode,
}

/// One completed turn's latency. Numbers and enums only (no text fields).
#[derive(Serialize, Debug, Clone, PartialEq, specta::Type)]
pub struct LatencyReport {
    pub turn: u32,
    pub endpoint_wait_ms: Option<u32>,
    pub transcribe_ms: Option<u32>,
    pub verify_ms: Option<u32>,
    pub brain_ms: Option<u32>,
    pub to_speech_ms: Option<u32>,
    pub total_ms: Option<u32>,
    pub mode: EndpointMode,
}

impl LatencyReport {
    /// Build a report. A missing `timing` yields every listen field None and mode Fixed.
    pub fn build(
        turn: u32,
        timing: Option<&ListenTiming>,
        brain_ms: Option<u32>,
        to_speech_ms: Option<u32>,
    ) -> Self {
        let _ = (turn, timing, brain_ms, to_speech_ms);
        todo!("converse_latency::LatencyReport::build")
    }
}

/// Number of most recent reports kept by [`RollingStats`].
pub const WINDOW: usize = 20;

/// Median and p90 for one field over the window (None when no Some values).
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq, specta::Type, Default)]
pub struct FieldStats {
    pub median: Option<u32>,
    pub p90: Option<u32>,
}

/// Per-field statistics over the rolling window.
#[derive(Serialize, Debug, Clone, PartialEq, Eq, specta::Type)]
pub struct LatencyStats {
    pub endpoint_wait: FieldStats,
    pub transcribe: FieldStats,
    pub verify: FieldStats,
    pub brain: FieldStats,
    pub to_speech: FieldStats,
    pub total: FieldStats,
    pub samples: u32,
}

/// Rolling window (last [`WINDOW`] reports) with per-field median/p90.
#[derive(Debug, Clone, Default)]
pub struct RollingStats {
    reports: Vec<LatencyReport>,
}

impl RollingStats {
    pub fn new() -> Self {
        todo!("converse_latency::RollingStats::new")
    }

    /// Push a report, evicting the oldest once the window is full.
    pub fn push(&mut self, r: &LatencyReport) {
        let _ = r;
        todo!("converse_latency::RollingStats::push")
    }

    /// Median = sorted[(n-1)/2], p90 = sorted[ceil(0.9*n)-1], per field over Some values only.
    pub fn stats(&self) -> LatencyStats {
        todo!("converse_latency::RollingStats::stats")
    }

    pub fn reset(&mut self) {
        todo!("converse_latency::RollingStats::reset")
    }

    pub fn len(&self) -> usize {
        todo!("converse_latency::RollingStats::len")
    }
}

/// Payload of the tauri event [`EVENT_NAME`].
#[derive(Serialize, Debug, Clone, PartialEq, specta::Type)]
pub struct LatencyEvent {
    pub report: LatencyReport,
    pub stats: LatencyStats,
}

/// Tauri event name. Placeholder value in the stub so the EVENT_NAME test is red.
pub const EVENT_NAME: &str = "";

/// Whether the converse-latency event is emitted for a turn.
pub fn event_enabled(setting_on: bool, report_present: bool) -> bool {
    let _ = (setting_on, report_present);
    todo!("converse_latency::event_enabled")
}

#[cfg(test)]
#[path = "converse_latency_tests.rs"]
mod tests;
