//! Spec for `natural_kokoro` (step S3b, macOS only). Authoritative: do not weaken an assertion.
//! Fast tests run everywhere on macOS; `natural_live_*` tests need the real model files:
//!   VOX_NATURAL_MODEL_DIR=<dir with kokoro-v1.0.int8.onnx and voices.bin> \
//!   cargo test --lib natural_live -- --ignored --nocapture --test-threads=1
use crate::managers::natural_engine::*;
use crate::managers::natural_kokoro::*;
use crate::managers::natural_sink::{AudioSink, CpalStreamFactory, ManagedSink};
use crate::managers::natural_voices::{ModelBytes, NATURAL_VOICES};
use crate::managers::tts_queue::{StopOutcome, Synth};
use kokoro_tts::Voice;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[test]
fn every_curated_voice_resolves_to_a_crate_voice() {
    for v in NATURAL_VOICES {
        assert!(
            voice_from_id(v.id, 1.0).is_some(),
            "no crate voice for {}",
            v.id
        );
    }
}

#[test]
fn unknown_voice_ids_are_none() {
    for bad in [
        "",
        "nope",
        "AF_HEART",
        "af_heart ",
        "zf_xiaoni",
        "../af_heart",
    ] {
        assert!(voice_from_id(bad, 1.0).is_none(), "{bad:?}");
    }
}

#[test]
fn the_speed_is_forwarded_into_the_voice() {
    match voice_from_id("af_heart", 1.25) {
        Some(Voice::AfHeart(s)) => assert!((s - 1.25).abs() < 1e-6),
        other => panic!("unexpected voice {other:?}"),
    }
    match voice_from_id("bm_george", 0.8) {
        Some(Voice::BmGeorge(s)) => assert!((s - 0.8).abs() < 1e-6),
        other => panic!("unexpected voice {other:?}"),
    }
}

#[test]
fn the_loader_reports_provider_errors_without_panicking() {
    let mut loader = KokoroLoader::new(Box::new(|| Err("model files are missing".to_string())));
    let err = loader.load().err().expect("must fail");
    assert!(err.contains("missing"), "{err}");
}

#[test]
fn the_loader_rejects_garbage_bytes_with_an_error_not_a_panic() {
    let mut loader = KokoroLoader::new(Box::new(|| {
        Ok(ModelBytes {
            model: vec![1, 2, 3, 4],
            voices: vec![9, 9, 9],
        })
    }));
    assert!(loader.load().is_err());
}

// ---- live tests (real model; ignored by default) ----------------------------------------------

fn live_dir() -> PathBuf {
    std::env::var_os("VOX_NATURAL_MODEL_DIR")
        .map(PathBuf::from)
        .expect("set VOX_NATURAL_MODEL_DIR to the directory holding the two model files")
}

fn live_provider(dir: PathBuf) -> BytesProvider {
    Box::new(move || {
        Ok(ModelBytes {
            model: std::fs::read(dir.join("kokoro-v1.0.int8.onnx")).map_err(|e| e.to_string())?,
            voices: std::fs::read(dir.join("voices.bin")).map_err(|e| e.to_string())?,
        })
    })
}

fn load_live() -> Box<dyn SynthEngine> {
    KokoroLoader::new(live_provider(live_dir()))
        .load()
        .expect("model must load")
}

#[test]
#[ignore]
fn natural_live_basic_synthesis() {
    let mut engine = load_live();
    let t = Instant::now();
    let samples = engine
        .synth("Hello there.", "af_heart", 1.0)
        .expect("synth");
    println!(
        "basic: {} samples ({:.2} s audio) in {:.2?}",
        samples.len(),
        samples.len() as f32 / 24_000.0,
        t.elapsed()
    );
    assert!(samples.len() > 12_000, "at least half a second of audio");
    assert!(samples.iter().all(|s| s.is_finite()));
    assert!(t.elapsed() < Duration::from_secs(8));
}

#[test]
#[ignore]
fn natural_live_every_curated_voice_synthesises() {
    let mut engine = load_live();
    for v in NATURAL_VOICES {
        let t = Instant::now();
        let samples = engine
            .synth("Hi there.", v.id, 1.0)
            .unwrap_or_else(|e| panic!("{}: {e}", v.id));
        println!(
            "{}: {:.2} s audio in {:.2?}",
            v.id,
            samples.len() as f32 / 24_000.0,
            t.elapsed()
        );
        assert!(samples.len() > 6_000, "{}", v.id);
    }
}

#[test]
#[ignore]
fn natural_live_overflow_text_is_an_error_not_a_panic() {
    let mut engine = load_live();
    let long = "word ".repeat(250); // far above the 510-token limit
    let out = engine.synth(&long, "af_heart", 1.0);
    println!(
        "overflow: {:?}",
        out.as_ref().map(|s| s.len()).map_err(|e| e.clone())
    );
    assert!(out.is_err(), "must be refused, never a panic");
    // The engine is still usable afterwards.
    assert!(engine.synth("Still alive.", "af_heart", 1.0).is_ok());
}

#[test]
#[ignore]
fn natural_live_empty_like_text_is_refused() {
    let mut engine = load_live();
    assert!(engine.synth("", "af_heart", 1.0).is_err());
    assert!(engine.synth("...", "af_heart", 1.0).is_err());
    assert!(engine.synth("hello", "no_such_voice", 1.0).is_err());
}

fn wait_for(timeout: Duration, mut cond: impl FnMut() -> bool) -> Option<Duration> {
    let t = Instant::now();
    while t.elapsed() < timeout {
        if cond() {
            return Some(t.elapsed());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

/// AUDIBLE. The acceptance harness for design checks A12 (latencies) and C9.
#[test]
#[ignore]
fn natural_live_speaks_through_the_real_stack() {
    let sink = Arc::new(ManagedSink::new(
        Arc::new(CpalStreamFactory::new(None)),
        Duration::from_secs(3),
    ));
    let health = NaturalHealth::new(DEFAULT_AUTO_RETRY_COOLDOWN);
    let mut synth = NaturalSynth::new(
        Box::new(KokoroLoader::new(live_provider(live_dir()))),
        sink.clone(),
        health.clone(),
        NaturalConfig::default(),
    );
    let text = "Hello! This is the natural voice speaking. It should sound a lot more human than the system voice. \
                Here is a second sentence, so we can hear how the chunks join together.";

    // 1. Cold start: first audio from idle (includes model load).
    let t0 = Instant::now();
    synth.speak(text, Some("af_heart"), None, 0.0);
    let first = wait_for(Duration::from_secs(20), || sink.audible()).expect("audio must start");
    println!("COLD first audio after {first:.2?}");
    assert!(first < Duration::from_secs(6), "cold first audio {first:?}");

    // 2. Let it play for 2.5 s, then stop and time the silence.
    std::thread::sleep(Duration::from_millis(2_500));
    let t = Instant::now();
    let outcome = synth.stop_immediate();
    let stop_took = t.elapsed();
    println!("STOP outcome {outcome:?} in {stop_took:.2?}");
    assert!(matches!(
        outcome,
        StopOutcome::Stopped | StopOutcome::Failed
    ));
    assert!(
        stop_took < Duration::from_millis(400),
        "stop took {stop_took:?}"
    );

    // 3. Replacement latency right after a stop (a stale synth may still be running).
    synth.speak(
        "Okay, that was the replacement.",
        Some("af_heart"),
        None,
        0.0,
    );
    let repl =
        wait_for(Duration::from_secs(15), || sink.audible()).expect("replacement must start");
    println!("REPLACEMENT first audio after {repl:.2?} (declared worst case 6.0 s)");
    assert!(
        repl < Duration::from_secs(6),
        "replacement latency {repl:?}"
    );

    // 4. Warm start after full silence.
    assert!(wait_for(Duration::from_secs(20), || !synth.is_speaking()).is_some());
    synth.speak("And a warm start.", Some("bf_emma"), None, 0.0);
    let warm = wait_for(Duration::from_secs(15), || sink.audible()).expect("warm audio");
    println!("WARM first audio after {warm:.2?}");
    assert!(warm < Duration::from_secs(4));
    assert!(wait_for(Duration::from_secs(20), || !synth.is_speaking()).is_some());
    assert!(health.is_ready(), "no failure during the whole run");
    println!("total {:.1?}", t0.elapsed());
}

/// AUDIBLE. The whole wired stack on real hardware, exactly as `lib.rs` builds it: staged model
/// install (READY marker, verified load) -> Kokoro -> NaturalSynth -> SpeakerCore/QueueBackend ->
/// EngineBackend -> TtsManager, then the recording-start interruption.
#[test]
#[ignore]
fn natural_live_manager_end_to_end() {
    use crate::managers::emotion::Emotion;
    use crate::managers::natural_model::*;
    use crate::managers::natural_router::{EngineBackend, RouterChoice, RouterConfig};
    use crate::managers::tts::{NullBackend, TtsManager};
    use crate::managers::tts_queue::{QueueBackend, SpeakerCore};
    use std::sync::Mutex;

    // Stage a real install exactly like a finished download.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("tts_models");
    let install = root.join(KOKORO_DIR_NAME);
    std::fs::create_dir_all(&install).unwrap();
    for file in [KOKORO_MODEL_FILE, KOKORO_VOICES_FILE] {
        std::fs::copy(live_dir().join(file), install.join(file)).unwrap();
    }
    std::fs::write(install.join(READY_MARKER), b"").unwrap();
    let coordinator = Arc::new(ModelCoordinator::new(
        root,
        kokoro_assets(),
        vec![KOKORO_URL_PREFIX.to_string()],
    ));
    assert_eq!(coordinator.state(), ModelState::Ready);
    let tv = Instant::now();
    let _verified = coordinator.load_verified_bytes().expect("verified load");
    println!(
        "STACK verified load (read + SHA-256 of ~120 MB) took {:.2?} [debug build: SHA is slow]",
        tv.elapsed()
    );
    drop(_verified);

    let health = NaturalHealth::new(DEFAULT_AUTO_RETRY_COOLDOWN);
    let c = coordinator.clone();
    let loader = KokoroLoader::new(Box::new(move || {
        c.load_verified_bytes().map_err(|e| e.to_string())
    }));
    let sink = Arc::new(ManagedSink::new(
        Arc::new(CpalStreamFactory::new(None)),
        Duration::from_secs(3),
    ));
    let synth = NaturalSynth::new(
        Box::new(loader),
        sink.clone(),
        health.clone(),
        NaturalConfig::default(),
    );
    let core = Arc::new(Mutex::new(SpeakerCore::new(synth)));
    let natural: Box<dyn crate::managers::tts::SpeechBackend> = Box::new(QueueBackend::new(core));
    let c2 = coordinator.clone();
    // System routes are NullBackend: if the natural route were skipped the speak would fail.
    let router = EngineBackend::new(
        Box::new(NullBackend),
        Box::new(NullBackend),
        Some(natural),
        health.clone(),
        RouterConfig {
            choice: Arc::new(|| RouterChoice {
                natural: true,
                voice: "af_heart".into(),
            }),
            model_ready: Arc::new(move || c2.state() == ModelState::Ready),
        },
    );
    let mgr = TtsManager::new(Box::new(router));

    // 1. A two-sentence burst through the manager's queued path.
    let epoch = mgr.begin_utterance().unwrap();
    let t0 = Instant::now();
    mgr.speak_queued_if_current(
        epoch,
        "Hello, this is the full stack test.",
        None,
        Some(180),
        Emotion::Neutral,
    )
    .expect("natural route must accept the first sentence");
    mgr.speak_queued_if_current(
        epoch,
        "And this second sentence is appended to the same queue.",
        None,
        Some(180),
        Emotion::Neutral,
    )
    .expect("append");
    let first = wait_for(Duration::from_secs(20), || sink.audible()).expect("audio must start");
    println!("STACK first audio after {first:.2?} (cold)");
    // Debug build (unoptimised hashing/inference): loose bound here; the release figure is
    // measured in the final verification.
    assert!(
        first < Duration::from_secs(12),
        "cold first audio {first:?}"
    );
    assert!(
        wait_for(Duration::from_secs(40), || !mgr.is_speaking()).is_some(),
        "burst must finish"
    );
    println!("STACK burst finished after {:.1?}", t0.elapsed());
    assert!(
        t0.elapsed() > Duration::from_secs(4),
        "two sentences take real time to speak"
    );
    assert!(health.is_ready());

    // 2. Recording start interrupts speech immediately.
    let epoch = mgr.begin_utterance().unwrap();
    mgr.speak_queued_if_current(
        epoch,
        "This sentence should be cut off when recording starts, because the user begins to talk.",
        None,
        Some(180),
        Emotion::Neutral,
    )
    .expect("accepted");
    assert!(wait_for(Duration::from_secs(20), || sink.audible()).is_some());
    std::thread::sleep(Duration::from_millis(1_200));
    let t = Instant::now();
    mgr.note_recording_started();
    let stop = t.elapsed();
    println!(
        "STACK recording-start stop took {stop:.2?}; speaking={}",
        mgr.is_speaking()
    );
    assert!(stop < Duration::from_millis(400), "stop took {stop:?}");
    assert!(!mgr.is_speaking());
    assert!(
        mgr.speak_queued_if_current(epoch, "Should be refused.", None, None, Emotion::Neutral)
            .is_err(),
        "speech is blocked while recording"
    );
    std::thread::sleep(Duration::from_millis(400));
    assert!(!sink.audible(), "silence must hold after the stop");
    mgr.note_recording_stopped();
    assert!(health.is_ready());
}
