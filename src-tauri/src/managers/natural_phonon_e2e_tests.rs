//! Spec for the macOS Phonon end-to-end path (DESIGN.md Rev 4, P4b + P9). Compiled on macOS
//! tests only. (a) `an_empty_phonon_root_falls_back_to_the_system_voice_exactly_once` runs
//! without a model. (b) `natural_phonon_e2e_live_*` is ignored and needs VOX_PHONON_DIR pointing
//! at a folder with the four pinned files (e.g. /tmp/vox-work/phonon31).
//! Authoritative: do not weaken an assertion; if a test is wrong, report it instead.
//!
//! Stack under test: `build_phonon_stack` (the same parts lib.rs builds for Kokoro, with an
//! injected fake stream factory) behind the real `EngineBackend::with_phonon` router, driven
//! through `TtsManager` exactly as the app drives it.

use crate::managers::emotion::Emotion;
use crate::managers::natural_engine::NaturalHealth;
use crate::managers::natural_model::{ModelState, PHONON_DIR_NAME, READY_MARKER};
use crate::managers::natural_phonon::{build_phonon_stack, PhononStack};
use crate::managers::natural_router::{EngineBackend, RouterChoice, RouterConfig};
use crate::managers::natural_test_support::{FakeDevice, FakeStreamFactory};
use crate::managers::tts::{SpeechBackend, SpeechHandle, TtsManager};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempDir;

const SR: f64 = 24_000.0;
const PINNED_FILES: [&str; 4] = [
    "model.q8.gguf",
    "default-voice.safetensors",
    "tokenizer.json",
    "config.json",
];
const SENTENCE: &str = "Hello from Phonon, this is an end to end test.";
const NEW_SENTENCE: &str = "A brand new sentence after the stop is audible again.";

/// Records every utterance it is asked to speak. Stands in for a SYSTEM route.
struct RecordingSystem {
    queue: bool,
    texts: Mutex<Vec<String>>,
}

impl RecordingSystem {
    fn new(queue: bool) -> Arc<Self> {
        Arc::new(Self {
            queue,
            texts: Mutex::new(Vec::new()),
        })
    }
    fn texts(&self) -> Vec<String> {
        self.texts.lock().unwrap().clone()
    }
}

/// A finished handle: the recorded system route never plays anything.
struct DoneHandle;

impl SpeechHandle for DoneHandle {
    fn kill(&mut self) -> bool {
        true
    }
    fn is_running(&mut self) -> bool {
        false
    }
}

/// Adapter so an `Arc<RecordingSystem>` can be handed to the router as `Box<dyn SpeechBackend>`.
struct SystemRoute(Arc<RecordingSystem>);

impl SpeechBackend for SystemRoute {
    fn spawn(
        &self,
        text: &str,
        _voice: Option<&str>,
        _rate_wpm: Option<u32>,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.0.texts.lock().unwrap().push(text.to_string());
        Ok(Box::new(DoneHandle))
    }
    fn supports_queue(&self) -> bool {
        self.0.queue
    }
    fn enqueue_styled(
        &self,
        text: &str,
        _voice: Option<&str>,
        _rate_wpm: Option<u32>,
        _emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.0.texts.lock().unwrap().push(text.to_string());
        Ok(Box::new(DoneHandle))
    }
}

fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    cond()
}

/// First and last non-zero sample indices (the audible span), if any.
fn voiced_span(pcm: &[f32]) -> Option<(usize, usize)> {
    let first = pcm.iter().position(|x| *x != 0.0)?;
    let last = pcm.iter().rposition(|x| *x != 0.0)?;
    Some((first, last))
}

fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    (sum / samples.len() as f64).sqrt()
}

/// Copies ONLY the four pinned files and writes READY the way the installer does.
fn install_from(source: &Path, root: &Path) {
    let install = root.join(PHONON_DIR_NAME);
    fs::create_dir_all(&install).unwrap();
    for name in PINNED_FILES {
        fs::copy(source.join(name), install.join(name))
            .unwrap_or_else(|e| panic!("copy {name} from VOX_PHONON_DIR: {e}"));
    }
    fs::File::create(install.join(READY_MARKER)).unwrap();
}

/// Router over the stack's Phonon backend, with Phonon selected and the RECORDED system routes
/// as the fallback. `phonon_ready` is the real coordinator state, as lib.rs will wire it.
fn phonon_router(
    stack: PhononStack,
    system: &Arc<RecordingSystem>,
    system_queue: &Arc<RecordingSystem>,
) -> EngineBackend {
    let PhononStack {
        backend,
        health,
        coordinator,
    } = stack;
    EngineBackend::with_phonon(
        Box::new(SystemRoute(system.clone())),
        Box::new(SystemRoute(system_queue.clone())),
        None,
        NaturalHealth::new(Duration::from_secs(300)),
        Some(backend),
        health,
        RouterConfig {
            choice: Arc::new(|| RouterChoice {
                natural: false,
                voice: String::new(),
                phonon: true,
            }),
            model_ready: Arc::new(|| false),
            phonon_ready: Arc::new(move || coordinator.state() == ModelState::Ready),
        },
    )
}

#[test]
fn an_empty_phonon_root_falls_back_to_the_system_voice_exactly_once() {
    let work = TempDir::new().unwrap();
    let root = work.path().join("tts_models");
    let scratch = work.path().join("scratch");
    fs::create_dir_all(&scratch).unwrap();
    let factory = FakeStreamFactory::new(24_000);
    let device = FakeDevice::start(factory.clone(), 240, Duration::from_millis(10));

    let stack = build_phonon_stack(root, scratch, factory.clone());
    assert_eq!(
        stack.coordinator.state(),
        ModelState::NotInstalled,
        "an empty root has no Phonon model"
    );
    let system = RecordingSystem::new(true);
    let system_queue = RecordingSystem::new(true);
    let router = phonon_router(stack, &system, &system_queue);
    let tts = TtsManager::new(Box::new(router));

    tts.speak(SENTENCE, None, None)
        .expect("speech must start on the fallback");

    assert_eq!(
        system.texts(),
        vec![SENTENCE.to_string()],
        "the fallback speaks the sentence exactly once"
    );
    assert!(
        system_queue.texts().is_empty(),
        "the queue fallback must not be used when the system route can queue"
    );
    assert_eq!(device.nonzero(), 0, "no Phonon audio without a model");
}

#[test]
#[ignore = "needs VOX_PHONON_DIR"]
fn natural_phonon_e2e_live_speaks_through_the_router_and_stops() {
    let Some(source) = std::env::var_os("VOX_PHONON_DIR") else {
        println!(
            "SKIPPED: VOX_PHONON_DIR is not set (point it at a folder with the four pinned files)"
        );
        return;
    };
    let source = PathBuf::from(source);
    let work = TempDir::new().unwrap();
    let root = work.path().join("tts_models");
    let scratch = work.path().join("scratch");
    fs::create_dir_all(&scratch).unwrap();
    install_from(&source, &root);

    let factory = FakeStreamFactory::new(24_000);
    let device = FakeDevice::start(factory.clone(), 240, Duration::from_millis(10));
    let stack = build_phonon_stack(root, scratch, factory.clone());
    // Handles kept for the readiness wait below; the stack itself moves into the router.
    let ready_coordinator = stack.coordinator.clone();
    let ready_health = stack.health.clone();
    let system = RecordingSystem::new(true);
    let system_queue = RecordingSystem::new(true);
    let router = phonon_router(stack, &system, &system_queue);
    let tts = TtsManager::new(Box::new(router));

    // Ready = the installed files verify AND the engine's health is Ready (bounded 60 s).
    assert!(
        wait_until(Duration::from_secs(60), || {
            ready_coordinator.state() == ModelState::Ready && ready_health.is_ready()
        }),
        "the Phonon model never became ready"
    );

    // 1) A sentence reaches the device as real, finite, audible PCM and then goes idle.
    let started = Instant::now();
    tts.speak(SENTENCE, None, None)
        .expect("speech must start on Phonon");
    assert!(
        wait_until(Duration::from_secs(60), || device.nonzero() > 0),
        "no PCM reached the fake device"
    );
    println!(
        "first_pcm_ms={:.0}",
        started.elapsed().as_secs_f64() * 1000.0
    );
    assert!(
        wait_until(Duration::from_secs(60), || !tts.is_speaking()),
        "speaking never went idle"
    );
    std::thread::sleep(Duration::from_millis(100));
    let pcm = device.rendered();
    assert!(
        pcm.iter().all(|x| x.is_finite()),
        "every device sample must be finite"
    );
    let (first, last) = voiced_span(&pcm).expect("voiced PCM");
    let span = &pcm[first..=last];
    let span_s = span.len() as f64 / SR;
    let span_rms = rms(span);
    println!("sentence_span_s={span_s:.3} sentence_rms={span_rms:.4}");
    assert!(
        (1.0..=8.0).contains(&span_s),
        "audible span {span_s:.3} s outside 1.0..=8.0 s"
    );
    assert!(span_rms > 0.005, "audible span RMS {span_rms:.5} too quiet");
    assert!(!tts.is_speaking(), "speaking flag must be false at the end");
    assert!(
        system.texts().is_empty() && system_queue.texts().is_empty(),
        "the system voice must never be used while Phonon is ready"
    );

    // 2) A long text, stopped 300 ms after its first audio: silence within the flush budget,
    //    the speaking flag goes false, and no stale PCM arrives afterwards.
    let long = "The quick brown fox jumps over the lazy dog while the engineers test the speech pipeline. ".repeat(6);
    assert!(long.chars().count() >= 500, "long text must be 500+ chars");
    let before_long = device.nonzero();
    tts.speak(&long, None, None)
        .expect("long speech must start on Phonon");
    assert!(
        wait_until(Duration::from_secs(60), || device.nonzero() > before_long),
        "the long text produced no PCM"
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        tts.is_speaking(),
        "the long text must still be speaking at stop"
    );
    let t = Instant::now();
    let stopped = tts.stop();
    let stop_ms = t.elapsed();
    println!(
        "stop_ms={:.1} stopped={stopped}",
        stop_ms.as_secs_f64() * 1000.0
    );
    assert!(stopped, "stop must be confirmed");
    assert!(
        stop_ms < Duration::from_millis(250),
        "stop took {stop_ms:?}, over the flush budget"
    );
    assert!(!tts.is_speaking(), "speaking flag must be false after stop");
    let at_stop = device.nonzero();
    std::thread::sleep(Duration::from_millis(500));
    let after = device.nonzero();
    println!("stale_pcm_samples_after_stop={}", after - at_stop);
    assert_eq!(
        after, at_stop,
        "no stale PCM may arrive for 500 ms after stop"
    );

    // 3) Stop does not poison the engine: a new sentence is audible again.
    let before_new = device.rendered().len();
    tts.speak(NEW_SENTENCE, None, None)
        .expect("speech must start again after stop");
    assert!(
        wait_until(Duration::from_secs(60), || device.nonzero() > after),
        "a sentence after stop must reach the device"
    );
    assert!(
        wait_until(Duration::from_secs(60), || !tts.is_speaking()),
        "the post-stop sentence never went idle"
    );
    std::thread::sleep(Duration::from_millis(100));
    let pcm = device.rendered();
    let fresh = &pcm[before_new..];
    let (first, last) = voiced_span(fresh).expect("voiced PCM after stop");
    let fresh_span = &fresh[first..=last];
    let fresh_s = fresh_span.len() as f64 / SR;
    let fresh_rms = rms(fresh_span);
    println!("post_stop_span_s={fresh_s:.3} post_stop_rms={fresh_rms:.4}");
    assert!(fresh_s >= 0.5, "post-stop audio too short: {fresh_s:.3} s");
    assert!(
        fresh_rms > 0.005,
        "post-stop audio too quiet: {fresh_rms:.5}"
    );
    assert!(
        fresh.iter().all(|x| x.is_finite()),
        "post-stop samples must be finite"
    );
    assert!(!tts.is_speaking());
}
