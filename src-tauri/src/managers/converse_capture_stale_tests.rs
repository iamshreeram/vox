//! V2: a STALE listen (id at or below the cancelled floor) must be ignored exactly as the
//! baseline ignored it: it answers `Aborted` and touches nothing else (no recorder call,
//! no in-flight state, no ring, no timing slot). Holds with the optional voice features
//! OFF (baseline behaviour) and ON. A FRESH listen still clears the pre-VAD ring and the
//! timing slot, so nothing leaks from the previous listen.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ------------------------- fakes -------------------------

/// Recorder port with counters and a settable pre-VAD tap.
#[derive(Default)]
struct SPort {
    active: AtomicBool,
    starts: AtomicUsize,
    cancels: AtomicUsize,
    tap: Mutex<Option<RawTap>>,
}

impl SPort {
    fn new() -> Arc<Self> {
        Arc::new(SPort::default())
    }
    fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }
    fn starts(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
    }
    fn cancels(&self) -> usize {
        self.cancels.load(Ordering::SeqCst)
    }
    fn tap_is_set(&self) -> bool {
        self.tap.lock().unwrap().is_some()
    }
    /// Delivers one raw 16 kHz frame to the installed tap (no-op when none).
    fn feed_raw(&self, frame: &[f32]) {
        let tap = self.tap.lock().unwrap().clone();
        if let Some(tap) = tap {
            tap(frame);
        }
    }
}

impl RecorderPort for SPort {
    fn start(&self) -> Result<(), StartError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.active.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn stop(&self) -> Option<Vec<f32>> {
        if self.active.swap(false, Ordering::SeqCst) {
            Some(vec![0.1; 16_000])
        } else {
            None
        }
    }
    fn cancel(&self) {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        self.active.store(false, Ordering::SeqCst);
    }
    fn set_raw_tap(&self, tap: Option<RawTap>) {
        *self.tap.lock().unwrap() = tap;
    }
}

struct STr;

impl Transcriber for STr {
    fn transcribe(&self, _samples: Vec<f32>) -> Result<String, TranscribeFailure> {
        Ok("hello world".to_string())
    }
}

/// Smart Turn detector that never answers (the endpoint falls back to the fixed rule).
struct NoAnswer;

impl TurnDetector for NoAnswer {
    fn done_probability(
        &self,
        _tail16k: &[f32],
        _deadline: Duration,
        _cancel: &AtomicBool,
    ) -> Option<f32> {
        None
    }
}

// ------------------------- harness -------------------------

const LOUD: &[f32] = &[0.0, 0.6, 0.2];
const QUIET: &[f32] = &[0.0, 0.0, 0.0];
/// One 50 ms step of 16 kHz audio fed to the pre-VAD tap.
const RAW_STEP: [f32; 800] = [0.05; 800];

fn fast() -> CaptureTimings {
    CaptureTimings {
        busy_retry: Duration::from_millis(10),
        busy_deadline: Duration::from_millis(150),
        readiness_deadline: Duration::from_millis(150),
        abs_slack: Duration::from_millis(150),
        tick: Duration::from_millis(5),
    }
}

fn cfg() -> EndpointConfig {
    EndpointConfig {
        silence_ms: 300,
        initial_grace_ms: 600,
        max_utterance_ms: 5000,
        min_speech_ms: 100,
        floor: 0.02,
        smart_turn: None,
    }
}

/// Both optional features OFF: the baseline capture path.
fn spawn_off(port: &Arc<SPort>) -> CaptureHandle {
    CaptureWorker::spawn(port.clone(), Arc::new(STr), fast())
}

/// Smart Turn ON (speaker filter OFF).
fn spawn_on(port: &Arc<SPort>) -> CaptureHandle {
    let detector: Arc<dyn TurnDetector> = Arc::new(NoAnswer);
    let options = CaptureOptions {
        turn_detector: Some(detector),
        speaker_gate: None,
    };
    CaptureWorker::spawn_with_options(port.clone(), Arc::new(STr), fast(), options)
}

async fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let started = Instant::now();
    while !cond() {
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// Starts a listen and waits until its recording is running.
async fn start_listen(
    h: &CaptureHandle,
    port: &Arc<SPort>,
) -> tokio::task::JoinHandle<ListenOutcome> {
    let h2 = h.clone();
    let task = tokio::spawn(async move { h2.listen(cfg()).await });
    wait_for("recording start", || port.is_active()).await;
    task
}

/// Speech then enough silence to end the utterance (virtual clock, as the inline tests).
fn speak_then_silence(h: &CaptureHandle) {
    let mut t = 0u64;
    while t < 400 {
        h.push_speech(t);
        h.push_levels(t, LOUD);
        t += 50;
    }
    while t < 1200 {
        h.push_levels(t, QUIET);
        t += 50;
    }
}

async fn finish(task: tokio::task::JoinHandle<ListenOutcome>) -> ListenOutcome {
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .expect("listen hung")
        .expect("listen task panicked")
}

/// Sends a Listen with an id at or below the floor, exactly as a late caller would.
async fn send_stale(h: &CaptureHandle, stale_id: u64) -> ListenOutcome {
    let (reply_tx, reply_rx) = oneshot::channel();
    let sent = h.inner.tx.send(Command::Listen {
        listen_id: stale_id,
        cfg: cfg(),
        reply: reply_tx,
    });
    assert!(sent.is_ok(), "worker must be alive");
    tokio::time::timeout(Duration::from_secs(3), reply_rx)
        .await
        .expect("stale listen was never answered")
        .expect("stale listen reply dropped")
}

/// Listen 1 is cancelled: the floor becomes 1 and its timing lands in the slot.
async fn cancel_listen_one(h: &CaptureHandle, port: &Arc<SPort>) {
    let task = start_listen(h, port).await;
    assert_eq!(h.last_listen_id(), 1, "the first listen must be id 1");
    h.inner.cancel_upto(1);
    assert_eq!(finish(task).await, ListenOutcome::Aborted);
    assert!(
        !port.is_active(),
        "the cancelled recording must be released"
    );
}

// ------------------------- features OFF (baseline) -------------------------

#[tokio::test]
async fn stale_listen_with_features_off_changes_nothing_in_flight() {
    let port = SPort::new();
    let h = spawn_off(&port);
    cancel_listen_one(&h, &port).await;

    // Nothing in flight: a stale listen leaves listen 1's timing in the slot.
    assert_eq!(send_stale(&h, 1).await, ListenOutcome::Aborted);
    assert!(
        h.take_timing(1).is_some(),
        "a stale listen must not clear the timing slot"
    );

    // Listen 2 in flight: a stale listen is ignored, the recording keeps going.
    let task2 = start_listen(&h, &port).await;
    let (starts, cancels) = (port.starts(), port.cancels());
    assert_eq!(send_stale(&h, 1).await, ListenOutcome::Aborted);
    assert!(
        port.is_active(),
        "a stale listen must not stop the recording"
    );
    assert_eq!(
        port.cancels(),
        cancels,
        "a stale listen must not cancel the recorder"
    );
    assert_eq!(
        port.starts(),
        starts,
        "a stale listen must not start the recorder"
    );
    assert!(
        h.capture_active(),
        "the capture claim must survive a stale listen"
    );
    assert!(!task2.is_finished(), "listen 2 must still be in flight");

    speak_then_silence(&h);
    assert_eq!(
        finish(task2).await,
        ListenOutcome::Utterance("hello world".to_string()),
        "listen 2 must complete normally"
    );
    assert!(
        h.take_timing(2).is_some(),
        "listen 2 must write its own timing"
    );
    h.shutdown();
}

#[tokio::test]
async fn fresh_listen_after_a_stale_one_starts_normally_with_features_off() {
    let port = SPort::new();
    let h = spawn_off(&port);
    cancel_listen_one(&h, &port).await;
    assert_eq!(send_stale(&h, 1).await, ListenOutcome::Aborted);

    let starts = port.starts();
    let task2 = start_listen(&h, &port).await;
    assert_eq!(
        port.starts(),
        starts + 1,
        "the fresh listen must start the recorder"
    );
    speak_then_silence(&h);
    assert_eq!(
        finish(task2).await,
        ListenOutcome::Utterance("hello world".to_string())
    );
    h.shutdown();
}

// ------------------------- features ON -------------------------

#[tokio::test]
async fn stale_listen_with_features_on_leaves_capture_and_ring_alone() {
    let port = SPort::new();
    let h = spawn_on(&port);
    cancel_listen_one(&h, &port).await;

    // Listen 2 in flight with the pre-VAD tap installed and frames in the ring.
    let task2 = start_listen(&h, &port).await;
    assert!(port.tap_is_set(), "the pre-VAD tap must be installed");
    port.feed_raw(&RAW_STEP);
    port.feed_raw(&RAW_STEP);
    let ring_before = h.ring_len();
    assert!(ring_before > 0, "the ring must hold the fed frames");

    let (starts, cancels) = (port.starts(), port.cancels());
    assert_eq!(send_stale(&h, 1).await, ListenOutcome::Aborted);
    assert!(
        port.is_active(),
        "a stale listen must not stop the recording"
    );
    assert_eq!(
        port.cancels(),
        cancels,
        "a stale listen must not cancel the recorder"
    );
    assert_eq!(
        port.starts(),
        starts,
        "a stale listen must not start the recorder"
    );
    assert!(
        port.tap_is_set(),
        "a stale listen must not remove the pre-VAD tap"
    );
    assert_eq!(
        h.ring_len(),
        ring_before,
        "a stale listen must not clear the ring"
    );
    assert!(
        h.capture_active(),
        "the capture claim must survive a stale listen"
    );
    assert!(!task2.is_finished(), "listen 2 must still be in flight");

    // A fresh listen supersedes listen 2 (as before) and leaves no ring or timing behind.
    let task3 = {
        let h3 = h.clone();
        tokio::spawn(async move { h3.listen(cfg()).await })
    };
    assert_eq!(finish(task2).await, ListenOutcome::Aborted);
    wait_for("fresh recording start", || port.is_active()).await;
    assert_eq!(
        h.ring_len(),
        0,
        "a fresh listen must clear the pre-VAD ring"
    );
    assert!(
        h.take_timing(2).is_none(),
        "a fresh listen must clear the timing slot"
    );
    h.shutdown();
    assert_eq!(finish(task3).await, ListenOutcome::Aborted);
}
