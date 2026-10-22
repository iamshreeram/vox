//! Stage B3a (DESIGN.md Rev 8, sections 12.2.2, 12.2.3, 12.2.6, 12.2.8, 12.3, 12.4, 12.7.A):
//! fake-seam tests for the barge-in monitor core. Fakes only: no audio device, no cpal,
//! no ONNX, no Tauri. Written BEFORE the implementation (red first).
//!
//! Time model: a frame is FRAME = 160 samples (10 ms at 16 kHz). Voiced frames carry 0.9 in
//! every sample, silent frames 0.0. FakeVad calls a frame voiced iff its first sample > 0.5.
//! Policy constants (converse_barge.rs): SUSTAIN 600 ms, GAP 250 ms, COOLDOWN 300 ms,
//! PREROLL 300 ms.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Barrier, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use anyhow::anyhow;
use tokio::sync::oneshot;

use crate::audio_toolkit::vad::{VadFrame, VoiceActivityDetector};
use crate::managers::converse::BoxFuture;
use crate::managers::converse_barge::{BargeCapture, BargeIn, BargeWatch};
use crate::managers::converse_barge_monitor::{
    BargeConfig, BargeMonitor, ClipGate, ClipTranscriber, MonitorEffects, MonitorInput,
    MonitorStream, VadFactory,
};

const FRAME: usize = 160;
const WAIT: Duration = Duration::from_secs(5);

fn cfg() -> BargeConfig {
    BargeConfig {
        endpoint_silence_ms: 300,
        max_utterance_ms: 5000,
        heartbeat_ms: 1000,
        channel_frames: 64,
        // Out of the way for the non-stall tests; stall tests set their own (60..150 ms).
        stall_ms: 10_000,
    }
}

fn frames(voiced: bool, count: usize) -> Vec<f32> {
    let value = if voiced { 0.9 } else { 0.0 };
    vec![value; count * FRAME]
}

/// Bounded polling. Never sleeps unboundedly.
fn wait_until(cond: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    cond()
}

/// Grace period for effects of the last frame (applied synchronously after the VAD call).
fn settle() {
    thread::sleep(Duration::from_millis(30));
}

struct ThreadWaker(Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn thread_waker() -> Waker {
    Waker::from(Arc::new(ThreadWaker(thread::current())))
}

fn poll_now<F: Future + Unpin>(fut: &mut F) -> Poll<F::Output> {
    let waker = thread_waker();
    let mut cx = Context::from_waker(&waker);
    Pin::new(fut).poll(&mut cx)
}

fn capture_within(watch: &mut dyn BargeWatch, timeout: Duration) -> Option<BargeCapture> {
    let waker = thread_waker();
    let mut cx = Context::from_waker(&waker);
    let deadline = Instant::now() + timeout;
    let mut fut = watch.capture();
    loop {
        if let Poll::Ready(value) = Pin::new(&mut fut).poll(&mut cx) {
            return Some(value);
        }
        let now = Instant::now();
        if now >= deadline {
            return None;
        }
        thread::park_timeout(deadline - now);
    }
}

// ---------------------------------------------------------------- fakes

struct Hold {
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}

struct HoldEnds {
    entered: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
}

fn hold_pair() -> (Hold, HoldEnds) {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    (
        Hold {
            entered: entered_tx,
            release: release_rx,
        },
        HoldEnds {
            entered: entered_rx,
            release: release_tx,
        },
    )
}

#[derive(Default)]
struct FakeEffects {
    log: Mutex<Vec<String>>,
    speaking: AtomicBool,
    stop_probe: Mutex<Option<Box<dyn Fn() -> String + Send + Sync>>>,
    hold_duck: Mutex<Option<Hold>>,
}

impl FakeEffects {
    fn note(&self, entry: impl Into<String>) {
        self.log.lock().unwrap().push(entry.into());
    }

    fn entries(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    fn set_speaking(&self, value: bool) {
        self.speaking.store(value, Ordering::SeqCst);
    }

    fn set_stop_probe(&self, probe: Box<dyn Fn() -> String + Send + Sync>) {
        *self.stop_probe.lock().unwrap() = Some(probe);
    }

    fn hold_next_duck(&self) -> HoldEnds {
        let (hold, ends) = hold_pair();
        *self.hold_duck.lock().unwrap() = Some(hold);
        ends
    }
}

impl MonitorEffects for FakeEffects {
    fn speaking(&self) -> bool {
        self.speaking.load(Ordering::SeqCst)
    }

    fn duck(&self) {
        self.note("duck");
        let hold = self.hold_duck.lock().unwrap().take();
        if let Some(hold) = hold {
            let _ = hold.entered.send(());
            let _ = hold.release.recv();
        }
    }

    fn restore(&self) {
        self.note("restore");
    }

    fn stop_speech(&self) {
        let extra = self
            .stop_probe
            .lock()
            .unwrap()
            .as_ref()
            .map(|probe| probe())
            .unwrap_or_default();
        self.note(format!("stop{extra}"));
    }
}

type Callback = Box<dyn Fn(&[f32]) + Send + Sync>;

#[derive(Default)]
struct FakeInput {
    callback: Arc<Mutex<Option<Callback>>>,
    fail_open: AtomicBool,
    opens: AtomicUsize,
    closes: Arc<AtomicUsize>,
}

impl FakeInput {
    /// Simulates the audio thread delivering one chunk.
    fn push(&self, samples: &[f32]) {
        let guard = self.callback.lock().unwrap();
        if let Some(callback) = guard.as_ref() {
            callback(samples);
        }
    }

    fn closes(&self) -> usize {
        self.closes.load(Ordering::SeqCst)
    }
}

impl MonitorInput for FakeInput {
    fn open(&self, on_samples: Callback) -> Result<Box<dyn MonitorStream>, String> {
        if self.fail_open.load(Ordering::SeqCst) {
            return Err("no input device".to_string());
        }
        self.opens.fetch_add(1, Ordering::SeqCst);
        *self.callback.lock().unwrap() = Some(on_samples);
        Ok(Box::new(FakeStream {
            callback: Arc::clone(&self.callback),
            closes: Arc::clone(&self.closes),
        }))
    }
}

struct FakeStream {
    callback: Arc<Mutex<Option<Callback>>>,
    closes: Arc<AtomicUsize>,
}

impl MonitorStream for FakeStream {
    fn close(self: Box<Self>) {
        *self.callback.lock().unwrap() = None;
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct Counters {
    processed: AtomicUsize,
    dropped: AtomicUsize,
}

#[derive(Default)]
struct VadSpec {
    fail_factory: bool,
    fail_at: Option<usize>,
    panic_at: Option<usize>,
    hold_at: Option<usize>,
    hold: Option<Hold>,
}

struct FakeVad {
    calls: usize,
    fail_at: Option<usize>,
    panic_at: Option<usize>,
    hold_at: Option<usize>,
    hold: Mutex<Option<Hold>>,
    counters: Arc<Counters>,
}

impl Drop for FakeVad {
    fn drop(&mut self) {
        self.counters.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

impl VoiceActivityDetector for FakeVad {
    fn push_frame<'a>(&'a mut self, frame: &'a [f32]) -> anyhow::Result<VadFrame<'a>> {
        let index = self.calls;
        self.calls += 1;
        if self.panic_at == Some(index) {
            panic!("injected VAD panic");
        }
        if self.hold_at == Some(index) {
            let hold = self.hold.lock().unwrap().take();
            if let Some(hold) = hold {
                let _ = hold.entered.send(());
                let _ = hold.release.recv();
            }
        }
        if self.fail_at == Some(index) {
            return Err(anyhow!("injected VAD failure"));
        }
        self.counters.processed.fetch_add(1, Ordering::SeqCst);
        if frame[0] > 0.5 {
            Ok(VadFrame::Speech(frame))
        } else {
            Ok(VadFrame::Noise)
        }
    }

    fn frame_samples(&self) -> usize {
        FRAME
    }
}

fn vad_factory(spec: VadSpec, counters: Arc<Counters>) -> VadFactory {
    let slot = Mutex::new(Some(spec));
    Box::new(move || {
        let spec = slot.lock().unwrap().take().unwrap_or_default();
        if spec.fail_factory {
            return Err("vad model missing".to_string());
        }
        Ok(Box::new(FakeVad {
            calls: 0,
            fail_at: spec.fail_at,
            panic_at: spec.panic_at,
            hold_at: spec.hold_at,
            hold: Mutex::new(spec.hold),
            counters: Arc::clone(&counters),
        }))
    })
}

#[derive(Default)]
struct FakeTranscriber {
    received: Mutex<Vec<Vec<f32>>>,
    pending: Mutex<VecDeque<oneshot::Sender<Result<String, String>>>>,
    /// Incremented when a clip's transcription future is dropped (its work is over).
    completed: Arc<AtomicUsize>,
    /// When set, every clip completes immediately with this result.
    instant: Mutex<Option<Result<String, String>>>,
}

struct CompletionGuard(Arc<AtomicUsize>);

impl Drop for CompletionGuard {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl FakeTranscriber {
    fn calls(&self) -> usize {
        self.received.lock().unwrap().len()
    }

    fn completed(&self) -> usize {
        self.completed.load(Ordering::SeqCst)
    }

    fn received(&self) -> Vec<Vec<f32>> {
        self.received.lock().unwrap().clone()
    }

    /// Completes the oldest pending transcription.
    fn finish(&self, result: Result<String, String>) {
        let sender = self
            .pending
            .lock()
            .unwrap()
            .pop_front()
            .expect("a transcription is pending");
        let _ = sender.send(result);
    }
}

impl ClipTranscriber for FakeTranscriber {
    fn transcribe(&self, samples: Vec<f32>) -> BoxFuture<'static, Result<String, String>> {
        let guard = CompletionGuard(Arc::clone(&self.completed));
        let instant = self.instant.lock().unwrap().clone();
        if let Some(result) = instant {
            self.received.lock().unwrap().push(samples);
            return Box::pin(async move {
                let _guard = guard;
                result
            });
        }
        let (tx, rx) = oneshot::channel();
        // Pending first, so a test that sees calls() == 1 can always call finish().
        self.pending.lock().unwrap().push_back(tx);
        self.received.lock().unwrap().push(samples);
        Box::pin(async move {
            let _guard = guard;
            rx.await.unwrap_or_else(|_| Err("dropped".to_string()))
        })
    }
}

struct FakeGate {
    accept: bool,
    calls: AtomicUsize,
}

impl ClipGate for FakeGate {
    fn accepts(&self, _samples: &[f32]) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.accept
    }
}

struct Rig {
    monitor: Arc<BargeMonitor>,
    input: Arc<FakeInput>,
    effects: Arc<FakeEffects>,
    transcriber: Arc<FakeTranscriber>,
    counters: Arc<Counters>,
    gate: Option<Arc<FakeGate>>,
}

impl Rig {
    fn new(cfg: BargeConfig, spec: VadSpec, gate_accepts: Option<bool>) -> Rig {
        let input = Arc::new(FakeInput::default());
        let effects = Arc::new(FakeEffects::default());
        let transcriber = Arc::new(FakeTranscriber::default());
        let counters = Arc::new(Counters::default());
        let gate = gate_accepts.map(|accept| {
            Arc::new(FakeGate {
                accept,
                calls: AtomicUsize::new(0),
            })
        });
        let clip_gate: Option<Arc<dyn ClipGate>> = gate
            .as_ref()
            .map(|fake| Arc::clone(fake) as Arc<dyn ClipGate>);
        let monitor = BargeMonitor::start(
            Arc::clone(&input) as Arc<dyn MonitorInput>,
            vad_factory(spec, Arc::clone(&counters)),
            Arc::clone(&effects) as Arc<dyn MonitorEffects>,
            Arc::clone(&transcriber) as Arc<dyn ClipTranscriber>,
            clip_gate,
            cfg,
        )
        .expect("monitor starts");
        Rig {
            monitor,
            input,
            effects,
            transcriber,
            counters,
            gate,
        }
    }

    fn push(&self, samples: &[f32]) {
        self.input.push(samples);
    }

    fn processed(&self) -> usize {
        self.counters.processed.load(Ordering::SeqCst)
    }

    fn wait_processed(&self, n: usize) {
        assert!(
            wait_until(|| self.processed() >= n),
            "VAD processed {} of {n} frames",
            self.processed()
        );
    }

    /// Pushes frames one at a time, waiting for each to be processed (never overflows).
    fn push_paced(&self, voiced: bool, count: usize) {
        for _ in 0..count {
            let base = self.processed();
            self.push(&frames(voiced, 1));
            self.wait_processed(base + 1);
        }
    }

    fn arm(&self) -> Box<dyn BargeWatch> {
        self.monitor.arm().expect("monitor arms")
    }
}

/// Speak 70 voiced frames (sustain reached at 600 ms) then 30 silent frames (endpoint).
fn speak_then_pause(rig: &Rig) {
    rig.push(&frames(true, 70));
    rig.push(&frames(false, 30));
}

// ---------------------------------------------------------------- tests

#[test]
fn sustained_speech_stops_the_reply_and_latches() {
    let rig = Rig::new(cfg(), VadSpec::default(), None);
    rig.effects.set_speaking(true);
    let armed = rig.arm();
    let gate = armed.gate();
    let reply = armed.reply_token();
    rig.effects.set_stop_probe(Box::new(move || {
        let closed = if gate.is_closed() { "closed" } else { "open" };
        let token = if *reply.borrow() { "set" } else { "unset" };
        format!(" (gate={closed} token={token})")
    }));
    speak_then_pause(&rig);
    assert!(wait_until(|| rig.effects.entries().len() == 3));
    settle();
    assert_eq!(
        rig.effects.entries(),
        vec!["duck", "stop (gate=closed token=set)", "restore"]
    );
    assert!(armed.cancel_latched());
    let reply = armed.reply_token();
    assert!(*reply.borrow());
}

#[test]
fn short_backchannel_restores_without_cancel() {
    let rig = Rig::new(cfg(), VadSpec::default(), None);
    rig.effects.set_speaking(true);
    let mut armed = rig.arm();
    rig.push(&frames(true, 30));
    rig.push(&frames(false, 30));
    assert!(wait_until(|| rig.effects.entries().len() == 2));
    assert_eq!(rig.effects.entries(), vec!["duck", "restore"]);
    assert!(!armed.cancel_latched());
    let reply = armed.reply_token();
    assert!(!*reply.borrow());
    assert!(capture_within(&mut *armed, Duration::from_millis(50)).is_none());
}

#[test]
fn heartbeat_refreshes_the_duck_until_cancel() {
    // DESIGN 12.2.6: heartbeat while Ducked. Ducked lasts at most SUSTAIN_MS (600 ms of audio)
    // before CancelReply, so a 2.5 s ducked window cannot occur; this test uses the longest
    // ducked window the policy allows. See NOTES in the B3a report.
    let mut config = cfg();
    config.heartbeat_ms = 100;
    let rig = Rig::new(config, VadSpec::default(), None);
    rig.effects.set_speaking(true);
    let _armed = rig.arm();
    rig.push(&frames(true, 62));
    assert!(wait_until(|| rig
        .effects
        .entries()
        .last()
        .map(String::as_str)
        == Some("restore")));
    let mut expected = vec!["duck"; 6];
    expected.push("stop");
    expected.push("restore");
    assert_eq!(rig.effects.entries(), expected);
}

#[test]
fn not_speaking_produces_no_effects_even_for_loud_audio() {
    let rig = Rig::new(cfg(), VadSpec::default(), None);
    rig.effects.set_speaking(false);
    let armed = rig.arm();
    rig.push(&frames(true, 100));
    rig.wait_processed(100);
    settle();
    assert!(rig.effects.entries().is_empty());
    assert_eq!(rig.transcriber.calls(), 0);
    assert!(!armed.cancel_latched());
}

#[test]
fn drop_while_ducked_restores_once_and_later_frames_are_silent() {
    let rig = Rig::new(cfg(), VadSpec::default(), None);
    rig.effects.set_speaking(true);
    let armed = rig.arm();
    rig.push(&frames(true, 5));
    assert!(wait_until(|| rig.effects.entries() == vec!["duck"]));
    drop(armed);
    assert_eq!(rig.effects.entries(), vec!["duck", "restore"]);
    rig.push(&frames(true, 20));
    settle();
    assert_eq!(rig.effects.entries(), vec!["duck", "restore"]);
}

#[test]
fn superseded_arm_never_affects_the_new_reply() {
    let rig = Rig::new(cfg(), VadSpec::default(), None);
    rig.effects.set_speaking(true);
    let mut old = rig.arm();
    rig.push(&frames(true, 5));
    assert!(wait_until(|| rig.effects.entries() == vec!["duck"]));
    let mut new = rig.arm();
    assert!(wait_until(
        || rig.effects.entries() == vec!["duck", "restore"]
    ));
    speak_then_pause(&rig);
    assert!(wait_until(|| rig.effects.entries().len() == 5));
    assert_eq!(
        rig.effects.entries(),
        vec!["duck", "restore", "duck", "stop", "restore"]
    );
    assert!(new.cancel_latched());
    assert!(!old.cancel_latched());
    let old_reply = old.reply_token();
    assert!(!*old_reply.borrow());
    assert_eq!(
        capture_within(&mut *old, WAIT),
        Some(BargeCapture::NoSpeech)
    );
    assert!(capture_within(&mut *new, Duration::from_millis(50)).is_none());
}

#[test]
fn drop_waits_for_an_in_flight_duck_and_nothing_follows() {
    let rig = Rig::new(cfg(), VadSpec::default(), None);
    rig.effects.set_speaking(true);
    let armed = rig.arm();
    let ends = rig.effects.hold_next_duck();
    rig.push(&frames(true, 3));
    ends.entered
        .recv_timeout(WAIT)
        .expect("duck entered and is blocked");
    let (done_tx, done_rx) = mpsc::channel();
    let dropper = thread::spawn(move || {
        drop(armed);
        let _ = done_tx.send(());
    });
    thread::sleep(Duration::from_millis(100));
    assert!(
        done_rx.try_recv().is_err(),
        "drop returned while a duck was still in flight"
    );
    ends.release.send(()).expect("release the duck");
    done_rx
        .recv_timeout(WAIT)
        .expect("drop returns after the duck");
    dropper.join().expect("dropper joins");
    let after_drop = rig.effects.entries();
    assert_eq!(after_drop, vec!["duck", "restore"]);
    rig.push(&frames(true, 20));
    settle();
    assert_eq!(rig.effects.entries(), after_drop);
}

#[test]
fn utterance_is_delivered_after_endpoint_with_lead_in() {
    let rig = Rig::new(cfg(), VadSpec::default(), None);
    rig.effects.set_speaking(true);
    let mut armed = rig.arm();
    rig.push(&frames(false, 5));
    speak_then_pause(&rig);
    assert!(wait_until(|| rig.transcriber.calls() == 1));
    {
        let mut pending = armed.capture();
        assert!(matches!(poll_now(&mut pending), Poll::Pending));
    }
    let received = rig.transcriber.received();
    assert_eq!(received[0].len(), 105 * FRAME);
    assert!(received[0][..5 * FRAME].iter().all(|s| *s == 0.0));
    assert!(received[0][5 * FRAME..75 * FRAME].iter().all(|s| *s == 0.9));
    rig.transcriber.finish(Ok("  open the door \n".to_string()));
    assert_eq!(
        capture_within(&mut *armed, WAIT),
        Some(BargeCapture::Utterance("open the door".to_string()))
    );
    assert!(armed.cancel_latched());
}

#[test]
fn blank_or_failed_transcription_is_no_speech() {
    let results = [
        Ok("   \n\t".to_string()),
        Ok(String::new()),
        Err("stt offline".to_string()),
    ];
    for result in results {
        let rig = Rig::new(cfg(), VadSpec::default(), None);
        rig.effects.set_speaking(true);
        let mut armed = rig.arm();
        speak_then_pause(&rig);
        assert!(wait_until(|| rig.transcriber.calls() == 1));
        rig.transcriber.finish(result);
        assert_eq!(
            capture_within(&mut *armed, WAIT),
            Some(BargeCapture::NoSpeech)
        );
    }
}

#[test]
fn gate_rejection_is_no_speech_and_never_transcribes() {
    let rig = Rig::new(cfg(), VadSpec::default(), Some(false));
    rig.effects.set_speaking(true);
    let mut armed = rig.arm();
    speak_then_pause(&rig);
    assert_eq!(
        capture_within(&mut *armed, WAIT),
        Some(BargeCapture::NoSpeech)
    );
    assert_eq!(rig.transcriber.calls(), 0);
    let gate = rig.gate.as_ref().expect("gate installed");
    assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn dropping_before_the_transcript_discards_it() {
    let rig = Rig::new(cfg(), VadSpec::default(), None);
    rig.effects.set_speaking(true);
    let first = rig.arm();
    speak_then_pause(&rig);
    assert!(wait_until(|| rig.transcriber.calls() == 1));
    drop(first);
    let mut second = rig.arm();
    rig.transcriber.finish(Ok("late words".to_string()));
    thread::sleep(Duration::from_millis(100));
    assert!(capture_within(&mut *second, Duration::from_millis(50)).is_none());
    assert!(!second.cancel_latched());
}

#[test]
fn overflow_becomes_missing_frames_and_never_cancels() {
    let mut config = cfg();
    config.channel_frames = 4;
    let (hold, ends) = hold_pair();
    let spec = VadSpec {
        hold_at: Some(45),
        hold: Some(hold),
        ..VadSpec::default()
    };
    let rig = Rig::new(config, spec, None);
    rig.effects.set_speaking(true);
    let mut armed = rig.arm();
    rig.push_paced(true, 45);
    rig.push(&frames(true, 1));
    ends.entered
        .recv_timeout(WAIT)
        .expect("VAD blocked on frame 45");
    // Flood while the worker is stuck: 4 fit in the channel, the rest overflow.
    for _ in 0..84 {
        rig.push(&frames(true, 1));
    }
    ends.release.send(()).expect("release the VAD");
    assert!(wait_until(|| rig
        .effects
        .entries()
        .last()
        .map(String::as_str)
        == Some("restore")));
    rig.wait_processed(50);
    settle();
    assert_eq!(rig.effects.entries(), vec!["duck", "restore"]);
    assert!(rig.monitor.is_healthy());
    assert!(capture_within(&mut *armed, Duration::from_millis(50)).is_none());
}

#[test]
fn vad_factory_error_fails_start() {
    let counters = Arc::new(Counters::default());
    let input = Arc::new(FakeInput::default());
    let result = BargeMonitor::start(
        Arc::clone(&input) as Arc<dyn MonitorInput>,
        vad_factory(
            VadSpec {
                fail_factory: true,
                ..VadSpec::default()
            },
            Arc::clone(&counters),
        ),
        Arc::new(FakeEffects::default()),
        Arc::new(FakeTranscriber::default()),
        None,
        cfg(),
    );
    match result {
        Err(message) => assert!(message.contains("vad model missing"), "{message}"),
        Ok(_) => panic!("start must fail when the VAD factory fails"),
    }
    assert_eq!(input.opens.load(Ordering::SeqCst), 0);
}

#[test]
fn stream_open_error_fails_start_and_releases_the_vad() {
    let counters = Arc::new(Counters::default());
    let input = Arc::new(FakeInput::default());
    input.fail_open.store(true, Ordering::SeqCst);
    let result = BargeMonitor::start(
        Arc::clone(&input) as Arc<dyn MonitorInput>,
        vad_factory(VadSpec::default(), Arc::clone(&counters)),
        Arc::new(FakeEffects::default()),
        Arc::new(FakeTranscriber::default()),
        None,
        cfg(),
    );
    assert!(
        result.is_err(),
        "start must fail when the stream cannot open"
    );
    assert_eq!(counters.dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn runtime_vad_error_is_fail_soft() {
    let rig = Rig::new(
        cfg(),
        VadSpec {
            fail_at: Some(20),
            ..VadSpec::default()
        },
        None,
    );
    rig.effects.set_speaking(true);
    let mut armed = rig.arm();
    rig.push(&frames(true, 25));
    assert!(wait_until(|| !rig.monitor.is_healthy()
        && rig.effects.entries().last().map(String::as_str)
            == Some("restore")));
    assert_eq!(rig.effects.entries(), vec!["duck", "restore"]);
    assert!(rig.monitor.arm().is_none());
    assert_eq!(
        capture_within(&mut *armed, WAIT),
        Some(BargeCapture::NoSpeech)
    );
    let reply = armed.reply_token();
    assert!(!*reply.borrow());
    assert_eq!(rig.input.closes(), 1);
}

#[test]
fn vad_panic_is_fail_soft() {
    let rig = Rig::new(
        cfg(),
        VadSpec {
            panic_at: Some(20),
            ..VadSpec::default()
        },
        None,
    );
    rig.effects.set_speaking(true);
    let mut armed = rig.arm();
    rig.push(&frames(true, 25));
    assert!(wait_until(|| !rig.monitor.is_healthy()
        && rig.effects.entries().last().map(String::as_str)
            == Some("restore")));
    assert_eq!(rig.effects.entries(), vec!["duck", "restore"]);
    assert!(rig.monitor.arm().is_none());
    assert_eq!(
        capture_within(&mut *armed, WAIT),
        Some(BargeCapture::NoSpeech)
    );
    let reply = armed.reply_token();
    assert!(!*reply.borrow());
}

#[test]
fn reported_input_failure_is_fail_soft() {
    let rig = Rig::new(cfg(), VadSpec::default(), None);
    rig.effects.set_speaking(true);
    let mut armed = rig.arm();
    rig.push(&frames(true, 5));
    assert!(wait_until(|| rig.effects.entries() == vec!["duck"]));
    rig.monitor.report_input_failure();
    assert!(wait_until(|| !rig.monitor.is_healthy()
        && rig.effects.entries().last().map(String::as_str)
            == Some("restore")));
    assert_eq!(rig.effects.entries(), vec!["duck", "restore"]);
    assert!(rig.monitor.arm().is_none());
    assert_eq!(
        capture_within(&mut *armed, WAIT),
        Some(BargeCapture::NoSpeech)
    );
}

#[test]
fn shutdown_closes_once_joins_restores_and_is_idempotent() {
    let rig = Rig::new(cfg(), VadSpec::default(), None);
    rig.effects.set_speaking(true);
    let mut armed = rig.arm();
    rig.push(&frames(true, 5));
    assert!(wait_until(|| rig.effects.entries() == vec!["duck"]));
    let barrier = Arc::new(Barrier::new(3));
    let handles: Vec<_> = (0..3)
        .map(|_| {
            let monitor = Arc::clone(&rig.monitor);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                monitor.shutdown();
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("shutdown thread joins");
    }
    assert_eq!(rig.effects.entries(), vec!["duck", "restore"]);
    assert_eq!(rig.input.closes(), 1);
    assert_eq!(rig.counters.dropped.load(Ordering::SeqCst), 1);
    assert!(!rig.monitor.is_healthy());
    assert!(rig.monitor.arm().is_none());
    let processed = rig.processed();
    rig.push(&frames(true, 10));
    settle();
    assert_eq!(rig.processed(), processed, "worker must be joined");
    rig.monitor.shutdown();
    assert_eq!(rig.input.closes(), 1);
    assert_eq!(
        capture_within(&mut *armed, WAIT),
        Some(BargeCapture::NoSpeech)
    );
    drop(armed);
    assert_eq!(rig.effects.entries(), vec!["duck", "restore"]);
}

#[test]
fn reframing_is_independent_of_chunk_size() {
    fn run(chunk: usize) -> (Vec<String>, Vec<Vec<f32>>) {
        let mut config = cfg();
        config.channel_frames = 4096;
        let rig = Rig::new(config, VadSpec::default(), None);
        rig.effects.set_speaking(true);
        let _armed = rig.arm();
        let mut audio = frames(false, 5);
        audio.extend(frames(true, 70));
        audio.extend(frames(false, 30));
        for piece in audio.chunks(chunk) {
            rig.push(piece);
        }
        assert!(wait_until(|| rig.transcriber.calls() == 1));
        (rig.effects.entries(), rig.transcriber.received())
    }
    let exact = run(FRAME);
    assert_eq!(exact.0, vec!["duck", "stop", "restore"]);
    assert_eq!(exact.1[0].len(), 105 * FRAME);
    assert_eq!(run(7), exact);
    assert_eq!(run(3 * FRAME), exact);
}

#[test]
fn audio_callback_never_blocks_and_memory_is_bounded() {
    let mut config = cfg();
    config.channel_frames = 4;
    let (hold, ends) = hold_pair();
    let spec = VadSpec {
        hold_at: Some(0),
        hold: Some(hold),
        ..VadSpec::default()
    };
    let rig = Rig::new(config, spec, None);
    rig.effects.set_speaking(true);
    let _armed = rig.arm();
    rig.push(&frames(true, 1));
    ends.entered
        .recv_timeout(WAIT)
        .expect("VAD blocked on the first frame");
    let input = Arc::clone(&rig.input);
    let (done_tx, done_rx) = mpsc::channel();
    let started = Instant::now();
    let pusher = thread::spawn(move || {
        let chunk = frames(true, 1);
        for _ in 0..1000 {
            input.push(&chunk);
        }
        let _ = done_tx.send(());
    });
    done_rx
        .recv_timeout(WAIT)
        .expect("the audio callback returned while the worker is stuck");
    assert!(started.elapsed() < Duration::from_secs(2));
    pusher.join().expect("pusher joins");
    ends.release.send(()).expect("release the VAD");
    assert!(wait_until(|| rig.processed() >= 5));
    settle();
    // One frame held inside the VAD plus at most channel_frames queued ones.
    assert!(rig.processed() <= 5, "processed {}", rig.processed());
    assert!(rig.monitor.is_healthy());
}

/// DESIGN 12.7.C / converse.rs run_turns: the loop reads `cancel_latched()` as soon as it sees
/// the reply token fire. So the latch and the closed gate must be visible BEFORE the token.
#[test]
fn reply_token_never_fires_before_the_latch_and_gate_close() {
    let rig = Rig::new(cfg(), VadSpec::default(), None);
    rig.effects.set_speaking(true);
    for iteration in 0..300 {
        let armed = rig.arm();
        let reply = armed.reply_token();
        let base = rig.processed();
        let helper = thread::spawn(move || {
            let watch = armed;
            let deadline = Instant::now() + WAIT;
            let outcome = loop {
                if *reply.borrow() {
                    break Some((watch.cancel_latched(), watch.gate().is_closed()));
                }
                if Instant::now() >= deadline {
                    break None;
                }
                thread::yield_now();
            };
            (outcome, watch)
        });
        // 62 voiced frames: onset, then CancelReply at 600 ms of audio (frame 61).
        rig.push(&frames(true, 62));
        rig.wait_processed(base + 62);
        let (outcome, watch) = helper.join().expect("helper joins");
        let (latched, closed) =
            outcome.unwrap_or_else(|| panic!("reply token never fired (iteration {iteration})"));
        assert!(
            latched,
            "token visible before cancel_latched (iteration {iteration})"
        );
        assert!(
            closed,
            "token visible before gate closed (iteration {iteration})"
        );
        drop(watch);
    }
}

fn stall_cfg(stall_ms: u64) -> BargeConfig {
    BargeConfig { stall_ms, ..cfg() }
}

/// DESIGN 12.2.6: no audio for `stall_ms` while armed AND speaking fails the monitor soft.
#[test]
fn stalled_input_while_speaking_fails_soft() {
    let rig = Rig::new(stall_cfg(100), VadSpec::default(), None);
    rig.effects.set_speaking(true);
    let mut armed = rig.arm();
    rig.push(&frames(true, 5));
    assert!(wait_until(|| rig.effects.entries() == vec!["duck"]));
    // The input now goes silent: nothing else is pushed.
    assert!(
        wait_until(|| !rig.monitor.is_healthy()
            && rig.effects.entries().last().map(String::as_str) == Some("restore")),
        "stall not detected within the bound"
    );
    assert_eq!(rig.effects.entries(), vec!["duck", "restore"]);
    assert!(rig.monitor.arm().is_none());
    assert_eq!(
        capture_within(&mut *armed, WAIT),
        Some(BargeCapture::NoSpeech)
    );
    assert!(!armed.cancel_latched());
    let reply = armed.reply_token();
    assert!(!*reply.borrow());
    assert_eq!(rig.input.closes(), 1);
}

#[test]
fn stalled_monitor_survives_shutdown_and_closes_once() {
    let rig = Rig::new(stall_cfg(100), VadSpec::default(), None);
    rig.effects.set_speaking(true);
    let armed = rig.arm();
    rig.push(&frames(true, 5));
    assert!(wait_until(|| rig.effects.entries() == vec!["duck"]));
    assert!(wait_until(|| !rig.monitor.is_healthy()));
    assert_eq!(rig.input.closes(), 1, "early close on stall");
    rig.monitor.shutdown();
    rig.monitor.shutdown();
    assert_eq!(rig.input.closes(), 1, "stream closed exactly once overall");
    drop(armed);
    assert_eq!(rig.effects.entries(), vec!["duck", "restore"]);
}

#[test]
fn silent_input_while_armed_but_not_speaking_stays_healthy() {
    let rig = Rig::new(stall_cfg(60), VadSpec::default(), None);
    rig.effects.set_speaking(false);
    let armed = rig.arm();
    thread::sleep(Duration::from_millis(400));
    assert!(rig.monitor.is_healthy());
    drop(armed);
    assert!(rig.effects.entries().is_empty());
}

#[test]
fn silent_input_while_not_armed_stays_healthy() {
    let rig = Rig::new(stall_cfg(60), VadSpec::default(), None);
    rig.effects.set_speaking(true);
    thread::sleep(Duration::from_millis(400));
    assert!(rig.monitor.is_healthy());
    let armed = rig.arm();
    drop(armed);
}

#[test]
fn steady_frames_slower_than_half_the_stall_limit_never_stall() {
    let rig = Rig::new(stall_cfg(150), VadSpec::default(), None);
    rig.effects.set_speaking(true);
    let armed = rig.arm();
    for _ in 0..12 {
        thread::sleep(Duration::from_millis(60));
        rig.push(&frames(false, 1));
        assert!(rig.monitor.is_healthy(), "stalled on a steady stream");
    }
    assert!(rig.effects.entries().is_empty());
    drop(armed);
}

/// The stall clock counts only while armed AND speaking: a long quiet period before speech
/// must not make the first speech frames look stalled.
#[test]
fn stall_clock_starts_when_speech_starts() {
    let rig = Rig::new(stall_cfg(150), VadSpec::default(), None);
    rig.effects.set_speaking(false);
    let armed = rig.arm();
    thread::sleep(Duration::from_millis(400));
    rig.effects.set_speaking(true);
    thread::sleep(Duration::from_millis(30));
    assert!(
        rig.monitor.is_healthy(),
        "stall fired right after speech began"
    );
    assert!(
        wait_until(|| !rig.monitor.is_healthy()),
        "stall not detected after speech began"
    );
    drop(armed);
}

/// DESIGN Rev 10 (a): an overflow while the clip is being CAPTURED discards that clip. The reply
/// was already cancelled; the capture resolves NoSpeech and the clip is never classified.
#[test]
fn overflow_while_capturing_discards_the_clip_and_later_replies_work() {
    let mut config = cfg();
    config.channel_frames = 4;
    let (hold, ends) = hold_pair();
    let spec = VadSpec {
        hold_at: Some(62),
        hold: Some(hold),
        ..VadSpec::default()
    };
    let rig = Rig::new(config, spec, Some(true));
    rig.effects.set_speaking(true);
    let mut first = rig.arm();
    rig.push_paced(true, 62); // cancel at frame 61; now Capturing
    assert!(wait_until(
        || rig.effects.entries() == vec!["duck", "stop", "restore"]
    ));
    rig.push(&frames(true, 1));
    ends.entered
        .recv_timeout(WAIT)
        .expect("VAD blocked in Capturing");
    for _ in 0..84 {
        rig.push(&frames(true, 1)); // overflow while capturing
    }
    ends.release.send(()).expect("release the VAD");
    rig.wait_processed(67);
    // Endpoint silence ends the (discarded) capture.
    rig.push_paced(false, 30);
    settle();
    assert_eq!(rig.transcriber.calls(), 0, "discarded clip was transcribed");
    assert_eq!(
        rig.gate
            .as_ref()
            .expect("gate installed")
            .calls
            .load(Ordering::SeqCst),
        0,
        "discarded clip reached the gate"
    );
    assert!(rig.monitor.is_healthy());
    assert!(first.cancel_latched());
    assert_eq!(
        capture_within(&mut *first, WAIT),
        Some(BargeCapture::NoSpeech)
    );
    drop(first);

    let mut second = rig.arm();
    speak_then_pause(&rig);
    assert!(wait_until(|| rig.transcriber.calls() == 1));
    rig.transcriber.finish(Ok("later words".to_string()));
    assert_eq!(
        capture_within(&mut *second, WAIT),
        Some(BargeCapture::Utterance("later words".to_string()))
    );
    assert!(second.cancel_latched());
}

/// DESIGN Rev 10 (a)/(b): an overflow while Ducked restores the duck and cancels nothing.
#[test]
fn overflow_while_ducked_restores_without_cancel() {
    let mut config = cfg();
    config.channel_frames = 4;
    let (hold, ends) = hold_pair();
    let spec = VadSpec {
        hold_at: Some(5),
        hold: Some(hold),
        ..VadSpec::default()
    };
    let rig = Rig::new(config, spec, None);
    rig.effects.set_speaking(true);
    let mut armed = rig.arm();
    rig.push_paced(true, 5);
    rig.push(&frames(true, 1));
    ends.entered
        .recv_timeout(WAIT)
        .expect("VAD blocked while ducked");
    for _ in 0..84 {
        rig.push(&frames(true, 1));
    }
    ends.release.send(()).expect("release the VAD");
    rig.wait_processed(10);
    assert!(wait_until(|| rig
        .effects
        .entries()
        .last()
        .map(String::as_str)
        == Some("restore")));
    settle();
    assert_eq!(rig.effects.entries(), vec!["duck", "restore"]);
    assert!(!armed.cancel_latched());
    let reply = armed.reply_token();
    assert!(!*reply.borrow());
    assert!(rig.monitor.is_healthy());
    assert!(capture_within(&mut *armed, Duration::from_millis(50)).is_none());
}

/// DESIGN Rev 10 (c): the discard flag is per capture. An overflow BEFORE the cancel must not
/// stop clean speech that follows it.
#[test]
fn overflow_before_the_cancel_does_not_stop_later_clean_speech() {
    let mut config = cfg();
    config.channel_frames = 4;
    let (hold, ends) = hold_pair();
    let spec = VadSpec {
        hold_at: Some(20),
        hold: Some(hold),
        ..VadSpec::default()
    };
    let rig = Rig::new(config, spec, None);
    rig.effects.set_speaking(true);
    let armed = rig.arm();
    rig.push_paced(true, 20);
    rig.push(&frames(true, 1));
    ends.entered
        .recv_timeout(WAIT)
        .expect("VAD blocked while ducked");
    for _ in 0..84 {
        rig.push(&frames(true, 1));
    }
    ends.release.send(()).expect("release the VAD");
    rig.wait_processed(25);
    assert!(wait_until(|| rig
        .effects
        .entries()
        .last()
        .map(String::as_str)
        == Some("restore")));
    rig.push_paced(true, 100);
    assert!(wait_until(|| rig.effects.entries().len() == 5));
    assert_eq!(
        rig.effects.entries(),
        vec!["duck", "restore", "duck", "stop", "restore"]
    );
    assert!(armed.cancel_latched());
    assert!(rig.monitor.is_healthy());
}

/// DESIGN Rev 10 (d): shutdown waits for clip threads, but only for a bounded total time.
#[test]
fn shutdown_gives_up_on_a_blocked_clip_after_the_bound() {
    let rig = Rig::new(cfg(), VadSpec::default(), None);
    rig.effects.set_speaking(true);
    let mut armed = rig.arm();
    speak_then_pause(&rig);
    assert!(wait_until(|| rig.transcriber.calls() == 1));
    let started = Instant::now();
    rig.monitor.shutdown();
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(3500),
        "shutdown waited {elapsed:?}"
    );
    assert_eq!(
        rig.transcriber.completed(),
        0,
        "clip still blocked at shutdown"
    );
    assert_eq!(
        capture_within(&mut *armed, WAIT),
        Some(BargeCapture::NoSpeech)
    );
    assert!(rig.monitor.arm().is_none());
    // The late result arrives after shutdown: the clip finishes on its own, nothing is delivered.
    rig.transcriber.finish(Ok("late words".to_string()));
    assert!(wait_until(|| rig.transcriber.completed() == 1));
    // Still NoSpeech: the late "late words" result was discarded, not delivered.
    assert_eq!(
        capture_within(&mut *armed, Duration::from_millis(50)),
        Some(BargeCapture::NoSpeech)
    );
}

/// DESIGN Rev 10 (d): a clip that finishes quickly is joined by shutdown.
#[test]
fn shutdown_joins_a_quick_clip_before_returning() {
    let rig = Rig::new(cfg(), VadSpec::default(), None);
    *rig.transcriber.instant.lock().unwrap() = Some(Ok("quick words".to_string()));
    rig.effects.set_speaking(true);
    let armed = rig.arm();
    speak_then_pause(&rig);
    assert!(wait_until(|| rig.transcriber.calls() == 1));
    rig.monitor.shutdown();
    assert_eq!(
        rig.transcriber.completed(),
        1,
        "clip work finished before shutdown returned"
    );
    drop(armed);
}

#[test]
fn compile_time_send_and_sync_guarantees() {
    fn assert_send<T: Send>(_: &T) {}
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<BargeMonitor>();
    let rig = Rig::new(cfg(), VadSpec::default(), None);
    let mut armed = rig.arm();
    let pending = armed.capture();
    assert_send(&pending);
    drop(pending);
}
