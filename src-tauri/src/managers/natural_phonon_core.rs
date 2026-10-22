//! Platform-neutral helpers for the Phonon TTS engine (DESIGN.md Rev 4, P3).
//!
//! Pure functions only: no I/O, no threads, no ptts types. The macOS-only glue that calls
//! the `ptts` crate lives in `natural_phonon.rs`. Spec = `natural_phonon_core_tests.rs`.
#![allow(dead_code)] // Consumers (natural_phonon.rs, router) land in later P3/P4 steps.

/// Native sample rate of the Phonon model (mono, float32 PCM).
pub const PHONON_SAMPLE_RATE: u32 = 24_000;

/// True when `text` contains at least one alphanumeric char. Text without one (empty,
/// whitespace, punctuation, emoji, symbols) is not worth an engine call.
pub fn has_speakable_chars(text: &str) -> bool {
    text.chars().any(char::is_alphanumeric)
}

/// Makes engine output safe to play: NaN and +/-inf become 0.0, finite samples are
/// clamped to [-1.0, 1.0].
// The `&mut Vec<f32>` signature is the frozen contract (natural_phonon_core_tests.rs).
#[allow(clippy::ptr_arg)]
pub fn sanitize_pcm(pcm: &mut Vec<f32>) {
    for sample in pcm.iter_mut() {
        *sample = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
    }
}

/// Removes the quiet tail after the last sample whose magnitude reaches
/// `threshold_dbfs` (linear threshold `10^(dbfs/20)`), keeping up to `keep_ms` of audio
/// after it. Interior samples are never touched.
///
/// If no sample reaches the threshold (all quiet or empty input) the buffer is cleared.
pub fn trim_trailing_silence(
    pcm: &mut Vec<f32>,
    sample_rate: u32,
    threshold_dbfs: f32,
    keep_ms: u32,
) {
    let threshold_lin = 10f32.powf(threshold_dbfs / 20.0);
    let Some(last_audible) = pcm.iter().rposition(|sample| sample.abs() >= threshold_lin) else {
        pcm.clear();
        return;
    };
    let keep_samples = u64::from(sample_rate) * u64::from(keep_ms) / 1000;
    let end = (last_audible as u64 + 1)
        .saturating_add(keep_samples)
        .min(pcm.len() as u64);
    pcm.truncate(end as usize);
}
