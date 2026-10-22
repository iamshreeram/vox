//! Stage B3a (DESIGN.md Rev 8, sections 12.2.2, 12.2.3, 12.2.6, 12.2.8, 12.3, 12.7.A-C):
//! the barge-in monitor CORE, built only against injected seams (fakes in tests). No audio
//! device, no cpal, no ONNX, no Tauri: the real adapters are step B3b.
//!
//! Threads: the audio callback only copies a chunk into a bounded channel (try_send; a full
//! channel sets an overflow flag). One monitor worker re-frames into exact VAD frames, derives
//! time from the COUNT of consumed samples (never the wall clock), runs the VAD, polls
//! `speaking()` once per frame and feeds `BargePolicy`. Effects are applied while holding the
//! arm lock, so disarming (dropping a watch, superseding it, shutdown, failure) returns only
//! after any in-flight effect of that arm finished. Completed clips are classified on a
//! short-lived blocking thread (speaker gate, then transcription) and delivered by generation.

use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::pin::{pin, Pin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tokio::sync::{oneshot, watch};

use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;
use crate::audio_toolkit::vad::VoiceActivityDetector;
use crate::managers::converse::BoxFuture;
use crate::managers::converse_barge::{
    BargeCapture, BargeIn, BargePolicy, BargeWatch, PolicyConfig, PolicyEvent, PolicyState,
    ReplyGate,
};

/// How long the worker waits for audio before re-checking the stop and failure flags.
const POLL: Duration = Duration::from_millis(20);
/// Bounded wait for the worker to exit during shutdown; past it the thread is detached.
const JOIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Receives audio chunks from the input stream (any size; the monitor re-frames).
pub type SampleCallback = Box<dyn Fn(&[f32]) + Send + Sync>;

/// Second input stream (16 kHz mono, any chunk size). The real adapter arrives in B3b.
pub trait MonitorInput: Send + Sync {
    fn open(&self, on_samples: SampleCallback) -> Result<Box<dyn MonitorStream>, String>;
}

/// An open input stream. `close` is called exactly once.
pub trait MonitorStream: Send {
    fn close(self: Box<Self>);
}

/// Playback side effects. Ducking is a gain change; `stop_speech` stops the reply.
pub trait MonitorEffects: Send + Sync {
    fn speaking(&self) -> bool;
    fn duck(&self);
    fn restore(&self);
    fn stop_speech(&self);
}

/// Builds one fresh VAD per monitor.
pub type VadFactory = Box<dyn Fn() -> Result<Box<dyn VoiceActivityDetector>, String> + Send + Sync>;

/// Transcribes one captured clip (16 kHz mono).
pub trait ClipTranscriber: Send + Sync {
    fn transcribe(&self, samples: Vec<f32>) -> BoxFuture<'static, Result<String, String>>;
}

/// Speaker gate for a captured clip. May block: only called from a blocking worker.
pub trait ClipGate: Send + Sync {
    fn accepts(&self, samples: &[f32]) -> bool;
}

/// Monitor tuning. `heartbeat_ms` refreshes the duck while Ducked; `channel_frames` bounds the
/// audio channel between the callback and the worker. `stall_ms` is the DESIGN 12.2.6 stall
/// limit: no audio for this long while armed AND speaking fails the monitor soft (production 1000).
#[derive(Debug, Clone, Copy)]
pub struct BargeConfig {
    pub endpoint_silence_ms: u64,
    pub max_utterance_ms: u64,
    pub heartbeat_ms: u64,
    pub channel_frames: usize,
    pub stall_ms: u64,
}

/// One arm of the monitor. Owned by the session; `arm()` hands out watches.
pub struct BargeMonitor {
    shared: Arc<Shared>,
    worker: Mutex<Option<(JoinHandle<()>, Receiver<()>)>>,
}

/// The single arm slot: at most one watch is armed at a time.
struct ArmState {
    next_id: u64,
    slot: Option<Armed>,
}

struct Armed {
    id: u64,
    policy: BargePolicy,
    reply_tx: watch::Sender<bool>,
    gate: Arc<ReplyGate>,
    latched: Arc<AtomicBool>,
    capture_tx: Option<oneshot::Sender<BargeCapture>>,
    ducked: bool,
    last_duck_ms: u64,
    /// Set when an overflow hits while this reply's utterance is being captured (Rev 10 (a)).
    discard: bool,
}

struct Shared {
    effects: Arc<dyn MonitorEffects>,
    transcriber: Arc<dyn ClipTranscriber>,
    gate: Option<Arc<dyn ClipGate>>,
    policy_cfg: PolicyConfig,
    heartbeat_ms: u64,
    arm: Mutex<ArmState>,
    stream: Mutex<Option<Box<dyn MonitorStream>>>,
    overflow: Arc<AtomicBool>,
    input_failed: AtomicBool,
    stop: AtomicBool,
    healthy: AtomicBool,
    /// Live clip threads: joined (bounded) by `shutdown`, pruned when finished.
    clips: Mutex<Vec<JoinHandle<()>>>,
}

/// The clip result of one watch: pending until delivered, then kept for repeated `capture()`.
enum CaptureSlot {
    Waiting(oneshot::Receiver<BargeCapture>),
    Resolved(BargeCapture),
}

/// The watch handed to one reply turn. Dropping it disarms the monitor.
struct MonitorWatch {
    id: u64,
    shared: Arc<Shared>,
    reply_rx: watch::Receiver<bool>,
    gate: Arc<ReplyGate>,
    latched: Arc<AtomicBool>,
    capture_rx: CaptureSlot,
}

struct Worker {
    shared: Arc<Shared>,
    vad: Box<dyn VoiceActivityDetector>,
    rx: Receiver<Vec<f32>>,
    frame_samples: usize,
    pending: Vec<f32>,
    consumed: u64,
    stall: Duration,
    last_chunk: Instant,
}

impl BargeMonitor {
    /// Opens the input stream and starts the worker. `Err` means the caller must run
    /// half-duplex (no monitor); nothing is left running on error.
    pub fn start(
        input: Arc<dyn MonitorInput>,
        vad: VadFactory,
        effects: Arc<dyn MonitorEffects>,
        transcriber: Arc<dyn ClipTranscriber>,
        gate: Option<Arc<dyn ClipGate>>,
        cfg: BargeConfig,
    ) -> Result<Arc<BargeMonitor>, String> {
        if cfg.channel_frames == 0 {
            return Err("barge-in channel_frames must be at least 1".to_string());
        }
        if cfg.heartbeat_ms == 0 {
            return Err("barge-in heartbeat_ms must be at least 1".to_string());
        }
        if cfg.stall_ms == 0 {
            return Err("barge-in stall_ms must be at least 1".to_string());
        }
        let detector = vad().map_err(|e| format!("barge-in VAD init failed: {e}"))?;
        let frame_samples = detector.frame_samples();
        if frame_samples == 0 {
            return Err("barge-in VAD reports a zero frame size".to_string());
        }
        let rate = WHISPER_SAMPLE_RATE as u64;
        let policy_cfg = PolicyConfig {
            endpoint_silence_ms: cfg.endpoint_silence_ms,
            max_utterance_ms: cfg.max_utterance_ms,
            frame_ms: frame_samples as u64 * 1000 / rate,
        };

        let (tx, rx): (SyncSender<Vec<f32>>, Receiver<Vec<f32>>) =
            mpsc::sync_channel(cfg.channel_frames);
        let overflow = Arc::new(AtomicBool::new(false));
        let callback_overflow = Arc::clone(&overflow);
        let on_samples: SampleCallback = Box::new(move |samples: &[f32]| {
            // Never blocks, never calls effects. A full channel only records the loss.
            if tx.try_send(samples.to_vec()).is_err() {
                callback_overflow.store(true, Ordering::SeqCst);
            }
        });
        let stream = input
            .open(on_samples)
            .map_err(|e| format!("barge-in input open failed: {e}"))?;

        let shared = Arc::new(Shared {
            effects,
            transcriber,
            gate,
            policy_cfg,
            heartbeat_ms: cfg.heartbeat_ms,
            arm: Mutex::new(ArmState {
                next_id: 1,
                slot: None,
            }),
            stream: Mutex::new(Some(stream)),
            overflow,
            input_failed: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            healthy: AtomicBool::new(true),
            clips: Mutex::new(Vec::new()),
        });

        let (done_tx, done_rx) = mpsc::channel::<()>();
        let mut worker = Worker {
            shared: Arc::clone(&shared),
            vad: detector,
            rx,
            frame_samples,
            pending: Vec::new(),
            consumed: 0,
            stall: Duration::from_millis(cfg.stall_ms),
            last_chunk: Instant::now(),
        };
        let worker_shared = Arc::clone(&shared);
        let spawned = thread::Builder::new()
            .name("barge-monitor".to_string())
            .spawn(move || {
                // Dropped on exit (also on unwind): shutdown uses it as the exit signal.
                let _done = done_tx;
                let outcome = catch_unwind(AssertUnwindSafe(|| worker.run()));
                match outcome {
                    Ok(Ok(())) => {}
                    Ok(Err(reason)) => fail(&worker_shared, &reason),
                    Err(_) => fail(&worker_shared, "monitor worker panicked"),
                }
            });
        match spawned {
            Ok(handle) => Ok(Arc::new(BargeMonitor {
                shared,
                worker: Mutex::new(Some((handle, done_rx))),
            })),
            Err(err) => {
                close_stream(&shared);
                Err(format!("barge-in worker failed to start: {err}"))
            }
        }
    }

    /// Stops the worker, closes the stream once, restores any duck, joins (bounded).
    /// Idempotent and safe from several threads. `arm()` returns `None` afterwards.
    pub fn shutdown(&self) {
        {
            let mut state = lock(&self.shared.arm);
            self.shared.stop.store(true, Ordering::SeqCst);
            self.shared.healthy.store(false, Ordering::SeqCst);
            disarm_locked(&self.shared, &mut state);
        }
        close_stream(&self.shared);
        let worker = lock(&self.worker).take();
        if let Some((handle, done)) = worker {
            match done.recv_timeout(JOIN_TIMEOUT) {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                    let _ = handle.join();
                }
                Err(RecvTimeoutError::Timeout) => {
                    log::warn!("barge-in worker did not stop within {JOIN_TIMEOUT:?}; detaching");
                }
            }
        }
        join_clips(&self.shared, JOIN_TIMEOUT);
    }

    /// False once any internal failure happened or after `shutdown`.
    // Used by B3b (health probe from the session); no production caller yet.
    #[allow(dead_code)]
    pub fn is_healthy(&self) -> bool {
        self.shared.healthy.load(Ordering::SeqCst)
    }

    /// Reported by the input adapter when its stream fails. Makes the monitor fail soft.
    // Used by B3b (the real input adapter reports stream failures); no production caller yet.
    #[allow(dead_code)]
    pub fn report_input_failure(&self) {
        self.shared.input_failed.store(true, Ordering::SeqCst);
    }
}

impl Drop for BargeMonitor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl BargeIn for BargeMonitor {
    fn arm(&self) -> Option<Box<dyn BargeWatch>> {
        let mut state = lock(&self.shared.arm);
        if !self.shared.healthy.load(Ordering::SeqCst) {
            return None;
        }
        // A newer arm supersedes the older one, with the same acknowledgement as a drop.
        disarm_locked(&self.shared, &mut state);
        let id = state.next_id;
        state.next_id += 1;
        let (reply_tx, reply_rx) = watch::channel(false);
        let gate = ReplyGate::new();
        let latched = Arc::new(AtomicBool::new(false));
        let (capture_tx, capture_rx) = oneshot::channel();
        state.slot = Some(Armed {
            id,
            policy: BargePolicy::new(self.shared.policy_cfg),
            reply_tx,
            gate: Arc::clone(&gate),
            latched: Arc::clone(&latched),
            capture_tx: Some(capture_tx),
            ducked: false,
            last_duck_ms: 0,
            discard: false,
        });
        Some(Box::new(MonitorWatch {
            id,
            shared: Arc::clone(&self.shared),
            reply_rx,
            gate,
            latched,
            capture_rx: CaptureSlot::Waiting(capture_rx),
        }))
    }
}

impl BargeWatch for MonitorWatch {
    fn reply_token(&self) -> watch::Receiver<bool> {
        self.reply_rx.clone()
    }

    fn cancel_latched(&self) -> bool {
        self.latched.load(Ordering::SeqCst)
    }

    fn gate(&self) -> Arc<ReplyGate> {
        Arc::clone(&self.gate)
    }

    fn capture(&mut self) -> BoxFuture<'_, BargeCapture> {
        // A oneshot receiver panics if polled after it completed, so the result is kept: every
        // call (also a repeated one) resolves to the same value.
        Box::pin(std::future::poll_fn(move |cx| {
            let slot = &mut self.capture_rx;
            let polled = match slot {
                CaptureSlot::Waiting(rx) => Some(Pin::new(rx).poll(cx)),
                CaptureSlot::Resolved(_) => None,
            };
            match polled {
                Some(Poll::Pending) => return Poll::Pending,
                Some(Poll::Ready(result)) => {
                    *slot = CaptureSlot::Resolved(result.unwrap_or(BargeCapture::NoSpeech));
                }
                None => {}
            }
            match slot {
                CaptureSlot::Resolved(value) => Poll::Ready(value.clone()),
                CaptureSlot::Waiting(_) => Poll::Pending,
            }
        }))
    }
}

impl Drop for MonitorWatch {
    fn drop(&mut self) {
        let mut state = lock(&self.shared.arm);
        if state.slot.as_ref().map(|armed| armed.id) == Some(self.id) {
            disarm_locked(&self.shared, &mut state);
        }
    }
}

impl Worker {
    fn run(&mut self) -> Result<(), String> {
        loop {
            if self.shared.stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            if self.shared.input_failed.load(Ordering::SeqCst) {
                return Err("input stream reported a failure".to_string());
            }
            match self.rx.recv_timeout(POLL) {
                Ok(chunk) => {
                    self.last_chunk = Instant::now();
                    if self.shared.overflow.swap(false, Ordering::SeqCst) {
                        self.missing();
                    }
                    self.feed(&chunk)?;
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self.shared.overflow.swap(false, Ordering::SeqCst) {
                        self.missing();
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    if self.shared.stop.load(Ordering::SeqCst) {
                        return Ok(());
                    }
                    return Err("input stream closed unexpectedly".to_string());
                }
            }
            self.check_stall()?;
        }
    }

    /// DESIGN 12.2.6: no audio for `stall` while armed AND speaking is a dead input. The clock
    /// only runs while both hold; otherwise it is held at "now", so quiet periods and the start
    /// of speech never count as stall time.
    fn check_stall(&mut self) -> Result<(), String> {
        let armed = lock(&self.shared.arm).slot.is_some();
        if !armed || !self.shared.effects.speaking() {
            self.last_chunk = Instant::now();
            return Ok(());
        }
        if self.last_chunk.elapsed() >= self.stall {
            return Err(format!(
                "input stalled: no audio for {} ms while speaking",
                self.stall.as_millis()
            ));
        }
        Ok(())
    }

    fn feed(&mut self, chunk: &[f32]) -> Result<(), String> {
        let mut pending = std::mem::take(&mut self.pending);
        pending.extend_from_slice(chunk);
        let mut start = 0;
        while pending.len() - start >= self.frame_samples {
            let end = start + self.frame_samples;
            self.process_frame(&pending[start..end])?;
            start = end;
        }
        pending.drain(..start);
        self.pending = pending;
        Ok(())
    }

    fn process_frame(&mut self, frame: &[f32]) -> Result<(), String> {
        let voiced = self
            .vad
            .is_voice(frame)
            .map_err(|e| format!("barge-in VAD failed: {e}"))?;
        self.consumed += frame.len() as u64;
        let speaking = self.shared.effects.speaking();
        let now = now_ms(self.consumed);
        let mut state = lock(&self.shared.arm);
        let Some(armed) = state.slot.as_mut() else {
            return Ok(());
        };
        let events = armed.policy.push_frame(now, voiced, speaking, frame);
        apply_events(&self.shared, armed, now, events);
        heartbeat(&self.shared, armed, now);
        Ok(())
    }

    /// One lost span (overflow or stalled stream): counted as one unvoiced frame of time.
    fn missing(&mut self) {
        self.consumed += self.frame_samples as u64;
        let now = now_ms(self.consumed);
        let mut state = lock(&self.shared.arm);
        if let Some(armed) = state.slot.as_mut() {
            if armed.policy.state() == PolicyState::Capturing {
                discard_capture(armed);
            }
            let events = armed.policy.push_missing(now);
            apply_events(&self.shared, armed, now, events);
        }
    }
}

/// DESIGN Rev 10 (a): a lost span inside an utterance makes that clip garbled. The capture resolves
/// NoSpeech now and the clip is never classified. The reply stays cancelled; the monitor stays up.
fn discard_capture(armed: &mut Armed) {
    armed.discard = true;
    if let Some(tx) = armed.capture_tx.take() {
        let _ = tx.send(BargeCapture::NoSpeech);
    }
}

fn now_ms(consumed_samples: u64) -> u64 {
    consumed_samples * 1000 / WHISPER_SAMPLE_RATE as u64
}

/// Poison-tolerant lock: a panicking holder must not wedge disarm or shutdown.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Disarms the current arm. Must be called with the arm lock held.
fn disarm_locked(shared: &Shared, state: &mut ArmState) {
    if let Some(armed) = state.slot.take() {
        if armed.ducked {
            shared.effects.restore();
        }
        if let Some(tx) = armed.capture_tx {
            let _ = tx.send(BargeCapture::NoSpeech);
        }
    }
}

/// Fail soft: unhealthy, disarm (restores the duck), release the input.
fn fail(shared: &Shared, reason: &str) {
    {
        let mut state = lock(&shared.arm);
        if shared.healthy.swap(false, Ordering::SeqCst) {
            log::warn!("barge-in monitor disabled for this session: {reason}");
        }
        disarm_locked(shared, &mut state);
    }
    close_stream(shared);
}

/// Closes the input stream at most once.
fn close_stream(shared: &Shared) {
    let stream = lock(&shared.stream).take();
    if let Some(stream) = stream {
        stream.close();
    }
}

fn apply_events(shared: &Arc<Shared>, armed: &mut Armed, now: u64, events: Vec<PolicyEvent>) {
    for event in events {
        match event {
            PolicyEvent::Duck => {
                shared.effects.duck();
                armed.ducked = true;
                armed.last_duck_ms = now;
            }
            PolicyEvent::Restore => {
                shared.effects.restore();
                armed.ducked = false;
            }
            PolicyEvent::CancelReply => {
                // Order matters (DESIGN 12.7.C): admissions closed first, then the latch, then the
                // token (the loop reads the latch as soon as the token fires), then playback stops,
                // then the volume returns so the next reply is not ducked.
                armed.gate.close();
                armed.latched.store(true, Ordering::SeqCst);
                armed.reply_tx.send_replace(true);
                shared.effects.stop_speech();
                shared.effects.restore();
                armed.ducked = false;
            }
            PolicyEvent::Done(samples) => {
                // DESIGN Rev 10 (a): an overflowed clip is never classified.
                if !armed.discard {
                    spawn_clip(shared, armed, samples);
                }
            }
        }
    }
}

/// Refreshes the duck while Ducked so the 3 s watchdog in the sink stays alive.
fn heartbeat(shared: &Shared, armed: &mut Armed, now: u64) {
    if armed.ducked
        && armed.policy.state() == PolicyState::Ducked
        && now.saturating_sub(armed.last_duck_ms) >= shared.heartbeat_ms
    {
        shared.effects.duck();
        armed.last_duck_ms = now;
    }
}

fn spawn_clip(shared: &Arc<Shared>, armed: &mut Armed, samples: Vec<f32>) {
    let id = armed.id;
    let clip_shared = Arc::clone(shared);
    let spawned = thread::Builder::new()
        .name("barge-clip".to_string())
        .spawn(move || {
            let result = classify_clip(&clip_shared, samples);
            deliver(&clip_shared, id, result);
        });
    match spawned {
        Ok(handle) => {
            let mut clips = lock(&shared.clips);
            clips.retain(|clip| !clip.is_finished());
            clips.push(handle);
        }
        Err(err) => {
            log::warn!("barge-in clip thread failed to start: {err}");
            // Already holding the arm lock: deliver directly, never re-lock.
            if let Some(tx) = armed.capture_tx.take() {
                let _ = tx.send(BargeCapture::NoSpeech);
            }
        }
    }
}

/// DESIGN Rev 10 (d): joins the live clip threads within a TOTAL `bound`. Threads still running
/// at the deadline are detached; their results are discarded (the monitor is disarmed).
fn join_clips(shared: &Shared, bound: Duration) {
    let mut pending = std::mem::take(&mut *lock(&shared.clips));
    let deadline = Instant::now() + bound;
    loop {
        let mut running = Vec::new();
        for handle in pending {
            if handle.is_finished() {
                let _ = handle.join();
            } else {
                running.push(handle);
            }
        }
        if running.is_empty() || Instant::now() >= deadline {
            return;
        }
        pending = running;
        thread::sleep(Duration::from_millis(5));
    }
}

fn classify_clip(shared: &Shared, samples: Vec<f32>) -> BargeCapture {
    if let Some(gate) = shared.gate.as_ref() {
        let accepted = catch_unwind(AssertUnwindSafe(|| gate.accepts(&samples))).unwrap_or(false);
        if !accepted {
            return BargeCapture::NoSpeech;
        }
    }
    let Ok(future) = catch_unwind(AssertUnwindSafe(|| shared.transcriber.transcribe(samples)))
    else {
        return BargeCapture::NoSpeech;
    };
    match catch_unwind(AssertUnwindSafe(|| block_on(future))) {
        Ok(Ok(text)) => {
            let text = text.trim();
            if text.is_empty() {
                BargeCapture::NoSpeech
            } else {
                BargeCapture::Utterance(text.to_string())
            }
        }
        _ => BargeCapture::NoSpeech,
    }
}

/// Hands a clip result to the watch of generation `id`, if that generation is still armed.
fn deliver(shared: &Shared, id: u64, result: BargeCapture) {
    let mut state = lock(&shared.arm);
    if let Some(armed) = state.slot.as_mut() {
        if armed.id == id {
            if let Some(tx) = armed.capture_tx.take() {
                let _ = tx.send(result);
            }
        }
    }
}

struct ThreadWaker(thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Minimal executor for the blocking clip thread (no async runtime there).
fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => thread::park(),
        }
    }
}
