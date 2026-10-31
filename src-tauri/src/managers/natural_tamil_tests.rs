//! Spec for the Tamil voice glue (`natural_tamil.rs`). CI-safe tests use temp folders with
//! sparse files of the pinned sizes, a scripted fake [`PocketBackend`] and a synthetic
//! SentencePiece model built in code. Nothing here needs the real pack, except the one
//! `#[ignore]` live test, which needs `VOX_TAMIL_DIR`.

use crate::managers::natural_engine::{
    HealthState, NaturalHealth, NaturalSynth, SynthEngine, TextMode, DEFAULT_AUTO_RETRY_COOLDOWN,
};
use crate::managers::natural_sink::{AudioSink, CommitResult, FlushOutcome};
use crate::managers::natural_tamil::{
    build_tamil_stack, tamil_loader, tamil_model_dir, tamil_natural_config, tamil_pack_status,
    TamilEngine, TamilPackStatus, TAMIL_MODEL_DIRNAME, TAMIL_PACK_FILES,
};
use crate::managers::natural_test_support::FakeStreamFactory;
use crate::managers::tamil_pocket_core::{
    prepare_voice_state, Bundle, Dtype, Fill, GenParams, LmOut, PocketBackend, StateSpec, Tensor,
    VoiceState,
};
use crate::managers::tamil_tokenizer::TamilTokenizer;
use crate::managers::tts_queue::Synth;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempDir;

const KIND_NORMAL: u64 = 1;
const KIND_UNKNOWN: u64 = 2;
const KIND_CONTROL: u64 = 3;
const MODEL_TYPE_BPE: u64 = 2;
const LATENT_DIM: usize = 32;
const SAMPLES_PER_FRAME: usize = 1920;

// ---- synthetic SentencePiece model (same minimal writer as the pipeline tests) ----

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let low = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(low);
            return;
        }
        out.push(low | 0x80);
    }
}

fn put_key(out: &mut Vec<u8>, field: u64, wire: u64) {
    put_varint(out, (field << 3) | wire);
}

fn put_bytes(out: &mut Vec<u8>, field: u64, data: &[u8]) {
    put_key(out, field, 2);
    put_varint(out, data.len() as u64);
    out.extend_from_slice(data);
}

fn put_uint(out: &mut Vec<u8>, field: u64, value: u64) {
    put_key(out, field, 0);
    put_varint(out, value);
}

fn put_float(out: &mut Vec<u8>, field: u64, value: f32) {
    put_key(out, field, 5);
    out.extend_from_slice(&value.to_le_bytes());
}

fn piece_msg(text: &str, score: f32, kind: u64) -> Vec<u8> {
    let mut msg = Vec::new();
    put_bytes(&mut msg, 1, text.as_bytes());
    put_float(&mut msg, 2, score);
    put_uint(&mut msg, 3, kind);
    msg
}

fn normalizer_msg() -> Vec<u8> {
    let mut msg = Vec::new();
    put_bytes(&mut msg, 1, b"nmt_nfkc");
    put_uint(&mut msg, 3, 1);
    put_uint(&mut msg, 4, 1);
    put_uint(&mut msg, 5, 1);
    msg
}

fn model_bytes(pieces: &[(String, u64)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (text, kind) in pieces {
        put_bytes(&mut out, 1, &piece_msg(text, 0.0, *kind));
    }
    let mut trainer = Vec::new();
    put_uint(&mut trainer, 3, MODEL_TYPE_BPE);
    put_uint(&mut trainer, 40, 0);
    put_bytes(&mut out, 2, &trainer);
    put_bytes(&mut out, 3, &normalizer_msg());
    out
}

const LATIN_SYMBOLS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.,;:!?";

/// One piece per character, so every character is exactly one token.
fn toy_tokenizer() -> TamilTokenizer {
    let mut pieces: Vec<(String, u64)> = vec![
        ("<unk>".to_string(), KIND_UNKNOWN),
        ("<s>".to_string(), KIND_CONTROL),
        ("</s>".to_string(), KIND_CONTROL),
        ("\u{2581}".to_string(), KIND_NORMAL),
    ];
    for ch in LATIN_SYMBOLS.chars() {
        pieces.push((ch.to_string(), KIND_NORMAL));
    }
    TamilTokenizer::from_bytes(&model_bytes(&pieces)).expect("synthetic model parses")
}

/// Engine bundle with the real dimensions (the flow cache starts at zero so states compare).
fn test_bundle() -> Bundle {
    let spec =
        |index: usize, name: &str, dtype: Dtype, fill: Fill, shape: Vec<usize>, key: &str| {
            StateSpec {
                index,
                input_name: name.to_string(),
                output_name: format!("out_{name}"),
                dtype,
                fill,
                shape,
                module: "m".to_string(),
                key: key.to_string(),
            }
        };
    Bundle {
        sample_rate: 24_000,
        frame_rate: 12.5,
        samples_per_frame: SAMPLES_PER_FRAME,
        latent_dim: LATENT_DIM,
        conditioning_dim: 1024,
        max_token_per_chunk: 50,
        insert_bos_before_voice: false,
        flow_state: vec![
            spec(
                0,
                "state_0",
                Dtype::F32,
                Fill::Zeros,
                vec![2, 1, 1000, 1, 1],
                "cache",
            ),
            spec(1, "state_1", Dtype::I64, Fill::Zeros, vec![1], "step"),
        ],
        mimi_state: vec![
            spec(0, "state_0", Dtype::Bool, Fill::Ones, vec![1], "first"),
            spec(
                1,
                "state_1",
                Dtype::F32,
                Fill::Zeros,
                vec![1, 4, 0],
                "previous",
            ),
        ],
        kv_capacity: 1000,
        recommended_frames_after_eos: None,
    }
}

// ---- fake backend ----

/// Fake model: every text embeds to 0.5, the flow head returns zeros, EOS fires on the first
/// generation step, and every decoded frame becomes `decode_value` samples.
struct FakeBackend {
    calls: Arc<AtomicUsize>,
    fail_text: bool,
    decode_value: f32,
}

impl FakeBackend {
    fn new(calls: Arc<AtomicUsize>, fail_text: bool, decode_value: f32) -> FakeBackend {
        FakeBackend {
            calls,
            fail_text,
            decode_value,
        }
    }

    fn count(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
    }
}

impl PocketBackend for FakeBackend {
    fn text_embed(&mut self, token_ids: &[i64]) -> Result<Tensor, String> {
        self.count();
        if self.fail_text {
            return Err("boom".to_string());
        }
        let len = token_ids.len();
        Tensor::f32(vec![1, len, 1024], vec![0.5; len * 1024])
    }

    fn lm_step(
        &mut self,
        _sequence: &Tensor,
        _text_embeddings: &Tensor,
        state: Vec<Tensor>,
    ) -> Result<LmOut, String> {
        self.count();
        Ok(LmOut {
            conditioning: vec![0.5; 1024],
            eos_logit: 5.0,
            state,
        })
    }

    fn flow(
        &mut self,
        _conditioning: &[f32],
        _s: f32,
        _t: f32,
        _x: &[f32],
    ) -> Result<Vec<f32>, String> {
        self.count();
        Ok(vec![0.0; LATENT_DIM])
    }

    fn decode(
        &mut self,
        latents: &Tensor,
        state: Vec<Tensor>,
    ) -> Result<(Vec<f32>, Vec<Tensor>), String> {
        self.count();
        let frames = latents.shape[1];
        Ok((vec![self.decode_value; frames * SAMPLES_PER_FRAME], state))
    }
}

fn no_noise() -> GenParams {
    GenParams {
        temperature: 0.0,
        lsd_steps: 1,
        eos_threshold: -4.0,
    }
}

/// Engine over the fake backend. The voice is prepared with the same fake (its calls are
/// counted too, so tests compare call counts before and after the action under test).
fn fake_engine(fail_text: bool, decode_value: f32) -> (TamilEngine, Arc<AtomicUsize>) {
    let bundle = test_bundle();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut backend = FakeBackend::new(calls.clone(), false, decode_value);
    let embedding = Tensor::f32(vec![1, 4, 1024], vec![0.5; 4 * 1024]).expect("embedding");
    let voice = prepare_voice_state(&mut backend, &bundle, &embedding, None).expect("voice");
    let backend = FakeBackend::new(calls.clone(), fail_text, decode_value);
    let engine = TamilEngine::from_parts(
        Box::new(backend),
        bundle,
        toy_tokenizer(),
        voice,
        no_noise(),
        7,
    );
    (engine, calls)
}

// ---- pack helpers ----

/// Writes every pack file as a sparse file of its pinned size.
fn write_pack(dir: &Path) {
    fs::create_dir_all(dir).expect("create pack dir");
    for (name, size) in TAMIL_PACK_FILES {
        let file = File::create(dir.join(name)).expect("create pack file");
        file.set_len(size).expect("set size");
    }
}

fn all_names() -> Vec<String> {
    TAMIL_PACK_FILES
        .iter()
        .map(|(name, _)| name.to_string())
        .collect()
}

fn load_err(r: Result<Box<dyn SynthEngine>, String>) -> String {
    match r {
        Ok(_) => panic!("expected the load to fail"),
        Err(e) => e,
    }
}

// ---- pack status ----

#[test]
fn pack_with_all_nine_files_at_exact_sizes_is_ready() {
    let dir = TempDir::new().unwrap();
    write_pack(dir.path());
    assert_eq!(tamil_pack_status(dir.path()), TamilPackStatus::Ready);
}

#[test]
fn pack_missing_one_file_names_that_file() {
    let dir = TempDir::new().unwrap();
    write_pack(dir.path());
    fs::remove_file(dir.path().join("tokenizer.model")).unwrap();
    assert_eq!(
        tamil_pack_status(dir.path()),
        TamilPackStatus::Missing(vec!["tokenizer.model".to_string()])
    );
}

#[test]
fn pack_wrong_size_names_that_file() {
    let dir = TempDir::new().unwrap();
    write_pack(dir.path());
    let file = File::create(dir.path().join("bundle.json")).unwrap();
    file.set_len(24_360).unwrap();
    assert_eq!(
        tamil_pack_status(dir.path()),
        TamilPackStatus::WrongSize(vec!["bundle.json".to_string()])
    );
}

#[test]
fn pack_missing_is_reported_before_wrong_size() {
    let dir = TempDir::new().unwrap();
    write_pack(dir.path());
    fs::remove_file(dir.path().join("sample_geetha.wav")).unwrap();
    File::create(dir.path().join("bundle.json"))
        .unwrap()
        .set_len(1)
        .unwrap();
    assert_eq!(
        tamil_pack_status(dir.path()),
        TamilPackStatus::Missing(vec!["sample_geetha.wav".to_string()])
    );
}

#[test]
fn pack_folder_that_does_not_exist_is_missing_all_nine() {
    let dir = TempDir::new().unwrap();
    let absent = dir.path().join("no-such-pack");
    assert_eq!(
        tamil_pack_status(&absent),
        TamilPackStatus::Missing(all_names())
    );
    assert_eq!(all_names().len(), 9);
}

#[test]
fn pack_extra_files_are_ignored() {
    let dir = TempDir::new().unwrap();
    write_pack(dir.path());
    fs::write(dir.path().join("README.txt"), b"notes").unwrap();
    fs::create_dir_all(dir.path().join("voices")).unwrap();
    assert_eq!(tamil_pack_status(dir.path()), TamilPackStatus::Ready);
}

#[test]
fn pack_directory_where_a_file_belongs_counts_as_missing() {
    let dir = TempDir::new().unwrap();
    write_pack(dir.path());
    fs::remove_file(dir.path().join("bos_before_voice.npy")).unwrap();
    fs::create_dir_all(dir.path().join("bos_before_voice.npy")).unwrap();
    assert_eq!(
        tamil_pack_status(dir.path()),
        TamilPackStatus::Missing(vec!["bos_before_voice.npy".to_string()])
    );
}

#[test]
fn pack_dir_is_the_named_folder_under_the_root() {
    let root = Path::new("/tmp/tts_models_root");
    assert_eq!(tamil_model_dir(root), root.join("pocket-tts-tamil-int4"));
    assert_eq!(TAMIL_MODEL_DIRNAME, "pocket-tts-tamil-int4");
}

// ---- TamilEngine contract on a fake backend ----

#[test]
fn engine_synth_returns_the_fake_audio() {
    let (mut engine, _calls) = fake_engine(false, 0.25);
    let audio = engine
        .synth("Hello there.", "any", 1.0)
        .expect("synth succeeds");
    assert!(!audio.is_empty(), "fake model produced audio");
    assert_eq!(audio.len() % SAMPLES_PER_FRAME, 0, "whole frames only");
    assert!(
        audio.iter().all(|&s| (s - 0.25).abs() < 1e-6),
        "every sample is the fake decoder value"
    );
}

#[test]
fn engine_ignores_voice_and_speed() {
    let (mut engine, _calls) = fake_engine(false, 0.25);
    let first = engine.synth("Hello there.", "alpha", 1.0).expect("first");
    let second = engine.synth("Hello there.", "beta", 3.0).expect("second");
    assert_eq!(
        first, second,
        "voice and speed arguments do not change output"
    );
}

#[test]
fn engine_empty_text_is_ok_empty_and_never_calls_the_backend() {
    let (mut engine, calls) = fake_engine(false, 0.25);
    let before = calls.load(Ordering::SeqCst);
    let audio = engine
        .synth("", "any", 1.0)
        .expect("empty text is not an error");
    assert!(audio.is_empty());
    let whitespace = engine
        .synth("   \n ", "any", 1.0)
        .expect("whitespace is not an error");
    assert!(whitespace.is_empty());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        before,
        "the backend saw no calls for empty text"
    );
}

#[test]
fn engine_backend_error_is_an_err_not_a_panic() {
    let (mut engine, _calls) = fake_engine(true, 0.25);
    let result = engine.synth("Hello there.", "any", 1.0);
    let error = result.expect_err("backend error must surface as Err");
    assert!(error.contains("boom"), "error keeps the cause: {error:?}");
}

#[test]
fn engine_keeps_the_voice_state_and_calls_are_independent() {
    let (mut engine, _calls) = fake_engine(false, 0.25);
    let voice_before = engine.voice().clone();
    let first = engine.synth("Hello there.", "any", 1.0).expect("first");
    let second = engine.synth("Hello there.", "any", 1.0).expect("second");
    assert_eq!(
        first, second,
        "zero-noise calls are independent and identical"
    );
    assert_eq!(
        engine.voice(),
        &voice_before,
        "synthesis never changes the cached voice state"
    );
    let _: &VoiceState = engine.voice();
}

// ---- loader ----

#[test]
fn loader_on_an_empty_folder_returns_the_missing_error_and_never_panics() {
    let dir = TempDir::new().unwrap();
    let mut loader = tamil_loader(dir.path().to_path_buf());
    let err = load_err(loader.load());
    assert!(err.contains("bundle.json"), "error names a file: {err:?}");
    assert!(
        err.contains(TAMIL_MODEL_DIRNAME) || err.contains(&dir.path().display().to_string()),
        "error names the pack folder: {err:?}"
    );
}

#[test]
fn loader_with_a_wrong_size_file_names_it() {
    let dir = TempDir::new().unwrap();
    let pack = dir.path().join(TAMIL_MODEL_DIRNAME);
    write_pack(&pack);
    File::create(pack.join("tokenizer.model"))
        .unwrap()
        .set_len(10)
        .unwrap();
    let mut loader = tamil_loader(pack);
    let err = load_err(loader.load());
    assert!(err.contains("tokenizer.model"), "got {err:?}");
    assert!(err.contains("wrong size"), "got {err:?}");
}

// ---- stack ----

#[test]
fn stack_builds_without_the_pack_and_reports_the_pack_missing() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let stack = build_tamil_stack(root, FakeStreamFactory::new(24_000));
    assert!(
        stack.health.is_ready(),
        "nothing failed yet: the model loads lazily, as Phonon's does"
    );
    assert_eq!(
        tamil_pack_status(&tamil_model_dir(root)),
        TamilPackStatus::Missing(all_names()),
        "the UI sees the whole pack missing"
    );
    let mut loader = tamil_loader(tamil_model_dir(root));
    let err = load_err(loader.load());
    assert!(err.contains("not installed"), "got {err:?}");
}

#[test]
fn stack_config_text_mode_is_tamil() {
    assert_eq!(tamil_natural_config().text_mode, TextMode::Tamil);
}

// ---- live (needs the real pack) ----

/// Sink that keeps the committed PCM in memory (no audio device).
struct CaptureSink {
    generation: AtomicU64,
    samples: Mutex<Vec<f32>>,
    first_commit: Mutex<Option<Instant>>,
}

impl CaptureSink {
    fn new() -> CaptureSink {
        CaptureSink {
            generation: AtomicU64::new(0),
            samples: Mutex::new(Vec::new()),
            first_commit: Mutex::new(None),
        }
    }
}

impl AudioSink for CaptureSink {
    fn device_rate(&self) -> Result<u32, String> {
        Ok(24_000)
    }
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
    fn commit(&self, generation: u64, samples: Vec<f32>) -> CommitResult {
        if generation != self.generation() {
            return CommitResult::Stale;
        }
        let mut first = self.first_commit.lock().unwrap_or_else(|e| e.into_inner());
        first.get_or_insert_with(Instant::now);
        drop(first);
        self.samples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(samples);
        CommitResult::Accepted
    }
    fn invalidate(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn flush(&self, _budget: Duration) -> FlushOutcome {
        FlushOutcome::Confirmed
    }
    fn audible(&self) -> bool {
        false
    }
    fn queued_secs(&self) -> f32 {
        0.0
    }
    fn is_errored(&self) -> bool {
        false
    }
    fn recover(&self) {}
    fn maintain(&self, _now: Instant) {}
}

#[test]
#[ignore = "needs the Tamil pack; set VOX_TAMIL_DIR"]
fn natural_tamil_live_speaks_through_the_real_stack() {
    let Some(dir) = std::env::var_os("VOX_TAMIL_DIR") else {
        println!(
            "SKIPPED: VOX_TAMIL_DIR is not set (point it at the pocket-tts-tamil-int4 folder)"
        );
        return;
    };
    let dir = PathBuf::from(dir);
    let sink = Arc::new(CaptureSink::new());
    let health = NaturalHealth::new(DEFAULT_AUTO_RETRY_COOLDOWN);
    let mut synth = NaturalSynth::new(
        tamil_loader(dir),
        sink.clone(),
        health.clone(),
        tamil_natural_config(),
    );

    let started = Instant::now();
    synth.speak("வணக்கம், நீங்கள் எப்படி இருக்கிறீர்கள்", None, None, 0.0);
    let deadline = started + Duration::from_secs(180);
    loop {
        let has_audio = !sink
            .samples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty();
        if has_audio && !synth.is_speaking() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no complete utterance within 180 s"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    let first_at = sink
        .first_commit
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .unwrap_or(started);
    println!(
        "time_to_first_audio_ms={:.0}",
        first_at.duration_since(started).as_secs_f64() * 1000.0
    );
    assert!(health.is_ready(), "the Tamil engine must stay Ready");
    assert_eq!(health.state(), HealthState::Ready);

    let pcm = sink
        .samples
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let duration = pcm.len() as f64 / 24_000.0;
    println!("duration_s={duration:.3} samples={}", pcm.len());
    assert!(
        pcm.iter().all(|s| s.is_finite()),
        "every sample must be finite"
    );
    assert!(
        (2.0..=6.0).contains(&duration),
        "duration {duration:.3} s outside 2.0..=6.0 s"
    );
    let rms = (pcm
        .iter()
        .map(|&x| f64::from(x) * f64::from(x))
        .sum::<f64>()
        / pcm.len() as f64)
        .sqrt();
    println!("rms={rms:.5}");
    assert!(rms > 0.005, "rms {rms:.5} is too quiet: not voiced");
    drop(synth);
}
