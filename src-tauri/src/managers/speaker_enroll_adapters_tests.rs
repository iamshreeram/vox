//! Tests for the speaker enrollment adapters. The embedder drives the real `VoiceModelService`
//! with a fake `SessionLoader` (no ONNX model). The recorder needs a microphone, so only its
//! pure helpers are tested here. Every wait is bounded by `EMBED_DEADLINE`.

use super::*;
use crate::managers::voice_models::{ErrKind, ModelSession, SessionLoader};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::Condvar;
use std::time::Duration;

type Script = Arc<dyn Fn(&[f32]) -> Option<Vec<f32>> + Send + Sync>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A park point: the session blocks in `wait` until the test calls `release`.
#[derive(Default)]
struct Park {
    open: Mutex<bool>,
    cv: Condvar,
}

impl Park {
    fn wait(&self) {
        let mut open = lock(&self.open);
        while !*open {
            open = self.cv.wait(open).unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn release(&self) {
        *lock(&self.open) = true;
        self.cv.notify_all();
    }
}

struct FakeLoader {
    script: Script,
    runs: Arc<AtomicUsize>,
}

impl SessionLoader for FakeLoader {
    fn load(&self, _kind: ModelKind, _path: &Path) -> Option<Box<dyn ModelSession>> {
        Some(Box::new(FakeSession {
            script: Arc::clone(&self.script),
            runs: Arc::clone(&self.runs),
        }))
    }
}

struct FakeSession {
    script: Script,
    runs: Arc<AtomicUsize>,
}

impl ModelSession for FakeSession {
    fn run(&mut self, audio16k: &[f32]) -> Option<Vec<f32>> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        (self.script)(audio16k)
    }
}

/// Real service. The speaker model is installed unless `installed` is false.
fn service(script: Script, installed: bool) -> (Arc<VoiceModelService>, Arc<AtomicUsize>) {
    let runs = Arc::new(AtomicUsize::new(0));
    let loader = FakeLoader {
        script,
        runs: Arc::clone(&runs),
    };
    let service = Arc::new(VoiceModelService::new(Arc::new(loader)));
    if installed {
        service.install(ModelKind::Speaker, PathBuf::from("fake-speaker.onnx"));
    }
    (service, runs)
}

fn constant(values: Vec<f32>) -> Script {
    Arc::new(move |_| Some(values.clone()))
}

fn unit_vector() -> Vec<f32> {
    let mut v = vec![0.0_f32; SPEAKER_SPEC.dim];
    v[0] = 1.0;
    v
}

fn speech() -> Vec<f32> {
    vec![0.1_f32; 32_000]
}

// ---------------------------------------------------------------------------------------------
// Clock
// ---------------------------------------------------------------------------------------------

#[test]
fn system_clock_advances_across_a_sleep() {
    let clock = SystemEnrollClock::default();
    let before = clock.now_ms();
    clock.sleep_ms(20);
    let after = clock.now_ms();
    assert!(
        after >= before + 20,
        "sleep must advance the clock: {before} -> {after}"
    );
}

// ---------------------------------------------------------------------------------------------
// Recorder helpers (pure)
// ---------------------------------------------------------------------------------------------

#[test]
fn every_admission_failure_maps_to_recorder_busy() {
    assert_eq!(map_admission_error("Already recording"), RecorderBusy);
    assert_eq!(map_admission_error("Recorder not available"), RecorderBusy);
    assert_eq!(
        map_admission_error(String::from("Failed to open microphone stream: no device")),
        RecorderBusy
    );
}

#[test]
fn readiness_without_samples_is_busy_and_with_samples_is_ok() {
    assert_eq!(first_sample_or_busy(true), Ok(()));
    assert_eq!(first_sample_or_busy(false), Err(RecorderBusy));
}

#[test]
fn captured_generation_is_taken_once() {
    let slot = Mutex::new(Some(7_u64));
    assert_eq!(take_generation(&slot), Some(7));
    assert_eq!(
        take_generation(&slot),
        None,
        "a second stop has nothing to match"
    );
}

#[test]
fn stop_without_a_captured_generation_yields_no_samples() {
    assert!(samples_or_empty(None).is_empty());
    assert_eq!(samples_or_empty(Some(vec![0.5, 0.25])), vec![0.5, 0.25]);
}

// ---------------------------------------------------------------------------------------------
// Embedding validation (pure)
// ---------------------------------------------------------------------------------------------

#[test]
fn accepted_embedding_must_have_the_spec_dimension_and_finite_values() {
    let dim = SPEAKER_SPEC.dim;
    assert_eq!(
        accept_embedding(Some(Reply::Output(unit_vector())), dim),
        Some(unit_vector())
    );
    assert_eq!(
        accept_embedding(Some(Reply::Output(vec![1.0; dim - 1])), dim),
        None,
        "wrong length"
    );
    let mut nan = unit_vector();
    nan[3] = f32::NAN;
    assert_eq!(accept_embedding(Some(Reply::Output(nan)), dim), None);
    let mut inf = unit_vector();
    inf[5] = f32::INFINITY;
    assert_eq!(accept_embedding(Some(Reply::Output(inf)), dim), None);
    assert_eq!(
        accept_embedding(Some(Reply::Failed(ErrKind::NoOutput)), dim),
        None
    );
    assert_eq!(accept_embedding(None, dim), None, "no reply");
}

// ---------------------------------------------------------------------------------------------
// Embedder over the real service
// ---------------------------------------------------------------------------------------------

#[test]
fn embedder_returns_the_model_embedding() {
    let (svc, runs) = service(constant(unit_vector()), true);
    let embedder = ServiceEnrollEmbedder::new(svc);
    let got = embedder.embed(&speech(), &AtomicBool::new(false));
    assert_eq!(got, Some(unit_vector()));
    assert_eq!(runs.load(Ordering::SeqCst), 1, "one inference per buffer");
}

#[test]
fn embedder_rejects_a_wrong_length_embedding() {
    let (svc, _runs) = service(constant(vec![1.0; SPEAKER_SPEC.dim - 1]), true);
    let embedder = ServiceEnrollEmbedder::new(svc);
    assert_eq!(embedder.embed(&speech(), &AtomicBool::new(false)), None);
}

#[test]
fn embedder_rejects_a_non_finite_embedding() {
    let mut bad = unit_vector();
    bad[9] = f32::NAN;
    let (svc, _runs) = service(constant(bad), true);
    let embedder = ServiceEnrollEmbedder::new(svc);
    assert_eq!(embedder.embed(&speech(), &AtomicBool::new(false)), None);
}

#[test]
fn embedder_model_without_output_is_none() {
    let (svc, _runs) = service(Arc::new(|_| None), true);
    let embedder = ServiceEnrollEmbedder::new(svc);
    assert_eq!(embedder.embed(&speech(), &AtomicBool::new(false)), None);
}

#[test]
fn embedder_without_the_speaker_model_is_none_and_submits_nothing() {
    let (svc, runs) = service(constant(unit_vector()), false);
    let embedder = ServiceEnrollEmbedder::new(svc);
    assert_eq!(embedder.embed(&speech(), &AtomicBool::new(false)), None);
    assert_eq!(runs.load(Ordering::SeqCst), 0, "unavailable means no run");
}

#[test]
fn embedder_empty_buffer_is_none_and_submits_nothing() {
    let (svc, runs) = service(constant(unit_vector()), true);
    let embedder = ServiceEnrollEmbedder::new(svc);
    assert_eq!(embedder.embed(&[], &AtomicBool::new(false)), None);
    assert_eq!(runs.load(Ordering::SeqCst), 0);
}

#[test]
fn embedder_already_cancelled_is_none_and_submits_nothing() {
    let (svc, runs) = service(constant(unit_vector()), true);
    let embedder = ServiceEnrollEmbedder::new(svc);
    assert_eq!(embedder.embed(&speech(), &AtomicBool::new(true)), None);
    assert_eq!(runs.load(Ordering::SeqCst), 0);
}

#[test]
fn embedder_gives_up_at_the_deadline_and_abandons_the_request() {
    let park = Arc::new(Park::default());
    let blocker = Arc::clone(&park);
    let script: Script = Arc::new(move |_| {
        blocker.wait();
        Some(unit_vector())
    });
    let (svc, _runs) = service(script, true);
    let embedder = ServiceEnrollEmbedder::new(Arc::clone(&svc));
    let started = Instant::now();
    let got = embedder.embed(&speech(), &AtomicBool::new(false));
    let elapsed = started.elapsed();
    // Release the parked worker before any assertion can unwind, so the service can shut down.
    park.release();
    assert_eq!(got, None, "a timed-out embedding is None");
    assert!(
        elapsed >= EMBED_DEADLINE,
        "must wait the full deadline, waited {elapsed:?}"
    );
    assert!(
        elapsed < EMBED_DEADLINE + Duration::from_secs(1),
        "must not overshoot the deadline by more than a slice, waited {elapsed:?}"
    );
}

#[test]
fn embedder_cancel_stops_waiting_early() {
    let park = Arc::new(Park::default());
    let blocker = Arc::clone(&park);
    let script: Script = Arc::new(move |_| {
        blocker.wait();
        Some(unit_vector())
    });
    let (svc, _runs) = service(script, true);
    let embedder = ServiceEnrollEmbedder::new(Arc::clone(&svc));
    let cancel = Arc::new(AtomicBool::new(false));
    let setter = Arc::clone(&cancel);
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        setter.store(true, Ordering::SeqCst);
    });
    let started = Instant::now();
    let got = embedder.embed(&speech(), &cancel);
    let elapsed = started.elapsed();
    park.release();
    let _ = canceller.join();
    assert_eq!(got, None);
    assert!(
        elapsed < EMBED_DEADLINE,
        "cancel must end the wait well before the deadline, waited {elapsed:?}"
    );
}
