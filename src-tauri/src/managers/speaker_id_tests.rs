//! Authoritative contract tests for speaker math, thresholds, enrollment, verification and the
//! voiceprint store (design D6, A8, A8', A9; constants from SD3). Written in step T6 BEFORE the
//! implementation: every test that exercises a `todo!()` body fails until S8 lands. Deterministic,
//! no network, temp dirs via `tempfile`. Nothing here logs embeddings.

use super::*;
use crate::settings::SpeakerSensitivity;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};

const DIM: usize = 256;

fn unit_e0() -> Vec<f32> {
    let mut v = vec![0.0; DIM];
    v[0] = 1.0;
    v
}

/// Vector in the first two dimensions whose cosine with `e0` is exactly `c` (up to f32 rounding).
fn planar(c: f32) -> Vec<f32> {
    let mut v = vec![0.0; DIM];
    v[0] = c;
    v[1] = (1.0 - c * c).sqrt();
    v
}

/// Vector in the first two dimensions at angle `theta` from `e0`.
fn at_angle(theta: f64) -> Vec<f32> {
    let mut v = vec![0.0; DIM];
    v[0] = theta.cos() as f32;
    v[1] = theta.sin() as f32;
    v
}

/// An embedding whose cosine with `e0` sits at the threshold. The +5e-7 bias keeps the value
/// inside the 1e-6 tolerance of the boundary while staying far above f32 rounding, so the
/// decision cannot flip on rounding. Bit-exact f32 equality cannot be constructed portably.
fn at_threshold(thr: f32) -> Vec<f32> {
    planar(thr + 5e-7)
}

fn enrolled_vp() -> Voiceprint {
    Voiceprint {
        version: 1,
        model_sha256: SPEAKER_SPEC.sha256.to_string(),
        dim: DIM,
        embedding: unit_e0(),
        clips: 3,
    }
}

fn decide(vp: &Voiceprint, emb: &[f32], sens: SpeakerSensitivity, ms: u32) -> Verdict {
    verify(vp, emb, sens, ms, SPEAKER_SPEC.sha256)
}

fn assert_close(actual: f32, expected: f32, what: &str) {
    assert!(
        (actual - expected).abs() <= 1e-6,
        "{what}: expected {expected}, got {actual}"
    );
}

// ---------------------------------------------------------------------------------------------
// Pure constants (these pass before implementation)
// ---------------------------------------------------------------------------------------------

#[test]
fn spec_constants_match_the_pinned_model() {
    assert_eq!(SPEAKER_SPEC.file, "wespeaker_en_voxceleb_resnet34.onnx");
    assert_eq!(
        SPEAKER_SPEC.sha256,
        "5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94"
    );
    assert_eq!(SPEAKER_SPEC.dim, 256);
    assert_eq!(SPEAKER_SPEC.min_verify_buffer_ms, 2000);
    assert_eq!(SPEAKER_SPEC.min_embed_samples, 16000);
    assert_eq!(MIN_ENROLL_CLIPS, 3);
    assert_eq!(ENROLL_MIN_PAIR_COSINE, 0.30);
}

#[test]
fn thresholds_are_ordered_lenient_balanced_strict() {
    assert!(SPEAKER_SPEC.lenient < SPEAKER_SPEC.balanced);
    assert!(SPEAKER_SPEC.balanced < SPEAKER_SPEC.strict);
    assert_close(SPEAKER_SPEC.lenient, 0.59, "lenient");
    assert_close(SPEAKER_SPEC.balanced, 0.69, "balanced");
    assert_close(SPEAKER_SPEC.strict, 0.74, "strict");
}

#[test]
fn threshold_fn_maps_each_sensitivity() {
    assert_close(threshold(SpeakerSensitivity::Lenient), 0.59, "lenient");
    assert_close(threshold(SpeakerSensitivity::Balanced), 0.69, "balanced");
    assert_close(threshold(SpeakerSensitivity::Strict), 0.74, "strict");
    assert!(threshold(SpeakerSensitivity::Lenient) < threshold(SpeakerSensitivity::Balanced));
    assert!(threshold(SpeakerSensitivity::Balanced) < threshold(SpeakerSensitivity::Strict));
}

#[test]
fn buffer_ms_is_samples_over_16_and_saturates() {
    assert_eq!(buffer_ms(0), 0);
    assert_eq!(buffer_ms(31999), 1999);
    assert_eq!(buffer_ms(32000), 2000);
    assert_eq!(buffer_ms(usize::MAX), u32::MAX);
}

// ---------------------------------------------------------------------------------------------
// Derivation of the thresholds from the committed capture matrix (design A8, SD3)
// ---------------------------------------------------------------------------------------------

/// Rule (A8): over buffers >= 2.0 s (buffer_ms key, which includes the 450 ms pads),
/// Balanced = midpoint(max impostor cosine, min owner cosine) rounded to 0.01;
/// Lenient = Balanced - 0.10; Strict = Balanced + 0.05.
#[test]
fn speaker_thresholds_are_derived_from_the_capture_matrix() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/voice/speaker_capture_matrix.json");
    let raw = fs::read_to_string(&path).expect("read speaker_capture_matrix.json");
    let doc: serde_json::Value = serde_json::from_str(&raw).expect("matrix is valid JSON");

    let mut impostor = Vec::new();
    let mut owner = Vec::new();
    for trial in doc["trials"].as_array().expect("trials array") {
        let buffer = trial["buffer_ms"].as_f64().expect("buffer_ms");
        if buffer < 2000.0 {
            continue;
        }
        let cosine = trial["cosine"].as_f64().expect("cosine");
        if trial["target"].as_bool().expect("target") {
            owner.push(cosine);
        } else {
            impostor.push(cosine);
        }
    }
    assert_eq!(owner.len(), 32, "owner trials at >= 2.0 s");
    assert_eq!(impostor.len(), 96, "impostor trials at >= 2.0 s");

    let max_impostor = impostor.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let min_owner = owner.iter().cloned().fold(f64::INFINITY, f64::min);
    let balanced = ((max_impostor + min_owner) / 2.0 * 100.0).round() / 100.0;
    let lenient = balanced - 0.10;
    let strict = balanced + 0.05;

    let stored = &doc["constants"]["thresholds_capture"];
    let stored_max = stored["max_impostor_cos"]
        .as_f64()
        .expect("max_impostor_cos");
    let stored_min = stored["min_owner_cos"].as_f64().expect("min_owner_cos");
    assert!((max_impostor - stored_max).abs() < 1e-6, "max impostor");
    assert!((min_owner - stored_min).abs() < 1e-6, "min owner");

    let tol = 1e-6;
    assert!(
        (SPEAKER_SPEC.balanced as f64 - balanced).abs() < tol,
        "balanced"
    );
    assert!(
        (SPEAKER_SPEC.lenient as f64 - lenient).abs() < tol,
        "lenient"
    );
    assert!((SPEAKER_SPEC.strict as f64 - strict).abs() < tol, "strict");

    let min_verify = doc["constants"]["MIN_VERIFY_BUFFER_MS"]
        .as_u64()
        .expect("MIN_VERIFY_BUFFER_MS");
    assert_eq!(u64::from(SPEAKER_SPEC.min_verify_buffer_ms), min_verify);
}

// ---------------------------------------------------------------------------------------------
// cosine / l2_normalize
// ---------------------------------------------------------------------------------------------

#[test]
fn cosine_identical_opposite_orthogonal() {
    assert_close(
        cosine(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]).expect("some"),
        1.0,
        "identical",
    );
    assert_close(
        cosine(&[1.0, 2.0], &[-1.0, -2.0]).expect("some"),
        -1.0,
        "opposite",
    );
    assert_close(
        cosine(&[1.0, 0.0], &[0.0, 5.0]).expect("some"),
        0.0,
        "orthogonal",
    );
}

#[test]
fn cosine_none_on_zero_nan_inf_mismatch_and_empty() {
    assert_eq!(cosine(&[0.0, 0.0], &[1.0, 2.0]), None, "zero a");
    assert_eq!(cosine(&[1.0, 2.0], &[0.0, 0.0]), None, "zero b");
    assert_eq!(cosine(&[f32::NAN, 1.0], &[1.0, 1.0]), None, "nan");
    assert_eq!(cosine(&[1.0, 1.0], &[f32::INFINITY, 1.0]), None, "inf");
    assert_eq!(
        cosine(&[1.0, 2.0], &[1.0, 2.0, 3.0]),
        None,
        "length mismatch"
    );
    assert_eq!(cosine(&[], &[]), None, "empty");
}

#[test]
fn cosine_is_scale_invariant() {
    let a = [0.3, -1.2, 2.5];
    let b = [1.1, 0.4, -0.7];
    let scaled: Vec<f32> = b.iter().map(|x| x * 7.0).collect();
    let base = cosine(&a, &b).expect("some");
    let scaled_cos = cosine(&a, &scaled).expect("some");
    assert!((base - scaled_cos).abs() < 1e-6);
}

#[test]
fn l2_normalize_gives_unit_norm_and_none_rules() {
    let v = l2_normalize(&[3.0, 4.0]).expect("some");
    assert_close(v[0], 0.6, "x");
    assert_close(v[1], 0.8, "y");
    assert_eq!(l2_normalize(&[0.0, 0.0]), None, "zero");
    assert_eq!(l2_normalize(&[f32::NAN, 1.0]), None, "nan");
    assert_eq!(l2_normalize(&[]), None, "empty");
}

// ---------------------------------------------------------------------------------------------
// enroll
// ---------------------------------------------------------------------------------------------

#[test]
fn enroll_rejects_two_clips() {
    let two = vec![unit_e0(), unit_e0()];
    assert_eq!(
        enroll(&two, SPEAKER_SPEC.sha256),
        Err(EnrollError::TooFewClips)
    );
}

#[test]
fn enroll_accepts_three_consistent_clips() {
    let three = vec![unit_e0(), unit_e0(), unit_e0()];
    let vp = enroll(&three, SPEAKER_SPEC.sha256).expect("three consistent clips enroll");
    assert_eq!(vp.version, 1);
    assert_eq!(vp.dim, DIM);
    assert_eq!(vp.clips, 3);
}

#[test]
fn enroll_rejects_mixed_dims() {
    let clips = vec![unit_e0(), unit_e0(), vec![1.0; DIM - 1]];
    assert_eq!(
        enroll(&clips, SPEAKER_SPEC.sha256),
        Err(EnrollError::DimMismatch)
    );
}

#[test]
fn enroll_rejects_wrong_dim_for_spec() {
    let clips = vec![vec![1.0; 128], vec![1.0; 128], vec![1.0; 128]];
    assert_eq!(
        enroll(&clips, SPEAKER_SPEC.sha256),
        Err(EnrollError::DimMismatch)
    );
}

#[test]
fn enroll_rejects_nan() {
    let mut bad = unit_e0();
    bad[5] = f32::NAN;
    let clips = vec![unit_e0(), unit_e0(), bad];
    assert_eq!(
        enroll(&clips, SPEAKER_SPEC.sha256),
        Err(EnrollError::NonFinite)
    );
}

#[test]
fn enroll_rejects_zero_vector() {
    let clips = vec![unit_e0(), unit_e0(), vec![0.0; DIM]];
    assert_eq!(
        enroll(&clips, SPEAKER_SPEC.sha256),
        Err(EnrollError::ZeroVector)
    );
}

#[test]
fn enroll_pair_cosine_below_030_is_inconsistent_and_031_passes() {
    // Pair (a, b) has cosine x; clip c sits halfway between a and b so that both of its pairs
    // are well above 0.30. Only the (a, b) pair decides the outcome.
    let build = |x: f64| {
        let a = at_angle(0.0);
        let b = at_angle(x.acos());
        let c = at_angle(x.acos() / 2.0);
        vec![a, b, c]
    };
    let bad = build(0.29);
    assert_eq!(
        enroll(&bad, SPEAKER_SPEC.sha256),
        Err(EnrollError::Inconsistent)
    );
    let good = build(0.31);
    assert!(
        enroll(&good, SPEAKER_SPEC.sha256).is_ok(),
        "0.31 pair must pass"
    );
}

#[test]
fn enroll_is_unit_norm_and_equals_per_clip_normalised_mean() {
    // Different magnitudes on purpose: per-clip normalisation must happen before the mean.
    let e1 = {
        let mut v = vec![0.0; DIM];
        v[0] = 3.0;
        v[1] = 4.0;
        v
    }; // unit: [0.6, 0.8]
    let e2 = {
        let mut v = vec![0.0; DIM];
        v[0] = 2.0;
        v[1] = 2.0;
        v
    }; // unit: [1/sqrt2, 1/sqrt2]
    let e3 = {
        let mut v = vec![0.0; DIM];
        v[0] = 100.0;
        v
    }; // unit: [1, 0]
    let vp = enroll(&[e1, e2, e3], SPEAKER_SPEC.sha256).expect("enrolls");

    // Hand-built expectation: normalise(sum of unit vectors), sum = [0.6 + s + 1, 0.8 + s].
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let (sx, sy) = (0.6 + s + 1.0, 0.8 + s);
    let norm = (sx * sx + sy * sy).sqrt();
    let expected = [(sx / norm) as f32, (sy / norm) as f32];
    assert_close(vp.embedding[0], expected[0], "x");
    assert_close(vp.embedding[1], expected[1], "y");
    assert!(vp.embedding[2..].iter().all(|x| *x == 0.0), "rest is zero");

    let n: f32 = vp.embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((n - 1.0).abs() < 1e-5, "unit norm, got {n}");
}

#[test]
fn enroll_caps_clip_count_at_255_and_stores_model_sha() {
    let many = vec![unit_e0(); 300];
    let vp = enroll(&many, "abc123").expect("enrolls");
    assert_eq!(vp.clips, 255);
    assert_eq!(vp.model_sha256, "abc123");
}

// ---------------------------------------------------------------------------------------------
// verify
// ---------------------------------------------------------------------------------------------

#[test]
fn verify_rejects_just_below_each_threshold() {
    let vp = enrolled_vp();
    for (sens, thr) in [
        (SpeakerSensitivity::Lenient, 0.59_f32),
        (SpeakerSensitivity::Balanced, 0.69),
        (SpeakerSensitivity::Strict, 0.74),
    ] {
        assert_eq!(
            decide(&vp, &planar(thr - 0.001), sens, 2000),
            Verdict::Reject,
            "{sens:?} at threshold - 0.001"
        );
    }
    // The task's explicit example for Lenient: 0.5899 is below 0.59.
    assert_eq!(
        decide(&vp, &planar(0.5899), SpeakerSensitivity::Lenient, 2000),
        Verdict::Reject
    );
    assert_eq!(
        decide(&vp, &planar(0.6899), SpeakerSensitivity::Balanced, 2000),
        Verdict::Reject
    );
    assert_eq!(
        decide(&vp, &planar(0.7399), SpeakerSensitivity::Strict, 2000),
        Verdict::Reject
    );
}

#[test]
fn verify_accepts_at_each_threshold() {
    let vp = enrolled_vp();
    for (sens, thr) in [
        (SpeakerSensitivity::Lenient, 0.59_f32),
        (SpeakerSensitivity::Balanced, 0.69),
        (SpeakerSensitivity::Strict, 0.74),
    ] {
        assert_eq!(
            decide(&vp, &at_threshold(thr), sens, 2000),
            Verdict::Accept,
            "{sens:?} exactly at threshold"
        );
    }
}

#[test]
fn verify_same_embedding_accepts_lenient_but_rejects_strict() {
    let vp = enrolled_vp();
    let emb = planar(0.65);
    assert_eq!(
        decide(&vp, &emb, SpeakerSensitivity::Lenient, 2000),
        Verdict::Accept
    );
    assert_eq!(
        decide(&vp, &emb, SpeakerSensitivity::Strict, 2000),
        Verdict::Reject
    );
}

#[test]
fn verify_buffer_below_2000_is_skipped_and_2000_is_decided() {
    let vp = enrolled_vp();
    let owner = planar(0.95);
    assert_eq!(
        decide(&vp, &owner, SpeakerSensitivity::Balanced, 1999),
        Verdict::SkippedTooShort
    );
    assert_eq!(
        decide(&vp, &owner, SpeakerSensitivity::Balanced, 2000),
        Verdict::Accept
    );
    let foreign = planar(0.0);
    assert_eq!(
        decide(&vp, &foreign, SpeakerSensitivity::Balanced, 2000),
        Verdict::Reject
    );
}

#[test]
fn verify_skipped_too_short_wins_over_a_clearly_foreign_embedding() {
    let vp = enrolled_vp();
    let foreign = planar(-0.9);
    assert_eq!(
        decide(&vp, &foreign, SpeakerSensitivity::Strict, 1999),
        Verdict::SkippedTooShort
    );
    // Also wins over a model-sha mismatch: the buffer check comes first.
    assert_eq!(
        verify(&vp, &foreign, SpeakerSensitivity::Strict, 0, "other-sha"),
        Verdict::SkippedTooShort
    );
}

#[test]
fn verify_sha_mismatch_accepts_not_enrolled_semantics() {
    let vp = enrolled_vp();
    let foreign = planar(0.0);
    assert_eq!(
        verify(
            &vp,
            &foreign,
            SpeakerSensitivity::Strict,
            2000,
            "a-different-model"
        ),
        Verdict::Accept
    );
}

#[test]
fn verify_dim_mismatch_accepts_not_enrolled_semantics() {
    let vp = enrolled_vp();
    let mut short = vec![0.0; 128];
    short[0] = 1.0;
    assert_eq!(
        decide(&vp, &short, SpeakerSensitivity::Strict, 2000),
        Verdict::Accept
    );
}

#[test]
fn verify_version_two_voiceprint_accepts_not_enrolled_semantics() {
    let mut vp = enrolled_vp();
    vp.version = 2;
    let foreign = planar(0.0);
    assert_eq!(
        decide(&vp, &foreign, SpeakerSensitivity::Strict, 2000),
        Verdict::Accept
    );
}

#[test]
fn verify_non_finite_or_zero_embedding_accepts_not_enrolled_semantics() {
    let vp = enrolled_vp();
    let mut nan = vec![0.0; DIM];
    nan[0] = f32::NAN;
    assert_eq!(
        decide(&vp, &nan, SpeakerSensitivity::Strict, 2000),
        Verdict::Accept,
        "nan"
    );
    let zero = vec![0.0; DIM];
    assert_eq!(
        decide(&vp, &zero, SpeakerSensitivity::Strict, 2000),
        Verdict::Accept,
        "zero"
    );
}

// ---------------------------------------------------------------------------------------------
// Store: helpers
// ---------------------------------------------------------------------------------------------

fn temp_store() -> (TempDir, VoiceprintStore) {
    let root = TempDir::new().expect("tempdir");
    let store = VoiceprintStore::new(store_dir(&root));
    (root, store)
}

fn store_dir(root: &TempDir) -> PathBuf {
    root.path().join("speaker")
}

/// The single entry in `dir` (the final voiceprint file). Panics if there is not exactly one.
fn only_entry(dir: &Path) -> PathBuf {
    let entries: Vec<PathBuf> = fs::read_dir(dir)
        .expect("read store dir")
        .map(|e| e.expect("dir entry").path())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "expected only the final file in {dir:?}, got {entries:?}"
    );
    entries.into_iter().next().expect("one entry")
}

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    fs::metadata(path).expect("stat").permissions().mode() & 0o777
}

/// Seeds a valid voiceprint, overwrites the final file with `bytes`, and checks that a fresh
/// store treats it as not enrolled and that the store still works afterwards.
fn assert_raw_file_loads_as_none(bytes: &[u8]) {
    let (root, store) = temp_store();
    store.save(&enrolled_vp()).expect("seed save");
    fs::write(only_entry(&store_dir(&root)), bytes).expect("overwrite with raw bytes");

    let fresh = VoiceprintStore::new(store_dir(&root));
    assert!(fresh.load().is_none(), "invalid file must load as None");
    assert!(
        !fresh.is_enrolled(),
        "invalid file must not count as enrolled"
    );

    fresh
        .save(&enrolled_vp())
        .expect("store keeps working after a bad file");
    assert_eq!(fresh.load().as_deref(), Some(&enrolled_vp()));
}

fn json_with_first_embedding_token(token: &str) -> Vec<u8> {
    let rest = vec!["0.0"; DIM - 1].join(",");
    format!(
        r#"{{"version":1,"model_sha256":"{}","dim":256,"embedding":[{},{}],"clips":3}}"#,
        SPEAKER_SPEC.sha256, token, rest
    )
    .into_bytes()
}

// ---------------------------------------------------------------------------------------------
// Store: load / save / delete / is_enrolled (behaviour that holds on every platform)
// ---------------------------------------------------------------------------------------------

#[test]
fn load_on_missing_file_is_none_and_not_enrolled() {
    let (_root, store) = temp_store();
    assert!(store.load().is_none());
    assert!(!store.is_enrolled());
}

#[test]
fn save_then_load_round_trips_exactly() {
    let (root, store) = temp_store();
    let vp = enrolled_vp();
    store.save(&vp).expect("save");
    assert!(store.is_enrolled());
    assert_eq!(store.load().as_deref(), Some(&vp));
    let fresh = VoiceprintStore::new(store_dir(&root));
    assert_eq!(
        fresh.load().as_deref(),
        Some(&vp),
        "round-trip through disk"
    );
}

#[test]
fn load_of_garbage_bytes_is_none() {
    assert_raw_file_loads_as_none(b"this is not json \x00\xff");
}

#[test]
fn load_of_empty_file_is_none() {
    assert_raw_file_loads_as_none(b"");
}

#[test]
fn load_of_truncated_json_is_none() {
    let full = serde_json::to_vec(&enrolled_vp()).expect("serialize");
    assert_raw_file_loads_as_none(&full[..full.len() / 2]);
}

#[test]
fn load_of_version_two_is_none() {
    let mut vp = enrolled_vp();
    vp.version = 2;
    assert_raw_file_loads_as_none(&serde_json::to_vec(&vp).expect("serialize"));
}

#[test]
fn load_of_wrong_dim_is_none() {
    let vp = Voiceprint {
        version: 1,
        model_sha256: SPEAKER_SPEC.sha256.to_string(),
        dim: 128,
        embedding: vec![1.0; 128],
        clips: 3,
    };
    assert_raw_file_loads_as_none(&serde_json::to_vec(&vp).expect("serialize"));
}

#[test]
fn load_of_wrong_clip_count_is_none() {
    for clips in [0_u8, 2] {
        let mut vp = enrolled_vp();
        vp.clips = clips;
        assert_raw_file_loads_as_none(&serde_json::to_vec(&vp).expect("serialize"));
    }
}

#[test]
fn load_of_non_finite_encoded_values_is_none() {
    // serde_json writes NaN as null; literal NaN / out-of-range numbers are not valid JSON numbers.
    assert_raw_file_loads_as_none(&json_with_first_embedding_token("NaN"));
    assert_raw_file_loads_as_none(&json_with_first_embedding_token("1e999"));
    assert_raw_file_loads_as_none(&json_with_first_embedding_token("null"));
}

#[test]
fn delete_removes_file_and_load_becomes_none() {
    let (root, store) = temp_store();
    store.save(&enrolled_vp()).expect("save");
    store.delete().expect("delete");
    assert!(store.load().is_none());
    assert!(!store.is_enrolled());
    assert_eq!(
        fs::read_dir(store_dir(&root)).expect("dir").count(),
        0,
        "file removed"
    );
}

#[test]
fn delete_when_absent_is_ok() {
    let (_root, store) = temp_store();
    assert_eq!(store.delete(), Ok(()));
}

// ---------------------------------------------------------------------------------------------
// Store: generation
// ---------------------------------------------------------------------------------------------

#[test]
fn generation_is_stable_for_an_untouched_store() {
    let (_r1, a) = temp_store();
    let (_r2, b) = temp_store();
    assert_eq!(a.generation(), a.generation());
    assert_eq!(
        a.generation(),
        b.generation(),
        "fresh stores start at the same value"
    );
}

#[test]
fn save_bumps_generation() {
    let (_root, store) = temp_store();
    let before = store.generation();
    store.save(&enrolled_vp()).expect("save");
    assert!(store.generation() > before);
}

#[test]
fn delete_bumps_generation_and_clears_enrollment() {
    let (_root, store) = temp_store();
    store.save(&enrolled_vp()).expect("save");
    let before = store.generation();
    store.delete().expect("delete");
    assert!(store.generation() > before);
    assert!(!store.is_enrolled());
}

#[test]
fn delete_when_absent_still_bumps_generation() {
    let (_root, store) = temp_store();
    let before = store.generation();
    assert_eq!(store.delete(), Ok(()));
    assert!(store.generation() > before);
}

// ---------------------------------------------------------------------------------------------
// Store: accept_after
// ---------------------------------------------------------------------------------------------

#[test]
fn accept_after_reject_with_unchanged_generation_and_enrolled_is_false() {
    let (_root, store) = temp_store();
    store.save(&enrolled_vp()).expect("save");
    let started = store.generation();
    assert!(!accept_after(started, &store, Verdict::Reject));
}

#[test]
fn accept_after_reject_when_delete_happened_is_true() {
    let (_root, store) = temp_store();
    store.save(&enrolled_vp()).expect("save");
    let started = store.generation();
    store.delete().expect("delete during verification");
    assert!(accept_after(started, &store, Verdict::Reject));
}

#[test]
fn accept_after_reject_when_re_enrolled_during_verification_is_true() {
    let (_root, store) = temp_store();
    store.save(&enrolled_vp()).expect("save");
    let started = store.generation();
    store
        .save(&enrolled_vp())
        .expect("re-enroll during verification");
    assert!(
        accept_after(started, &store, Verdict::Reject),
        "generation changed => accept"
    );
}

#[test]
fn accept_after_reject_when_not_enrolled_is_true() {
    let (_root, store) = temp_store();
    let started = store.generation();
    assert!(accept_after(started, &store, Verdict::Reject));
}

#[test]
fn accept_after_accept_and_skipped_are_always_true() {
    let (_root, store) = temp_store();
    store.save(&enrolled_vp()).expect("save");
    let started = store.generation();
    assert!(accept_after(started, &store, Verdict::Accept));
    assert!(accept_after(started, &store, Verdict::SkippedTooShort));
}

// ---------------------------------------------------------------------------------------------
// Store: concurrency
// ---------------------------------------------------------------------------------------------

#[test]
fn concurrent_loads_during_saves_never_panic_or_return_partial_state() {
    let root = TempDir::new().expect("tempdir");
    let store = Arc::new(VoiceprintStore::new(store_dir(&root)));
    let expected = enrolled_vp();

    let readers: Vec<_> = (0..8)
        .map(|_| {
            let shared = Arc::clone(&store);
            let expected = expected.clone();
            thread::spawn(move || {
                for _ in 0..200 {
                    match shared.load() {
                        None => {}
                        Some(vp) => {
                            assert_eq!(*vp, expected, "load must return a whole voiceprint")
                        }
                    }
                }
            })
        })
        .collect();

    for _ in 0..20 {
        store.save(&expected).expect("save during concurrent loads");
    }
    for reader in readers {
        reader.join().expect("reader thread must not panic");
    }
}

// ---------------------------------------------------------------------------------------------
// Store: owner-only storage (design A9). Unix only; cannot run on non-Unix hosts.
// ---------------------------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn save_creates_dir_0700_and_file_0600() {
    let (root, store) = temp_store();
    store.save(&enrolled_vp()).expect("save");
    assert_eq!(mode_of(&store_dir(&root)), 0o700, "dir mode");
    assert_eq!(mode_of(&only_entry(&store_dir(&root))), 0o600, "file mode");
}

#[cfg(unix)]
#[test]
fn save_leaves_no_temp_file_behind() {
    let (root, store) = temp_store();
    store.save(&enrolled_vp()).expect("save");
    store.save(&enrolled_vp()).expect("second save");
    let count = fs::read_dir(store_dir(&root)).expect("dir").count();
    assert_eq!(count, 1, "only the final file may remain");
}

#[cfg(unix)]
#[test]
fn symlinked_directory_is_refused_and_target_untouched() {
    let root = TempDir::new().expect("tempdir");
    let target = root.path().join("real");
    fs::create_dir(&target).expect("mkdir target");
    let link = root.path().join("link");
    symlink(&target, &link).expect("symlink dir");

    let store = VoiceprintStore::new(link);
    assert_eq!(store.save(&enrolled_vp()), Err(StoreError::SymlinkRefused));
    assert_eq!(
        fs::read_dir(&target).expect("read target").count(),
        0,
        "nothing written"
    );
}

#[cfg(unix)]
#[test]
fn symlinked_final_path_is_refused_and_target_untouched() {
    let (root, store) = temp_store();
    store.save(&enrolled_vp()).expect("seed save");
    let final_path = only_entry(&store_dir(&root));
    fs::remove_file(&final_path).expect("remove real file");

    let victim = root.path().join("victim.bin");
    fs::write(&victim, b"SENTINEL").expect("write victim");
    symlink(&victim, &final_path).expect("symlink final path");

    let mut other = enrolled_vp();
    other.clips = 4;
    assert_eq!(store.save(&other), Err(StoreError::SymlinkRefused));
    assert_eq!(
        fs::read(&victim).expect("read victim"),
        b"SENTINEL",
        "target untouched"
    );
}

#[cfg(unix)]
#[test]
fn existing_owned_dir_with_0755_is_repaired_to_0700() {
    // Decision (A9): an owned directory is repaired with chmod 0700. If the mode cannot be
    // verified as 0700 afterwards, save returns Err(Io) instead (not testable without root).
    let root = TempDir::new().expect("tempdir");
    let dir = store_dir(&root);
    fs::create_dir(&dir).expect("mkdir");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).expect("chmod 0755");

    let store = VoiceprintStore::new(dir.clone());
    store
        .save(&enrolled_vp())
        .expect("owned 0755 dir is repaired and save succeeds");
    assert_eq!(mode_of(&dir), 0o700, "dir repaired to 0700");
    assert_eq!(mode_of(&only_entry(&dir)), 0o600, "file mode");
}

#[cfg(not(unix))]
#[test]
fn save_is_unsupported_off_unix() {
    // Documented: this branch is compiled only on non-Unix hosts and cannot run on this macOS
    // worktree. Enrollment must fail closed there.
    let (_root, store) = temp_store();
    assert_eq!(
        store.save(&enrolled_vp()),
        Err(StoreError::UnsupportedPlatform)
    );
}
