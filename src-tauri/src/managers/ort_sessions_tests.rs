//! Tests for the ONNX session adapters (design A2', D3b, A10).
//!
//! The pure tests run in the normal suite. The live tests are `#[ignore]` and need the real
//! models: `VOX_SMART_TURN_MODEL` and `VOX_SPEAKER_MODEL`. Run them with
//! `cargo test --lib managers::ort_sessions -- --ignored --nocapture`.
//! A missing variable or file fails with `GATE UNMET: <what>`; nothing is skipped silently.

use super::*;
use crate::managers::aux_model::{aux_spec, AuxKind};
use crate::managers::smart_turn::TurnDetector;
use crate::managers::speaker_id::{cosine, SPEAKER_SPEC};
use crate::managers::voice_adapters::ServiceTurnDetector;
use crate::managers::voice_models::{ModelKind, Reply, VoiceModelService};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

fn close(actual: Option<f32>, want: f64) {
    let got = f64::from(actual.expect("expected a value"));
    assert!((got - want).abs() < 1e-5, "got {got}, want {want}");
}

#[test]
fn zero_half_and_one_are_kept() {
    close(normalize_probability(0.0), 0.0);
    close(normalize_probability(0.5), 0.5);
    close(normalize_probability(1.0), 1.0);
}

#[test]
fn value_above_one_goes_through_sigmoid() {
    close(normalize_probability(1.2), sigmoid(1.2));
}

#[test]
fn value_below_zero_goes_through_sigmoid() {
    close(normalize_probability(-3.0), sigmoid(-3.0));
}

#[test]
fn non_finite_values_give_none() {
    for raw in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(normalize_probability(raw), None, "raw {raw}");
    }
}

#[test]
fn output_is_always_a_probability() {
    for raw in [-1e6_f32, -50.0, -1.0001, 1.0001, 50.0, 1e6] {
        let p = normalize_probability(raw).expect("finite input");
        assert!((0.0..=1.0).contains(&p), "raw {raw} gave {p}");
    }
}

// ---------------------------------------------------------------------------------------------
// Live tests (need the real models; see the module comment)
// ---------------------------------------------------------------------------------------------

const SMART_TURN_ENV: &str = "VOX_SMART_TURN_MODEL";
const SPEAKER_ENV: &str = "VOX_SPEAKER_MODEL";
const LIVE_TIMEOUT: Duration = Duration::from_secs(10);

fn gate_unmet(what: &str) -> ! {
    panic!("GATE UNMET: {what}")
}

fn require_model(var: &str) -> PathBuf {
    let Ok(value) = std::env::var(var) else {
        gate_unmet(&format!("environment variable {var} is not set"))
    };
    let path = PathBuf::from(value);
    if !path.is_file() {
        gate_unmet(&format!(
            "{var} does not point to a file: {}",
            path.display()
        ));
    }
    path
}

fn fixture_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/voice")
        .join(rel)
}

fn fixture_bytes(rel: &str) -> Vec<u8> {
    let path = fixture_path(rel);
    std::fs::read(&path)
        .unwrap_or_else(|e| gate_unmet(&format!("fixture {} unreadable: {e}", path.display())))
}

fn f32_le(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn fixture_f32(rel: &str) -> Vec<f32> {
    f32_le(&fixture_bytes(rel))
}

fn fixture_prob(rel: &str) -> f32 {
    let text = fixture_bytes(rel);
    String::from_utf8_lossy(&text)
        .trim()
        .parse::<f32>()
        .unwrap_or_else(|e| gate_unmet(&format!("fixture {rel} is not a number: {e}")))
}

/// Reads a 16 kHz mono 16-bit WAV fixture into f32 samples in [-1, 1).
fn fixture_wav16k(rel: &str) -> Vec<f32> {
    let path = fixture_path(rel);
    let reader = hound::WavReader::open(&path)
        .unwrap_or_else(|e| gate_unmet(&format!("wav {} unreadable: {e}", path.display())));
    let spec = reader.spec();
    if spec.sample_rate != 16_000 || spec.channels != 1 || spec.bits_per_sample != 16 {
        gate_unmet(&format!("{rel} is not 16 kHz mono 16-bit"));
    }
    reader
        .into_samples::<i16>()
        .map(|s| s.map(|v| f32::from(v) / 32768.0))
        .collect::<Result<Vec<f32>, _>>()
        .unwrap_or_else(|e| gate_unmet(&format!("wav {rel} samples unreadable: {e}")))
}

fn sha256_hex(path: &Path) -> String {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|e| gate_unmet(&format!("cannot read {}: {e}", path.display())));
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    format!("{:x}", hasher.finalize())
}

fn live_service(smart_turn: &Path, speaker: &Path) -> Arc<VoiceModelService> {
    let service = Arc::new(VoiceModelService::new(Arc::new(OrtLoader)));
    service.install(ModelKind::SmartTurn, smart_turn.to_path_buf());
    service.install(ModelKind::Speaker, speaker.to_path_buf());
    service
}

/// One request through the real service, waited for synchronously. `None` on failure.
fn service_reply(
    service: &VoiceModelService,
    kind: ModelKind,
    audio: Vec<f32>,
) -> Option<Vec<f32>> {
    let id = service
        .try_submit(kind, audio)
        .expect("service accepts the live request");
    let started = Instant::now();
    loop {
        match service.poll(id) {
            Some(Reply::Output(values)) => return Some(values),
            Some(Reply::Failed(_)) => return None,
            None => {}
        }
        if started.elapsed() > LIVE_TIMEOUT {
            service.abandon(id);
            panic!("live request timed out after {LIVE_TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
#[ignore = "needs the real models"]
fn live_a_model_files_match_the_pinned_sha256() {
    let turn = require_model(SMART_TURN_ENV);
    let speaker = require_model(SPEAKER_ENV);
    assert_eq!(
        sha256_hex(&turn),
        aux_spec(AuxKind::SmartTurn).sha256,
        "Smart Turn model sha256 differs from the pin"
    );
    assert_eq!(
        sha256_hex(&speaker),
        aux_spec(AuxKind::SpeakerResnet34).sha256,
        "speaker model sha256 differs from the pin"
    );
}

#[test]
#[ignore = "needs the real models"]
fn live_b_smart_turn_matches_reference_probabilities() {
    let turn = require_model(SMART_TURN_ENV);
    let speaker = require_model(SPEAKER_ENV);
    let service = live_service(&turn, &speaker);
    let detector = ServiceTurnDetector::new(Arc::clone(&service));
    let cancel = AtomicBool::new(false);

    for (wav, prob) in [
        ("smartturn/c1.wav", "smartturn/c1.prob.txt"),
        ("smartturn/i2.wav", "smartturn/i2.prob.txt"),
    ] {
        let audio = fixture_wav16k(wav);
        let got = detector
            .done_probability(&audio, LIVE_TIMEOUT, &cancel)
            .unwrap_or_else(|| panic!("no probability for {wav}"));
        let want = fixture_prob(prob);
        println!("{wav}: got {got:.6}, reference {want:.6}");
        assert!(
            (got - want).abs() <= 0.02,
            "{wav}: got {got}, reference {want}"
        );
    }

    for rel in [
        "smartturn/exact_8s.input.f32",
        "smartturn/long_10s.input.f32",
    ] {
        let raw = service_reply(&service, ModelKind::SmartTurn, fixture_f32(rel))
            .unwrap_or_else(|| panic!("no output for {rel}"));
        assert_eq!(raw.len(), 1, "{rel}: one probability expected");
        assert!(raw[0].is_finite(), "{rel}: non-finite output {raw:?}");
        println!("{rel}: raw output {:.6}", raw[0]);
    }

    let audio = fixture_wav16k("smartturn/c1.wav");
    let mut millis: Vec<f64> = Vec::with_capacity(20);
    for run in 0..20 {
        let started = Instant::now();
        let got = detector.done_probability(&audio, LIVE_TIMEOUT, &cancel);
        millis.push(started.elapsed().as_secs_f64() * 1000.0);
        assert!(got.is_some(), "timed run {run} gave no probability");
    }
    millis.sort_by(f64::total_cmp);
    let median = (millis[9] + millis[10]) / 2.0;
    println!(
        "smart turn latency over 20 runs (c1.wav, 8 s window): median {median:.1} ms, min {:.1} ms, max {:.1} ms",
        millis[0], millis[19]
    );
}

#[test]
#[ignore = "needs the real models"]
fn live_c_speaker_embeddings_match_the_reference() {
    let turn = require_model(SMART_TURN_ENV);
    let speaker = require_model(SPEAKER_ENV);
    let service = live_service(&turn, &speaker);

    let mut embeddings: Vec<(&str, Vec<f32>)> = Vec::new();
    for name in ["s1a_2.0s", "s2a_1.5s", "s3a_3.0s"] {
        let audio = fixture_f32(&format!("speaker/inputs/{name}.input.f32"));
        let got = service_reply(&service, ModelKind::Speaker, audio)
            .unwrap_or_else(|| panic!("no embedding for {name}"));
        let want = fixture_f32(&format!(
            "speaker/resnet34__wespeaker_ref/{name}.embedding.f32"
        ));
        let cos = cosine(&got, &want).expect("cosine of two non-zero embeddings");
        println!("{name}: cosine to reference {cos:.6}");
        assert!(cos >= 0.999, "{name}: cosine {cos} below 0.999");
        embeddings.push((name, got));
    }
    // Different speakers only (one clip per speaker is committed): printed, not asserted.
    for (i, (a, ea)) in embeddings.iter().enumerate() {
        for (b, eb) in &embeddings[i + 1..] {
            println!("cross-speaker cosine {a} vs {b}: {:?}", cosine(ea, eb));
        }
    }

    for name in ["silence_0.5s", "tone_399"] {
        let audio = fixture_f32(&format!("speaker/inputs/{name}.input.f32"));
        assert!(
            service_reply(&service, ModelKind::Speaker, audio).is_none(),
            "{name} must give no embedding"
        );
    }

    let tone = fixture_f32("speaker/inputs/tone_16000.input.f32");
    let got =
        service_reply(&service, ModelKind::Speaker, tone).expect("tone_16000 gives an embedding");
    assert_eq!(got.len(), SPEAKER_SPEC.dim);
    assert!(
        got.iter().all(|v| v.is_finite()),
        "tone_16000 must be finite"
    );
}
