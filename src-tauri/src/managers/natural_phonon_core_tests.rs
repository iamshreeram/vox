//! Contract tests for the platform-neutral Phonon helpers (DESIGN.md Rev 4, P3).
//! These are the authoritative spec: do not weaken or delete assertions.

use super::natural_phonon_core::*;

const SR: u32 = PHONON_SAMPLE_RATE;

fn samples(ms: u32) -> usize {
    (SR as usize) * (ms as usize) / 1000
}

/// Constant-amplitude "speech" so the last audible sample is exactly known.
fn loud(ms: u32) -> Vec<f32> {
    vec![0.5; samples(ms)]
}

fn quiet(ms: u32) -> Vec<f32> {
    vec![0.0; samples(ms)]
}

fn cat(parts: &[Vec<f32>]) -> Vec<f32> {
    parts.iter().flat_map(|p| p.iter().copied()).collect()
}

#[test]
fn sample_rate_is_24khz() {
    assert_eq!(PHONON_SAMPLE_RATE, 24_000);
}

#[test]
fn speakable_text_is_detected() {
    for text in ["Hello", "  hi ", "42", "\u{1F600} ok", "你好", "مرحبا", "a"] {
        assert!(has_speakable_chars(text), "{text:?} must be speakable");
    }
}

#[test]
fn unspeakable_text_is_rejected_without_calling_the_engine() {
    for text in ["", "   ", "\n\t", "...!?", "\u{1F600}", "\0", "—", "()"] {
        assert!(!has_speakable_chars(text), "{text:?} must NOT be speakable");
    }
}

#[test]
fn sanitize_replaces_non_finite_and_clamps() {
    let mut pcm = vec![
        0.25,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        2.5,
        -3.0,
        -0.75,
    ];
    sanitize_pcm(&mut pcm);
    assert_eq!(pcm, vec![0.25, 0.0, 0.0, 0.0, 1.0, -1.0, -0.75]);
}

#[test]
fn sanitize_keeps_empty_empty() {
    let mut pcm: Vec<f32> = Vec::new();
    sanitize_pcm(&mut pcm);
    assert!(pcm.is_empty());
}

#[test]
fn trim_removes_long_quiet_tail_but_keeps_requested_margin() {
    let mut pcm = cat(&[loud(500), quiet(800)]);
    trim_trailing_silence(&mut pcm, SR, -50.0, 150);
    assert_eq!(pcm.len(), samples(500) + samples(150));
}

#[test]
fn trim_leaves_short_tail_untouched() {
    let mut pcm = cat(&[loud(500), quiet(100)]);
    let before = pcm.len();
    trim_trailing_silence(&mut pcm, SR, -50.0, 150);
    assert_eq!(pcm.len(), before);
}

#[test]
fn trim_leaves_audio_without_tail_untouched() {
    let mut pcm = loud(400);
    trim_trailing_silence(&mut pcm, SR, -50.0, 150);
    assert_eq!(pcm.len(), samples(400));
}

#[test]
fn trim_preserves_interior_silence() {
    let mut pcm = cat(&[loud(200), quiet(400), loud(200), quiet(800)]);
    trim_trailing_silence(&mut pcm, SR, -50.0, 150);
    assert_eq!(pcm.len(), samples(200 + 400 + 200) + samples(150));
}

#[test]
fn trim_of_all_silence_is_empty() {
    let mut pcm = quiet(900);
    trim_trailing_silence(&mut pcm, SR, -50.0, 150);
    assert!(pcm.is_empty());
}

#[test]
fn trim_of_empty_is_empty() {
    let mut pcm: Vec<f32> = Vec::new();
    trim_trailing_silence(&mut pcm, SR, -50.0, 150);
    assert!(pcm.is_empty());
}

#[test]
fn trim_with_zero_margin_cuts_right_after_last_audible_sample() {
    let mut pcm = cat(&[loud(300), quiet(500)]);
    trim_trailing_silence(&mut pcm, SR, -50.0, 0);
    assert_eq!(pcm.len(), samples(300));
}

#[test]
fn trim_treats_low_level_dither_as_silence() {
    // 0.001 is about -60 dBFS: below the -50 dBFS threshold.
    let mut pcm = cat(&[loud(300), vec![0.001; samples(1000)]]);
    trim_trailing_silence(&mut pcm, SR, -50.0, 150);
    assert_eq!(pcm.len(), samples(300) + samples(150));
}

#[test]
fn trim_threshold_boundary_is_minus_50_dbfs() {
    // -50 dBFS is 10^(-50/20) = 0.0031623.
    let mut audible = cat(&[vec![0.0032; 10], quiet(800)]);
    trim_trailing_silence(&mut audible, SR, -50.0, 0);
    assert_eq!(audible.len(), 10, "0.0032 is above the threshold");

    let mut inaudible = cat(&[vec![0.0030; 10], quiet(800)]);
    trim_trailing_silence(&mut inaudible, SR, -50.0, 0);
    assert!(inaudible.is_empty(), "0.0030 is below the threshold");
}

#[test]
fn trim_negative_samples_count_by_magnitude() {
    let mut pcm = cat(&[vec![-0.5; 100], quiet(800)]);
    trim_trailing_silence(&mut pcm, SR, -50.0, 0);
    assert_eq!(pcm.len(), 100);
}
