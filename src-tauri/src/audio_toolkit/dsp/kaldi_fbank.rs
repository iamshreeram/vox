//! Kaldi-style fbank frontend for the speaker embedding model (design D6a, A10).
//!
//! Reproduces `torchaudio.compliance.kaldi.fbank` as used by the WeSpeaker ResNet34
//! reference (`gen/common.py::frontend("wespeaker_ref")`):
//! samples x 32768, 80 mel bins, 25 ms frames (400) every 10 ms (160), snip_edges,
//! dither 0, remove DC offset per frame, pre-emphasis 0.97 (replicate-padded first
//! sample), symmetric Hamming window (alpha 0.54, beta 0.46), zero-pad to 512 (FFT),
//! power spectrum, Kaldi mel banks (1127 ln(1 + f/700), low 20 Hz, high = Nyquist,
//! triangular in mel domain, no VTLN), floor at f32::EPSILON, natural log.
//! Then per-bin mean subtraction over time (CMN) for the model input.
//!
//! STAGE OF THE FIXTURES: the committed `speaker/resnet34__wespeaker_ref/*.fbank.f32`
//! files store the output AFTER CMN (their per-column means are ~1e-6). The model
//! input is therefore `wespeaker_features` (fbank + CMN); `wespeaker_fbank` is the
//! pre-CMN stage.

use rustfft::{num_complex::Complex, FftPlanner};
use std::f64::consts::PI;

/// Feature dimension (mel bins) of the model input.
pub const FBANK_BINS: usize = 80;

const SAMPLE_RATE_HZ: f64 = 16000.0;
/// 25 ms at 16 kHz.
const FRAME_LEN: usize = 400;
/// 10 ms at 16 kHz.
const FRAME_SHIFT: usize = 160;
/// Next power of two >= FRAME_LEN (torchaudio round_to_power_of_two).
const FFT_LEN: usize = 512;
/// Spectrum bins DC..=Nyquist. The last (Nyquist) column of the mel banks is zero,
/// matching torchaudio's zero-padded `mel_energies`.
const SPEC_BINS: usize = FFT_LEN / 2 + 1;
const LOW_FREQ_HZ: f64 = 20.0;
/// torchaudio high_freq = 0 means Nyquist.
const HIGH_FREQ_HZ: f64 = SAMPLE_RATE_HZ / 2.0;
const PREEMPHASIS: f32 = 0.97;
/// WeSpeaker scales float samples in [-1, 1] to the int16 range.
const SAMPLE_SCALE: f32 = 32768.0;

/// Fbank features, row-major `[frames, FBANK_BINS]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Fbank {
    pub frames: usize,
    pub data: Vec<f32>,
}

/// Kaldi mel scale.
fn mel_scale(hz: f64) -> f64 {
    1127.0 * (1.0 + hz / 700.0).ln()
}

/// Triangular mel filters, `[FBANK_BINS, SPEC_BINS]` row-major, as torchaudio's
/// `get_mel_banks`: the filter edges are equally spaced on the mel scale and the
/// Nyquist column is zero.
fn mel_banks() -> Vec<f32> {
    let mel_low = mel_scale(LOW_FREQ_HZ);
    let mel_high = mel_scale(HIGH_FREQ_HZ);
    let delta = (mel_high - mel_low) / (FBANK_BINS as f64 + 1.0);
    let bin_width_hz = SAMPLE_RATE_HZ / FFT_LEN as f64;
    let mut banks = vec![0f32; FBANK_BINS * SPEC_BINS];
    for (i, row) in banks.chunks_mut(SPEC_BINS).enumerate() {
        let left = mel_low + i as f64 * delta;
        let center = mel_low + (i + 1) as f64 * delta;
        let right = mel_low + (i + 2) as f64 * delta;
        for (j, weight) in row.iter_mut().take(SPEC_BINS - 1).enumerate() {
            let mel = mel_scale(bin_width_hz * j as f64);
            let up = (mel - left) / (center - left);
            let down = (right - mel) / (right - center);
            *weight = up.min(down).max(0.0) as f32;
        }
    }
    banks
}

/// Symmetric Hamming window of length FRAME_LEN (torch.hamming_window, periodic=False).
fn hamming_window() -> Vec<f32> {
    let denom = (FRAME_LEN - 1) as f64;
    (0..FRAME_LEN)
        .map(|n| (0.54 - 0.46 * (2.0 * PI * n as f64 / denom).cos()) as f32)
        .collect()
}

/// Povey window: symmetric Hann ^ 0.85 (test-only variant used for negative controls).
#[cfg(test)]
fn povey_window() -> Vec<f32> {
    let denom = (FRAME_LEN - 1) as f64;
    (0..FRAME_LEN)
        .map(|n| (0.5 - 0.5 * (2.0 * PI * n as f64 / denom).cos()).powf(0.85) as f32)
        .collect()
}

/// Number of frames torchaudio produces with snip_edges=True (0 if the input is shorter).
fn frame_count(samples: usize) -> usize {
    if samples < FRAME_LEN {
        0
    } else {
        1 + (samples - FRAME_LEN) / FRAME_SHIFT
    }
}

/// Core fbank computation with a switchable window and pre-emphasis coefficient.
/// Non-finite samples are replaced by 0.0 first.
fn compute(samples: &[f32], window: &[f32], preemphasis: f32) -> Fbank {
    let clean: Vec<f32> = samples
        .iter()
        .map(|&s| if s.is_finite() { s } else { 0.0 })
        .collect();
    let banks = mel_banks();
    let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT_LEN);
    let mut data = Vec::with_capacity(frame_count(clean.len()) * FBANK_BINS);
    let mut buf = vec![Complex::new(0f32, 0f32); FFT_LEN];
    let mut frame = [0f32; FRAME_LEN];
    let mut power = [0f32; SPEC_BINS];

    for start in (0..frame_count(clean.len())).map(|f| f * FRAME_SHIFT) {
        let Some(src) = clean.get(start..start + FRAME_LEN) else {
            break;
        };
        for (dst, s) in frame.iter_mut().zip(src) {
            *dst = s * SAMPLE_SCALE;
        }
        // Remove the DC offset of this frame.
        let mean = (frame.iter().map(|&v| f64::from(v)).sum::<f64>() / FRAME_LEN as f64) as f32;
        for v in frame.iter_mut() {
            *v -= mean;
        }
        // Pre-emphasis y[i] = x[i] - c * x[i-1], with x[-1] replicated from x[0].
        let mut prev = frame.first().copied().unwrap_or(0.0);
        for (slot, (&x, &w)) in buf.iter_mut().zip(frame.iter().zip(window)) {
            let y = x - preemphasis * prev;
            prev = x;
            *slot = Complex::new(y * w, 0.0);
        }
        // Zero-pad to the FFT length (the tail beyond FRAME_LEN stays zero).
        for slot in buf.iter_mut().skip(FRAME_LEN) {
            *slot = Complex::new(0.0, 0.0);
        }
        fft.process(&mut buf);
        for (p, c) in power.iter_mut().zip(buf.iter()) {
            *p = c.norm_sqr();
        }
        for row in banks.chunks(SPEC_BINS) {
            let energy: f32 = row.iter().zip(power.iter()).map(|(w, p)| w * p).sum();
            data.push(energy.max(f32::EPSILON).ln());
        }
    }
    Fbank {
        frames: data.len() / FBANK_BINS,
        data,
    }
}

/// Log-mel fbank exactly as the WeSpeaker reference computes it BEFORE CMN.
/// Input: float samples in [-1, 1]; non-finite samples are treated as 0.0.
pub fn wespeaker_fbank(samples_pm1: &[f32]) -> Fbank {
    compute(samples_pm1, &hamming_window(), PREEMPHASIS)
}

/// Subtract the per-bin mean over time (Kaldi/WeSpeaker CMN). No-op for 0 frames.
pub fn apply_cmn(f: &mut Fbank) {
    if f.frames == 0 {
        return;
    }
    let mut sums = [0f64; FBANK_BINS];
    for row in f.data.chunks(FBANK_BINS) {
        for (sum, &v) in sums.iter_mut().zip(row) {
            *sum += f64::from(v);
        }
    }
    let means = sums.map(|s| (s / f.frames as f64) as f32);
    for row in f.data.chunks_mut(FBANK_BINS) {
        for (v, mean) in row.iter_mut().zip(means.iter()) {
            *v -= mean;
        }
    }
}

/// The speaker model input: fbank followed by CMN.
pub fn wespeaker_features(samples_pm1: &[f32]) -> Fbank {
    let mut f = wespeaker_fbank(samples_pm1);
    apply_cmn(&mut f);
    f
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq)]
enum WindowKind {
    Hamming,
    Povey,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq)]
struct FbankVariant {
    window: WindowKind,
    preemphasis: f32,
}

/// Test hook: same pipeline with a switchable window and pre-emphasis, so the tests can
/// prove that wrong variants are detected by the golden tolerances.
#[cfg(test)]
fn fbank_with(samples: &[f32], cfg: &FbankVariant) -> Fbank {
    let window = match cfg.window {
        WindowKind::Hamming => hamming_window(),
        WindowKind::Povey => povey_window(),
    };
    compute(samples, &window, cfg.preemphasis)
}

// The test file sits next to this file (dsp/), not in a dsp/kaldi_fbank/ directory,
// so the module needs an explicit path.
#[cfg(test)]
#[path = "kaldi_fbank_tests.rs"]
mod kaldi_fbank_tests;
