//! Acceptance tests for the Smart Turn log-mel frontend (golden fixtures under
//! `tests/fixtures/voice/smartturn`). Included from `whisper_mel.rs`.

use super::*;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Instant;

const FEATURE_LEN: usize = SMART_TURN_MELS * SMART_TURN_FRAMES;
const GOLDEN_MAX_TOL: f32 = 1e-4;
const GOLDEN_MEAN_TOL: f64 = 1e-6;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/voice"))
}

fn smartturn_dir() -> PathBuf {
    fixtures_dir().join("smartturn")
}

fn read_f32(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert_eq!(bytes.len() % 4, 0, "{} is not f32 data", path.display());
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// 16 kHz mono PCM16 WAV -> f32 in [-1, 1) (int / 32768).
fn read_wav_f32(path: &Path) -> Vec<f32> {
    let mut reader =
        hound::WavReader::open(path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 16_000);
    assert_eq!(spec.channels, 1);
    assert_eq!(spec.bits_per_sample, 16);
    assert_eq!(spec.sample_format, hound::SampleFormat::Int);
    reader
        .samples::<i16>()
        .map(|s| f32::from(s.expect("valid PCM16 sample")) / 32768.0)
        .collect()
}

/// (max |a-b|, mean |a-b|).
fn diff_stats(a: &[f32], b: &[f32]) -> (f32, f64) {
    assert_eq!(a.len(), b.len(), "length mismatch");
    let mut max = 0f32;
    let mut sum = 0f64;
    for (x, y) in a.iter().zip(b) {
        let d = (x - y).abs();
        max = max.max(d);
        sum += f64::from(d);
    }
    (max, sum / a.len().max(1) as f64)
}

fn assert_golden(name: &str, got: &[f32], want: &[f32]) {
    assert_eq!(got.len(), FEATURE_LEN, "{name}: length");
    let (max, mean) = diff_stats(got, want);
    println!("golden {name}: max={max:e} mean={mean:e}");
    assert!(
        max <= GOLDEN_MAX_TOL && mean <= GOLDEN_MEAN_TOL,
        "{name}: max|diff|={max:e} (tol {GOLDEN_MAX_TOL:e}), mean|diff|={mean:e} (tol {GOLDEN_MEAN_TOL:e})"
    );
}

// (a) every file listed in manifest.json exists and matches its sha256 and size.
#[test]
fn manifest_hashes_match_every_fixture() {
    let root = fixtures_dir();
    let text = std::fs::read_to_string(root.join("manifest.json")).expect("manifest.json");
    let manifest: serde_json::Value = serde_json::from_str(&text).expect("manifest JSON");
    let files = manifest["files"].as_array().expect("files array");
    assert!(!files.is_empty());
    let mut failures = Vec::new();
    for entry in files {
        let rel = entry["path"].as_str().expect("path");
        let want_sha = entry["sha256"].as_str().expect("sha256");
        let want_bytes = entry["bytes"].as_u64().expect("bytes");
        match std::fs::read(root.join(rel)) {
            Err(e) => failures.push(format!("{rel}: missing ({e})")),
            Ok(data) => {
                let got_sha = format!("{:x}", Sha256::digest(&data));
                if data.len() as u64 != want_bytes {
                    failures.push(format!("{rel}: size {} != {want_bytes}", data.len()));
                }
                if got_sha != want_sha {
                    failures.push(format!("{rel}: sha256 {got_sha} != {want_sha}"));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "manifest mismatches:\n{}",
        failures.join("\n")
    );
}

// (b) golden: real speech clips against the official numpy features.
#[test]
fn golden_c1_matches_official_features() {
    let dir = smartturn_dir();
    let audio = read_wav_f32(&dir.join("c1.wav"));
    assert_golden(
        "c1",
        &smart_turn_features(&audio),
        &read_f32(&dir.join("c1.mel.f32")),
    );
}

#[test]
fn golden_i2_matches_official_features() {
    let dir = smartturn_dir();
    let audio = read_wav_f32(&dir.join("i2.wav"));
    assert_golden(
        "i2",
        &smart_turn_features(&audio),
        &read_f32(&dir.join("i2.mel.f32")),
    );
}

// (c) raw-input goldens: silence, short/exact/long clips, NaN/inf input, reversed speech.
#[test]
fn golden_edge_case_inputs_match() {
    let dir = smartturn_dir();
    for name in [
        "silence_1s",
        "short_0.5s",
        "exact_8s",
        "long_10s",
        "nan_input",
        "reversed_c1",
    ] {
        let input = read_f32(&dir.join(format!("{name}.input.f32")));
        let want = read_f32(&dir.join(format!("{name}.mel.f32")));
        assert_golden(name, &smart_turn_features(&input), &want);
    }
}

// (d) negative controls: wrong variants of c1 must be clearly distinguishable.
#[test]
fn negative_controls_are_distinguishable() {
    let dir = smartturn_dir();
    let audio = read_wav_f32(&dir.join("c1.wav"));
    let rust_c1 = smart_turn_features(&audio);
    let golden_c1 = read_f32(&dir.join("c1.mel.f32"));

    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir.join("negative"))
        .expect("negative dir")
        .map(|e| e.expect("dir entry").path())
        .collect();
    paths.sort();
    assert!(
        paths.len() >= 3,
        "expected >=3 negative controls, got {}",
        paths.len()
    );
    for path in &paths {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let neg = read_f32(path);
        let (rust_vs_neg, _) = diff_stats(&rust_c1, &neg);
        let (golden_vs_neg, _) = diff_stats(&golden_c1, &neg);
        println!(
            "negative {name}: rust-vs-neg max={rust_vs_neg:e} golden-vs-neg max={golden_vs_neg:e}"
        );
        assert!(
            rust_vs_neg > 1e-2,
            "{name}: rust output too close to negative"
        );
        assert!(golden_vs_neg > 1e-2, "{name}: golden too close to negative");
    }

    let reversed = read_f32(&dir.join("reversed_c1.input.f32"));
    let (_, mean) = diff_stats(&smart_turn_features(&reversed), &golden_c1);
    println!("reversed_c1 vs c1.mel: mean={mean:e}");
    assert!(mean > 0.05, "reversed speech too close to c1 (mean {mean})");
}

// (e) properties: fixed output length and finiteness for every input shape.
#[test]
fn output_is_always_finite_and_fixed_length() {
    let mut cases: Vec<(String, Vec<f32>)> = vec![
        ("empty".into(), vec![]),
        ("one_sample".into(), vec![0.25]),
        ("len_399".into(), vec![0.1; 399]),
        ("len_400".into(), vec![0.1; 400]),
        ("len_401".into(), vec![0.1; 401]),
        ("len_128000".into(), vec![0.1; 128_000]),
        ("len_128001".into(), vec![0.1; 128_001]),
        ("len_300000".into(), vec![0.1; 300_000]),
        ("all_nan".into(), vec![f32::NAN; 16_000]),
    ];
    cases.push(("all_inf".into(), vec![f32::INFINITY; 1_000]));
    for (name, audio) in cases {
        let feats = smart_turn_features(&audio);
        assert_eq!(feats.len(), FEATURE_LEN, "{name}: length");
        assert!(
            feats.iter().all(|v| v.is_finite()),
            "{name}: non-finite output"
        );
    }
}

// (f) left-pad invariance: explicit zero left-padding changes nothing.
#[test]
fn left_padding_is_implicit() {
    let dir = smartturn_dir();
    let clip = read_f32(&dir.join("short_0.5s.input.f32"));
    let mut padded = vec![0f32; SMART_TURN_SAMPLES - clip.len()];
    padded.extend_from_slice(&clip);
    assert_eq!(smart_turn_features(&clip), smart_turn_features(&padded));
}

// (g) timing: 20 calls on 8 s of audio must stay well inside the budget, even in debug.
#[test]
fn twenty_calls_on_eight_seconds_are_fast() {
    let dir = smartturn_dir();
    let audio = read_wav_f32(&dir.join("c1.wav"));
    let input = &audio[..audio.len().min(SMART_TURN_SAMPLES)];
    let start = Instant::now();
    for i in 0..20 {
        let call = Instant::now();
        let feats = smart_turn_features(input);
        assert_eq!(feats.len(), FEATURE_LEN);
        println!("call {i}: {:.2} ms", call.elapsed().as_secs_f64() * 1e3);
    }
    let total = start.elapsed().as_secs_f64();
    println!("total 20 calls: {:.3} s", total);
    assert!(total < 3.0, "20 calls took {total:.3} s (budget 3 s)");
}
