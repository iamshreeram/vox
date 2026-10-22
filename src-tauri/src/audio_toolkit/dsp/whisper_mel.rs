//! Whisper-style log-mel frontend for the Smart Turn model (design D3a, A5). Filled by step S1.
//!
//! Follows the official Smart Turn preprocessing: keep the last 8 s (left-pad with zeros),
//! per-buffer waveform normalisation, centred STFT (n_fft 400, hop 160, periodic Hann),
//! 80 Slaney mel bins (area-normalised), log10, drop the last frame, clamp to 8 nats below the
//! maximum and rescale to roughly [-1, 1].

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::f64::consts::PI;
use std::sync::{Arc, OnceLock};

/// Smart Turn analysis window: 8 seconds at 16 kHz.
pub const SMART_TURN_SAMPLES: usize = 128_000;
/// Number of mel bins expected by Smart Turn.
pub const SMART_TURN_MELS: usize = 80;
/// Number of time frames expected by Smart Turn (after the last STFT frame is dropped).
pub const SMART_TURN_FRAMES: usize = 800;

const N_FFT: usize = 400;
const HOP: usize = 160;
const N_BINS: usize = N_FFT / 2 + 1;
const CENTER_PAD: usize = N_FFT / 2;
const SAMPLE_RATE_HZ: f64 = 16_000.0;
const F_MIN_HZ: f64 = 0.0;
const F_MAX_HZ: f64 = SAMPLE_RATE_HZ / 2.0;
const MEL_FLOOR: f32 = 1e-10;
const DYNAMIC_RANGE: f32 = 8.0;
const NORM_EPS: f64 = 1e-7;

// Slaney mel scale: linear below 1 kHz, logarithmic above.
const SLANEY_F_SP: f64 = 200.0 / 3.0;
const SLANEY_MIN_LOG_HZ: f64 = 1000.0;
const SLANEY_LOGSTEP: f64 = 0.068_751_777_420_949_16; // ln(6.4) / 27

struct MelFilter {
    /// First FFT bin with a non-zero weight.
    start: usize,
    weights: Vec<f32>,
}

struct Tables {
    window: Vec<f32>,
    filters: Vec<MelFilter>,
    fft: Arc<dyn Fft<f32>>,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(build_tables)
}

fn build_tables() -> Tables {
    // Periodic Hann: np.hanning(N + 1)[:-1].
    let window = (0..N_FFT)
        .map(|n| (0.5 - 0.5 * (2.0 * PI * n as f64 / N_FFT as f64).cos()) as f32)
        .collect();
    let fft = FftPlanner::new().plan_fft_forward(N_FFT);
    Tables {
        window,
        filters: slaney_filters(),
        fft,
    }
}

fn hz_to_slaney_mel(hz: f64) -> f64 {
    let min_log_mel = SLANEY_MIN_LOG_HZ / SLANEY_F_SP;
    if hz >= SLANEY_MIN_LOG_HZ {
        min_log_mel + (hz / SLANEY_MIN_LOG_HZ).ln() / SLANEY_LOGSTEP
    } else {
        hz / SLANEY_F_SP
    }
}

fn slaney_mel_to_hz(mel: f64) -> f64 {
    let min_log_mel = SLANEY_MIN_LOG_HZ / SLANEY_F_SP;
    if mel >= min_log_mel {
        SLANEY_MIN_LOG_HZ * (SLANEY_LOGSTEP * (mel - min_log_mel)).exp()
    } else {
        mel * SLANEY_F_SP
    }
}

/// Triangular Slaney-scale mel filters with area normalisation, one row per mel bin.
fn slaney_filters() -> Vec<MelFilter> {
    let mel_lo = hz_to_slaney_mel(F_MIN_HZ);
    let mel_hi = hz_to_slaney_mel(F_MAX_HZ);
    let points = SMART_TURN_MELS + 2;
    // Filter edge frequencies: linspace(mel_lo, mel_hi, points) mapped back to Hz.
    let edges: Vec<f64> = (0..points)
        .map(|i| {
            let mel = mel_lo + (mel_hi - mel_lo) * i as f64 / (points - 1) as f64;
            slaney_mel_to_hz(mel)
        })
        .collect();

    (0..SMART_TURN_MELS)
        .map(|m| {
            let lo = edges[m];
            let centre = edges[m + 1];
            let hi = edges[m + 2];
            let enorm = 2.0 / (hi - lo);
            let raw: Vec<f64> = (0..N_BINS)
                .map(|k| {
                    let freq = k as f64 * F_MAX_HZ / (N_BINS - 1) as f64;
                    let rising = (freq - lo) / (centre - lo);
                    let falling = (hi - freq) / (hi - centre);
                    rising.min(falling).max(0.0) * enorm
                })
                .collect();
            let start = raw.iter().position(|&w| w > 0.0).unwrap_or(0);
            let end = raw.iter().rposition(|&w| w > 0.0).map_or(start, |e| e + 1);
            let weights = raw
                .get(start..end)
                .unwrap_or(&[])
                .iter()
                .map(|&w| w as f32)
                .collect();
            MelFilter { start, weights }
        })
        .collect()
}

/// Keep the last `SMART_TURN_SAMPLES` (left-pad with zeros), sanitise non-finite samples
/// to zero and normalise the whole buffer to zero mean and unit variance.
fn normalised_window(audio16k: &[f32]) -> Vec<f32> {
    let kept_from = audio16k.len().saturating_sub(SMART_TURN_SAMPLES);
    let kept = audio16k.get(kept_from..).unwrap_or(&[]);
    let pad = SMART_TURN_SAMPLES - kept.len();

    let mut samples = Vec::with_capacity(SMART_TURN_SAMPLES);
    samples.resize(pad, 0.0f64);
    samples.extend(
        kept.iter()
            .map(|&x| if x.is_finite() { f64::from(x) } else { 0.0 }),
    );

    let n = SMART_TURN_SAMPLES as f64;
    let mean = samples.iter().sum::<f64>() / n;
    let var = samples
        .iter()
        .map(|&x| (x - mean) * (x - mean))
        .sum::<f64>()
        / n;
    let denom = (var + NORM_EPS).sqrt();
    samples
        .iter()
        .map(|&x| ((x - mean) / denom) as f32)
        .collect()
}

/// Reflect-pad by `CENTER_PAD` on both sides (numpy 'reflect' mode, no edge repeat).
fn reflect_pad(signal: &[f32]) -> Vec<f32> {
    let n = signal.len() as isize;
    let pad = CENTER_PAD as isize;
    (0..n + 2 * pad)
        .map(|i| {
            let mut j = i - pad;
            if j < 0 {
                j = -j;
            }
            if j >= n {
                j = 2 * (n - 1) - j;
            }
            usize::try_from(j)
                .ok()
                .and_then(|j| signal.get(j).copied())
                .unwrap_or(0.0)
        })
        .collect()
}

/// Power spectra of two real frames `a` and `b` that were packed into one complex FFT
/// (`a` in the real part, `b` in the imaginary part). Uses conjugate symmetry:
/// A[k] = (Z[k] + conj Z[N-k]) / 2, B[k] = (Z[k] - conj Z[N-k]) / (2i).
fn split_two_real_spectra(packed: &[Complex<f32>], pa: &mut [f32], pb: &mut [f32]) {
    for (k, (a, b)) in pa.iter_mut().zip(pb.iter_mut()).enumerate() {
        let zk = packed.get(k).copied().unwrap_or_default();
        let zn = packed.get((N_FFT - k) % N_FFT).copied().unwrap_or_default();
        // Z[N-k] conjugated.
        let (cr, ci) = (zn.re, -zn.im);
        let sum_re = (zk.re + cr) * 0.5;
        let sum_im = (zk.im + ci) * 0.5;
        let dif_re = (zk.re - cr) * 0.5;
        let dif_im = (zk.im - ci) * 0.5;
        // dif / (i): (x + iy) / i = y - ix.
        *a = sum_re * sum_re + sum_im * sum_im;
        *b = dif_im * dif_im + dif_re * dif_re;
    }
}

/// Adds one log10 mel column per filter row for the given power spectrum.
fn push_log_mel(rows: &mut [Vec<f32>], filters: &[MelFilter], power: &[f32]) {
    for (row, filter) in rows.iter_mut().zip(filters) {
        let energy: f32 = power
            .iter()
            .skip(filter.start)
            .zip(&filter.weights)
            .map(|(p, w)| p * w)
            .sum();
        row.push(energy.max(MEL_FLOOR).log10());
    }
}

/// Smart Turn log-mel features of 16 kHz mono audio.
///
/// Returns exactly `SMART_TURN_MELS * SMART_TURN_FRAMES` values, row-major `[mel][time]`,
/// finite for every input (empty, short, long, NaN or infinite samples).
pub fn smart_turn_features(audio16k: &[f32]) -> Vec<f32> {
    let tables = tables();
    let padded = reflect_pad(&normalised_window(audio16k));

    let mut fft_buf = vec![Complex::new(0f32, 0f32); N_FFT];
    let mut scratch = vec![Complex::new(0f32, 0f32); tables.fft.get_inplace_scratch_len()];
    let mut power_a = [0f32; N_BINS];
    let mut power_b = [0f32; N_BINS];
    let mut rows: Vec<Vec<f32>> = (0..SMART_TURN_MELS)
        .map(|_| Vec::with_capacity(SMART_TURN_FRAMES))
        .collect();

    // Frames are processed in pairs: two real frames share one complex FFT.
    for pair in 0..SMART_TURN_FRAMES / 2 {
        let first = 2 * pair * HOP;
        let second = first + HOP;
        let (Some(xa), Some(xb)) = (
            padded.get(first..first + N_FFT),
            padded.get(second..second + N_FFT),
        ) else {
            break;
        };
        for (((slot, &a), &b), &w) in fft_buf.iter_mut().zip(xa).zip(xb).zip(&tables.window) {
            *slot = Complex::new(a * w, b * w);
        }
        tables.fft.process_with_scratch(&mut fft_buf, &mut scratch);
        split_two_real_spectra(&fft_buf, &mut power_a, &mut power_b);
        push_log_mel(&mut rows, &tables.filters, &power_a);
        push_log_mel(&mut rows, &tables.filters, &power_b);
    }

    let max = rows
        .iter()
        .flatten()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    let floor = max - DYNAMIC_RANGE;
    for v in rows.iter_mut().flatten() {
        *v = (v.max(floor) + 4.0) / 4.0;
    }
    rows.concat()
}

#[cfg(test)]
#[path = "whisper_mel_tests.rs"]
mod whisper_mel_tests;
