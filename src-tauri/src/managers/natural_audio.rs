//! Pure audio helpers for the natural voice (24 kHz mono f32 in, device-rate mono f32 out).
//!
//! Spec = `natural_audio_tests.rs`.
#![allow(dead_code)]

use rubato::{FftFixedIn, Resampler};

/// Samples whose absolute value is at or below this are "silence" for trimming.
pub const TRIM_THRESHOLD: f32 = 0.01;
/// Padding kept on both sides of the audible part when trimming.
pub const TRIM_PAD_MS: u32 = 20;

/// Input chunk size handed to the FFT resampler.
const RESAMPLE_CHUNK: usize = 1024;
/// Safety bound on the number of zero-input flush calls used to drain the resampler.
const MAX_FLUSH_CALLS: usize = 64;

/// Returns the sub-slice from `TRIM_PAD_MS` before the first sample above `TRIM_THRESHOLD`
/// to `TRIM_PAD_MS` after the last one (clamped to the slice). All-silent or empty input
/// returns an empty slice.
pub fn trim_silence(samples: &[f32], sample_rate: u32) -> &[f32] {
    let audible = |s: &f32| s.abs() > TRIM_THRESHOLD;
    let (Some(first), Some(last)) = (
        samples.iter().position(audible),
        samples.iter().rposition(audible),
    ) else {
        return &[];
    };
    let pad = (u64::from(sample_rate) * u64::from(TRIM_PAD_MS) / 1000) as usize;
    let start = first.saturating_sub(pad);
    let end = last
        .saturating_add(1)
        .saturating_add(pad)
        .min(samples.len());
    samples.get(start..end).unwrap_or(&[])
}

/// Mono resampling (use `rubato`, already a dependency). Same rate returns a copy; empty
/// input returns an empty Vec; a zero rate returns `Err`. The output length must be
/// `input_len * to / from` within 1% (compensate the resampler delay/padding) and the
/// signal level must be preserved.
pub fn resample_mono(samples: &[f32], from_hz: u32, to_hz: u32) -> Result<Vec<f32>, String> {
    if from_hz == 0 || to_hz == 0 {
        return Err("sample rate must be non-zero".to_string());
    }
    if samples.is_empty() {
        return Ok(Vec::new());
    }
    if from_hz == to_hz {
        return Ok(samples.to_vec());
    }
    let expected = (samples.len() as u64 * u64::from(to_hz) / u64::from(from_hz)) as usize;
    let mut resampler =
        FftFixedIn::<f32>::new(from_hz as usize, to_hz as usize, RESAMPLE_CHUNK, 2, 1)
            .map_err(|e| format!("resampler construction failed: {e}"))?;
    let delay = resampler.output_delay();
    let mut out = run_resampler(&mut resampler, samples)?;
    drain_resampler(&mut resampler, &mut out, delay + expected)?;
    Ok(out.into_iter().skip(delay).take(expected).collect())
}

/// Feeds every input sample through `resampler` (last partial chunk zero-padded).
fn run_resampler(resampler: &mut FftFixedIn<f32>, samples: &[f32]) -> Result<Vec<f32>, String> {
    let mut out = Vec::new();
    let mut rest = samples;
    loop {
        let need = resampler.input_frames_next();
        let chunk = if rest.len() >= need {
            let (head, tail) = rest.split_at(need);
            rest = tail;
            resampler.process(&[head], None)
        } else if !rest.is_empty() {
            let head = rest;
            rest = &[];
            resampler.process_partial(Some(&[head]), None)
        } else {
            break;
        };
        append_first_channel(&mut out, chunk)?;
    }
    Ok(out)
}

/// Pushes zero input until `out` holds at least `wanted` samples (bounded).
fn drain_resampler(
    resampler: &mut FftFixedIn<f32>,
    out: &mut Vec<f32>,
    wanted: usize,
) -> Result<(), String> {
    for _ in 0..MAX_FLUSH_CALLS {
        if out.len() >= wanted {
            break;
        }
        let chunk = resampler.process_partial::<&[f32]>(None, None);
        append_first_channel(out, chunk)?;
    }
    Ok(())
}

/// Appends the first channel of a resampler result to `out`.
fn append_first_channel(
    out: &mut Vec<f32>,
    chunk: rubato::ResampleResult<Vec<Vec<f32>>>,
) -> Result<(), String> {
    let channels = chunk.map_err(|e| format!("resampling failed: {e}"))?;
    if let Some(first) = channels.first() {
        out.extend_from_slice(first);
    }
    Ok(())
}

/// Kokoro speed factor from the app's words-per-minute setting: `None` or 180 wpm = 1.0,
/// linear `wpm / 180`, clamped to `0.7..=1.3` (0 wpm clamps to 0.7).
pub fn wpm_to_speed(rate_wpm: Option<u32>) -> f32 {
    match rate_wpm {
        None => 1.0,
        Some(wpm) => (wpm as f32 / 180.0).clamp(0.7, 1.3),
    }
}
