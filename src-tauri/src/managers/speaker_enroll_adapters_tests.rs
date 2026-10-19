//! Tests for the speaker enrollment adapters. The embedder drives the real `VoiceModelService`
//! with a fake `SessionLoader` (no ONNX model). The recorder needs a microphone, so only its
//! pure helpers are tested here. Every wait is bounded by `EMBED_DEADLINE`.

use super::*;
use crate::managers::ownership::OwnershipGate;
use crate::managers::speaker_enroll::{
    EnrollClock, EnrollConfig, EnrollEmbedder, EnrollError, EnrollRecorder, EnrollmentController,
};
use crate::managers::speaker_id::VoiceprintStore;
use crate::managers::voice_models::{ErrKind, ModelSession, SessionLoader};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::{mpsc, Condvar};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

fn join_or_resume<T>(handle: thread::JoinHandle<T>) -> T {
    match handle.join() {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

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

// ---------------------------------------------------------------------------------------------
// Bounded readiness wait: pure decision logic on a virtual clock
// ---------------------------------------------------------------------------------------------

#[test]
fn readiness_arriving_in_time_is_ready() {
    let now = Cell::new(Duration::ZERO);
    let polls = Cell::new(0_u32);
    let outcome = await_readiness(
        || {
            polls.set(polls.get() + 1);
            (polls.get() >= 4).then_some(true)
        },
        || false,
        || now.get(),
        |d| now.set(now.get() + d),
        READINESS_TIMEOUT,
        READINESS_POLL_SLICE,
    );
    assert_eq!(outcome, ReadinessOutcome::Ready);
    assert!(now.get() < READINESS_TIMEOUT, "ready before the deadline");
}

#[test]
fn readiness_that_never_arrives_times_out_at_the_deadline() {
    let now = Cell::new(Duration::ZERO);
    let outcome = await_readiness(
        || None,
        || false,
        || now.get(),
        |d| now.set(now.get() + d),
        READINESS_TIMEOUT,
        READINESS_POLL_SLICE,
    );
    assert_eq!(outcome, ReadinessOutcome::TimedOut);
    assert_eq!(now.get(), READINESS_TIMEOUT);
}

#[test]
fn abort_while_waiting_is_seen_within_one_slice() {
    let now = Cell::new(Duration::ZERO);
    let outcome = await_readiness(
        || None,
        || now.get() >= Duration::from_millis(50),
        || now.get(),
        |d| now.set(now.get() + d),
        READINESS_TIMEOUT,
        READINESS_POLL_SLICE,
    );
    assert_eq!(outcome, ReadinessOutcome::Aborted);
    assert!(now.get() <= Duration::from_millis(50) + READINESS_POLL_SLICE);
}

#[test]
fn a_failed_readiness_report_is_failed() {
    let now = Cell::new(Duration::ZERO);
    let outcome = await_readiness(
        || Some(false),
        || false,
        || now.get(),
        |d| now.set(now.get() + d),
        READINESS_TIMEOUT,
        READINESS_POLL_SLICE,
    );
    assert_eq!(outcome, ReadinessOutcome::Failed);
}

#[test]
fn readiness_reported_exactly_at_the_deadline_still_counts() {
    let now = Cell::new(Duration::ZERO);
    let outcome = await_readiness(
        || (now.get() >= READINESS_TIMEOUT).then_some(true),
        || false,
        || now.get(),
        |d| now.set(now.get() + d),
        READINESS_TIMEOUT,
        READINESS_POLL_SLICE,
    );
    assert_eq!(
        outcome,
        ReadinessOutcome::Ready,
        "the poll precedes the deadline check"
    );
}

// ---------------------------------------------------------------------------------------------
// Bounded readiness wait: real helper thread (manager stand-in)
// ---------------------------------------------------------------------------------------------

/// Counts live readiness helpers. The count drops when the helper's closure returns, also on
/// unwinding, so a leaked helper stays visible.
struct LiveCount {
    live: Arc<AtomicUsize>,
}

impl LiveCount {
    fn enter(live: &Arc<AtomicUsize>, max: &Arc<AtomicUsize>) -> Self {
        let now = live.fetch_add(1, Ordering::SeqCst) + 1;
        max.fetch_max(now, Ordering::SeqCst);
        LiveCount {
            live: Arc::clone(live),
        }
    }
}

impl Drop for LiveCount {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Polls `live` until it reaches zero or one second passes.
fn wait_until_no_helpers(live: &AtomicUsize) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(1) {
        if live.load(Ordering::SeqCst) == 0 {
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    live.load(Ordering::SeqCst) == 0
}

#[test]
fn ready_recording_starts_without_releasing_anything() {
    let releases = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&releases);
    let result = start_with_readiness(
        || true,
        || false,
        move || {
            r.fetch_add(1, Ordering::SeqCst);
        },
        READINESS_TIMEOUT,
        READINESS_POLL_SLICE,
    );
    assert_eq!(result, Ok(()));
    assert_eq!(releases.load(Ordering::SeqCst), 0);
}

#[test]
fn readiness_that_never_arrives_fails_within_the_bound_and_releases_once() {
    let (tx, rx) = mpsc::channel::<()>();
    let releases = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&releases);
    // The stand-in for the manager: its cancel makes the blocked wait return (false).
    let wait = move || {
        let _ = rx.recv();
        false
    };
    let release = move || {
        r.fetch_add(1, Ordering::SeqCst);
        let _ = tx.send(());
    };
    let started = Instant::now();
    let result = start_with_readiness(
        wait,
        || false,
        release,
        READINESS_TIMEOUT,
        READINESS_POLL_SLICE,
    );
    let elapsed = started.elapsed();
    assert_eq!(result, Err(RecorderBusy));
    assert!(elapsed >= READINESS_TIMEOUT, "waited {elapsed:?}");
    assert!(
        elapsed < READINESS_TIMEOUT + Duration::from_millis(60),
        "must fail within the bound, took {elapsed:?}"
    );
    assert_eq!(
        releases.load(Ordering::SeqCst),
        1,
        "the admission is released exactly once"
    );
}

#[test]
fn abort_while_waiting_fails_within_40_ms_and_releases_once() {
    let (tx, rx) = mpsc::channel::<()>();
    let releases = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&releases);
    let wait = move || {
        let _ = rx.recv();
        false
    };
    let release = move || {
        r.fetch_add(1, Ordering::SeqCst);
        let _ = tx.send(());
    };
    let abort = Arc::new(AtomicBool::new(false));
    let abort_flag = Arc::clone(&abort);
    let set_at = Arc::new(Mutex::new(None::<Instant>));
    let set_at_writer = Arc::clone(&set_at);
    let setter = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        *lock(&set_at_writer) = Some(Instant::now());
        abort_flag.store(true, Ordering::SeqCst);
    });
    let result = start_with_readiness(
        wait,
        move || abort.load(Ordering::SeqCst),
        release,
        READINESS_TIMEOUT,
        READINESS_POLL_SLICE,
    );
    let returned = Instant::now();
    join_or_resume(setter);
    assert_eq!(result, Err(RecorderBusy));
    let raised = lock(&set_at).expect("the abort was raised");
    let latency = returned.duration_since(raised);
    assert!(
        latency < Duration::from_millis(40),
        "abort must be honoured within 40 ms, took {latency:?}"
    );
    assert_eq!(releases.load(Ordering::SeqCst), 1);
}

#[test]
fn readiness_reporting_failure_releases_once_without_waiting() {
    let releases = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&releases);
    let started = Instant::now();
    let result = start_with_readiness(
        || false,
        || false,
        move || {
            r.fetch_add(1, Ordering::SeqCst);
        },
        READINESS_TIMEOUT,
        READINESS_POLL_SLICE,
    );
    assert_eq!(result, Err(RecorderBusy));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "a failed capture is not waited out"
    );
    assert_eq!(releases.load(Ordering::SeqCst), 1);
}

#[test]
fn late_readiness_after_the_timeout_neither_starts_nor_releases_again() {
    let (tx, rx) = mpsc::channel::<()>();
    let releases = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&releases);
    // The helper reports "ready" only after the release has already been issued.
    let wait = move || {
        let _ = rx.recv();
        true
    };
    let release = move || {
        r.fetch_add(1, Ordering::SeqCst);
        let _ = tx.send(());
    };
    let result = start_with_readiness(
        wait,
        || false,
        release,
        Duration::from_millis(100),
        READINESS_POLL_SLICE,
    );
    assert_eq!(result, Err(RecorderBusy), "a timed-out start stays failed");
    thread::sleep(Duration::from_millis(50));
    assert_eq!(
        releases.load(Ordering::SeqCst),
        1,
        "no second release for the late report"
    );
}

#[test]
fn ten_timed_out_attempts_never_leave_more_than_one_helper_alive() {
    let live = Arc::new(AtomicUsize::new(0));
    let max = Arc::new(AtomicUsize::new(0));
    for attempt in 0..10 {
        let (tx, rx) = mpsc::channel::<()>();
        let (l, m) = (Arc::clone(&live), Arc::clone(&max));
        let wait = move || {
            let _count = LiveCount::enter(&l, &m);
            let _ = rx.recv();
            false
        };
        let release = move || {
            let _ = tx.send(());
        };
        let result = start_with_readiness(
            wait,
            || false,
            release,
            Duration::from_millis(100),
            READINESS_POLL_SLICE,
        );
        assert_eq!(result, Err(RecorderBusy), "attempt {attempt}");
        assert!(
            wait_until_no_helpers(&live),
            "attempt {attempt}: the helper must end after the release"
        );
    }
    assert_eq!(
        max.load(Ordering::SeqCst),
        1,
        "at most one helper per attempt"
    );
    assert_eq!(
        live.load(Ordering::SeqCst),
        0,
        "none after the attempts end"
    );
}

// ---------------------------------------------------------------------------------------------
// Controller: cancel during a pending start
// ---------------------------------------------------------------------------------------------

/// A recorder whose start parks until `abort_start` is called, then reports busy.
#[derive(Default)]
struct ParkedRecorder {
    entered: AtomicUsize,
    aborted: AtomicBool,
    stops: AtomicUsize,
}

impl EnrollRecorder for ParkedRecorder {
    fn start(&self) -> Result<(), RecorderBusy> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !self.aborted.load(Ordering::SeqCst) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        Err(RecorderBusy)
    }

    fn stop(&self) -> Vec<f32> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        Vec::new()
    }

    fn abort_start(&self) {
        self.aborted.store(true, Ordering::SeqCst);
    }
}

struct NeverEmbedder;

impl EnrollEmbedder for NeverEmbedder {
    fn embed(&self, _buffer16k: &[f32], _cancel: &AtomicBool) -> Option<Vec<f32>> {
        None
    }
}

#[test]
fn cancel_during_a_pending_start_ends_the_step_as_cancelled_and_frees_the_lease() {
    let dir = TempDir::new().expect("temp dir");
    let gate = OwnershipGate::new();
    let recorder = Arc::new(ParkedRecorder::default());
    let controller = Arc::new(EnrollmentController::new(
        Arc::clone(&gate),
        Arc::clone(&recorder) as Arc<dyn EnrollRecorder>,
        Arc::new(NeverEmbedder) as Arc<dyn EnrollEmbedder>,
        Arc::new(VoiceprintStore::new(dir.path().join("voiceprint"))),
        Arc::new(SystemEnrollClock::new()) as Arc<dyn EnrollClock>,
        EnrollConfig::default(),
    ));
    let id = controller.begin().expect("begin");
    let worker = {
        let controller = Arc::clone(&controller);
        thread::spawn(move || controller.step(id, 0))
    };
    let started = Instant::now();
    while recorder.entered.load(Ordering::SeqCst) == 0 && started.elapsed() < Duration::from_secs(2)
    {
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        recorder.entered.load(Ordering::SeqCst),
        1,
        "the start must be pending"
    );
    assert_eq!(controller.cancel(id), Ok(()));
    let result = join_or_resume(worker);
    assert_eq!(
        result,
        Err(EnrollError::Cancelled),
        "the pending start was aborted"
    );
    assert!(!controller.has_session());
    assert_eq!(
        gate.current(),
        None,
        "the lease is released with the session"
    );
    assert_eq!(
        recorder.stops.load(Ordering::SeqCst),
        0,
        "a start that never succeeded is not stopped"
    );
}
