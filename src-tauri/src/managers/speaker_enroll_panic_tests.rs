//! Panic safety of the enrollment step (S13). A panic inside `step` must not leave the session
//! `stepping`: the step guard clears the flag, stops a recording the step started (exactly once),
//! and the session stays cancellable and expirable, so the Enrollment lease is always releasable.
//! Included from speaker_enroll.rs as a child module. Real `OwnershipGate` and temp-dir store;
//! fake recorder/embedder/clock that panic on request. Nothing here logs embeddings.

#![allow(dead_code)]

use super::*;
use crate::managers::ownership::{Owner, OwnershipGate};
use crate::managers::speaker_id::VoiceprintStore;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use tempfile::TempDir;

const DIM: usize = 256;
/// 3 s of 16 kHz mono: enough speech for the default config (`min_speech_ms` = 2000).
const SAMPLES: usize = 16_000 * 3;

/// One-shot faults. Each `swap(false)` makes the fault fire exactly once.
#[derive(Default)]
struct Faults {
    panic_on_start: AtomicBool,
    panic_on_stop: AtomicBool,
    panic_on_sleep: AtomicBool,
    panic_on_embed: AtomicBool,
}

fn fire(flag: &AtomicBool) -> bool {
    flag.swap(false, Ordering::SeqCst)
}

struct PanickyRecorder {
    faults: Arc<Faults>,
    starts: AtomicUsize,
    stops: AtomicUsize,
    running: AtomicBool,
}

impl EnrollRecorder for PanickyRecorder {
    fn start(&self) -> Result<(), RecorderBusy> {
        if fire(&self.faults.panic_on_start) {
            panic!("recorder start exploded");
        }
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.running.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn stop(&self) -> Vec<f32> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        self.running.store(false, Ordering::SeqCst);
        if fire(&self.faults.panic_on_stop) {
            panic!("recorder stop exploded");
        }
        vec![0.1; SAMPLES]
    }
}

struct PanickyClock {
    faults: Arc<Faults>,
    now: AtomicU64,
}

impl PanickyClock {
    fn advance(&self, ms: u64) {
        self.now.fetch_add(ms, Ordering::SeqCst);
    }
}

impl EnrollClock for PanickyClock {
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }

    fn sleep_ms(&self, ms: u64) {
        if fire(&self.faults.panic_on_sleep) {
            panic!("clock sleep exploded");
        }
        self.advance(ms);
    }
}

struct PanickyEmbedder {
    faults: Arc<Faults>,
}

impl EnrollEmbedder for PanickyEmbedder {
    fn embed(&self, _buffer16k: &[f32], _cancel: &AtomicBool) -> Option<Vec<f32>> {
        if fire(&self.faults.panic_on_embed) {
            panic!("embedder exploded");
        }
        Some(vec![1.0; DIM])
    }
}

struct Rig {
    gate: Arc<OwnershipGate>,
    faults: Arc<Faults>,
    recorder: Arc<PanickyRecorder>,
    clock: Arc<PanickyClock>,
    controller: EnrollmentController,
    _dir: TempDir,
}

fn rig() -> Rig {
    let dir = TempDir::new().expect("temp dir");
    let faults = Arc::new(Faults::default());
    let gate = OwnershipGate::new();
    let recorder = Arc::new(PanickyRecorder {
        faults: Arc::clone(&faults),
        starts: AtomicUsize::new(0),
        stops: AtomicUsize::new(0),
        running: AtomicBool::new(false),
    });
    let clock = Arc::new(PanickyClock {
        faults: Arc::clone(&faults),
        now: AtomicU64::new(0),
    });
    let embedder = Arc::new(PanickyEmbedder {
        faults: Arc::clone(&faults),
    });
    let store = Arc::new(VoiceprintStore::new(dir.path().join("voiceprint")));
    let controller = EnrollmentController::new(
        Arc::clone(&gate),
        Arc::clone(&recorder) as Arc<dyn EnrollRecorder>,
        embedder as Arc<dyn EnrollEmbedder>,
        store,
        Arc::clone(&clock) as Arc<dyn EnrollClock>,
        EnrollConfig::default(),
    );
    Rig {
        gate,
        faults,
        recorder,
        clock,
        controller,
        _dir: dir,
    }
}

/// Runs one step that must unwind; the unwind is caught so the test can inspect the controller.
fn step_panics(rig: &Rig, id: u64, step: u8) {
    let outcome = catch_unwind(AssertUnwindSafe(|| rig.controller.step(id, step)));
    assert!(outcome.is_err(), "the step must panic");
}

/// The session can still be cancelled, the lease is free, and a new session can begin.
fn assert_cancellable_and_free(rig: &Rig, id: u64) {
    rig.controller
        .cancel(id)
        .expect("cancel after a panicked step must succeed");
    assert!(!rig.controller.has_session(), "cancel removes the session");
    assert_eq!(rig.gate.current(), None, "cancel releases the lease");
    assert!(
        rig.gate.try_acquire(Owner::Dictation).is_ok(),
        "dictation can take the gate after a panicked step"
    );
    let next = rig.controller.begin().expect("a new begin works");
    rig.controller.cancel(next).expect("cancel the new session");
}

#[test]
fn a_panic_when_the_recorder_starts_leaves_the_session_cancellable() {
    let rig = rig();
    rig.faults.panic_on_start.store(true, Ordering::SeqCst);
    let id = rig.controller.begin().expect("begin");
    step_panics(&rig, id, 0);
    assert_eq!(
        rig.recorder.starts.load(Ordering::SeqCst),
        rig.recorder.stops.load(Ordering::SeqCst),
        "no recording was started, so none may be stopped"
    );
    assert_cancellable_and_free(&rig, id);
}

#[test]
fn a_panic_in_the_embedder_after_recording_stops_the_recording_once() {
    let rig = rig();
    rig.faults.panic_on_embed.store(true, Ordering::SeqCst);
    let id = rig.controller.begin().expect("begin");
    step_panics(&rig, id, 0);
    assert_eq!(rig.recorder.starts.load(Ordering::SeqCst), 1);
    assert_eq!(
        rig.recorder.stops.load(Ordering::SeqCst),
        1,
        "stop count must equal start count"
    );
    assert!(
        !rig.recorder.running.load(Ordering::SeqCst),
        "no recording may be left running"
    );
    assert_cancellable_and_free(&rig, id);
}

#[test]
fn a_panic_in_the_recorder_stop_is_not_followed_by_a_second_stop() {
    let rig = rig();
    rig.faults.panic_on_stop.store(true, Ordering::SeqCst);
    let id = rig.controller.begin().expect("begin");
    step_panics(&rig, id, 0);
    assert_eq!(rig.recorder.starts.load(Ordering::SeqCst), 1);
    assert_eq!(rig.recorder.stops.load(Ordering::SeqCst), 1);
    assert!(!rig.recorder.running.load(Ordering::SeqCst));
    assert_cancellable_and_free(&rig, id);
}

#[test]
fn a_panic_while_the_window_sleeps_stops_the_recording() {
    let rig = rig();
    rig.faults.panic_on_sleep.store(true, Ordering::SeqCst);
    let id = rig.controller.begin().expect("begin");
    step_panics(&rig, id, 0);
    assert_eq!(rig.recorder.starts.load(Ordering::SeqCst), 1);
    assert_eq!(
        rig.recorder.stops.load(Ordering::SeqCst),
        1,
        "the recording the step started must be stopped"
    );
    assert!(!rig.recorder.running.load(Ordering::SeqCst));
    assert_cancellable_and_free(&rig, id);
}

#[test]
fn expire_if_idle_still_expires_the_session_after_a_panicked_step() {
    let rig = rig();
    rig.faults.panic_on_embed.store(true, Ordering::SeqCst);
    let id = rig.controller.begin().expect("begin");
    step_panics(&rig, id, 0);
    rig.clock.advance(EnrollConfig::default().idle_expiry_ms);
    rig.controller.expire_if_idle();
    assert!(
        !rig.controller.has_session(),
        "an idle session must expire, so the step cannot leave it stepping"
    );
    assert_eq!(rig.gate.current(), None);
    assert!(rig.gate.try_acquire(Owner::Dictation).is_ok());
}

#[test]
fn the_same_step_can_be_retried_after_a_panicked_step() {
    let rig = rig();
    rig.faults.panic_on_embed.store(true, Ordering::SeqCst);
    let id = rig.controller.begin().expect("begin");
    step_panics(&rig, id, 0);
    let retried = rig
        .controller
        .step(id, 0)
        .expect("the retry must not be Busy: the panicked step cleared `stepping`");
    assert!(retried.ok, "the retried step counts");
    assert_eq!(rig.recorder.starts.load(Ordering::SeqCst), 2);
    assert_eq!(rig.recorder.stops.load(Ordering::SeqCst), 2);
    rig.controller.cancel(id).expect("cancel");
    assert!(!rig.controller.has_session());
}

#[test]
fn a_normal_step_is_unchanged_by_the_guard() {
    let rig = rig();
    let id = rig.controller.begin().expect("begin");
    let result = rig.controller.step(id, 0).expect("normal step");
    assert!(result.ok);
    assert_eq!(rig.recorder.starts.load(Ordering::SeqCst), 1);
    assert_eq!(rig.recorder.stops.load(Ordering::SeqCst), 1);
    assert!(
        rig.controller.has_session(),
        "a counted step keeps the session"
    );
    rig.controller.cancel(id).expect("cancel");
}
