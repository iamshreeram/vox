//! Per-turn latency report and rolling statistics (design D4, A7).
//! Numbers and enums only: no text can enter these types.

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
        let endpoint_wait_ms = timing.and_then(|t| t.endpoint_wait_ms);
        let transcribe_ms = timing.and_then(|t| t.transcribe_ms);
        let verify_ms = timing.and_then(|t| t.verify_ms);
        let mode = timing.map(|t| t.mode).unwrap_or_default();
        // Total needs every mandatory instant; verify is optional and adds only when present.
        let total_ms = match (endpoint_wait_ms, transcribe_ms, brain_ms, to_speech_ms) {
            (Some(ep), Some(tr), Some(br), Some(ts)) => {
                let mut sum = ep.saturating_add(tr).saturating_add(br).saturating_add(ts);
                if let Some(vf) = verify_ms {
                    sum = sum.saturating_add(vf);
                }
                Some(sum)
            }
            _ => None,
        };
        Self {
            turn,
            endpoint_wait_ms,
            transcribe_ms,
            verify_ms,
            brain_ms,
            to_speech_ms,
            total_ms,
            mode,
        }
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

/// Median and p90 of `values` (sorted in place). Empty input gives all-None.
fn field_stats(mut values: Vec<u32>) -> FieldStats {
    let n = values.len();
    if n == 0 {
        return FieldStats::default();
    }
    values.sort_unstable();
    // median = sorted[(n-1)/2]; p90 = sorted[ceil(0.9*n)-1] = sorted[(9n+9)/10 - 1]
    let median_idx = (n - 1) / 2;
    let p90_idx = (9 * n + 9) / 10 - 1;
    FieldStats {
        median: values.get(median_idx).copied(),
        p90: values.get(p90_idx).copied(),
    }
}

/// Collect one field across the window, keeping Some values only.
fn collect_field(reports: &[LatencyReport], pick: fn(&LatencyReport) -> Option<u32>) -> FieldStats {
    field_stats(reports.iter().filter_map(pick).collect())
}

impl RollingStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a report, evicting the oldest once the window is full.
    pub fn push(&mut self, r: &LatencyReport) {
        if self.reports.len() >= WINDOW {
            self.reports.remove(0);
        }
        self.reports.push(r.clone());
    }

    /// Median = sorted[(n-1)/2], p90 = sorted[ceil(0.9*n)-1], per field over Some values only.
    pub fn stats(&self) -> LatencyStats {
        let reports = &self.reports;
        LatencyStats {
            endpoint_wait: collect_field(reports, |r| r.endpoint_wait_ms),
            transcribe: collect_field(reports, |r| r.transcribe_ms),
            verify: collect_field(reports, |r| r.verify_ms),
            brain: collect_field(reports, |r| r.brain_ms),
            to_speech: collect_field(reports, |r| r.to_speech_ms),
            total: collect_field(reports, |r| r.total_ms),
            samples: u32::try_from(reports.len()).unwrap_or(u32::MAX),
        }
    }

    pub fn reset(&mut self) {
        self.reports.clear();
    }

    pub fn len(&self) -> usize {
        self.reports.len()
    }
}

/// Payload of the tauri event [`EVENT_NAME`].
#[derive(Serialize, Debug, Clone, PartialEq, specta::Type)]
pub struct LatencyEvent {
    pub report: LatencyReport,
    pub stats: LatencyStats,
}

/// Tauri event name.
pub const EVENT_NAME: &str = "converse-latency";

/// Whether the converse-latency event is emitted for a turn.
pub fn event_enabled(setting_on: bool, report_present: bool) -> bool {
    setting_on && report_present
}

#[cfg(test)]
#[path = "converse_latency_tests.rs"]
mod tests;
