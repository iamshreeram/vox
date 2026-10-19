//! T9a: AUTHORITATIVE integration contract for Smart Turn and the speaker gate inside
//! the REAL `CaptureWorker`, driven with fakes (recorder port, transcriber, turn detector,
//! speaker gate). Tests marked NEW need the T9b behaviour and are RED until it lands;
//! tests marked BASELINE pin today's behaviour and must pass now.
//!
//! Clock model (DESIGN.md A4/A4'): the test is the audio thread. Speech (VAD-kept)
//! frames are pushed every 50 ms until 850 ms, i.e. a 450 ms hangover after the last
//! acoustic sample at 400 ms, as the real SmoothedVad does. The worker keeps a frame
//! "active" for SPEECH_GAP_MS (150), so last_loud = 1000 and:
//!   * fixed endpoint      = last_loud + silence_ms (1200)            = 2200
//!   * Smart Turn complete = last_loud + min_silence_ms (200) (+ tick) = 1200..=1260
//!   * Smart Turn incomplete = last_loud + max(2500, 1200)            = 3500
//! Feeds are on the virtual clock; the worker's answer arrives in real time, so the
//! driver paces the steps around the decision window.

use super::*;
use crate::managers::converse_latency::EndpointMode;
use crate::managers::smart_turn::RING_SAMPLES;
use crate::managers::speaker_id::{GateResult, Verdict};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ------------------------- fakes -------------------------

type Log = Arc<Mutex<Vec<&'static str>>>;

const LOUD: &[f32] = &[0.0, 0.6, 0.2];
const QUIET: &[f32] = &[0.0, 0.0, 0.0];
/// One 50 ms step of 16 kHz audio fed to the pre-VAD tap.
const RAW_STEP: [f32; 800] = [0.05; 800];

/// Recorder port: logs every call, records tap installs, and feeds the installed tap
/// on demand (the tap runs synchronously on the caller's thread).
struct VPort {
    log: Log,
    active: AtomicBool,
    tap: Mutex<Option<Arc<dyn Fn(&[f32]) + Send + Sync>>>,
    tap_some_calls: AtomicUsize,
}

impl VPort {
    fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }
    fn tap_is_set(&self) -> bool {
        self.tap.lock().unwrap().is_some()
    }
    fn tap_some_calls(&self) -> usize {
        self.tap_some_calls.load(Ordering::SeqCst)
    }
    /// Delivers one raw 16 kHz frame to the installed tap (no-op when none).
    fn feed_raw(&self, frame: &[f32]) {
        let tap = self.tap.lock().unwrap().clone();
        if let Some(tap) = tap {
            tap(frame);
        }
    }
}

impl RecorderPort for VPort {
    fn start(&self) -> Result<(), StartError> {
        self.log.lock().unwrap().push("start");
        self.active.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn stop(&self) -> Option<Vec<f32>> {
        self.log.lock().unwrap().push("stop");
        if self.active.swap(false, Ordering::SeqCst) {
            Some(vec![0.1; 16_000])
        } else {
            None
        }
    }
    fn cancel(&self) {
        self.log.lock().unwrap().push("cancel");
        self.active.store(false, Ordering::SeqCst);
    }
    fn set_raw_tap(&self, tap: Option<Arc<dyn Fn(&[f32]) + Send + Sync>>) {
        if tap.is_some() {
            self.tap_some_calls.fetch_add(1, Ordering::SeqCst);
            self.log.lock().unwrap().push("tap+");
        } else {
            self.log.lock().unwrap().push("tap-");
        }
        *self.tap.lock().unwrap() = tap;
    }
}

struct VTr {
    log: Log,
    calls: AtomicUsize,
}

impl Transcriber for VTr {
    fn transcribe(&self, _samples: Vec<f32>) -> Result<String, TranscribeFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.log.lock().unwrap().push("transcribe");
        Ok("hello world".to_string())
    }
}

#[derive(Clone, Copy)]
enum Behaviour {
    /// Returns this probability at once.
    Prob(f32),
    /// Returns None at once (model unavailable).
    NoAnswer,
    /// Blocks for this long, then returns None (honours cancel only if asked to).
    Block(Duration),
    /// Panics inside the decider.
    Panic,
}

struct FakeDetector {
    behaviour: Behaviour,
    honour_cancel: bool,
    calls: AtomicUsize,
    in_flight: AtomicUsize,
    last_tail: AtomicUsize,
}

/// Decrements the in-flight count on every exit path, including panic unwind.
struct InFlight<'a>(&'a AtomicUsize);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl FakeDetector {
    fn new(behaviour: Behaviour, honour_cancel: bool) -> Self {
        FakeDetector {
            behaviour,
            honour_cancel,
            calls: AtomicUsize::new(0),
            in_flight: AtomicUsize::new(0),
            last_tail: AtomicUsize::new(0),
        }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }
    fn last_tail(&self) -> usize {
        self.last_tail.load(Ordering::SeqCst)
    }
}

impl TurnDetector for FakeDetector {
    fn done_probability(
        &self,
        tail16k: &[f32],
        _deadline: Duration,
        cancel: &AtomicBool,
    ) -> Option<f32> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.last_tail.store(tail16k.len(), Ordering::SeqCst);
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        let _in_flight = InFlight(&self.in_flight);
        match self.behaviour {
            Behaviour::Prob(p) => Some(p),
            Behaviour::NoAnswer => None,
            Behaviour::Block(for_how_long) => {
                let started = Instant::now();
                while started.elapsed() < for_how_long {
                    if self.honour_cancel && cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                None
            }
            Behaviour::Panic => panic!("fake detector panic (test)"),
        }
    }
}

struct FakeGate {
    verdict: Verdict,
    verify_ms: Option<u32>,
    block_for: Duration,
    honour_cancel: bool,
    calls: AtomicUsize,
    last_len: AtomicUsize,
}

impl FakeGate {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    fn last_len(&self) -> usize {
        self.last_len.load(Ordering::SeqCst)
    }
}

impl SpeakerGate for FakeGate {
    fn check(&self, buffer16k: &[f32], cancel: &AtomicBool) -> GateResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.last_len.store(buffer16k.len(), Ordering::SeqCst);
        let started = Instant::now();
        while started.elapsed() < self.block_for {
            if self.honour_cancel && cancel.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        GateResult {
            verdict: self.verdict,
            verify_ms: self.verify_ms,
        }
    }
}

fn detector(behaviour: Behaviour) -> Arc<FakeDetector> {
    Arc::new(FakeDetector::new(behaviour, false))
}

fn gate(verdict: Verdict, verify_ms: Option<u32>) -> Arc<FakeGate> {
    gate_with(verdict, verify_ms, Duration::ZERO, false)
}

fn gate_with(
    verdict: Verdict,
    verify_ms: Option<u32>,
    block_for: Duration,
    honour_cancel: bool,
) -> Arc<FakeGate> {
    Arc::new(FakeGate {
        verdict,
        verify_ms,
        block_for,
        honour_cancel,
        calls: AtomicUsize::new(0),
        last_len: AtomicUsize::new(0),
    })
}

struct Rig {
    log: Log,
    port: Arc<VPort>,
    tr: Arc<VTr>,
}

fn make_rig() -> Rig {
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    Rig {
        port: Arc::new(VPort {
            log: Arc::clone(&log),
            active: AtomicBool::new(false),
            tap: Mutex::new(None),
            tap_some_calls: AtomicUsize::new(0),
        }),
        tr: Arc::new(VTr {
            log: Arc::clone(&log),
            calls: AtomicUsize::new(0),
        }),
        log,
    }
}

impl Rig {
    fn transcribe_calls(&self) -> usize {
        self.tr.calls.load(Ordering::SeqCst)
    }
    fn log_snapshot(&self) -> Vec<&'static str> {
        self.log.lock().unwrap().clone()
    }
}

fn fast() -> CaptureTimings {
    CaptureTimings {
        busy_retry: Duration::from_millis(10),
        busy_deadline: Duration::from_millis(150),
        readiness_deadline: Duration::from_millis(150),
        abs_slack: Duration::from_millis(150),
        tick: Duration::from_millis(5),
    }
}

fn spawn_with(
    rig: &Rig,
    det: Option<Arc<FakeDetector>>,
    g: Option<Arc<FakeGate>>,
) -> CaptureHandle {
    let options = CaptureOptions {
        turn_detector: det.map(|d| d as Arc<dyn TurnDetector>),
        speaker_gate: g.map(|g| g as Arc<dyn SpeakerGate>),
    };
    CaptureWorker::spawn_with_options(rig.port.clone(), rig.tr.clone(), fast(), options)
}

fn cfg() -> EndpointConfig {
    cfg_with_max(5000)
}

fn cfg_with_max(max_utterance_ms: u64) -> EndpointConfig {
    EndpointConfig {
        silence_ms: 1200,
        initial_grace_ms: 600,
        max_utterance_ms,
        min_speech_ms: 100,
        floor: 0.02,
        smart_turn: None,
    }
}

/// What the simulated audio thread delivers on each 50 ms step.
#[derive(Clone, Copy)]
struct Script {
    /// VAD speech frames are pushed for every step up to this time (None = never).
    speech_until: Option<u64>,
    /// Levels are LOUD up to this time, QUIET after (levels do not drive endpointing).
    loud_until: u64,
}

/// Acoustic speech 0..=400, VAD speech frames 0..=850 (450 ms hangover).
const TURN: Script = Script {
    speech_until: Some(850),
    loud_until: 400,
};

fn step(h: &CaptureHandle, port: &VPort, script: Script, t: u64) {
    if let Some(until) = script.speech_until {
        if t <= until {
            h.push_speech(t);
        }
    }
    h.push_levels(t, if t <= script.loud_until { LOUD } else { QUIET });
    port.feed_raw(&RAW_STEP);
}

async fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let started = Instant::now();
    while !cond() {
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// One listen in flight, driven step by step on the virtual clock.
struct Run {
    h: CaptureHandle,
    port: Arc<VPort>,
    task: tokio::task::JoinHandle<ListenOutcome>,
    id: u64,
    script: Script,
    /// Pace the steps inside the decision window (needed when a detector answers).
    paced: bool,
    step_sleep: Duration,
}

impl Run {
    async fn start(h: &CaptureHandle, rig: &Rig, cfg: EndpointConfig, script: Script) -> Run {
        let h2 = h.clone();
        let task = tokio::spawn(async move { h2.listen(cfg).await });
        let port = Arc::clone(&rig.port);
        wait_until("recording start", || port.is_active() || task.is_finished()).await;
        assert!(port.is_active(), "listen ended before the recorder started");
        let id = h.inner.next_id.load(Ordering::SeqCst);
        Run {
            h: h.clone(),
            port,
            task,
            id,
            script,
            paced: false,
            step_sleep: Duration::from_millis(3),
        }
    }

    /// Feeds steps from `from` to `to` (inclusive, 50 ms apart). Returns the step at
    /// which the listen was seen to end, if it ended.
    async fn feed(&self, from: u64, to: u64) -> Option<u64> {
        for t in (from..=to).step_by(50) {
            step(&self.h, &self.port, self.script, t);
            tokio::time::sleep(self.step_sleep).await;
            if self.paced && (1150..=1400).contains(&t) {
                // Give the decider and the 5 ms worker tick time to answer in real time.
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
            if self.task.is_finished() {
                return Some(t);
            }
        }
        None
    }

    async fn settle(self) -> ListenOutcome {
        tokio::time::timeout(Duration::from_secs(3), self.task)
            .await
            .expect("listen hung")
            .expect("listen task panicked")
    }
}

struct Turn {
    outcome: ListenOutcome,
    end_ms: u64,
    id: u64,
}

async fn run_turn_with(h: &CaptureHandle, rig: &Rig, paced: bool, step_sleep: Duration) -> Turn {
    let mut run = Run::start(h, rig, cfg(), TURN).await;
    run.paced = paced;
    run.step_sleep = step_sleep;
    let end_ms = run
        .feed(0, 4000)
        .await
        .expect("listen did not end by 4000 ms of virtual time");
    let id = run.id;
    let outcome = run.settle().await;
    Turn {
        outcome,
        end_ms,
        id,
    }
}

async fn run_turn(h: &CaptureHandle, rig: &Rig, paced: bool) -> Turn {
    run_turn_with(h, rig, paced, Duration::from_millis(3)).await
}

fn utterance() -> ListenOutcome {
    ListenOutcome::Utterance("hello world".into())
}

// ------------------------- Smart Turn endpoint (NEW) -------------------------

/// NEW (T9b): complete answer (p=0.9) ends min_silence after the last active frame,
/// and at least 700 ms before the fixed path on the same input.
#[tokio::test]
async fn complete_answer_ends_min_silence_after_last_active_and_700ms_before_fixed() {
    let fixed_rig = make_rig();
    let fixed_h = spawn_with(&fixed_rig, None, None);
    let fixed = run_turn(&fixed_h, &fixed_rig, false).await;
    fixed_h.shutdown();
    assert_eq!(fixed.outcome, utterance());
    assert!(
        (2150..=2250).contains(&fixed.end_ms),
        "fixed endpoint at {} ms, expected 2200 (last_loud 1000 + silence 1200)",
        fixed.end_ms
    );

    let rig = make_rig();
    let det = detector(Behaviour::Prob(0.9));
    let h = spawn_with(&rig, Some(Arc::clone(&det)), None);
    let smart = run_turn(&h, &rig, true).await;
    h.shutdown();
    assert_eq!(smart.outcome, utterance());
    assert!(
        (1200..=1260).contains(&smart.end_ms),
        "complete answer ended at {} ms, expected within 1000 + 200 + 60",
        smart.end_ms
    );
    assert!(
        fixed.end_ms >= smart.end_ms + 700,
        "smart {} must be >= 700 ms before fixed {}",
        smart.end_ms,
        fixed.end_ms
    );
    assert_eq!(det.calls(), 1, "one decision per pause");
    assert!(
        det.last_tail() > 0 && det.last_tail() <= RING_SAMPLES,
        "the decider must get the pre-VAD tail (got {} samples)",
        det.last_tail()
    );
}

/// NEW (T9b): incomplete answer (p=0.1) is not earlier than the fixed path and ends at
/// last_loud + max(incomplete_wait 2500, silence 1200) = 3500 (+ one tick).
#[tokio::test]
async fn incomplete_answer_waits_the_incomplete_wait_and_is_never_earlier_than_fixed() {
    let fixed_rig = make_rig();
    let fixed_h = spawn_with(&fixed_rig, None, None);
    let fixed = run_turn(&fixed_h, &fixed_rig, false).await;
    fixed_h.shutdown();

    let rig = make_rig();
    let det = detector(Behaviour::Prob(0.1));
    let h = spawn_with(&rig, Some(Arc::clone(&det)), None);
    let smart = run_turn(&h, &rig, true).await;
    h.shutdown();
    assert_eq!(smart.outcome, utterance());
    assert!(
        (3500..=3560).contains(&smart.end_ms),
        "incomplete answer ended at {} ms, expected 3500..=3560",
        smart.end_ms
    );
    assert!(
        smart.end_ms >= fixed.end_ms,
        "incomplete answer must never end before the fixed path"
    );
    assert_eq!(det.calls(), 1);
}

/// NEW (T9b): detector returns None => the endpoint is the fixed moment, within one tick.
#[tokio::test]
async fn detector_returning_none_keeps_the_fixed_endpoint_and_is_a_fallback() {
    let rig = make_rig();
    let det = detector(Behaviour::NoAnswer);
    let h = spawn_with(&rig, Some(Arc::clone(&det)), None);
    let turn = run_turn(&h, &rig, true).await;
    assert_eq!(turn.outcome, utterance());
    assert!(
        (2150..=2250).contains(&turn.end_ms),
        "endpoint at {} ms, expected the fixed 2200 within one tick",
        turn.end_ms
    );
    assert_eq!(
        h.take_timing(turn.id).map(|t| t.mode),
        Some(EndpointMode::SmartTurnFallback)
    );
    h.shutdown();
}

/// NEW (T9b): a decider blocking 400 ms (longer than the 250 ms decision deadline) must
/// NOT delay the fixed endpoint; the listen completes at the fixed moment.
#[tokio::test]
async fn blocking_detector_never_delays_the_fixed_endpoint() {
    let rig = make_rig();
    let det = detector(Behaviour::Block(Duration::from_millis(400)));
    let h = spawn_with(&rig, Some(Arc::clone(&det)), None);
    let turn = run_turn(&h, &rig, true).await;
    assert_eq!(turn.outcome, utterance());
    assert!(
        (2150..=2250).contains(&turn.end_ms),
        "a blocked decider delayed the endpoint to {} ms (fixed is 2200)",
        turn.end_ms
    );
    assert_eq!(
        h.take_timing(turn.id).map(|t| t.mode),
        Some(EndpointMode::SmartTurnFallback)
    );
    h.shutdown();
}

/// NEW (T9b): a panicking decider must not break the listen; it falls back to the fixed rule.
#[tokio::test]
async fn panicking_detector_keeps_the_fixed_endpoint_and_the_listen_completes() {
    let rig = make_rig();
    let det = detector(Behaviour::Panic);
    let h = spawn_with(&rig, Some(Arc::clone(&det)), None);
    let turn = run_turn(&h, &rig, true).await;
    assert_eq!(turn.outcome, utterance());
    assert!(
        (2150..=2250).contains(&turn.end_ms),
        "endpoint at {} ms, expected the fixed 2200 within one tick",
        turn.end_ms
    );
    assert_eq!(det.calls(), 1);
    assert_eq!(
        h.take_timing(turn.id).map(|t| t.mode),
        Some(EndpointMode::SmartTurnFallback)
    );
    h.shutdown();
}

// ------------------------- pre-VAD tap and ring (NEW, except the baseline) -------------------------

/// NEW (T9b): 20 s of raw audio leaves exactly the 8 s window; the ring is allocated only
/// while a detector is configured, the tap is installed once and cleared at listen end.
#[tokio::test]
async fn pre_vad_ring_is_bounded_to_the_model_window_and_cleared_at_listen_end() {
    let rig = make_rig();
    let det = detector(Behaviour::Prob(0.9));
    let h = spawn_with(&rig, Some(det), None);
    let mut run = Run::start(
        &h,
        &rig,
        cfg_with_max(30_000),
        Script {
            speech_until: Some(20_000),
            loud_until: 20_000,
        },
    )
    .await;
    run.step_sleep = Duration::from_millis(1);
    assert!(
        run.feed(0, 20_000).await.is_none(),
        "continuous speech must not end the listen"
    );
    assert_eq!(h.tap_installed_count(), 1, "tap installed once per listen");
    assert!(
        h.ring_allocated(),
        "ring allocated while a detector is configured"
    );
    assert_eq!(
        h.ring_len(),
        RING_SAMPLES,
        "20 s of audio must leave exactly the 8 s window"
    );
    assert!(rig.port.tap_is_set(), "tap is installed during the listen");
    h.cancel_all();
    assert_eq!(run.settle().await, ListenOutcome::Aborted);
    wait_until("ring cleared at listen end", || h.ring_len() == 0).await;
    assert!(!rig.port.tap_is_set(), "tap cleared at listen end");
    h.shutdown();
}

/// BASELINE: without a detector, no tap is ever installed and no ring is allocated.
#[tokio::test]
async fn without_a_detector_no_tap_is_installed_and_no_ring_is_allocated() {
    let rig = make_rig();
    let h = spawn_with(&rig, None, None);
    let turn = run_turn(&h, &rig, false).await;
    assert_eq!(turn.outcome, utterance());
    assert_eq!(
        rig.port.tap_some_calls(),
        0,
        "set_raw_tap must never be called with Some when no detector is configured"
    );
    assert_eq!(h.tap_installed_count(), 0);
    assert!(!h.ring_allocated());
    assert_eq!(h.ring_len(), 0);
    h.shutdown();
}

// ------------------------- decider threads and cancel (NEW) -------------------------

/// NEW (T9b): cancel while a decision is pending aborts within 100 ms, releases the
/// recorder, and the decider has exited (no decider thread left running).
#[tokio::test]
async fn cancel_while_a_decision_is_pending_aborts_within_100ms_and_the_decider_exits() {
    let rig = make_rig();
    let det = Arc::new(FakeDetector::new(
        Behaviour::Block(Duration::from_secs(2)),
        true,
    ));
    let h = spawn_with(&rig, Some(Arc::clone(&det)), None);
    let run = Run::start(&h, &rig, cfg(), TURN).await;
    assert!(
        run.feed(0, 1200).await.is_none(),
        "the pause must still be open at 1200 ms"
    );
    wait_until("decider called", || det.calls() == 1).await;
    let started = Instant::now();
    h.cancel_all();
    let outcome = tokio::time::timeout(Duration::from_millis(100), run.task)
        .await
        .expect("cancel did not abort the listen within 100 ms")
        .expect("listen task panicked");
    assert!(started.elapsed() < Duration::from_millis(100));
    assert_eq!(outcome, ListenOutcome::Aborted);
    assert!(!rig.port.is_active(), "the recorder must be released");
    wait_until("decider thread exited", || {
        h.decider_threads_live() == 0 && det.in_flight() == 0
    })
    .await;
    h.shutdown();
}

/// NEW (T9b): 20 consecutive listens with a blocking detector create at most 20 decider
/// threads (one per listen, at most); with no detector there are none.
#[tokio::test]
async fn twenty_listens_create_at_most_twenty_deciders_and_none_without_a_detector() {
    let rig = make_rig();
    let det = Arc::new(FakeDetector::new(
        Behaviour::Block(Duration::from_millis(400)),
        true,
    ));
    let h = spawn_with(&rig, Some(Arc::clone(&det)), None);
    for _ in 0..20 {
        let turn = run_turn_with(&h, &rig, false, Duration::from_millis(1)).await;
        assert_eq!(turn.outcome, utterance());
    }
    assert_eq!(det.calls(), 20, "one decision per listen");
    let spawned = h.decider_threads_spawned();
    assert!(
        (1..=20).contains(&spawned),
        "decider threads spawned: {spawned}, expected 1..=20"
    );
    h.shutdown();

    // BASELINE half: no detector, no decider thread ever.
    let rig2 = make_rig();
    let h2 = spawn_with(&rig2, None, None);
    for _ in 0..20 {
        let turn = run_turn_with(&h2, &rig2, false, Duration::from_millis(1)).await;
        assert_eq!(turn.outcome, utterance());
    }
    assert_eq!(h2.decider_threads_spawned(), 0);
    h2.shutdown();
}

// ------------------------- timing slot (A7, NEW) -------------------------

/// NEW (T9b): the timing slot is cleared at the start of every listen: a stale timing
/// from listen 1 is not visible while listen 2 runs; listen 2's timing comes out once.
#[tokio::test]
async fn timing_slot_is_cleared_at_listen_start() {
    let rig = make_rig();
    let h = spawn_with(&rig, None, None);
    let first = run_turn(&h, &rig, false).await;
    assert_eq!(first.outcome, utterance());

    let mut second = Run::start(&h, &rig, cfg(), TURN).await;
    second.step_sleep = Duration::from_millis(1);
    assert_eq!(
        h.take_timing(first.id),
        None,
        "stale timing from listen 1 must not be visible during listen 2"
    );
    assert!(second.feed(0, 4000).await.is_some());
    let second_id = second.id;
    assert_eq!(second.settle().await, utterance());
    assert!(h.take_timing(second_id).is_some());
    assert_eq!(h.take_timing(second_id), None, "a second take is None");
    h.shutdown();
}

/// NEW (T9b): take_timing returns only for the matching listen id; a second take is None.
#[tokio::test]
async fn take_timing_is_only_for_the_matching_listen_and_is_taken_once() {
    let rig = make_rig();
    let h = spawn_with(&rig, None, None);
    let turn = run_turn(&h, &rig, false).await;
    assert_eq!(h.take_timing(turn.id + 1000), None, "wrong listen id");
    assert!(h.take_timing(turn.id).is_some(), "matching id is returned");
    assert_eq!(h.take_timing(turn.id), None, "second take is None");
    h.shutdown();
}

/// NEW (T9b): mode per case: fixed without a detector; SmartTurn when an answer ended the
/// turn (complete or incomplete); SmartTurnFallback when the end came from the fixed rule.
#[tokio::test]
async fn mode_is_fixed_without_a_detector_and_smart_turn_only_when_an_answer_ended_it() {
    async fn mode_for(behaviour: Option<Behaviour>) -> EndpointMode {
        let rig = make_rig();
        let det = behaviour.map(detector);
        let h = spawn_with(&rig, det, None);
        let turn = run_turn(&h, &rig, behaviour.is_some()).await;
        let timing = h
            .take_timing(turn.id)
            .expect("a finished listen has timing");
        h.shutdown();
        timing.mode
    }
    assert_eq!(mode_for(None).await, EndpointMode::Fixed);
    assert_eq!(
        mode_for(Some(Behaviour::Prob(0.9))).await,
        EndpointMode::SmartTurn
    );
    assert_eq!(
        mode_for(Some(Behaviour::Prob(0.1))).await,
        EndpointMode::SmartTurn
    );
    assert_eq!(
        mode_for(Some(Behaviour::NoAnswer)).await,
        EndpointMode::SmartTurnFallback
    );
    assert_eq!(
        mode_for(Some(Behaviour::Block(Duration::from_millis(400)))).await,
        EndpointMode::SmartTurnFallback
    );
    assert_eq!(
        mode_for(Some(Behaviour::Panic)).await,
        EndpointMode::SmartTurnFallback
    );
}

/// NEW (T9b): endpoint_wait is measured from the last speech frame (850) to the
/// decision: fixed 2200 - 850 = 1350, complete Smart Turn 1250 - 850 = 400 (+-60 ms).
/// transcribe_ms is present for a transcribed utterance.
#[tokio::test]
async fn endpoint_wait_and_transcribe_numbers_come_from_the_worker_clock() {
    let rig = make_rig();
    let h = spawn_with(&rig, None, None);
    let fixed = run_turn(&h, &rig, false).await;
    let t = h.take_timing(fixed.id).expect("timing");
    h.shutdown();
    let wait = t.endpoint_wait_ms.expect("endpoint_wait_ms") as i64;
    assert!(
        (wait - 1350).abs() <= 60,
        "fixed endpoint_wait_ms {wait}, expected ~1350"
    );
    assert!(t.transcribe_ms.is_some());
    assert_eq!(t.verify_ms, None, "no gate: no verify duration");

    let rig = make_rig();
    let h = spawn_with(&rig, Some(detector(Behaviour::Prob(0.9))), None);
    let smart = run_turn(&h, &rig, true).await;
    let t = h.take_timing(smart.id).expect("timing");
    h.shutdown();
    let wait = t.endpoint_wait_ms.expect("endpoint_wait_ms") as i64;
    assert!(
        (wait - 400).abs() <= 60,
        "smart endpoint_wait_ms {wait}, expected ~400"
    );
    assert_eq!(t.mode, EndpointMode::SmartTurn);
}

/// NEW (T9b): verify_ms is Some only when the gate ran and produced a duration.
#[tokio::test]
async fn verify_ms_is_present_only_when_the_gate_ran_with_a_duration() {
    async fn verify_ms_with(arg: Option<(Verdict, Option<u32>)>) -> Option<u32> {
        let rig = make_rig();
        let g = arg.map(|(v, ms)| gate(v, ms));
        let h = spawn_with(&rig, None, g);
        let turn = run_turn(&h, &rig, false).await;
        let t = h.take_timing(turn.id).expect("timing");
        h.shutdown();
        t.verify_ms
    }
    assert_eq!(
        verify_ms_with(Some((Verdict::Accept, Some(120)))).await,
        Some(120)
    );
    assert_eq!(
        verify_ms_with(Some((Verdict::SkippedTooShort, None))).await,
        None
    );
    assert_eq!(verify_ms_with(None).await, None);
}

// ------------------------- speaker gate (NEW, except the baseline) -------------------------

/// NEW (T9b): Reject => Ignored, the transcriber is NEVER called, transcribe_ms is None,
/// and the gate's own duration is reported.
#[tokio::test]
async fn gate_reject_is_ignored_and_the_transcriber_is_never_called() {
    let rig = make_rig();
    let g = gate(Verdict::Reject, Some(900));
    let h = spawn_with(&rig, None, Some(Arc::clone(&g)));
    let turn = run_turn(&h, &rig, false).await;
    assert_eq!(turn.outcome, ListenOutcome::Ignored);
    assert_eq!(g.calls(), 1);
    assert_eq!(
        rig.transcribe_calls(),
        0,
        "a rejected utterance is never transcribed"
    );
    assert!(!rig.log_snapshot().contains(&"transcribe"));
    let t = h.take_timing(turn.id).expect("timing");
    assert_eq!(t.transcribe_ms, None);
    assert_eq!(t.verify_ms, Some(900));
    assert!(!rig.port.is_active());
    h.shutdown();
}

/// NEW (T9b): Accept and SkippedTooShort both reach the transcriber; the gate receives
/// the VAD-kept buffer (non-empty) after stop().
#[tokio::test]
async fn gate_accept_and_skipped_too_short_are_transcribed() {
    for verdict in [Verdict::Accept, Verdict::SkippedTooShort] {
        let rig = make_rig();
        let g = gate(verdict, None);
        let h = spawn_with(&rig, None, Some(Arc::clone(&g)));
        let turn = run_turn(&h, &rig, false).await;
        assert_eq!(turn.outcome, utterance(), "{verdict:?} must be transcribed");
        assert_eq!(rig.transcribe_calls(), 1, "{verdict:?}");
        assert_eq!(g.calls(), 1, "{verdict:?}");
        assert!(
            g.last_len() > 0,
            "{verdict:?}: the gate must get the buffer"
        );
        h.shutdown();
    }
}

/// NEW (T9b): a gate that blocks past its own deadline is the gate's job: the fake
/// returns Accept after 600 ms and the listen completes normally (not Aborted).
#[tokio::test]
async fn gate_blocking_past_its_deadline_still_completes_with_its_verdict() {
    let rig = make_rig();
    let g = gate_with(Verdict::Accept, None, Duration::from_millis(600), false);
    let h = spawn_with(&rig, None, Some(Arc::clone(&g)));
    // The gate blocks 600 ms of REAL time while the 4000 ms of virtual audio is fed in about
    // 0.3 s, so the listen legitimately outlives the feed window. Feed all the audio, then wait
    // for the listen (settle() still fails the test on a hang after 3 s).
    let run = Run::start(&h, &rig, cfg(), TURN).await;
    let _ = run.feed(0, 4000).await;
    let outcome = run.settle().await;
    assert_eq!(outcome, utterance());
    assert_eq!(rig.transcribe_calls(), 1);
    h.shutdown();
}

/// NEW (T9b): cancel during the gate => Aborted promptly and nothing is transcribed.
#[tokio::test]
async fn cancel_during_the_gate_aborts_promptly_without_transcription() {
    let rig = make_rig();
    let g = gate_with(Verdict::Accept, None, Duration::from_secs(4), true);
    let h = spawn_with(&rig, None, Some(Arc::clone(&g)));
    let run = Run::start(&h, &rig, cfg(), TURN).await;
    assert!(
        run.feed(0, 2200).await.is_none(),
        "the gate holds the listen open"
    );
    wait_until("gate entered", || g.calls() == 1).await;
    let started = Instant::now();
    h.cancel_all();
    let outcome = tokio::time::timeout(Duration::from_millis(100), run.task)
        .await
        .expect("cancel during the gate did not abort within 100 ms")
        .expect("listen task panicked");
    assert!(started.elapsed() < Duration::from_millis(100));
    assert_eq!(outcome, ListenOutcome::Aborted);
    assert_eq!(rig.transcribe_calls(), 0);
    h.shutdown();
}

/// BASELINE: with no gate, a speech turn is transcribed exactly as today (no gate calls
/// exist to count; the transcriber runs once).
#[tokio::test]
async fn gate_none_transcribes_exactly_as_today() {
    let rig = make_rig();
    let h = spawn_with(&rig, None, None);
    let turn = run_turn(&h, &rig, false).await;
    assert_eq!(turn.outcome, utterance());
    assert_eq!(rig.transcribe_calls(), 1);
    h.shutdown();
}

/// NEW (T9b): complete answer + gate Reject => Ignored, exactly one decision, zero
/// transcription.
#[tokio::test]
async fn complete_answer_then_gate_reject_is_ignored_with_one_decision_and_no_transcription() {
    let rig = make_rig();
    let det = detector(Behaviour::Prob(0.9));
    let g = gate(Verdict::Reject, Some(700));
    let h = spawn_with(&rig, Some(Arc::clone(&det)), Some(Arc::clone(&g)));
    let turn = run_turn(&h, &rig, true).await;
    assert_eq!(turn.outcome, ListenOutcome::Ignored);
    assert_eq!(det.calls(), 1, "exactly one decision");
    assert_eq!(g.calls(), 1);
    assert_eq!(rig.transcribe_calls(), 0);
    h.shutdown();
}

// ------------------------- off-switch identity (BASELINE) -------------------------

/// BASELINE: with both None the recorder and transcriber see exactly today's sequence
/// for a speech turn: start, stop, transcribe. No tap calls at all.
#[tokio::test]
async fn off_switch_call_sequence_for_a_speech_turn_is_exactly_today_s() {
    let rig = make_rig();
    let h = CaptureWorker::spawn(rig.port.clone(), rig.tr.clone(), fast());
    let turn = run_turn(&h, &rig, false).await;
    assert_eq!(turn.outcome, utterance());
    assert_eq!(rig.log_snapshot(), vec!["start", "stop", "transcribe"]);
    h.shutdown();
}

/// BASELINE: with both None a silent listen sees exactly start then cancel (NoSpeech).
#[tokio::test]
async fn off_switch_call_sequence_for_a_silent_listen_is_exactly_today_s() {
    let rig = make_rig();
    let h = CaptureWorker::spawn(rig.port.clone(), rig.tr.clone(), fast());
    let run = Run::start(
        &h,
        &rig,
        cfg(),
        Script {
            speech_until: None,
            loud_until: 0,
        },
    )
    .await;
    assert!(
        run.feed(0, 4000).await.is_some(),
        "silence must end the listen"
    );
    assert_eq!(run.settle().await, ListenOutcome::NoSpeech);
    assert_eq!(rig.log_snapshot(), vec!["start", "cancel"]);
    assert_eq!(rig.transcribe_calls(), 0);
    h.shutdown();
}
