//! Golden, edge and negative-control tests for the Kaldi fbank frontend (step S2).
//!
//! Fixtures: `tests/fixtures/voice`. The committed `*.fbank.f32` files were produced by
//! `gen/common.py::frontend("wespeaker_ref")`, i.e. AFTER per-bin mean subtraction (CMN).

use super::*;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const FIXTURE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/voice");

const GOLDEN_CASES: [&str; 7] = [
    "s1a_2.0s",
    "s2a_1.5s",
    "s3a_3.0s",
    "silence_0.5s",
    "tone_16000",
    "tone_400",
    "tone_401",
];

const GOLDEN_MAX_ABS_TOL: f32 = 5e-3;
const GOLDEN_MEAN_ABS_TOL: f64 = 5e-4;

fn fixture(rel: &str) -> PathBuf {
    Path::new(FIXTURE_ROOT).join(rel)
}

fn read_f32(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert_eq!(bytes.len() % 4, 0, "{} is not f32 data", path.display());
    bytes
        .chunks(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn read_json(path: &Path) -> serde_json::Value {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn input(case: &str) -> Vec<f32> {
    read_f32(&fixture(&format!("speaker/inputs/{case}.input.f32")))
}

fn golden(case: &str) -> Vec<f32> {
    read_f32(&fixture(&format!(
        "speaker/resnet34__wespeaker_ref/{case}.fbank.f32"
    )))
}

/// (max |a-b|, mean |a-b|) over two equally long slices.
fn diff_stats(a: &[f32], b: &[f32]) -> (f32, f64) {
    assert_eq!(a.len(), b.len(), "length mismatch");
    let mut max = 0f32;
    let mut sum = 0f64;
    for (x, y) in a.iter().zip(b) {
        let d = (x - y).abs();
        max = max.max(d);
        sum += f64::from(d);
    }
    let mean = if a.is_empty() {
        0.0
    } else {
        sum / a.len() as f64
    };
    (max, mean)
}

fn expected_frames(n: usize) -> usize {
    if n < 400 {
        0
    } else {
        1 + (n - 400) / 160
    }
}

#[test]
fn manifest_hashes_match_every_listed_file() {
    let root = Path::new(FIXTURE_ROOT);
    let manifest = read_json(&root.join("manifest.json"));
    let files = manifest["files"].as_array().expect("manifest files array");
    assert!(!files.is_empty(), "manifest lists no files");
    let mut failures = Vec::new();
    for entry in files {
        let rel = entry["path"].as_str().expect("manifest path");
        let want_sha = entry["sha256"].as_str().expect("manifest sha256");
        let want_bytes = entry["bytes"].as_u64().expect("manifest bytes");
        match std::fs::read(root.join(rel)) {
            Ok(data) => {
                let got_sha: String = Sha256::digest(&data)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                if data.len() as u64 != want_bytes || got_sha != want_sha {
                    failures.push(format!("{rel}: hash/size mismatch"));
                }
            }
            Err(e) => failures.push(format!("{rel}: missing ({e})")),
        }
    }
    assert!(
        failures.is_empty(),
        "{} manifest failures: {failures:?}",
        failures.len()
    );
}

#[test]
fn golden_matches_committed_fbank_within_tolerance() {
    for case in GOLDEN_CASES {
        let want = golden(case);
        assert_eq!(want.len() % FBANK_BINS, 0, "{case}: golden is not [T,80]");
        let got = wespeaker_features(&input(case));
        assert_eq!(got.frames, want.len() / FBANK_BINS, "{case}: frame count");
        let (max_abs, mean_abs) = diff_stats(&got.data, &want);
        eprintln!(
            "golden {case}: frames={} max_abs={max_abs:.3e} mean_abs={mean_abs:.3e}",
            got.frames
        );
        assert!(
            max_abs <= GOLDEN_MAX_ABS_TOL,
            "{case}: max|diff| {max_abs} > {GOLDEN_MAX_ABS_TOL}"
        );
        assert!(
            mean_abs <= GOLDEN_MEAN_ABS_TOL,
            "{case}: mean|diff| {mean_abs} > {GOLDEN_MEAN_ABS_TOL}"
        );
    }
}

#[test]
fn tone_399_returns_zero_frames_consistent_with_reference_error() {
    let samples = input("tone_399");
    assert_eq!(samples.len(), 399);
    let err = read_json(&fixture(
        "speaker/resnet34__wespeaker_ref/tone_399.expected_error.json",
    ));
    assert_eq!(err["input_samples"].as_u64(), Some(samples.len() as u64));
    assert_eq!(err["error_type"].as_str(), Some("AssertionError"));
    assert!(
        err["message"].as_str().unwrap_or("").contains("400"),
        "reference error message does not mention window size 400"
    );
    let raw = wespeaker_fbank(&samples);
    assert_eq!(raw.frames, 0);
    assert!(raw.data.is_empty());
    let feats = wespeaker_features(&samples);
    assert_eq!(feats.frames, 0);
    assert!(feats.data.is_empty());
}

#[test]
fn frame_count_and_finiteness_for_edge_lengths() {
    let speech = input("s1a_2.0s");
    for n in [0usize, 1, 399, 400, 401, 16000, 32000] {
        let feats = wespeaker_features(&speech[..n]);
        assert_eq!(feats.frames, expected_frames(n), "n={n}: frames");
        assert_eq!(
            feats.data.len(),
            feats.frames * FBANK_BINS,
            "n={n}: data len"
        );
        assert!(
            feats.data.iter().all(|v| v.is_finite()),
            "n={n}: non-finite output"
        );
    }
}

#[test]
fn non_finite_samples_are_zeroed_before_processing() {
    let clean = input("s1a_2.0s");
    let mut dirty = clean.clone();
    dirty[100] = f32::NAN;
    dirty[2000] = f32::INFINITY;
    dirty[5000] = f32::NEG_INFINITY;
    let mut zeroed = clean;
    zeroed[100] = 0.0;
    zeroed[2000] = 0.0;
    zeroed[5000] = 0.0;
    let got = wespeaker_features(&dirty);
    assert_eq!(got.frames, 198);
    assert!(got.data.iter().all(|v| v.is_finite()));
    assert_eq!(got, wespeaker_features(&zeroed));
}

#[test]
fn model_input_of_s1a_has_198_frames() {
    let feats = wespeaker_features(&input("s1a_2.0s"));
    assert_eq!(feats.frames, 198);
    assert_eq!(feats.data.len(), 198 * FBANK_BINS);
}

#[test]
fn wrong_window_or_missing_preemphasis_is_detected_by_tolerance() {
    let speech = input("s1a_2.0s");
    let want = golden("s1a_2.0s");

    // Sanity: the correct variant through the test hook equals the production path.
    let correct = FbankVariant {
        window: WindowKind::Hamming,
        preemphasis: 0.97,
    };
    let mut correct_feats = fbank_with(&speech, &correct);
    apply_cmn(&mut correct_feats);
    assert_eq!(correct_feats, wespeaker_features(&speech));

    let povey = FbankVariant {
        window: WindowKind::Povey,
        preemphasis: 0.97,
    };
    let no_preemph = FbankVariant {
        window: WindowKind::Hamming,
        preemphasis: 0.0,
    };
    for (name, cfg) in [("povey_window", povey), ("no_preemphasis", no_preemph)] {
        let mut f = fbank_with(&speech, &cfg);
        apply_cmn(&mut f);
        let (max_abs, _) = diff_stats(&f.data, &want);
        eprintln!("negative control {name}: max_abs vs golden = {max_abs:.3e}");
        assert!(
            max_abs > 0.05,
            "{name}: max|diff| {max_abs} must exceed 0.05 so the golden tolerance can fail"
        );
    }
}

#[test]
fn omitting_cmn_changes_features_materially() {
    let speech = input("s1a_2.0s");
    let raw = wespeaker_fbank(&speech);
    let cmn = wespeaker_features(&speech);
    assert_eq!(raw.frames, cmn.frames);
    let (_, mean_abs) = diff_stats(&raw.data, &cmn.data);
    eprintln!("negative control no_cmn: mean |raw - cmn| = {mean_abs:.3e}");
    assert!(mean_abs > 0.5, "mean|raw-cmn| {mean_abs} must exceed 0.5");
}

#[test]
fn cmn_zeroes_every_column_mean() {
    for case in GOLDEN_CASES {
        let feats = wespeaker_features(&input(case));
        if feats.frames == 0 {
            continue;
        }
        for bin in 0..FBANK_BINS {
            let mean: f64 = (0..feats.frames)
                .map(|t| f64::from(feats.data[t * FBANK_BINS + bin]))
                .sum::<f64>()
                / feats.frames as f64;
            assert!(mean.abs() < 1e-4, "{case}: column {bin} mean {mean}");
        }
    }
    let mut empty = Fbank {
        frames: 0,
        data: Vec::new(),
    };
    apply_cmn(&mut empty);
    assert_eq!(empty.frames, 0);
    assert!(empty.data.is_empty());
}
