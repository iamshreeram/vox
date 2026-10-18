//! Pure audio helpers for the natural voice (24 kHz mono f32 in, device-rate mono f32 out).
//!
//! Spec = `natural_audio_tests.rs`. STUB: bodies are `todo!()` until step S1.
#![allow(dead_code)]

/// Samples whose absolute value is at or below this are "silence" for trimming.
pub const TRIM_THRESHOLD: f32 = 0.01;
/// Padding kept on both sides of the audible part when trimming.
pub const TRIM_PAD_MS: u32 = 20;

/// Returns the sub-slice from `TRIM_PAD_MS` before the first sample above `TRIM_THRESHOLD`
/// to `TRIM_PAD_MS` after the last one (clamped to the slice). All-silent or empty input
/// returns an empty slice.
pub fn trim_silence(samples: &[f32], sample_rate: u32) -> &[f32] {
    let _ = (samples, sample_rate);
    todo!("S1")
}

/// Mono resampling (use `rubato`, already a dependency). Same rate returns a copy; empty
/// input returns an empty Vec; a zero rate returns `Err`. The output length must be
/// `input_len * to / from` within 1% (compensate the resampler delay/padding) and the
/// signal level must be preserved.
pub fn resample_mono(samples: &[f32], from_hz: u32, to_hz: u32) -> Result<Vec<f32>, String> {
    let _ = (samples, from_hz, to_hz);
    todo!("S1")
}

/// Kokoro speed factor from the app's words-per-minute setting: `None` or 180 wpm = 1.0,
/// linear `wpm / 180`, clamped to `0.7..=1.3` (0 wpm clamps to 0.7).
pub fn wpm_to_speed(rate_wpm: Option<u32>) -> f32 {
    let _ = rate_wpm;
    todo!("S1")
}
