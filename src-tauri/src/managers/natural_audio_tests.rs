//! Spec for `natural_audio` (step S1). Authoritative: do not weaken an assertion.
use crate::managers::natural_audio::*;

fn sine(freq: f32, sample_rate: u32, secs: f32, amp: f32) -> Vec<f32> {
    let n = (sample_rate as f32 * secs) as usize;
    (0..n)
        .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / sample_rate as f32).sin())
        .collect()
}

fn rms(s: &[f32]) -> f32 {
    (s.iter().map(|x| x * x).sum::<f32>() / s.len().max(1) as f32).sqrt()
}

#[test]
fn trim_removes_leading_and_trailing_silence_but_keeps_a_pad() {
    let sr = 24_000;
    let pad = (sr as usize * TRIM_PAD_MS as usize) / 1000; // 480
    let tone = sine(440.0, sr, 0.5, 0.5);
    let mut clip = vec![0.0f32; 8_000];
    clip.extend_from_slice(&tone);
    clip.extend(vec![0.0f32; 9_000]);
    let trimmed = trim_silence(&clip, sr);
    assert!(
        trimmed.len() >= tone.len(),
        "must keep every audible sample"
    );
    assert!(
        trimmed.len() <= tone.len() + 2 * pad + 2,
        "len {}",
        trimmed.len()
    );
    assert!(trimmed.iter().any(|s| s.abs() > 0.4));
}

#[test]
fn trim_all_silence_or_empty_returns_empty() {
    assert!(trim_silence(&vec![0.0f32; 5_000], 24_000).is_empty());
    assert!(trim_silence(&vec![TRIM_THRESHOLD * 0.5; 5_000], 24_000).is_empty());
    assert!(trim_silence(&[], 24_000).is_empty());
}

#[test]
fn trim_leaves_a_clip_without_silence_unchanged() {
    let tone = sine(300.0, 24_000, 0.2, 0.5);
    assert_eq!(trim_silence(&tone, 24_000).len(), tone.len());
}

#[test]
fn trim_clamps_the_pad_at_the_slice_edges() {
    let mut clip = vec![0.0f32; 100];
    clip.extend(vec![0.5f32; 100]);
    let trimmed = trim_silence(&clip, 24_000);
    assert!(trimmed.len() <= 200);
    assert!(trimmed.len() >= 100);
}

#[test]
fn wpm_mapping_and_clamping() {
    assert_eq!(wpm_to_speed(None), 1.0);
    assert_eq!(wpm_to_speed(Some(180)), 1.0);
    assert!((wpm_to_speed(Some(200)) - 200.0 / 180.0).abs() < 1e-3);
    assert_eq!(wpm_to_speed(Some(90)), 0.7);
    assert_eq!(wpm_to_speed(Some(0)), 0.7);
    assert_eq!(wpm_to_speed(Some(360)), 1.3);
    assert_eq!(wpm_to_speed(Some(u32::MAX)), 1.3);
}

#[test]
fn resample_same_rate_is_a_copy() {
    let tone = sine(440.0, 24_000, 0.1, 0.5);
    assert_eq!(resample_mono(&tone, 24_000, 24_000).unwrap(), tone);
}

#[test]
fn resample_empty_is_empty_and_zero_rate_is_an_error() {
    assert!(resample_mono(&[], 24_000, 48_000).unwrap().is_empty());
    assert!(resample_mono(&[0.1; 100], 0, 48_000).is_err());
    assert!(resample_mono(&[0.1; 100], 24_000, 0).is_err());
}

#[test]
fn resample_length_and_level_for_common_device_rates() {
    for to in [44_100u32, 48_000, 96_000] {
        let input = sine(440.0, 24_000, 1.0, 0.5);
        let out = resample_mono(&input, 24_000, to).unwrap();
        let expected = (input.len() as f64 * to as f64 / 24_000.0) as i64;
        let tolerance = expected / 100; // 1%
        assert!(
            (out.len() as i64 - expected).abs() <= tolerance,
            "to {to}: len {} expected {expected}",
            out.len()
        );
        let ratio = rms(&out) / rms(&input);
        assert!((0.9..=1.1).contains(&ratio), "to {to}: level ratio {ratio}");
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak <= 0.6, "to {to}: peak {peak}");
    }
}

#[test]
fn resample_handles_a_very_short_clip() {
    let out = resample_mono(&sine(440.0, 24_000, 0.005, 0.5), 24_000, 48_000).unwrap();
    assert!(!out.is_empty());
}
