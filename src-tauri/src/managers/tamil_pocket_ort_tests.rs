//! Tests for the ONNX Runtime Pocket TTS backend (DESIGN.md Rev 5, S4).
//!
//! CI-safe tests build their own inputs (npy bytes, wav files, bundle text) and
//! never need the pack. The live tests are `#[ignore]`d and read the pack from
//! `VOX_TAMIL_DIR`; they compare against the committed golden fixture.

use super::tamil_pocket_core::{
    decode_latents, generate_chunk_latents, GenParams, Rng, TensorData,
};
use super::tamil_pocket_ort::{load_voice_wav, read_npy_f32, OrtPocket, PocketModel};
use super::tamil_text::{estimate_max_frames, prepare_text_prompt};
use std::path::{Path, PathBuf};
use std::time::Instant;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Builds a version 1/2/3 .npy file from a header dict literal and raw data bytes.
fn npy_bytes(version: u8, dict: &str, data: &[u8]) -> Vec<u8> {
    let mut header = dict.as_bytes().to_vec();
    let prefix_len = if version == 1 { 10 } else { 12 };
    // Pad so that the whole preamble is a multiple of 64 bytes, ending in '\n'.
    while !(prefix_len + header.len() + 1).is_multiple_of(64) {
        header.push(b' ');
    }
    header.push(b'\n');
    let mut out = b"\x93NUMPY".to_vec();
    out.push(version);
    out.push(0);
    if version == 1 {
        let len = u16::try_from(header.len()).unwrap_or(u16::MAX);
        out.extend_from_slice(&len.to_le_bytes());
    } else {
        let len = u32::try_from(header.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&len.to_le_bytes());
    }
    out.extend_from_slice(&header);
    out.extend_from_slice(data);
    out
}

/// Extracts the error of a result that must fail; panics with `what` if it succeeded.
fn err_of<T>(result: Result<T, String>, what: &str) -> String {
    match result {
        Err(error) => error,
        Ok(_) => panic!("{what}: expected an error"),
    }
}

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

const DICT_F32_1_1_4: &str = "{'descr': '<f4', 'fortran_order': False, 'shape': (1, 1, 4), }";

fn write_temp(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write temp file");
    path
}

/// Minimal bundle.json that passes `Bundle::parse` (one cache entry, one mimi entry).
const MINIMAL_BUNDLE: &str = r#"{
  "schema_version": 2,
  "sample_rate": 24000,
  "frame_rate": 12.5,
  "samples_per_frame": 1920,
  "latent_dim": 32,
  "conditioning_dim": 1024,
  "insert_bos_before_voice": true,
  "flow_lm_state_manifest": [
    {"dtype": "float32", "fill": "nan", "index": 0, "input_name": "state_0", "key": "cache", "module": "m", "output_name": "out_state_0", "path": "m/cache", "shape": [2, 1, 1000, 16, 64]}
  ],
  "mimi_state_manifest": [
    {"dtype": "bool", "fill": "ones", "index": 0, "input_name": "state_0", "key": "first", "module": "d", "output_name": "out_state_0", "path": "d/first", "shape": [1]}
  ]
}"#;

fn pack_dir() -> PathBuf {
    std::env::var("VOX_TAMIL_DIR")
        .map(PathBuf::from)
        .expect("VOX_TAMIL_DIR must point to the Tamil pack folder")
}

fn golden() -> serde_json::Value {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tamil/tamil_engine_golden.json");
    let text = std::fs::read_to_string(&path).expect("read golden fixture");
    serde_json::from_str(&text).expect("parse golden fixture")
}

/// Flattens a (possibly nested) JSON array of numbers into row-major f32 values.
fn golden_f32s(value: &serde_json::Value) -> Vec<f32> {
    fn walk(value: &serde_json::Value, out: &mut Vec<f32>) {
        match value {
            serde_json::Value::Array(items) => items.iter().for_each(|v| walk(v, out)),
            other => out.push(other.as_f64().expect("golden number") as f32),
        }
    }
    let mut out = Vec::new();
    walk(value, &mut out);
    out
}

// ---------------------------------------------------------------------------
// read_npy_f32 (CI-safe)
// ---------------------------------------------------------------------------

#[test]
fn npy_v1_f32_reads_shape_and_values() {
    let dir = tempfile::tempdir().expect("tempdir");
    let values = [0.5f32, -1.25, 3.0, 7.5];
    let path = write_temp(
        dir.path(),
        "a.npy",
        &npy_bytes(1, DICT_F32_1_1_4, &f32_bytes(&values)),
    );
    let tensor = read_npy_f32(&path).expect("valid npy");
    assert_eq!(tensor.shape, vec![1, 1, 4]);
    assert_eq!(tensor.data, TensorData::F32(values.to_vec()));
}

#[test]
fn npy_v2_and_v3_headers_are_accepted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let values = [1.0f32, 2.0, 3.0, 4.0];
    for version in [2u8, 3u8] {
        let bytes = npy_bytes(version, DICT_F32_1_1_4, &f32_bytes(&values));
        let path = write_temp(dir.path(), "v.npy", &bytes);
        let tensor = read_npy_f32(&path).expect("valid npy");
        assert_eq!(tensor.shape, vec![1, 1, 4], "version {version}");
    }
}

#[test]
fn npy_wrong_dtype_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dict = "{'descr': '<f8', 'fortran_order': False, 'shape': (1, 1, 4), }";
    let bytes = npy_bytes(1, dict, &[0u8; 32]);
    let path = write_temp(dir.path(), "f8.npy", &bytes);
    let err = err_of(read_npy_f32(&path), "f64 must be rejected");
    assert!(err.contains("<f8"), "error should name the dtype: {err}");
}

#[test]
fn npy_big_endian_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dict = "{'descr': '>f4', 'fortran_order': False, 'shape': (1, 1, 4), }";
    let path = write_temp(
        dir.path(),
        "be.npy",
        &npy_bytes(1, dict, &f32_bytes(&[1.0; 4])),
    );
    assert!(read_npy_f32(&path).is_err());
}

#[test]
fn npy_fortran_order_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dict = "{'descr': '<f4', 'fortran_order': True, 'shape': (1, 1, 4), }";
    let path = write_temp(
        dir.path(),
        "fo.npy",
        &npy_bytes(1, dict, &f32_bytes(&[1.0; 4])),
    );
    let err = err_of(read_npy_f32(&path), "fortran order must be rejected");
    assert!(err.to_lowercase().contains("fortran"), "error: {err}");
}

#[test]
fn npy_truncated_data_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut bytes = npy_bytes(1, DICT_F32_1_1_4, &f32_bytes(&[1.0; 4]));
    bytes.pop();
    let path = write_temp(dir.path(), "trunc.npy", &bytes);
    assert!(read_npy_f32(&path).is_err());
}

#[test]
fn npy_size_mismatch_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Header says 4 floats, data holds 3.
    let bytes = npy_bytes(1, DICT_F32_1_1_4, &f32_bytes(&[1.0; 3]));
    let path = write_temp(dir.path(), "short.npy", &bytes);
    assert!(read_npy_f32(&path).is_err());
}

#[test]
fn npy_garbage_is_rejected_without_panic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_temp(dir.path(), "junk.npy", b"not a numpy file at all");
    assert!(read_npy_f32(&path).is_err());
    let empty = write_temp(dir.path(), "empty.npy", b"");
    assert!(read_npy_f32(&empty).is_err());
    let short = write_temp(dir.path(), "magic.npy", b"\x93NUMPY\x01");
    assert!(read_npy_f32(&short).is_err());
}

#[test]
fn npy_huge_header_length_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut bytes = b"\x93NUMPY".to_vec();
    bytes.extend_from_slice(&[2, 0]);
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(b"{'descr': '<f4'");
    let path = write_temp(dir.path(), "huge.npy", &bytes);
    assert!(read_npy_f32(&path).is_err());
}

#[test]
fn npy_unknown_version_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bytes = npy_bytes(9, DICT_F32_1_1_4, &f32_bytes(&[1.0; 4]));
    let path = write_temp(dir.path(), "v9.npy", &bytes);
    assert!(read_npy_f32(&path).is_err());
}

#[test]
fn npy_zero_size_reads_empty_tensor() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dict = "{'descr': '<f4', 'fortran_order': False, 'shape': (1, 0, 4), }";
    let path = write_temp(dir.path(), "zero.npy", &npy_bytes(1, dict, &[]));
    let tensor = read_npy_f32(&path).expect("zero-size npy is valid");
    assert_eq!(tensor.shape, vec![1, 0, 4]);
    assert_eq!(tensor.numel(), 0);
}

#[test]
fn npy_missing_file_names_the_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("nope.npy");
    let err = err_of(read_npy_f32(&missing), "missing file");
    assert!(err.contains("nope.npy"), "error: {err}");
}

// ---------------------------------------------------------------------------
// load_voice_wav (CI-safe)
// ---------------------------------------------------------------------------

fn write_wav_i16(path: &Path, rate: u32, channels: u16, samples: &[i16]) {
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
    for s in samples {
        writer.write_sample(*s).expect("write sample");
    }
    writer.finalize().expect("finalize wav");
}

#[test]
fn wav_24k_mono_16bit_loads_unchanged_length() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("m16.wav");
    write_wav_i16(&path, 24_000, 1, &vec![16_384i16; 24_000]);
    let audio = load_voice_wav(&path).expect("valid wav");
    assert_eq!(audio.len(), 24_000);
    assert!(
        (audio[0] - 0.5).abs() < 1e-4,
        "sample scaled to [-1,1): {}",
        audio[0]
    );
}

#[test]
fn wav_48k_stereo_is_mixed_and_resampled_to_24k() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("s48.wav");
    // 48 000 frames of stereo: left 0.5, right 0.25 -> mono mix 0.375 -> 24 000 frames at 24 kHz.
    let mut samples = Vec::with_capacity(96_000);
    for _ in 0..48_000 {
        samples.push(16_384i16);
        samples.push(8_192i16);
    }
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).expect("create wav");
    for s in &samples {
        writer.write_sample(*s).expect("write");
    }
    writer.finalize().expect("finalize");
    let audio = load_voice_wav(&path).expect("valid stereo wav");
    let len = audio.len();
    assert!((23_700..=24_300).contains(&len), "resampled length {len}");
    let mid = audio[len / 2];
    assert!((mid - 0.375).abs() < 0.02, "mono mix level {mid}");
}

#[test]
fn wav_24bit_mono_is_scaled() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("m24.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 24_000,
        bits_per_sample: 24,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).expect("create wav");
    for _ in 0..1_000 {
        writer.write_sample(1i32 << 22).expect("write");
    }
    writer.finalize().expect("finalize");
    let audio = load_voice_wav(&path).expect("valid 24-bit wav");
    assert_eq!(audio.len(), 1_000);
    assert!(
        (audio[0] - 0.5).abs() < 1e-4,
        "24-bit scaling: {}",
        audio[0]
    );
}

#[test]
fn wav_float32_mono_is_kept() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("f32.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 24_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(&path, spec).expect("create wav");
    for _ in 0..500 {
        writer.write_sample(0.25f32).expect("write");
    }
    writer.finalize().expect("finalize");
    let audio = load_voice_wav(&path).expect("valid float wav");
    assert_eq!(audio.len(), 500);
    assert!((audio[10] - 0.25).abs() < 1e-6);
}

#[test]
fn wav_empty_file_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_temp(dir.path(), "empty.wav", b"");
    assert!(load_voice_wav(&path).is_err());
}

#[test]
fn wav_garbage_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_temp(
        dir.path(),
        "junk.wav",
        b"this is definitely not a RIFF file",
    );
    assert!(load_voice_wav(&path).is_err());
}

#[test]
fn wav_header_only_without_samples_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("none.wav");
    write_wav_i16(&path, 24_000, 1, &[]);
    assert!(load_voice_wav(&path).is_err());
}

// ---------------------------------------------------------------------------
// OrtPocket::load and PocketModel::load failure paths (CI-safe)
// ---------------------------------------------------------------------------

#[test]
fn ort_load_nonexistent_dir_names_bundle_json() {
    let missing = std::env::temp_dir().join("vox-tamil-no-such-pack-dir-xyz");
    let err = err_of(OrtPocket::load(&missing), "missing dir must fail");
    assert!(err.contains("bundle.json"), "error: {err}");
}

#[test]
fn ort_load_empty_dir_names_bundle_json() {
    let dir = tempfile::tempdir().expect("tempdir");
    let err = err_of(OrtPocket::load(dir.path()), "empty dir must fail");
    assert!(err.contains("bundle.json"), "error: {err}");
}

#[test]
fn ort_load_without_onnx_files_names_the_first_missing_model() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_temp(dir.path(), "bundle.json", MINIMAL_BUNDLE.as_bytes());
    let err = err_of(OrtPocket::load(dir.path()), "no onnx files");
    assert!(err.contains("text_conditioner.onnx"), "error: {err}");
}

#[test]
fn pocket_model_load_empty_dir_fails_with_message() {
    let dir = tempfile::tempdir().expect("tempdir");
    let err = err_of(PocketModel::load(dir.path()), "empty dir must fail");
    assert!(err.contains("bundle.json"), "error: {err}");
}

// ---------------------------------------------------------------------------
// LIVE tests (need the pack): cargo test --lib tamil_pocket_ort -- --ignored
//
// Parity is checked against tests/fixtures/tamil/tamil_engine_golden.json, which was
// generated with the same ONNX Runtime line the ort crate bundles (1.24.x). The
// fixture's `onnxruntime_version` must match that runtime, or the tests fail and the
// fixture has to be regenerated.
// ---------------------------------------------------------------------------

/// Relative tolerance for the voice embedding sum against the golden.
const VOICE_SUM_REL_TOL: f64 = 1e-4;
/// Max abs diff over the first 3 latent frames against the golden.
const FIRST_FRAME_TOL: f32 = 2e-3;
/// Relative RMS tolerance against the golden.
const RMS_REL_TOL: f64 = 0.05;
/// Samples per latent frame (12.5 Hz at 24 kHz).
const SAMPLES_PER_FRAME: usize = 1920;
/// Latent width.
const LATENT_DIM: usize = 32;

/// Runtime version reported by `ort::info()`, e.g. "1.24.2" from "...git-branch=rel-1.24.2,...".
fn runtime_version() -> Option<String> {
    let info = ort::info();
    let rest = info.split("rel-").nth(1)?;
    Some(
        rest.split(|c: char| c == ',' || c.is_whitespace())
            .next()?
            .to_string(),
    )
}

/// Fails unless the golden's `onnxruntime_version` is a prefix of the runtime's version.
fn require_matching_golden(fixture: &serde_json::Value) {
    let golden_version = fixture["onnxruntime_version"]
        .as_str()
        .expect("golden fixture must record onnxruntime_version");
    let runtime =
        runtime_version().unwrap_or_else(|| format!("unknown (ort::info: {})", ort::info()));
    println!("ort: {}", ort::info());
    println!("golden onnxruntime_version: {golden_version}");
    assert!(
        runtime.starts_with(golden_version),
        "golden fixture was made with onnxruntime {golden_version} but the ort crate runs {runtime}: \
         regenerate src-tauri/tests/fixtures/tamil/tamil_engine_golden.json with a matching onnxruntime"
    );
}

/// Measurements for one golden text.
struct Measured {
    key: &'static str,
    frames: usize,
    want_frames: i64,
    first_diff: f32,
    audio_len: usize,
    rms: f64,
    want_rms: f64,
    rtf: f64,
}

/// Generates and decodes one golden text at temperature 0 and prints the measurements.
/// Assertions are left to the caller, so every text is measured before any check fails.
fn measure_text(
    model: &mut PocketModel,
    fixture: &serde_json::Value,
    key: &'static str,
) -> Measured {
    let entry = &fixture["texts"][key];
    let text = entry["text"].as_str().expect("text");
    let want_frames = entry["frames"].as_i64().expect("frames");
    let want_rms = entry["rms"].as_f64().expect("rms");
    let want_first = golden_f32s(&entry["first_latents"]);

    let (prepared, guess) = prepare_text_prompt(text).expect("prepare text");
    let ids: Vec<i64> = model
        .tokenizer
        .encode(&prepared)
        .into_iter()
        .map(i64::from)
        .collect();
    // Reference rule: the bundle's recommended value, else guess + 2 (null in this pack).
    let frames_after_eos = model
        .bundle
        .recommended_frames_after_eos
        .unwrap_or(guess + 2);
    let max_frames = estimate_max_frames(ids.len());
    let params = GenParams {
        temperature: 0.0,
        lsd_steps: 1,
        eos_threshold: -4.0,
    };
    let mut rng = Rng::new(1);

    let started = Instant::now();
    let latents = generate_chunk_latents(
        &mut model.backend,
        &model.bundle,
        &model.voice,
        &ids,
        frames_after_eos,
        max_frames,
        &params,
        &mut rng,
    )
    .expect("generate latents");
    let frames = latents.len() / LATENT_DIM;
    assert!(frames >= 3, "{key}: need at least 3 frames to compare");
    let first_diff = latents[..want_first.len()]
        .iter()
        .zip(want_first.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);

    let audio = decode_latents(&mut model.backend, &model.bundle, &latents, 15).expect("decode");
    let elapsed = started.elapsed().as_secs_f64();
    let sum_sq: f64 = audio.iter().map(|v| f64::from(*v) * f64::from(*v)).sum();
    let rms = (sum_sq / audio.len().max(1) as f64).sqrt();
    let audio_seconds = audio.len() as f64 / 24_000.0;
    let rtf = elapsed / audio_seconds;
    println!(
        "{key}: frames {frames} (golden {want_frames}), first-3-frame max abs diff {first_diff:.2e}, \
         rms {rms:.5} (golden {want_rms:.5}, rel {:.4}), audio {} samples, RTF {rtf:.3}",
        (rms - want_rms).abs() / want_rms,
        audio.len(),
    );
    Measured {
        key,
        frames,
        want_frames,
        first_diff,
        audio_len: audio.len(),
        rms,
        want_rms,
        rtf,
    }
}

/// Every failed check for one text (empty when it passes).
fn failures_for(m: &Measured) -> Vec<String> {
    let mut failures = Vec::new();
    if m.frames as i64 != m.want_frames {
        failures.push(format!(
            "{}: frames {} vs golden {} (exact)",
            m.key, m.frames, m.want_frames
        ));
    }
    if m.first_diff >= FIRST_FRAME_TOL {
        failures.push(format!(
            "{}: first-3-frame max abs diff {:.3e} (tolerance {FIRST_FRAME_TOL})",
            m.key, m.first_diff
        ));
    }
    if m.audio_len != m.frames * SAMPLES_PER_FRAME {
        failures.push(format!(
            "{}: audio {} samples vs frames*1920",
            m.key, m.audio_len
        ));
    }
    let rms_rel = (m.rms - m.want_rms).abs() / m.want_rms;
    if rms_rel >= RMS_REL_TOL {
        failures.push(format!(
            "{}: rms rel {rms_rel:.4} (tolerance {RMS_REL_TOL})",
            m.key
        ));
    }
    if m.rtf >= 1.0 {
        failures.push(format!("{}: RTF {:.3} must stay below 1.0", m.key, m.rtf));
    }
    failures
}

#[test]
#[ignore = "needs the Tamil pack; set VOX_TAMIL_DIR"]
fn live_voice_embedding_matches_reference() {
    let dir = pack_dir();
    let fixture = golden();
    require_matching_golden(&fixture);
    let mut backend = OrtPocket::load(&dir).expect("load pack");
    let emb = backend
        .encode_voice(&dir.join("sample_geetha.wav"), &dir)
        .expect("encode voice");
    let want_shape: Vec<usize> = fixture["voice_emb_shape"]
        .as_array()
        .expect("shape array")
        .iter()
        .map(|v| v.as_u64().expect("dim") as usize)
        .collect();
    assert_eq!(emb.shape, want_shape, "voice embedding shape");
    assert_eq!(emb.shape, vec![1, 72, 1024], "voice embedding shape");
    let sum: f64 = emb
        .as_f32()
        .expect("f32")
        .iter()
        .map(|v| f64::from(*v))
        .sum();
    let want = fixture["voice_emb_sum"].as_f64().expect("voice_emb_sum");
    let rel = (sum - want).abs() / want.abs();
    println!("voice embedding sum {sum:.5} (golden {want:.5}, rel {rel:.2e})");
    assert!(
        rel < VOICE_SUM_REL_TOL,
        "voice embedding sum {sum} vs golden {want} (rel {rel:.2e}, tolerance {VOICE_SUM_REL_TOL})"
    );
}

#[test]
#[ignore = "needs the Tamil pack; set VOX_TAMIL_DIR"]
fn live_text_t1_matches_reference() {
    end_to_end_text("t1");
}

#[test]
#[ignore = "needs the Tamil pack; set VOX_TAMIL_DIR"]
fn live_text_t2_matches_reference() {
    end_to_end_text("t2");
}

#[test]
#[ignore = "needs the Tamil pack; set VOX_TAMIL_DIR"]
fn live_text_t3_matches_reference() {
    end_to_end_text("t3");
}

/// One text through the real `PocketModel::load` path (Rust-computed voice embedding).
fn end_to_end_text(key: &'static str) {
    let dir = pack_dir();
    let fixture = golden();
    require_matching_golden(&fixture);
    let mut model = PocketModel::load(&dir).expect("load pocket model");
    let measured = measure_text(&mut model, &fixture, key);
    let failures = failures_for(&measured);
    assert!(
        failures.is_empty(),
        "{key} failures:\n{}",
        failures.join("\n")
    );
}

#[test]
#[ignore = "needs the Tamil pack; set VOX_TAMIL_DIR"]
fn live_pocket_model_loads_twice() {
    let dir = pack_dir();
    let first = PocketModel::load(&dir).expect("first load");
    assert_eq!(first.voice.positions, 73, "BOS + 72 voice frames");
    assert_eq!(first.voice.state.len(), first.bundle.flow_state.len());
    drop(first);
    let second = PocketModel::load(&dir).expect("second load in the same process");
    assert_eq!(second.voice.positions, 73);
}
