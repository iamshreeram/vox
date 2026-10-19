//! Authoritative contract tests for the speaker enrollment controller and the Enrollment
//! ownership (design A3, A3', A8', A9, G8c). Written in step T10 BEFORE the implementation: every
//! test that reaches a `todo!()` body fails until the implementation lands.
//!
//! Real `OwnershipGate`, real `VoiceprintStore` (temp dir), fake recorder/embedder and a virtual
//! clock. No real sleeps, no network, no audio device. Nothing here logs embeddings.

#![allow(dead_code)]

use super::*;
use crate::managers::ownership::{Busy, Lease, Owner, OwnershipGate};
use crate::managers::speaker_id::{self, VoiceprintStore, SPEAKER_SPEC};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use tempfile::TempDir;

const DIM: usize = 256;
const WINDOW_MS: u64 = 6000;
/// Samples per second of the 16 kHz buffers the recorder hands over.
const SAMPLES_PER_MS: usize = 16;
/// First-sample code that makes the fake embedder fail (`None`).
const FAIL_CODE: f32 = -1.0;
/// Simulated converse phases. The gate models only the owner, so each phase must be refused the
/// same way: the lease is held across all of them.
const PHASES: [&str; 5] = ["listening", "thinking", "speaking", "acting", "draining"];

// ---------------------------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------------------------

/// Virtual clock: `sleep_ms` advances time instantly and records the sleep.
struct VirtualClock {
    now: Mutex<u64>,
    sleeps: Mutex<Vec<u64>>,
}

impl VirtualClock {
    fn new() -> Arc<Self> {
        Arc::new(VirtualClock {
            now: Mutex::new(0),
            sleeps: Mutex::new(Vec::new()),
        })
    }

    fn advance(&self, ms: u64) {
        *self.now.lock().unwrap() += ms;
    }

    fn sleeps(&self) -> Vec<u64> {
        self.sleeps.lock().unwrap().clone()
    }
}

impl EnrollClock for VirtualClock {
    fn now_ms(&self) -> u64 {
        *self.now.lock().unwrap()
    }

    fn sleep_ms(&self, ms: u64) {
        self.sleeps.lock().unwrap().push(ms);
        *self.now.lock().unwrap() += ms;
    }
}

/// Fake of the atomic admission slot the real recorder uses (`AudioRecordingManager`). Only one
/// recording can run at a time; `foreign_slot` models another owner's recording holding it.
struct FakeRecorder {
    clock: Arc<VirtualClock>,
    /// Samples returned by the next `stop`.
    buffer: Mutex<Vec<f32>>,
    /// Number of upcoming `start` calls that report `RecorderBusy`.
    refusals: Mutex<usize>,
    /// Another owner's recording holds the admission slot.
    foreign_slot: AtomicBool,
    running: AtomicBool,
    starts: AtomicUsize,
    stops: AtomicUsize,
    /// `stop` calls made while no enrollment recording was running (must stay 0).
    stops_while_idle: AtomicUsize,
    /// ("start" | "stop", virtual ms) in call order.
    events: Mutex<Vec<(&'static str, u64)>>,
}

impl FakeRecorder {
    fn new(clock: Arc<VirtualClock>) -> Self {
        FakeRecorder {
            clock,
            buffer: Mutex::new(Vec::new()),
            refusals: Mutex::new(0),
            foreign_slot: AtomicBool::new(false),
            running: AtomicBool::new(false),
            starts: AtomicUsize::new(0),
            stops: AtomicUsize::new(0),
            stops_while_idle: AtomicUsize::new(0),
            events: Mutex::new(Vec::new()),
        }
    }

    fn set_buffer(&self, samples: Vec<f32>) {
        *self.buffer.lock().unwrap() = samples;
    }

    fn refuse_next_starts(&self, count: usize) {
        *self.refusals.lock().unwrap() = count;
    }

    fn set_foreign_recording(&self, held: bool) {
        self.foreign_slot.store(held, Ordering::SeqCst);
    }

    fn foreign_recording_still_running(&self) -> bool {
        self.foreign_slot.load(Ordering::SeqCst)
    }

    fn starts(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
    }

    fn stops(&self) -> usize {
        self.stops.load(Ordering::SeqCst)
    }

    fn stops_while_idle(&self) -> usize {
        self.stops_while_idle.load(Ordering::SeqCst)
    }

    fn events(&self) -> Vec<(&'static str, u64)> {
        self.events.lock().unwrap().clone()
    }
}

impl EnrollRecorder for FakeRecorder {
    fn start(&self) -> Result<(), RecorderBusy> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        {
            let mut refusals = self.refusals.lock().unwrap();
            if *refusals > 0 {
                *refusals -= 1;
                return Err(RecorderBusy);
            }
        }
        if self.foreign_slot.load(Ordering::SeqCst) {
            return Err(RecorderBusy);
        }
        if self.running.swap(true, Ordering::SeqCst) {
            return Err(RecorderBusy);
        }
        self.events
            .lock()
            .unwrap()
            .push(("start", self.clock.now_ms()));
        Ok(())
    }

    fn stop(&self) -> Vec<f32> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        if !self.running.swap(false, Ordering::SeqCst) {
            self.stops_while_idle.fetch_add(1, Ordering::SeqCst);
        }
        self.events
            .lock()
            .unwrap()
            .push(("stop", self.clock.now_ms()));
        std::mem::take(&mut *self.buffer.lock().unwrap())
    }
}

/// Fake embedder. The embedding is chosen by the buffer's first sample (a "code"), so the outcome
/// does not depend on how many times the implementation calls it.
#[derive(Default)]
struct FakeEmbedder {
    calls: AtomicUsize,
    seen_lens: Mutex<Vec<usize>>,
}

impl FakeEmbedder {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn seen_lens(&self) -> Vec<usize> {
        self.seen_lens.lock().unwrap().clone()
    }
}

impl EnrollEmbedder for FakeEmbedder {
    fn embed(&self, buffer16k: &[f32], cancel: &AtomicBool) -> Option<Vec<f32>> {
        let _ = cancel;
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen_lens.lock().unwrap().push(buffer16k.len());
        clip_for(*buffer16k.first()?)
    }
}

/// Consistent clip k (0..3): pairwise cosine 1 / (1 + 0.3^2) ~= 0.917, well above 0.30.
fn consistent_clip(k: usize) -> Vec<f32> {
    let mut v = vec![0.0_f32; DIM];
    v[0] = 1.0;
    v[1 + k] = 0.3;
    v
}

/// Inconsistent set: clip 0 and clip 2 are the same axis, clip 1 has cosine 0.29 with both.
fn inconsistent_clip(k: usize) -> Vec<f32> {
    match k {
        1 => planar(0.29),
        _ => {
            let mut v = vec![0.0_f32; DIM];
            v[0] = 1.0;
            v
        }
    }
}

/// Unit-ish vector in the first two dimensions whose cosine with axis 0 is `c`.
fn planar(c: f32) -> Vec<f32> {
    let mut v = vec![0.0_f32; DIM];
    v[0] = c;
    v[1] = (1.0 - c * c).sqrt();
    v
}

fn consistent_clips() -> Vec<Vec<f32>> {
    (0..3).map(consistent_clip).collect()
}

fn clip_for(code: f32) -> Option<Vec<f32>> {
    if code == FAIL_CODE {
        None
    } else if code >= 10.0 {
        Some(inconsistent_clip((code - 10.0) as usize))
    } else {
        Some(consistent_clip(code as usize))
    }
}

/// `len` samples of VAD-kept audio whose first sample is `code`.
fn samples(code: f32, len: usize) -> Vec<f32> {
    let mut v = vec![0.1_f32; len];
    if let Some(first) = v.first_mut() {
        *first = code;
    }
    v
}

/// A buffer of exactly `ms` milliseconds of speech.
fn speech(code: f32, ms: usize) -> Vec<f32> {
    samples(code, ms * SAMPLES_PER_MS)
}

// ---------------------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------------------

struct Harness {
    gate: Arc<OwnershipGate>,
    clock: Arc<VirtualClock>,
    recorder: Arc<FakeRecorder>,
    embedder: Arc<FakeEmbedder>,
    store: Arc<VoiceprintStore>,
    store_dir: PathBuf,
    controller: Arc<EnrollmentController>,
    _dir: TempDir,
}

fn harness() -> Harness {
    harness_with(EnrollConfig::default())
}

fn harness_with(cfg: EnrollConfig) -> Harness {
    let dir = TempDir::new().expect("temp dir");
    let store_dir = dir.path().join("voiceprint");
    let gate = OwnershipGate::new();
    let clock = VirtualClock::new();
    let recorder = Arc::new(FakeRecorder::new(Arc::clone(&clock)));
    let embedder = Arc::new(FakeEmbedder::default());
    let store = Arc::new(VoiceprintStore::new(store_dir.clone()));
    let controller = Arc::new(EnrollmentController::new(
        Arc::clone(&gate),
        Arc::clone(&recorder) as Arc<dyn EnrollRecorder>,
        Arc::clone(&embedder) as Arc<dyn EnrollEmbedder>,
        Arc::clone(&store),
        Arc::clone(&clock) as Arc<dyn EnrollClock>,
        cfg,
    ));
    Harness {
        gate,
        clock,
        recorder,
        embedder,
        store,
        store_dir,
        controller,
        _dir: dir,
    }
}

/// Runs one successful step whose embedding is `clip_for(code)`.
fn ok_step_with(h: &Harness, id: u64, step: u8, code: f32) -> StepResult {
    h.recorder.set_buffer(speech(code, 2000));
    let result = h
        .controller
        .step(id, step)
        .unwrap_or_else(|e| panic!("step {step} must succeed, got {e:?}"));
    assert!(result.ok, "step {step} must count");
    result
}

fn ok_step(h: &Harness, id: u64, step: u8) -> StepResult {
    ok_step_with(h, id, step, f32::from(step))
}

fn complete_consistent(h: &Harness, id: u64) {
    for step in 0..3 {
        ok_step(h, id, step);
    }
}

/// Asserts that dictation and conversation are both refused by the Enrollment lease.
fn assert_blocked(h: &Harness, when: &str) {
    assert_eq!(
        h.gate.try_acquire(Owner::Dictation).err(),
        Some(Busy(Owner::Enrollment)),
        "dictation must be refused {when}"
    );
    assert_eq!(
        h.gate.try_acquire(Owner::Conversation).err(),
        Some(Busy(Owner::Enrollment)),
        "conversation must be refused {when}"
    );
    assert_eq!(h.gate.current(), Some(Owner::Enrollment), "{when}");
}

fn join_or_resume<T>(handle: thread::JoinHandle<T>) -> T {
    match handle.join() {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

// ---------------------------------------------------------------------------------------------
// Lease: acquisition, refusal and release
// ---------------------------------------------------------------------------------------------

#[test]
fn t10_begin_acquires_the_enrollment_lease() {
    let h = harness();
    let _id = h.controller.begin().expect("begin on a free gate");
    assert_eq!(h.gate.current(), Some(Owner::Enrollment));
    assert!(h.controller.has_session());
}

#[test]
fn t10_lease_blocks_dictation_and_conversation_between_steps() {
    let h = harness();
    let id = h.controller.begin().expect("begin");
    assert_blocked(&h, "right after begin");
    ok_step(&h, id, 0);
    assert_blocked(&h, "between step 0 and step 1");
    ok_step(&h, id, 1);
    assert_blocked(&h, "between step 1 and step 2");
    h.controller.cancel(id).expect("cancel");
    assert!(h.gate.try_acquire(Owner::Dictation).is_ok());
}

#[test]
fn t10_begin_is_busy_during_dictation_and_leaves_no_session() {
    let h = harness();
    let dictation = h.gate.try_acquire(Owner::Dictation).expect("dictation");
    assert_eq!(h.controller.begin(), Err(EnrollError::Busy));
    assert!(!h.controller.has_session());
    assert_eq!(h.gate.current(), Some(Owner::Dictation));
    drop(dictation);
    assert!(
        h.controller.begin().is_ok(),
        "begin works once dictation ends"
    );
}

#[test]
fn t10_begin_is_busy_during_every_conversation_phase_and_leaves_no_session() {
    // The gate models only the owner; every phase must hold the same refusal.
    for phase in PHASES {
        let h = harness();
        let conversation = h
            .gate
            .try_acquire(Owner::Conversation)
            .expect("conversation");
        assert_eq!(
            h.controller.begin(),
            Err(EnrollError::Busy),
            "begin must be refused while conversation is {phase}"
        );
        assert!(!h.controller.has_session(), "phase {phase}");
        assert_eq!(h.gate.current(), Some(Owner::Conversation), "phase {phase}");
        drop(conversation);
    }
}

#[test]
fn t10_second_begin_is_busy_and_the_first_session_survives() {
    let h = harness();
    let id = h.controller.begin().expect("first begin");
    assert_eq!(h.controller.begin(), Err(EnrollError::Busy));
    assert_eq!(h.gate.current(), Some(Owner::Enrollment));
    assert!(ok_step(&h, id, 0).ok, "the first session still works");
}

#[test]
fn t10_unsupported_platform_fails_before_taking_the_lease() {
    // Simulated: `begin_with_platform(false)` stands for a host without owner-private files
    // (VoiceprintStore::save fails closed off Unix). Production begin passes cfg!(unix).
    let h = harness();
    assert_eq!(
        h.controller.begin_with_platform(false),
        Err(EnrollError::UnsupportedPlatform)
    );
    assert_eq!(h.gate.current(), None, "no lease may be taken");
    assert!(!h.controller.has_session());
    assert_eq!(h.recorder.starts(), 0, "no recording may start");
    assert!(
        h.controller.begin_with_platform(true).is_ok(),
        "a supported platform opens normally"
    );
}

#[test]
fn t10_lease_is_released_on_finish() {
    let h = harness();
    let id = h.controller.begin().expect("begin");
    complete_consistent(&h, id);
    h.controller.finish(id).expect("finish");
    assert!(!h.controller.has_session());
    assert_eq!(h.gate.current(), None);
    assert!(h.gate.try_acquire(Owner::Dictation).is_ok());
}

#[test]
fn t10_lease_is_released_on_cancel() {
    let h = harness();
    let id = h.controller.begin().expect("begin");
    ok_step(&h, id, 0);
    h.controller.cancel(id).expect("cancel");
    assert!(!h.controller.has_session());
    assert_eq!(h.gate.current(), None);
    assert!(h.gate.try_acquire(Owner::Conversation).is_ok());
}

#[test]
fn t10_lease_is_released_when_the_controller_is_dropped() {
    let h = harness();
    let gate = Arc::clone(&h.gate);
    let controller = Arc::clone(&h.controller);
    drop(h);
    let _id = controller.begin().expect("begin");
    assert_eq!(gate.current(), Some(Owner::Enrollment));
    drop(controller);
    assert_eq!(
        gate.current(),
        None,
        "dropping the controller releases the lease"
    );
    assert!(gate.try_acquire(Owner::Dictation).is_ok());
}

#[test]
fn t10_panic_while_holding_the_controller_releases_the_lease() {
    let h = harness();
    let gate = Arc::clone(&h.gate);
    let controller = Arc::clone(&h.controller);
    drop(h);
    let _id = controller.begin().expect("begin before the panic");
    assert_eq!(gate.current(), Some(Owner::Enrollment));
    let outcome = catch_unwind(AssertUnwindSafe(move || {
        let _held = controller;
        panic!("simulated enrollment failure");
    }));
    assert!(outcome.is_err(), "the closure must panic");
    assert_eq!(
        gate.current(),
        None,
        "unwinding must release the enrollment lease"
    );
    assert!(gate.try_acquire(Owner::Conversation).is_ok());
}

#[test]
fn t10_idle_expiry_fires_at_exactly_idle_expiry_ms_and_releases_everything() {
    // Boundary as written in the test plan: 119_999 ms idle keeps, 120_000 ms idle expires.
    let h = harness();
    let id = h.controller.begin().expect("begin at t=0");
    h.clock.advance(119_999);
    h.controller.expire_if_idle();
    assert!(
        h.controller.has_session(),
        "119_999 ms idle keeps the session"
    );
    assert_eq!(h.gate.current(), Some(Owner::Enrollment));
    h.clock.advance(1);
    h.controller.expire_if_idle();
    assert!(
        !h.controller.has_session(),
        "120_000 ms idle expires the session"
    );
    assert_eq!(h.gate.current(), None, "expiry releases the lease");
    assert_eq!(h.controller.step(id, 0), Err(EnrollError::StaleSession));
    assert!(h.gate.try_acquire(Owner::Dictation).is_ok());
    assert!(h.store.load().is_none(), "expiry persists nothing");
}

#[test]
fn t10_barrier_race_enrollment_vs_dictation_vs_conversation_has_exactly_one_owner() {
    const ROUNDS: usize = 50;
    enum Outcome {
        Begin(Result<u64, EnrollError>),
        Lease(Result<Lease, Busy>, Owner),
    }
    let racers = [
        Owner::Dictation,
        Owner::Dictation,
        Owner::Dictation,
        Owner::Conversation,
        Owner::Conversation,
        Owner::Conversation,
    ];
    for round in 0..ROUNDS {
        let h = harness();
        let barrier = Arc::new(Barrier::new(3 + racers.len()));
        let mut handles = Vec::new();
        for _ in 0..3 {
            let controller = Arc::clone(&h.controller);
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                Outcome::Begin(controller.begin())
            }));
        }
        for owner in racers {
            let gate = Arc::clone(&h.gate);
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                Outcome::Lease(gate.try_acquire(owner), owner)
            }));
        }
        // Leases and sessions stay alive in `outcomes` until the round is checked.
        let outcomes: Vec<Outcome> = handles.into_iter().map(join_or_resume).collect();

        let winners: Vec<Owner> = outcomes
            .iter()
            .filter_map(|o| match o {
                Outcome::Begin(Ok(_)) => Some(Owner::Enrollment),
                Outcome::Lease(Ok(_), owner) => Some(*owner),
                _ => None,
            })
            .collect();
        assert_eq!(
            winners.len(),
            1,
            "round {round}: exactly one racer may own the gate"
        );
        let winner = winners[0];
        assert_eq!(h.gate.current(), Some(winner), "round {round}");
        for outcome in &outcomes {
            match outcome {
                Outcome::Begin(Ok(_)) | Outcome::Lease(Ok(_), _) => {}
                Outcome::Begin(Err(e)) => {
                    assert_eq!(*e, EnrollError::Busy, "round {round}: losing begin")
                }
                Outcome::Lease(Err(busy), _) => {
                    assert_eq!(
                        busy.0, winner,
                        "round {round}: a loser must name the winner"
                    )
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Session ids and step order
// ---------------------------------------------------------------------------------------------

#[test]
fn t10_stale_and_unknown_session_ids_are_rejected_and_ids_are_not_reused() {
    let h = harness();
    let old = h.controller.begin().expect("first begin");
    h.controller.cancel(old).expect("cancel the first session");
    let new = h.controller.begin().expect("second begin");
    assert_ne!(old, new, "a cancelled session id must not be reused");
    assert_eq!(h.controller.step(old, 0), Err(EnrollError::StaleSession));
    assert_eq!(h.controller.cancel(old), Err(EnrollError::StaleSession));
    assert_eq!(h.controller.finish(old), Err(EnrollError::StaleSession));
    let unknown = new.wrapping_add(1000);
    assert_eq!(
        h.controller.step(unknown, 0),
        Err(EnrollError::StaleSession)
    );
    assert_eq!(
        h.recorder.starts(),
        0,
        "a rejected id must not start the recorder"
    );
    assert!(ok_step(&h, new, 0).ok, "the live session is unaffected");
}

#[test]
fn t10_calls_without_any_session_fail_closed() {
    let h = harness();
    let step = h.controller.step(1, 0).expect_err("no session");
    assert!(
        matches!(
            step,
            EnrollError::StaleSession | EnrollError::NoActiveSession
        ),
        "got {step:?}"
    );
    let cancel = h.controller.cancel(1).expect_err("no session");
    assert!(matches!(
        cancel,
        EnrollError::StaleSession | EnrollError::NoActiveSession
    ));
    let finish = h.controller.finish(1).expect_err("no session");
    assert!(matches!(
        finish,
        EnrollError::StaleSession | EnrollError::NoActiveSession
    ));
    assert_eq!(h.recorder.starts(), 0);
}

#[test]
fn t10_out_of_order_or_repeated_steps_are_bad_step_and_never_record() {
    let h = harness();
    let id = h.controller.begin().expect("begin");
    for bad in [1_u8, 2, 3, 200] {
        assert_eq!(
            h.controller.step(id, bad),
            Err(EnrollError::BadStep),
            "step {bad} before step 0"
        );
    }
    assert_eq!(
        h.recorder.starts(),
        0,
        "a BadStep must not start the recorder"
    );
    ok_step(&h, id, 0);
    assert_eq!(
        h.controller.step(id, 0),
        Err(EnrollError::BadStep),
        "repeat of step 0"
    );
    assert_eq!(
        h.controller.step(id, 2),
        Err(EnrollError::BadStep),
        "skip of step 1"
    );
    assert_eq!(h.recorder.starts(), 1);
    ok_step(&h, id, 1);
}

// ---------------------------------------------------------------------------------------------
// Recording windows
// ---------------------------------------------------------------------------------------------

#[test]
fn t10_each_step_records_exactly_one_window_of_window_ms_on_the_virtual_clock() {
    let h = harness();
    let id = h.controller.begin().expect("begin");
    ok_step(&h, id, 0);
    assert_eq!(h.recorder.starts(), 1, "start exactly once");
    assert_eq!(h.recorder.stops(), 1, "stop exactly once");
    assert_eq!(h.recorder.stops_while_idle(), 0);
    assert_eq!(
        h.clock.sleeps(),
        vec![WINDOW_MS],
        "one fixed sleep of window_ms"
    );
    let events = h.recorder.events();
    assert_eq!(events.len(), 2, "events: {events:?}");
    assert_eq!(events[0].0, "start");
    assert_eq!(events[1].0, "stop");
    assert_eq!(
        events[1].1 - events[0].1,
        WINDOW_MS,
        "the recorder window must be exactly window_ms on the virtual clock"
    );
    assert_eq!(
        h.embedder.seen_lens(),
        vec![2000 * SAMPLES_PER_MS],
        "the embedder gets the window's buffer"
    );
    assert!(ok_step(&h, id, 1).ok);
    assert_eq!(h.recorder.starts(), 2);
    assert_eq!(h.recorder.stops(), 2);
}

#[test]
fn t10_recorder_busy_is_busy_for_that_step_and_keeps_the_session() {
    let h = harness();
    let id = h.controller.begin().expect("begin");
    h.recorder.refuse_next_starts(1);
    h.recorder.set_buffer(speech(0.0, 2000));
    assert_eq!(h.controller.step(id, 0), Err(EnrollError::Busy));
    assert!(
        h.controller.has_session(),
        "a busy recorder keeps the session"
    );
    assert_eq!(h.gate.current(), Some(Owner::Enrollment));
    assert_eq!(
        h.recorder.stops(),
        0,
        "a refused start must not stop anything"
    );
    assert_eq!(
        h.controller.step(id, 1),
        Err(EnrollError::BadStep),
        "the refused step was not counted"
    );
    ok_step(&h, id, 0);
    assert_eq!(h.recorder.starts(), 2, "the retry starts once more");
    assert_eq!(h.recorder.stops(), 1);
}

#[test]
fn t10_enrollment_never_stops_a_foreign_recording() {
    let h = harness();
    let id = h.controller.begin().expect("begin");
    h.recorder.set_foreign_recording(true);
    h.recorder.set_buffer(speech(0.0, 2000));
    assert_eq!(h.controller.step(id, 0), Err(EnrollError::Busy));
    assert_eq!(
        h.recorder.stops(),
        0,
        "enrollment must not stop a recording it did not start"
    );
    assert_eq!(h.controller.cancel(id), Ok(()));
    assert_eq!(
        h.recorder.stops(),
        0,
        "cancel must not stop a foreign recording"
    );
    assert_eq!(h.recorder.stops_while_idle(), 0);
    assert!(h.recorder.foreign_recording_still_running());
}

#[test]
fn t10_too_little_speech_is_not_counted_and_the_retry_succeeds() {
    let h = harness();
    let id = h.controller.begin().expect("begin");
    // 31_999 samples = 1999 ms of VAD-kept speech: one sample short of min_speech_ms.
    h.recorder.set_buffer(samples(0.0, 31_999));
    assert_eq!(h.controller.step(id, 0), Err(EnrollError::TooLittleSpeech));
    assert_eq!(
        h.controller.step(id, 1),
        Err(EnrollError::BadStep),
        "a too-little step is not counted"
    );
    // 32_000 samples = 2000 ms: exactly min_speech_ms passes and reports the speech estimate.
    h.recorder.set_buffer(samples(0.0, 32_000));
    assert_eq!(
        h.controller.step(id, 0),
        Ok(StepResult {
            speech_ms: 2000,
            ok: true
        })
    );
    assert_eq!(h.recorder.stops(), 2, "both attempts ran one window each");
}

#[test]
fn t10_embedder_failure_is_embed_failed_and_not_counted() {
    let h = harness();
    let id = h.controller.begin().expect("begin");
    h.recorder.set_buffer(samples(FAIL_CODE, 32_000));
    assert_eq!(h.controller.step(id, 0), Err(EnrollError::EmbedFailed));
    assert_eq!(
        h.controller.step(id, 1),
        Err(EnrollError::BadStep),
        "an embed failure is not counted"
    );
    ok_step(&h, id, 0);
}

#[test]
fn t10_embeddings_stay_in_memory_until_finish() {
    let h = harness();
    let id = h.controller.begin().expect("begin");
    let generation = h.store.generation();
    complete_consistent(&h, id);
    assert_eq!(h.embedder.calls(), 3, "each step embeds its buffer once");
    assert!(h.store.load().is_none(), "no voiceprint before finish");
    assert_eq!(
        h.store.generation(),
        generation,
        "the store is untouched before finish"
    );
    assert!(
        !h.store_dir.exists()
            || std::fs::read_dir(&h.store_dir)
                .expect("read store dir")
                .next()
                .is_none(),
        "nothing may be written to disk before finish"
    );
    h.controller.finish(id).expect("finish");
    assert!(h.store.load().is_some(), "finish persists the voiceprint");
}

// ---------------------------------------------------------------------------------------------
// Finish, persistence and cancel
// ---------------------------------------------------------------------------------------------

#[test]
fn t10_finish_with_two_ok_steps_is_not_enough_steps_and_the_session_stays_open() {
    let h = harness();
    let id = h.controller.begin().expect("begin");
    ok_step(&h, id, 0);
    ok_step(&h, id, 1);
    assert_eq!(h.controller.finish(id), Err(EnrollError::NotEnoughSteps));
    assert!(h.store.load().is_none(), "nothing is stored with two steps");
    assert!(
        h.controller.has_session(),
        "the session stays open to finish the third step"
    );
    ok_step(&h, id, 2);
    h.controller
        .finish(id)
        .expect("finish after the third step");
}

#[test]
fn t10_finish_stores_a_unit_256_dim_voiceprint_from_three_consistent_steps() {
    let h = harness();
    let id = h.controller.begin().expect("begin");
    complete_consistent(&h, id);
    h.controller.finish(id).expect("finish");
    let vp = h.store.load().expect("stored voiceprint");
    assert_eq!(vp.version, 1);
    assert_eq!(vp.dim, DIM);
    assert_eq!(vp.embedding.len(), DIM);
    assert_eq!(vp.clips, 3);
    assert_eq!(vp.model_sha256, SPEAKER_SPEC.sha256);
    let norm: f64 = vp
        .embedding
        .iter()
        .map(|x| f64::from(*x) * f64::from(*x))
        .sum::<f64>()
        .sqrt();
    assert!(
        (norm - 1.0).abs() < 1e-5,
        "the voiceprint must be unit length, got {norm}"
    );
    let reference = speaker_id::enroll(&consistent_clips(), SPEAKER_SPEC.sha256)
        .expect("reference enrollment of the same clips");
    let max_diff = vp
        .embedding
        .iter()
        .zip(&reference.embedding)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        max_diff < 1e-6,
        "stored embedding must equal speaker_id::enroll, max diff {max_diff}"
    );
    assert!(!h.controller.has_session());
    assert_eq!(h.gate.current(), None, "finish releases the lease");
}

#[test]
fn t10_inconsistent_embeddings_store_nothing_and_keep_the_session_for_a_retry() {
    let h = harness();
    let id = h.controller.begin().expect("begin");
    let generation = h.store.generation();
    // Clip 0 and clip 1 have cosine 0.29 < 0.30 (ENROLL_MIN_PAIR_COSINE).
    for step in 0..3_u8 {
        ok_step_with(&h, id, step, 10.0 + f32::from(step));
    }
    assert_eq!(h.controller.finish(id), Err(EnrollError::Inconsistent));
    assert!(
        h.store.load().is_none(),
        "an inconsistent enrollment stores nothing"
    );
    assert_eq!(h.store.generation(), generation);
    assert!(h.controller.has_session(), "the session stays open");
    assert_eq!(
        h.gate.current(),
        Some(Owner::Enrollment),
        "the lease is kept for the retry"
    );
    // Retry: the steps restart at 0 (assumption documented in NOTES), a consistent set finishes.
    complete_consistent(&h, id);
    h.controller.finish(id).expect("retry finish");
    assert!(h.store.load().is_some());
}

#[test]
fn t10_cancel_mid_way_persists_nothing_and_releases_everything() {
    let h = harness();
    let id = h.controller.begin().expect("begin");
    ok_step(&h, id, 0);
    ok_step(&h, id, 1);
    let generation = h.store.generation();
    let stops_before = h.recorder.stops();
    h.controller.cancel(id).expect("cancel");
    assert!(h.store.load().is_none(), "cancel persists nothing");
    assert_eq!(
        h.store.generation(),
        generation,
        "cancel never touches the store, so the generation is unchanged"
    );
    assert_eq!(
        h.recorder.stops(),
        stops_before,
        "no recording is running between steps"
    );
    assert!(!h.controller.has_session());
    assert_eq!(h.gate.current(), None);
    assert_eq!(h.controller.finish(id), Err(EnrollError::StaleSession));
    assert!(h.gate.try_acquire(Owner::Dictation).is_ok());
}

#[test]
fn t10_a_finished_session_cannot_be_reused() {
    let h = harness();
    let id = h.controller.begin().expect("begin");
    complete_consistent(&h, id);
    h.controller.finish(id).expect("finish");
    assert_eq!(h.controller.step(id, 0), Err(EnrollError::StaleSession));
    assert_eq!(h.controller.cancel(id), Err(EnrollError::StaleSession));
    assert_eq!(h.controller.finish(id), Err(EnrollError::StaleSession));
}

// ---------------------------------------------------------------------------------------------
// Configuration and wire shapes
// ---------------------------------------------------------------------------------------------

#[test]
fn t10_default_config_matches_the_design_constants() {
    assert_eq!(
        EnrollConfig::default(),
        EnrollConfig {
            window_ms: 6000,
            min_speech_ms: 2000,
            idle_expiry_ms: 120_000,
            steps: 3,
        }
    );
}

#[test]
fn t10_wire_shapes_are_bare_names_and_numbers_and_booleans() {
    assert_eq!(
        serde_json::to_string(&EnrollError::TooLittleSpeech).expect("serialize"),
        "\"TooLittleSpeech\""
    );
    let step = serde_json::to_value(StepResult {
        speech_ms: 2000,
        ok: true,
    })
    .expect("serialize");
    assert_eq!(step, serde_json::json!({ "speech_ms": 2000, "ok": true }));
}
