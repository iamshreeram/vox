//! CaptureWorker: the ONLY starter/stopper/canceller of the session's
//! `converse` recording. One thread owns the recorder port; the audio
//! callback only enqueues levels (never blocks, never stops or transcribes).
//! No tauri types: hardware is behind ports. Contract: DESIGN.md 3.4 and the
//! AMENDMENTS section (Revision 4).

use crate::managers::converse::{ListenFailure, ListenOutcome};
use crate::managers::converse_audio::{EndpointConfig, EndpointEvent, Endpointer, SmartTurnPolicy};
use crate::managers::converse_latency::{EndpointMode, ListenTiming};
use crate::managers::smart_turn::{TurnDetector, DECISION_DEADLINE, DONE_THRESHOLD, RING_SAMPLES};
use crate::managers::speaker_id::{GateResult, SpeakerGate, Verdict};
use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Condvar, Mutex, PoisonError, TryLockError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartError {
    Busy,
    NoMicrophone,
    Permission,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscribeFailure;

/// Pre-VAD frame tap installed via [`RecorderPort::set_raw_tap`].
pub type RawTap = Arc<dyn Fn(&[f32]) + Send + Sync>;

/// Hardware port. Real impl: AudioRecordingManager with binding "converse".
pub trait RecorderPort: Send + Sync + 'static {
    /// Begin recording. Only ever succeeds when the recorder was idle.
    fn start(&self) -> Result<(), StartError>;
    /// Stop OUR recording and return the samples (None if there is none).
    fn stop(&self) -> Option<Vec<f32>>;
    /// Discard OUR recording; must never touch a foreign recording.
    fn cancel(&self);
    /// Smart Turn (T9b): install (`Some`) or clear (`None`) a PRE-VAD tap that receives
    /// every post-resample 16 kHz frame, speech or not. Default: no-op, so ports without
    /// a pre-VAD hook are unchanged (their detector tail stays empty).
    fn set_raw_tap(&self, _tap: Option<RawTap>) {}
}

pub trait Transcriber: Send + Sync + 'static {
    fn transcribe(&self, samples: Vec<f32>) -> Result<String, TranscribeFailure>;
}

#[derive(Debug, Clone, Copy)]
pub struct CaptureTimings {
    /// Pause between `start()` attempts while the recorder is busy.
    pub busy_retry: Duration,
    /// Give up (Failed(Busy)) after this long of continuous Busy.
    pub busy_deadline: Duration,
    /// No levels within this long of a successful start => Failed(NoMicrophone).
    pub readiness_deadline: Duration,
    /// Absolute deadline = initial_grace + max_utterance + this slack => Failed(Other).
    pub abs_slack: Duration,
    /// Worker wake-up period for watchdogs (recv timeout).
    pub tick: Duration,
}

impl Default for CaptureTimings {
    fn default() -> Self {
        CaptureTimings {
            busy_retry: Duration::from_millis(300),
            busy_deadline: Duration::from_secs(5),
            readiness_deadline: Duration::from_millis(3000),
            abs_slack: Duration::from_millis(3000),
            tick: Duration::from_millis(50),
        }
    }
}

/// Optional voice features of one worker. `Default` (both None) is today's behaviour.
#[derive(Clone, Default)]
pub struct CaptureOptions {
    /// Smart Turn end-of-turn detector. When Some, the endpointer runs the Smart Turn policy.
    pub turn_detector: Option<Arc<dyn TurnDetector>>,
    /// Speaker filter, run after `stop()` and before transcription.
    pub speaker_gate: Option<Arc<dyn SpeakerGate>>,
}

/// Cloneable handle to the worker thread. Implementers may change the private
/// fields freely; the public methods are the contract.
#[derive(Clone)]
pub struct CaptureHandle {
    inner: Arc<Inner>,
}

struct Inner {
    tx: SyncSender<Command>,
    /// Id of the armed listen (0 = none). Read by the audio thread.
    armed: AtomicU64,
    next_id: AtomicU64,
    // Read only by the test-only `CaptureHandle::capture_active` accessor.
    #[cfg(test)]
    capture_active: Arc<AtomicBool>,
    /// Highest cancelled listen id. Raised by the caller before the Cancel is
    /// queued, so the worker sees it even while `start()` is blocked.
    floor: Arc<AtomicU64>,
    join: Mutex<Option<JoinHandle<()>>>,
    /// Counters and the A7 timing slot, shared with the worker and its helpers.
    #[cfg_attr(not(test), allow(dead_code))]
    shared: Arc<Shared>,
}

/// State shared between the worker, its helper threads and the handle.
#[derive(Default)]
struct Shared {
    /// Pre-VAD taps installed (cumulative, one per listen with a detector).
    #[cfg_attr(not(test), allow(dead_code))]
    tap_installed: AtomicU64,
    /// Samples currently held by the pre-VAD ring.
    #[cfg_attr(not(test), allow(dead_code))]
    ring_len: AtomicU64,
    #[cfg_attr(not(test), allow(dead_code))]
    ring_allocated: AtomicBool,
    /// Decision helper threads created (cumulative).
    #[cfg_attr(not(test), allow(dead_code))]
    decider_spawned: AtomicU64,
    /// Decision helper threads that have not finished yet.
    #[cfg_attr(not(test), allow(dead_code))]
    decider_live: AtomicU64,
    /// T9b (A7): timing of the last listen that ended, keyed by listen id.
    timing_slot: Mutex<Option<(u64, ListenTiming)>>,
}

impl Inner {
    fn cancel_upto(&self, upto: u64) {
        self.floor.fetch_max(upto, Ordering::SeqCst);
        let _ = self.tx.send(Command::Cancel { upto });
    }
}

/// Bounded queue between callers and the single worker thread.
const QUEUE_CAPACITY: usize = 1024;

/// A VAD speech frame keeps speech "active" for this long. Speech frames
/// arrive about every 30 ms, so this is more than 4 frames of slack.
const SPEECH_GAP_MS: u64 = 150;

/// Pre-VAD ring (T9b): the newest RING_SAMPLES of raw 16 kHz audio. The recorder's
/// tap writes it with `try_lock` only (a contended frame is dropped and counted);
/// the worker reads and clears it.
struct Ring {
    state: Mutex<RingState>,
    dropped: AtomicU64,
    shared: Arc<Shared>,
}

struct RingState {
    samples: VecDeque<f32>,
    /// False once the listen ended: late frames are ignored.
    active: bool,
}

impl Ring {
    fn new(shared: Arc<Shared>) -> Ring {
        Ring {
            state: Mutex::new(RingState {
                samples: VecDeque::with_capacity(RING_SAMPLES),
                active: true,
            }),
            dropped: AtomicU64::new(0),
            shared,
        }
    }

    /// Audio-thread entry point: never blocks, never grows past RING_SAMPLES.
    fn push(&self, frame: &[f32]) {
        let mut guard = match self.state.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        if !guard.active {
            return;
        }
        let frame = &frame[frame.len().saturating_sub(RING_SAMPLES)..];
        let state = &mut *guard;
        let overflow = (state.samples.len() + frame.len()).saturating_sub(RING_SAMPLES);
        state.samples.drain(..overflow);
        state.samples.extend(frame.iter().copied());
        self.shared
            .ring_len
            .store(state.samples.len() as u64, Ordering::SeqCst);
    }

    /// The newest audio (at most RING_SAMPLES), copied for the decider.
    fn tail(&self) -> Vec<f32> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.samples.iter().copied().collect()
    }

    /// Ends the ring: later frames are ignored and the samples are released.
    fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.active = false;
        state.samples.clear();
        self.shared.ring_len.store(0, Ordering::SeqCst);
        drop(state);
        let dropped = self.dropped.load(Ordering::Relaxed);
        if dropped > 0 {
            log::debug!("converse pre-VAD ring: {dropped} frames skipped under contention");
        }
    }
}

/// A per-frame callback (the pre-VAD tap).
type FrameTap = Arc<dyn Fn(&[f32]) + Send + Sync>;

/// The callback the recorder runs for every 16 kHz frame. A panic is contained here so
/// a bad tap cannot reach the audio thread.
fn raw_tap(ring: Arc<Ring>) -> FrameTap {
    Arc::new(move |frame: &[f32]| {
        let _ = catch_unwind(AssertUnwindSafe(|| ring.push(frame)));
    })
}

/// Single-slot hand-off between the worker and one listen's decision helper.
struct DeciderShared {
    state: Mutex<DeciderState>,
    wake: Condvar,
}

#[derive(Default)]
struct DeciderState {
    /// The one request not yet taken by the helper (a newer one replaces it).
    request: Option<(u32, Vec<f32>)>,
    /// The last answer: (pause id, probability or None when unavailable).
    reply: Option<(u32, Option<f32>)>,
    /// Set when the listen ended: the helper exits once no request is pending.
    closed: bool,
}

/// Per-listen helper thread that runs the Smart Turn detector off the tick loop.
struct Decider {
    shared: Arc<DeciderShared>,
    cancel: Arc<AtomicBool>,
}

/// Decrements the live-helper count when the helper thread exits, on every path.
struct LiveGuard(Arc<Shared>);

impl Drop for LiveGuard {
    fn drop(&mut self) {
        self.0.decider_live.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Decider {
    /// Starts the helper. None when the thread cannot start (the caller then treats
    /// every answer as unavailable).
    fn spawn(detector: Arc<dyn TurnDetector>, telemetry: &Arc<Shared>) -> Option<Decider> {
        let shared = Arc::new(DeciderShared {
            state: Mutex::new(DeciderState::default()),
            wake: Condvar::new(),
        });
        let cancel = Arc::new(AtomicBool::new(false));
        telemetry.decider_live.fetch_add(1, Ordering::SeqCst);
        let live = LiveGuard(Arc::clone(telemetry));
        let thread_shared = Arc::clone(&shared);
        let thread_cancel = Arc::clone(&cancel);
        let spawned = std::thread::Builder::new()
            .name("converse-decider".into())
            .spawn(move || {
                let _live = live;
                decider_loop(detector.as_ref(), &thread_shared, &thread_cancel);
            });
        match spawned {
            Ok(_) => {
                telemetry.decider_spawned.fetch_add(1, Ordering::SeqCst);
                Some(Decider { shared, cancel })
            }
            Err(_) => None,
        }
    }

    /// Hands one request over, replacing an unanswered one. Never blocks.
    fn submit(&self, pause_id: u32, tail: Vec<f32>) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.request = Some((pause_id, tail));
        drop(state);
        self.shared.wake.notify_one();
    }

    /// The answer for `pause_id` if the helper has produced it (taken once).
    fn take_reply(&self, pause_id: u32) -> Option<Option<f32>> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let reply = state.reply;
        match reply {
            Some((id, answer)) if id == pause_id => {
                state.reply = None;
                Some(answer)
            }
            _ => None,
        }
    }
}

impl Drop for Decider {
    /// Ends the helper: it exits after the detector returns (the detector is told to
    /// cancel). Never joins, so the tick loop cannot wait on it.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.closed = true;
        drop(state);
        self.shared.wake.notify_all();
    }
}

/// Helper body: runs each request through the detector, containing panics (an
/// unavailable answer). A request taken before the listen ended is still answered.
fn decider_loop(detector: &dyn TurnDetector, shared: &DeciderShared, cancel: &AtomicBool) {
    loop {
        let (pause_id, tail) = {
            let mut state = shared.state.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if let Some(request) = state.request.take() {
                    break request;
                }
                if state.closed {
                    return;
                }
                state = shared
                    .wake
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let answer = catch_unwind(AssertUnwindSafe(|| {
            detector.done_probability(&tail, DECISION_DEADLINE, cancel)
        }))
        .ok()
        .flatten();
        let mut state = shared.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.reply = Some((pause_id, answer));
    }
}

/// Smart Turn decision the endpointer is waiting for.
#[derive(Clone, Copy)]
struct Outstanding {
    pause_id: u32,
    issued: Instant,
}

enum Command {
    Listen {
        listen_id: u64,
        cfg: EndpointConfig,
        reply: oneshot::Sender<ListenOutcome>,
    },
    // Constructed by push_levels, which the audio level hook will call once
    // audio.rs forwards levels (not yet wired).
    #[allow(dead_code)]
    Levels {
        listen_id: u64,
        now_ms: u64,
        levels: Vec<f32>,
    },
    /// One VAD-kept (speech) audio frame was seen for `listen_id`.
    Speech {
        listen_id: u64,
        now_ms: u64,
    },
    Cancel {
        upto: u64,
    },
    Shutdown {
        ack: mpsc::Sender<()>,
    },
}

/// Dropping an unsettled listen future cancels its listen id and disarms it.
struct ListenGuard {
    inner: Arc<Inner>,
    id: u64,
    settled: bool,
}

impl Drop for ListenGuard {
    fn drop(&mut self) {
        if !self.settled {
            self.inner.cancel_upto(self.id);
        }
        let _ = self
            .inner
            .armed
            .compare_exchange(self.id, 0, Ordering::SeqCst, Ordering::SeqCst);
    }
}

struct Pending {
    id: u64,
    cfg: EndpointConfig,
    reply: oneshot::Sender<ListenOutcome>,
    busy_since: Instant,
    next_at: Instant,
}

/// Identity and diagnostics of one listen, carried to its end.
struct Meta {
    id: u64,
    reply: oneshot::Sender<ListenOutcome>,
    started_at: Instant,
    /// Number of VAD speech frames accepted for this listen (diagnostic).
    speech_frames: u32,
}

struct Recording {
    meta: Meta,
    abs_deadline: Duration,
    endpointer: Endpointer,
    heard: bool,
    /// Latest VAD speech frame time for this listen (None until one arrives).
    last_speech_ms: Option<u64>,
    /// Smart Turn: the pre-VAD ring (None without a detector).
    ring: Option<Arc<Ring>>,
    /// Smart Turn: the pre-VAD tap is installed on the port for this listen.
    tap_on: bool,
    /// Smart Turn: the decision helper, spawned at most once per listen.
    decider: Option<Decider>,
    decider_tried: bool,
    /// Smart Turn: the decision waiting for an answer.
    outstanding: Option<Outstanding>,
    /// A decision was asked for in this listen (timing mode).
    decision_asked: bool,
    /// A real answer (complete or not) reached the endpointer (timing mode).
    answered: bool,
}

/// A speaker check running on its helper thread. The utterance waits for it.
struct Gating {
    meta: Meta,
    endpoint_wait_ms: Option<u32>,
    mode: EndpointMode,
    cancel: Arc<AtomicBool>,
    result: Arc<Mutex<Option<GateReply>>>,
}

/// What the gate helper hands back: the samples (for transcription) and the verdict.
/// `result` is None when the gate panicked, which counts as Accept.
struct GateReply {
    samples: Vec<f32>,
    result: Option<GateResult>,
}

enum State {
    Idle,
    Waiting(Pending),
    Recording(Recording),
    Gating(Gating),
}

struct Worker {
    port: Arc<dyn RecorderPort>,
    transcriber: Arc<dyn Transcriber>,
    timings: CaptureTimings,
    capture_active: Arc<AtomicBool>,
    floor: Arc<AtomicU64>,
    state: State,
    /// T9b: Smart Turn detector (None = fixed endpoint only).
    turn_detector: Option<Arc<dyn TurnDetector>>,
    /// T9b: speaker gate (None = no filter).
    speaker_gate: Option<Arc<dyn SpeakerGate>>,
    shared: Arc<Shared>,
}

pub struct CaptureWorker;

impl CaptureWorker {
    /// Spawn the worker thread with default options (test helper; production
    /// uses `spawn_with_options`).
    #[cfg(test)]
    pub fn spawn(
        port: Arc<dyn RecorderPort>,
        transcriber: Arc<dyn Transcriber>,
        timings: CaptureTimings,
    ) -> CaptureHandle {
        Self::spawn_with_options(port, transcriber, timings, CaptureOptions::default())
    }

    /// Spawn the worker thread with the optional voice features (T9b contract).
    pub fn spawn_with_options(
        port: Arc<dyn RecorderPort>,
        transcriber: Arc<dyn Transcriber>,
        timings: CaptureTimings,
        options: CaptureOptions,
    ) -> CaptureHandle {
        let (tx, rx) = mpsc::sync_channel::<Command>(QUEUE_CAPACITY);
        let capture_active = Arc::new(AtomicBool::new(false));
        let floor = Arc::new(AtomicU64::new(0));
        let shared = Arc::new(Shared::default());
        let worker = Worker {
            port,
            transcriber,
            timings,
            capture_active: Arc::clone(&capture_active),
            floor: Arc::clone(&floor),
            state: State::Idle,
            turn_detector: options.turn_detector,
            speaker_gate: options.speaker_gate,
            shared: Arc::clone(&shared),
        };
        // If the thread cannot be spawned, `rx` is dropped with the closure, so
        // every later command sees a closed channel and listens resolve Aborted.
        let join = std::thread::Builder::new()
            .name("converse-capture".into())
            .spawn(move || worker.run(rx))
            .ok();
        CaptureHandle {
            inner: Arc::new(Inner {
                tx,
                armed: AtomicU64::new(0),
                next_id: AtomicU64::new(0),
                #[cfg(test)]
                capture_active,
                floor,
                join: Mutex::new(join),
                shared,
            }),
        }
    }
}

impl Worker {
    fn run(mut self, rx: mpsc::Receiver<Command>) {
        let ack = loop {
            match rx.recv_timeout(self.timings.tick) {
                Ok(cmd) => {
                    if let Some(ack) = self.handle(cmd) {
                        break Some(ack);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break None,
            }
            self.poll();
        };
        self.drop_state();
        self.set_inactive();
        if let Some(ack) = ack {
            let _ = ack.send(());
        }
    }

    /// Returns the shutdown ack when the loop must exit.
    fn handle(&mut self, cmd: Command) -> Option<mpsc::Sender<()>> {
        match cmd {
            Command::Listen {
                listen_id,
                cfg,
                reply,
            } => {
                // A stale listen's outcome is written before the new listen clears the slot.
                self.drop_state();
                self.clear_timing();
                if listen_id <= self.floor.load(Ordering::SeqCst) {
                    let _ = reply.send(ListenOutcome::Aborted);
                } else {
                    let cfg = self.with_policy(cfg);
                    let now = Instant::now();
                    self.attempt(Pending {
                        id: listen_id,
                        cfg,
                        reply,
                        busy_since: now,
                        next_at: now,
                    });
                }
            }
            Command::Levels {
                listen_id,
                now_ms,
                levels,
            } => self.on_levels(listen_id, now_ms, &levels),
            Command::Speech { listen_id, now_ms } => self.on_speech(listen_id, now_ms),
            Command::Cancel { upto } => {
                self.cancel_upto(upto);
            }
            Command::Shutdown { ack } => return Some(ack),
        }
        None
    }

    /// Smart Turn: with a detector, the endpoint runs the Smart Turn policy.
    fn with_policy(&self, cfg: EndpointConfig) -> EndpointConfig {
        if self.turn_detector.is_some() {
            EndpointConfig {
                smart_turn: Some(SmartTurnPolicy::default_policy()),
                ..cfg
            }
        } else {
            cfg
        }
    }

    /// One `start()` attempt for `p`. Amendment A1: the capture flag is raised
    /// BEFORE `start()` so a dictation start that checks it after this point is
    /// refused (a start that already passed its check loses harmlessly at the
    /// recorder, which only starts from Idle). Every non-success path lowers it
    /// again, so a foreign (Busy) recording never leaves us marked active.
    fn attempt(&mut self, p: Pending) {
        self.capture_active.store(true, Ordering::SeqCst);
        match self.port.start() {
            Ok(()) => {
                if self.floor.load(Ordering::SeqCst) >= p.id {
                    // Cancelled while start() was blocked: release it now.
                    self.port.cancel();
                    self.set_inactive();
                    let _ = p.reply.send(ListenOutcome::Aborted);
                } else {
                    let mut recording = Recording {
                        meta: Meta {
                            id: p.id,
                            reply: p.reply,
                            started_at: Instant::now(),
                            speech_frames: 0,
                        },
                        abs_deadline: Duration::from_millis(
                            p.cfg
                                .initial_grace_ms
                                .saturating_add(p.cfg.max_utterance_ms),
                        ) + self.timings.abs_slack,
                        endpointer: Endpointer::new(p.cfg),
                        heard: false,
                        last_speech_ms: None,
                        ring: None,
                        tap_on: false,
                        decider: None,
                        decider_tried: false,
                        outstanding: None,
                        decision_asked: false,
                        answered: false,
                    };
                    if self.turn_detector.is_some() {
                        self.install_tap(&mut recording);
                    }
                    self.state = State::Recording(recording);
                }
            }
            Err(StartError::Busy) => {
                self.set_inactive();
                if p.busy_since.elapsed() >= self.timings.busy_deadline {
                    let _ = p.reply.send(ListenOutcome::Failed(ListenFailure::Busy));
                } else {
                    let next_at = Instant::now() + self.timings.busy_retry;
                    self.state = State::Waiting(Pending { next_at, ..p });
                }
            }
            Err(StartError::NoMicrophone) => {
                self.set_inactive();
                let _ = p
                    .reply
                    .send(ListenOutcome::Failed(ListenFailure::NoMicrophone));
            }
            Err(StartError::Permission) => {
                self.set_inactive();
                let _ = p
                    .reply
                    .send(ListenOutcome::Failed(ListenFailure::Permission));
            }
            Err(StartError::Other) => {
                self.set_inactive();
                let _ = p.reply.send(ListenOutcome::Failed(ListenFailure::Other));
            }
        }
    }

    /// Smart Turn: allocates this listen's ring and installs the pre-VAD tap.
    fn install_tap(&self, r: &mut Recording) {
        let ring = Arc::new(Ring::new(Arc::clone(&self.shared)));
        self.shared.ring_allocated.store(true, Ordering::SeqCst);
        self.shared.tap_installed.fetch_add(1, Ordering::SeqCst);
        self.port.set_raw_tap(Some(raw_tap(Arc::clone(&ring))));
        r.ring = Some(ring);
        r.tap_on = true;
    }

    /// Timer work: busy retries, speaker checks and the readiness / absolute watchdogs.
    fn poll(&mut self) {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Idle => {}
            State::Waiting(p) => {
                if p.busy_since.elapsed() >= self.timings.busy_deadline {
                    let _ = p.reply.send(ListenOutcome::Failed(ListenFailure::Busy));
                } else if Instant::now() >= p.next_at {
                    self.attempt(p);
                } else {
                    self.state = State::Waiting(p);
                }
            }
            State::Gating(g) => self.poll_gate(g),
            State::Recording(r) => {
                let elapsed = r.meta.started_at.elapsed();
                if !r.heard && elapsed >= self.timings.readiness_deadline {
                    self.end_with_cancel(r, ListenOutcome::Failed(ListenFailure::NoMicrophone));
                } else if elapsed >= r.abs_deadline {
                    self.end_with_cancel(r, ListenOutcome::Failed(ListenFailure::Other));
                } else {
                    self.state = State::Recording(r);
                }
            }
        }
    }

    /// Records one VAD speech frame. Only the recording with this listen id
    /// accepts it; speech for any other (or no) listen is ignored.
    fn on_speech(&mut self, listen_id: u64, now_ms: u64) {
        if let State::Recording(r) = &mut self.state {
            if r.meta.id == listen_id {
                r.last_speech_ms = Some(match r.last_speech_ms {
                    Some(prev) => prev.max(now_ms),
                    None => now_ms,
                });
                r.meta.speech_frames = r.meta.speech_frames.saturating_add(1);
            }
        }
    }

    /// The endpointer is driven by VAD speech activity, not by level values:
    /// room tone on the visualizer scale is always above the level floor, so
    /// level-driven endpointing never detects the end of speech.
    fn on_levels(&mut self, listen_id: u64, now_ms: u64, _levels: &[f32]) {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Recording(mut r) if r.meta.id == listen_id => {
                r.heard = true;
                let speech_active = r
                    .last_speech_ms
                    .is_some_and(|t| now_ms.saturating_sub(t) <= SPEECH_GAP_MS);
                if speech_active {
                    // Speech resumed: an open pause and its answer are stale.
                    r.outstanding = None;
                }
                match r.endpointer.feed_activity(now_ms, speech_active) {
                    None => {
                        if self.apply_answer(&mut r, now_ms) {
                            self.finish_speech(r, now_ms);
                        } else {
                            self.state = State::Recording(r);
                        }
                    }
                    Some(EndpointEvent::NoSpeech) => {
                        self.end_with_cancel(r, ListenOutcome::NoSpeech)
                    }
                    Some(EndpointEvent::MaxLength) => {
                        self.end_with_cancel(r, ListenOutcome::TooLong)
                    }
                    Some(EndpointEvent::SpeechEnded) => self.finish_speech(r, now_ms),
                    Some(EndpointEvent::NeedTurnDecision { pause_id }) => {
                        self.ask_decision(&mut r, pause_id, now_ms);
                        self.state = State::Recording(r);
                    }
                }
            }
            other => self.state = other,
        }
    }

    /// Asks the helper about pause `pause_id`. The helper is spawned at most once per listen.
    fn ask_decision(&self, r: &mut Recording, pause_id: u32, now_ms: u64) {
        r.decision_asked = true;
        if r.decider.is_none() && !r.decider_tried {
            r.decider_tried = true;
            if let Some(detector) = self.turn_detector.as_ref() {
                r.decider = Decider::spawn(Arc::clone(detector), &self.shared);
            }
        }
        match r.decider.as_ref() {
            Some(decider) => {
                let tail = r.ring.as_ref().map(|ring| ring.tail()).unwrap_or_default();
                r.outstanding = Some(Outstanding {
                    pause_id,
                    issued: Instant::now(),
                });
                decider.submit(pause_id, tail);
            }
            None => {
                let _ = r.endpointer.answer_unavailable(pause_id, now_ms);
            }
        }
    }

    /// Applies the helper's answer (or the deadline lapse) to the endpointer. Returns
    /// true when the answer ends the utterance now.
    fn apply_answer(&self, r: &mut Recording, now_ms: u64) -> bool {
        let Some(outstanding) = r.outstanding else {
            return false;
        };
        let reply = r
            .decider
            .as_ref()
            .and_then(|decider| decider.take_reply(outstanding.pause_id));
        match reply {
            Some(answer) => {
                r.outstanding = None;
                match answer.filter(|p| p.is_finite()) {
                    Some(p) => {
                        r.answered = true;
                        matches!(
                            r.endpointer.answer_turn(
                                outstanding.pause_id,
                                p >= DONE_THRESHOLD,
                                now_ms
                            ),
                            Some(EndpointEvent::SpeechEnded)
                        )
                    }
                    None => {
                        let _ = r
                            .endpointer
                            .answer_unavailable(outstanding.pause_id, now_ms);
                        false
                    }
                }
            }
            None if outstanding.issued.elapsed() >= DECISION_DEADLINE => {
                r.outstanding = None;
                let _ = r
                    .endpointer
                    .answer_unavailable(outstanding.pause_id, now_ms);
                false
            }
            None => false,
        }
    }

    /// The utterance ended after VAD speech: stop the recorder, then either run the
    /// speaker gate (off-thread) or transcribe straight away.
    fn finish_speech(&mut self, mut r: Recording, now_ms: u64) {
        let samples = self.port.stop();
        self.release_listen(&mut r);
        self.set_inactive();
        let endpoint_wait_ms = r.last_speech_ms.map(|t| ms_u32(now_ms.saturating_sub(t)));
        let mode = endpoint_mode(&r);
        let meta = r.meta;
        match samples {
            None => self.conclude(
                meta,
                ListenOutcome::Aborted,
                ListenTiming {
                    endpoint_wait_ms,
                    transcribe_ms: None,
                    verify_ms: None,
                    mode,
                },
            ),
            Some(samples) => match self.speaker_gate.clone() {
                None => {
                    let (outcome, transcribe_ms) = self.transcribe(samples);
                    self.conclude(
                        meta,
                        outcome,
                        ListenTiming {
                            endpoint_wait_ms,
                            transcribe_ms,
                            verify_ms: None,
                            mode,
                        },
                    );
                }
                Some(gate) => self.start_gate(meta, samples, gate, endpoint_wait_ms, mode),
            },
        }
    }

    /// Runs the speaker gate on its own helper thread so cancel and the tick loop
    /// are never held by it.
    fn start_gate(
        &mut self,
        meta: Meta,
        samples: Vec<f32>,
        gate: Arc<dyn SpeakerGate>,
        endpoint_wait_ms: Option<u32>,
        mode: EndpointMode,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        let result: Arc<Mutex<Option<GateReply>>> = Arc::new(Mutex::new(None));
        let thread_cancel = Arc::clone(&cancel);
        let thread_result = Arc::clone(&result);
        let spawned = std::thread::Builder::new()
            .name("converse-speaker-gate".into())
            .spawn(move || {
                let verdict =
                    catch_unwind(AssertUnwindSafe(|| gate.check(&samples, &thread_cancel))).ok();
                *thread_result.lock().unwrap_or_else(PoisonError::into_inner) = Some(GateReply {
                    samples,
                    result: verdict,
                });
            });
        match spawned {
            Ok(_) => {
                self.state = State::Gating(Gating {
                    meta,
                    endpoint_wait_ms,
                    mode,
                    cancel,
                    result,
                })
            }
            Err(_) => {
                // No thread: the samples are gone with the closure, so the utterance
                // can be neither checked nor transcribed.
                self.conclude(
                    meta,
                    ListenOutcome::Failed(ListenFailure::Other),
                    ListenTiming {
                        endpoint_wait_ms,
                        transcribe_ms: None,
                        verify_ms: None,
                        mode,
                    },
                );
            }
        }
    }

    fn poll_gate(&mut self, g: Gating) {
        let reply = g
            .result
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        match reply {
            Some(reply) => self.finish_gate(g, reply),
            None => self.state = State::Gating(g),
        }
    }

    /// Reject => Ignored with no transcription; anything else is transcribed.
    /// A gate failure already returns Accept (its contract), and a panic counts as Accept.
    fn finish_gate(&mut self, g: Gating, reply: GateReply) {
        let GateReply { samples, result } = reply;
        let verify_ms = result.and_then(|r| r.verify_ms);
        let verdict = result.map_or(Verdict::Accept, |r| r.verdict);
        let (outcome, transcribe_ms) = if verdict == Verdict::Reject {
            (ListenOutcome::Ignored, None)
        } else {
            self.transcribe(samples)
        };
        self.conclude(
            g.meta,
            outcome,
            ListenTiming {
                endpoint_wait_ms: g.endpoint_wait_ms,
                transcribe_ms,
                verify_ms,
                mode: g.mode,
            },
        );
    }

    /// Transcribes the samples and returns the outcome with the transcription time.
    fn transcribe(&self, samples: Vec<f32>) -> (ListenOutcome, Option<u32>) {
        let started = Instant::now();
        let result = self.transcriber.transcribe(samples);
        let outcome = match result {
            Ok(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    ListenOutcome::NoSpeech
                } else {
                    ListenOutcome::Utterance(trimmed.to_owned())
                }
            }
            Err(_) => ListenOutcome::Failed(ListenFailure::ModelUnavailable),
        };
        (outcome, Some(duration_ms_u32(started.elapsed())))
    }

    fn cancel_upto(&mut self, upto: u64) {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Waiting(p) if p.id <= upto => {
                let _ = p.reply.send(ListenOutcome::Aborted);
            }
            State::Recording(r) if r.meta.id <= upto => {
                self.end_with_cancel(r, ListenOutcome::Aborted)
            }
            State::Gating(g) if g.meta.id <= upto => self.abort_gate(g),
            other => self.state = other,
        }
    }

    /// Ends whatever is pending, recording or checking, used on shutdown and new listens.
    fn drop_state(&mut self) {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Idle => {}
            State::Waiting(p) => {
                let _ = p.reply.send(ListenOutcome::Aborted);
            }
            State::Recording(r) => self.end_with_cancel(r, ListenOutcome::Aborted),
            State::Gating(g) => self.abort_gate(g),
        }
    }

    /// Aborts a speaker check in progress: the helper is told to stop, nothing is transcribed.
    fn abort_gate(&self, g: Gating) {
        g.cancel.store(true, Ordering::SeqCst);
        let timing = ListenTiming {
            endpoint_wait_ms: g.endpoint_wait_ms,
            transcribe_ms: None,
            verify_ms: None,
            mode: g.mode,
        };
        self.conclude(g.meta, ListenOutcome::Aborted, timing);
    }

    /// Cancels the recording this worker started and answers its listen.
    fn end_with_cancel(&mut self, mut r: Recording, outcome: ListenOutcome) {
        self.port.cancel();
        self.release_listen(&mut r);
        self.set_inactive();
        let timing = ListenTiming {
            endpoint_wait_ms: None,
            transcribe_ms: None,
            verify_ms: None,
            mode: endpoint_mode(&r),
        };
        self.conclude(r.meta, outcome, timing);
    }

    /// Releases the tap, the ring and the helper of one listen. Called on every end path.
    fn release_listen(&self, r: &mut Recording) {
        if r.tap_on {
            self.port.set_raw_tap(None);
            r.tap_on = false;
        }
        if let Some(ring) = r.ring.take() {
            ring.close();
            self.shared.ring_allocated.store(false, Ordering::SeqCst);
        }
        // Dropping the helper cancels it and lets it exit.
        r.decider = None;
        r.outstanding = None;
    }

    /// Ends one listen: the timing slot is written BEFORE the outcome is sent (A7).
    fn conclude(&self, meta: Meta, outcome: ListenOutcome, timing: ListenTiming) {
        log_listen_end(end_reason(&outcome), &meta);
        *self
            .shared
            .timing_slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some((meta.id, timing));
        let _ = meta.reply.send(outcome);
    }

    /// A7: the timing slot is cleared at the start of every listen.
    fn clear_timing(&self) {
        *self
            .shared
            .timing_slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn set_inactive(&self) {
        self.capture_active.store(false, Ordering::SeqCst);
    }
}

/// The endpoint mode of a listen: SmartTurn when an answer was applied, Fallback when a
/// decision was asked but never answered in time, Fixed otherwise.
fn endpoint_mode(r: &Recording) -> EndpointMode {
    if r.answered {
        EndpointMode::SmartTurn
    } else if r.decision_asked {
        EndpointMode::SmartTurnFallback
    } else {
        EndpointMode::Fixed
    }
}

/// Milliseconds as u32, saturating (timing numbers only).
fn ms_u32(ms: u64) -> u32 {
    u32::try_from(ms).unwrap_or(u32::MAX)
}

fn duration_ms_u32(d: Duration) -> u32 {
    u32::try_from(d.as_millis()).unwrap_or(u32::MAX)
}

/// Short, fixed reason for the end of a listen (diagnostic log only).
fn end_reason(outcome: &ListenOutcome) -> &'static str {
    match outcome {
        ListenOutcome::Utterance(_) => "speech_ended",
        ListenOutcome::NoSpeech => "no_speech",
        ListenOutcome::TooLong => "too_long",
        ListenOutcome::Aborted => "aborted",
        ListenOutcome::Failed(_) => "failed",
        ListenOutcome::Ignored => "ignored",
    }
}

/// F4 diagnostic: one numeric/enum-only line per ended listen. Never audio,
/// text, device names, ids or paths.
fn log_listen_end(reason: &str, meta: &Meta) {
    log::info!(
        "converse capture ended: reason={} dur_ms={} speech_frames={}",
        reason,
        meta.started_at.elapsed().as_millis(),
        meta.speech_frames
    );
}

impl CaptureHandle {
    /// One listen turn. Assigns the next listen id, asks the worker to record
    /// until `cfg` says the utterance is over, and resolves exactly once.
    /// Dropping the returned future (e.g. the session loop's cancel race)
    /// cancels that listen id (a drop guard), so a recording can never leak.
    /// A worker that is already gone => `Aborted`.
    pub async fn listen(&self, cfg: EndpointConfig) -> ListenOutcome {
        let inner = Arc::clone(&self.inner);
        let id = inner.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        inner.armed.store(id, Ordering::SeqCst);
        let mut guard = ListenGuard {
            inner: Arc::clone(&inner),
            id,
            settled: false,
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        if inner
            .tx
            .send(Command::Listen {
                listen_id: id,
                cfg,
                reply: reply_tx,
            })
            .is_err()
        {
            // Worker already gone: nothing to cancel.
            guard.settled = true;
            return ListenOutcome::Aborted;
        }
        match reply_rx.await {
            Ok(outcome) => {
                guard.settled = true;
                outcome
            }
            Err(_) => ListenOutcome::Aborted,
        }
    }

    /// Called from the audio callback thread: NON-BLOCKING (`try_send` on a
    /// bounded channel); a no-op unless a listen is armed; never stops,
    /// transcribes or waits.
    // Not called yet: audio.rs does not forward levels to the capture hook.
    #[allow(dead_code)]
    pub fn push_levels(&self, now_ms: u64, levels: &[f32]) {
        let listen_id = self.inner.armed.load(Ordering::SeqCst);
        if listen_id == 0 {
            return;
        }
        // Never blocks: a full queue drops the sample.
        let _ = self.inner.tx.try_send(Command::Levels {
            listen_id,
            now_ms,
            levels: levels.to_vec(),
        });
    }

    /// Called from the audio callback thread for each frame the VAD kept as
    /// speech: NON-BLOCKING, a no-op unless a listen is armed.
    pub fn push_speech(&self, now_ms: u64) {
        let listen_id = self.inner.armed.load(Ordering::SeqCst);
        if listen_id == 0 {
            return;
        }
        let _ = self
            .inner
            .tx
            .try_send(Command::Speech { listen_id, now_ms });
    }

    /// Cancel every listen (current and future ids) of this worker.
    // Not called yet by the session finalizer path.
    #[allow(dead_code)]
    pub fn cancel_all(&self) {
        self.inner.cancel_upto(u64::MAX);
    }

    /// True while the worker owns the recorder (between a successful
    /// `start()` and the end of that recording).
    #[cfg(test)]
    pub fn capture_active(&self) -> bool {
        self.inner.capture_active.load(Ordering::SeqCst)
    }

    /// Id of the most recent listen started on this handle (0 = none yet).
    // Used by the converse wiring (RealEars::take_timing); not yet reachable in lib builds.
    #[allow(dead_code)]
    pub fn last_listen_id(&self) -> u64 {
        self.inner.next_id.load(Ordering::SeqCst)
    }

    /// T9b test-only: number of pre-VAD taps the worker installed (cumulative).
    #[cfg(test)]
    pub fn tap_installed_count(&self) -> usize {
        self.inner.shared.tap_installed.load(Ordering::SeqCst) as usize
    }

    /// T9b test-only: samples currently held by the pre-VAD ring.
    #[cfg(test)]
    pub fn ring_len(&self) -> usize {
        self.inner.shared.ring_len.load(Ordering::SeqCst) as usize
    }

    /// T9b test-only: whether the pre-VAD ring buffer is allocated.
    #[cfg(test)]
    pub fn ring_allocated(&self) -> bool {
        self.inner.shared.ring_allocated.load(Ordering::SeqCst)
    }

    /// T9b test-only: decider (helper) threads created so far (cumulative).
    #[cfg(test)]
    pub fn decider_threads_spawned(&self) -> usize {
        self.inner.shared.decider_spawned.load(Ordering::SeqCst) as usize
    }

    /// T9b test-only: decider threads that have not finished yet.
    #[cfg(test)]
    pub fn decider_threads_live(&self) -> usize {
        self.inner.shared.decider_live.load(Ordering::SeqCst) as usize
    }

    /// T9b (A7): the timing of listen `listen_id` if it is the last listen that ended.
    /// Returns it only for the matching id and clears the slot (a second take is None).
    #[allow(dead_code)]
    pub fn take_timing(&self, listen_id: u64) -> Option<ListenTiming> {
        let mut slot = self
            .inner
            .shared
            .timing_slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let current = *slot;
        match current {
            Some((id, timing)) if id == listen_id => {
                *slot = None;
                Some(timing)
            }
            _ => None,
        }
    }

    /// Cancel any active recording, answer any pending listen with `Aborted`,
    /// stop the thread and JOIN it. Blocking. Idempotent.
    pub fn shutdown(&self) {
        let (ack_tx, ack_rx) = mpsc::channel();
        if self
            .inner
            .tx
            .send(Command::Shutdown { ack: ack_tx })
            .is_ok()
        {
            // Err means the worker dropped the message while exiting: also fine.
            let _ = ack_rx.recv();
        }
        let join = self
            .inner
            .join
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(handle) = join {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{mpsc, Mutex};
    use std::time::Instant;

    // ------------------------- fakes -------------------------

    #[derive(Default)]
    struct FakePort {
        active: AtomicBool,
        starts: AtomicUsize,
        stops: AtomicUsize,
        cancels: AtomicUsize,
        start_script: Mutex<VecDeque<Result<(), StartError>>>,
        /// When the script is empty: this result.
        start_default: Mutex<Option<Result<(), StartError>>>,
        stop_script: Mutex<VecDeque<Option<Vec<f32>>>>,
        /// If set, `start()` blocks until a message arrives.
        start_gate: Mutex<Option<mpsc::Receiver<()>>>,
    }
    impl FakePort {
        fn new() -> Arc<Self> {
            Arc::new(FakePort::default())
        }
        fn script_start(&self, items: Vec<Result<(), StartError>>) {
            self.start_script.lock().unwrap().extend(items);
        }
        fn set_default(&self, r: Result<(), StartError>) {
            *self.start_default.lock().unwrap() = Some(r);
        }
        fn is_active(&self) -> bool {
            self.active.load(Ordering::SeqCst)
        }
    }
    impl RecorderPort for FakePort {
        fn start(&self) -> Result<(), StartError> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = self.start_gate.lock().unwrap().take() {
                let _ = gate.recv();
            }
            let r = self
                .start_script
                .lock()
                .unwrap()
                .pop_front()
                .or(*self.start_default.lock().unwrap())
                .unwrap_or(Ok(()));
            if r.is_ok() {
                self.active.store(true, Ordering::SeqCst);
            }
            r
        }
        fn stop(&self) -> Option<Vec<f32>> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            let was = self.active.swap(false, Ordering::SeqCst);
            let scripted = self.stop_script.lock().unwrap().pop_front();
            match scripted {
                Some(v) => v,
                None if was => Some(vec![0.1; 160]),
                None => None,
            }
        }
        fn cancel(&self) {
            self.cancels.fetch_add(1, Ordering::SeqCst);
            self.active.store(false, Ordering::SeqCst);
        }
    }

    struct FakeTranscriber {
        script: Mutex<VecDeque<Result<String, TranscribeFailure>>>,
        calls: AtomicUsize,
        gate: Mutex<Option<mpsc::Receiver<()>>>,
    }
    impl FakeTranscriber {
        fn new() -> Arc<Self> {
            Arc::new(FakeTranscriber {
                script: Mutex::new(VecDeque::new()),
                calls: AtomicUsize::new(0),
                gate: Mutex::new(None),
            })
        }
    }
    impl Transcriber for FakeTranscriber {
        fn transcribe(&self, _samples: Vec<f32>) -> Result<String, TranscribeFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(g) = self.gate.lock().unwrap().take() {
                let _ = g.recv();
            }
            self.script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok("hello world".to_string()))
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

    const LOUD: &[f32] = &[0.0, 0.6, 0.2];
    const QUIET: &[f32] = &[0.0, 0.0, 0.0];

    fn spawn(port: &Arc<FakePort>, tr: &Arc<FakeTranscriber>) -> CaptureHandle {
        CaptureWorker::spawn(port.clone(), tr.clone(), fast())
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

    /// Speech of ~400 ms followed by enough silence to end the utterance.
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

    async fn listen_with_speech(h: &CaptureHandle, port: &Arc<FakePort>) -> ListenOutcome {
        let h2 = h.clone();
        let task = tokio::spawn(async move { h2.listen(cfg()).await });
        wait_for("recording start", || port.is_active()).await;
        speak_then_silence(h);
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("listen hung")
            .unwrap()
    }

    // ------------------------- happy path and outcomes -------------------------

    #[tokio::test]
    async fn happy_path_returns_the_transcript_and_releases_the_recorder() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        assert!(!h.capture_active());
        let h2 = h.clone();
        let task = tokio::spawn(async move { h2.listen(cfg()).await });
        wait_for("recording", || port.is_active()).await;
        assert!(h.capture_active());
        speak_then_silence(&h);
        let out = task.await.unwrap();
        assert_eq!(out, ListenOutcome::Utterance("hello world".into()));
        assert!(!port.is_active());
        assert_eq!(port.stops.load(Ordering::SeqCst), 1);
        wait_for("capture flag cleared", || !h.capture_active()).await;
        h.shutdown();
    }

    #[tokio::test]
    async fn transcript_is_trimmed_and_blank_means_no_speech() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        tr.script
            .lock()
            .unwrap()
            .extend([Ok("  spaced out \n".to_string()), Ok("   ".to_string())]);
        let h = spawn(&port, &tr);
        assert_eq!(
            listen_with_speech(&h, &port).await,
            ListenOutcome::Utterance("spaced out".into())
        );
        assert_eq!(listen_with_speech(&h, &port).await, ListenOutcome::NoSpeech);
        h.shutdown();
    }

    #[tokio::test]
    async fn transcriber_failure_is_model_unavailable() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        tr.script.lock().unwrap().push_back(Err(TranscribeFailure));
        let h = spawn(&port, &tr);
        assert_eq!(
            listen_with_speech(&h, &port).await,
            ListenOutcome::Failed(ListenFailure::ModelUnavailable)
        );
        assert!(!port.is_active());
        h.shutdown();
    }

    #[tokio::test]
    async fn missing_samples_after_speech_is_aborted() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        port.stop_script.lock().unwrap().push_back(None);
        let h = spawn(&port, &tr);
        assert_eq!(listen_with_speech(&h, &port).await, ListenOutcome::Aborted);
        assert_eq!(tr.calls.load(Ordering::SeqCst), 0);
        h.shutdown();
    }

    #[tokio::test]
    async fn silence_only_is_no_speech_without_transcribing() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let h2 = h.clone();
        let task = tokio::spawn(async move { h2.listen(cfg()).await });
        wait_for("recording", || port.is_active()).await;
        let mut t = 0u64;
        while t <= 700 {
            h.push_levels(t, QUIET);
            t += 50;
        }
        assert_eq!(task.await.unwrap(), ListenOutcome::NoSpeech);
        assert_eq!(
            tr.calls.load(Ordering::SeqCst),
            0,
            "silence must never be transcribed"
        );
        assert!(!port.is_active());
        assert_eq!(port.cancels.load(Ordering::SeqCst), 1);
        h.shutdown();
    }

    #[tokio::test]
    async fn endless_speech_is_too_long_and_never_transcribed() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let h2 = h.clone();
        let mut c = cfg();
        c.max_utterance_ms = 500;
        let task = tokio::spawn(async move { h2.listen(c).await });
        wait_for("recording", || port.is_active()).await;
        let mut t = 0u64;
        while t <= 600 {
            h.push_speech(t);
            h.push_levels(t, LOUD);
            t += 50;
        }
        assert_eq!(task.await.unwrap(), ListenOutcome::TooLong);
        assert_eq!(tr.calls.load(Ordering::SeqCst), 0);
        assert!(!port.is_active());
        h.shutdown();
    }

    #[tokio::test]
    async fn sequential_listens_each_get_exactly_one_reply() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        tr.script
            .lock()
            .unwrap()
            .extend([Ok("one".to_string()), Ok("two".to_string())]);
        let h = spawn(&port, &tr);
        assert_eq!(
            listen_with_speech(&h, &port).await,
            ListenOutcome::Utterance("one".into())
        );
        assert_eq!(
            listen_with_speech(&h, &port).await,
            ListenOutcome::Utterance("two".into())
        );
        assert_eq!(port.starts.load(Ordering::SeqCst), 2);
        h.shutdown();
    }

    #[tokio::test]
    async fn late_levels_after_the_reply_are_ignored() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        assert_eq!(
            listen_with_speech(&h, &port).await,
            ListenOutcome::Utterance("hello world".into())
        );
        for t in 0..20u64 {
            h.push_speech(2000 + t * 50);
            h.push_levels(2000 + t * 50, LOUD);
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(port.stops.load(Ordering::SeqCst), 1);
        assert_eq!(port.cancels.load(Ordering::SeqCst), 0);
        h.shutdown();
    }

    // ------------------- VAD speech activity (design rev 2) -------------------
    //
    // Root cause being pinned: the display-scale levels of a REAL room are far
    // above the 0.02 floor, so level-driven endpointing never saw silence and
    // every listen ran to the hard cap ("That was too long for me", forever).
    // Speech activity must come from the VAD-kept frames, not from level values.

    /// Room tone on the visualizer scale (about -48 dBFS maps to ~0.77).
    const AMBIENT: &[f32] = &[0.70, 0.77, 0.75];

    async fn start_listen(
        h: &CaptureHandle,
        port: &Arc<FakePort>,
    ) -> (tokio::task::JoinHandle<ListenOutcome>, u64) {
        let h2 = h.clone();
        let task = tokio::spawn(async move { h2.listen(cfg()).await });
        wait_for("recording start", || port.is_active()).await;
        wait_for("listen armed", || h.inner.armed.load(Ordering::SeqCst) != 0).await;
        let id = h.inner.armed.load(Ordering::SeqCst);
        (task, id)
    }

    fn inject_speech(h: &CaptureHandle, listen_id: u64, now_ms: u64) {
        let _ = h.inner.tx.try_send(Command::Speech { listen_id, now_ms });
    }

    /// VAD speech frames every 30 ms for 400 ms while the levels stay at room
    /// tone every 50 ms; levels stop at 890 ms = last speech (390) +
    /// SPEECH_GAP_MS (150) + silence_ms (300) + one levels period (50), so the
    /// utterance MUST complete within that bound (design C3).
    async fn drive_speech_then_gap(send_speech: &dyn Fn(u64), h: &CaptureHandle) {
        // (time, kind): kind 0 = VAD speech frame, 1 = levels tick. Sorted, so a
        // speech frame at the same instant is delivered before the levels tick.
        let mut events: Vec<(u64, u8)> = (0..400u64)
            .step_by(30)
            .map(|t| (t, 0u8))
            .chain((0..=890u64).step_by(50).map(|t| (t, 1u8)))
            .collect();
        events.sort();
        for (i, (t, kind)) in events.iter().enumerate() {
            if *kind == 0 {
                send_speech(*t);
            } else {
                h.push_levels(*t, AMBIENT);
            }
            if i % 10 == 9 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
    }

    async fn finish(task: tokio::task::JoinHandle<ListenOutcome>) -> ListenOutcome {
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("listen hung: end of speech was never detected")
            .unwrap()
    }

    #[tokio::test]
    async fn high_ambient_levels_without_speech_frames_give_no_speech_not_too_long() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let (task, _id) = start_listen(&h, &port).await;
        // Past max_utterance_ms (5000): level-driven code reports TooLong here.
        let mut t = 0u64;
        let mut n = 0u32;
        while t <= 5600 {
            h.push_levels(t, AMBIENT);
            n += 1;
            if n % 10 == 9 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            t += 50;
        }
        assert_eq!(finish(task).await, ListenOutcome::NoSpeech);
        assert_eq!(tr.calls.load(Ordering::SeqCst), 0);
        h.shutdown();
    }

    #[tokio::test]
    async fn speech_frames_then_a_gap_finish_the_utterance_even_with_high_ambient_levels() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let (task, _id) = start_listen(&h, &port).await;
        let hh = h.clone();
        drive_speech_then_gap(&move |t| hh.push_speech(t), &h).await;
        assert_eq!(
            finish(task).await,
            ListenOutcome::Utterance("hello world".into())
        );
        assert_eq!(tr.calls.load(Ordering::SeqCst), 1);
        assert_eq!(port.stops.load(Ordering::SeqCst), 1);
        h.shutdown();
    }

    #[tokio::test]
    async fn speech_for_the_current_listen_id_is_accepted_via_the_tagged_command() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let (task, id) = start_listen(&h, &port).await;
        let hh = h.clone();
        drive_speech_then_gap(&move |t| inject_speech(&hh, id, t), &h).await;
        assert_eq!(
            finish(task).await,
            ListenOutcome::Utterance("hello world".into())
        );
        h.shutdown();
    }

    #[tokio::test]
    async fn speech_tagged_with_a_previous_listen_id_is_ignored() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        // Listen A completes normally.
        let (task_a, a_id) = start_listen(&h, &port).await;
        let hh = h.clone();
        drive_speech_then_gap(&move |t| hh.push_speech(t), &h).await;
        assert_eq!(
            finish(task_a).await,
            ListenOutcome::Utterance("hello world".into())
        );
        wait_for("capture flag cleared", || !h.capture_active()).await;
        // Listen B: only A-tagged speech arrives (a late notification), plus room tone.
        let (task_b, b_id) = start_listen(&h, &port).await;
        assert_ne!(a_id, b_id);
        for t in (0..400u64).step_by(30) {
            inject_speech(&h, a_id, t);
        }
        let mut t = 0u64;
        while t <= 700 {
            h.push_levels(t, AMBIENT);
            tokio::time::sleep(Duration::from_millis(1)).await;
            t += 50;
        }
        assert_eq!(finish(task_b).await, ListenOutcome::NoSpeech);
        assert_eq!(
            tr.calls.load(Ordering::SeqCst),
            1,
            "only listen A may be transcribed"
        );
        h.shutdown();
    }

    #[tokio::test]
    async fn speech_with_no_listen_armed_is_dropped_and_never_leaks_into_the_next_listen() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        h.push_speech(10);
        h.push_speech(20);
        tokio::time::sleep(Duration::from_millis(20)).await;
        let (task, _id) = start_listen(&h, &port).await;
        let mut t = 0u64;
        while t <= 700 {
            h.push_levels(t, AMBIENT);
            tokio::time::sleep(Duration::from_millis(1)).await;
            t += 50;
        }
        assert_eq!(finish(task).await, ListenOutcome::NoSpeech);
        assert_eq!(tr.calls.load(Ordering::SeqCst), 0);
        h.shutdown();
    }

    #[test]
    fn push_speech_never_blocks_even_when_nobody_is_listening_or_the_queue_is_full() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let (gate_tx, gate_rx) = mpsc::channel();
        *port.start_gate.lock().unwrap() = Some(gate_rx);
        let h = CaptureWorker::spawn(port.clone(), tr.clone(), fast());
        let started = Instant::now();
        for t in 0..10_000u64 {
            h.push_speech(t); // not armed: no-op
        }
        let h2 = h.clone();
        let jh = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                tokio::time::timeout(Duration::from_millis(300), h2.listen(cfg()))
                    .await
                    .is_err()
            })
        });
        std::thread::sleep(Duration::from_millis(50));
        for t in 0..10_000u64 {
            h.push_speech(t); // armed, worker blocked, queue fills: must still not block
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "push_speech blocked the caller"
        );
        gate_tx.send(()).unwrap();
        let _ = jh.join();
        h.shutdown();
    }

    // ------------------------- start errors -------------------------

    #[tokio::test]
    async fn permanent_start_errors_are_not_retried() {
        for (e, f) in [
            (StartError::NoMicrophone, ListenFailure::NoMicrophone),
            (StartError::Permission, ListenFailure::Permission),
            (StartError::Other, ListenFailure::Other),
        ] {
            let port = FakePort::new();
            port.set_default(Err(e));
            let tr = FakeTranscriber::new();
            let h = spawn(&port, &tr);
            assert_eq!(h.listen(cfg()).await, ListenOutcome::Failed(f));
            assert_eq!(
                port.starts.load(Ordering::SeqCst),
                1,
                "{e:?} must not be retried"
            );
            assert!(!h.capture_active());
            h.shutdown();
        }
    }

    #[tokio::test]
    async fn busy_is_retried_until_the_recorder_frees_up() {
        let port = FakePort::new();
        port.script_start(vec![Err(StartError::Busy), Err(StartError::Busy)]);
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        assert_eq!(
            listen_with_speech(&h, &port).await,
            ListenOutcome::Utterance("hello world".into())
        );
        assert_eq!(port.starts.load(Ordering::SeqCst), 3);
        h.shutdown();
    }

    #[tokio::test]
    async fn busy_forever_fails_busy_and_never_cancels_a_foreign_recording() {
        let port = FakePort::new();
        port.set_default(Err(StartError::Busy));
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let started = Instant::now();
        assert_eq!(
            h.listen(cfg()).await,
            ListenOutcome::Failed(ListenFailure::Busy)
        );
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert_eq!(port.cancels.load(Ordering::SeqCst), 0);
        h.shutdown();
        assert_eq!(
            port.cancels.load(Ordering::SeqCst),
            0,
            "shutdown must not cancel a recording we never owned"
        );
    }

    #[tokio::test]
    async fn cancel_honoured_at_a_busy_retry_boundary() {
        let port = FakePort::new();
        port.set_default(Err(StartError::Busy));
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let h2 = h.clone();
        let task = tokio::spawn(async move { h2.listen(cfg()).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        h.cancel_all();
        let started = Instant::now();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap(),
            ListenOutcome::Aborted
        );
        assert!(
            started.elapsed() < Duration::from_millis(120),
            "must not wait out the busy deadline"
        );
        assert_eq!(port.cancels.load(Ordering::SeqCst), 0);
        h.shutdown();
    }

    // ------------------------- cancellation / shutdown -------------------------

    #[tokio::test]
    async fn cancel_before_listen_never_starts_the_recorder() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        h.cancel_all();
        assert_eq!(h.listen(cfg()).await, ListenOutcome::Aborted);
        assert_eq!(port.starts.load(Ordering::SeqCst), 0);
        h.shutdown();
    }

    #[tokio::test]
    async fn dropping_the_listen_future_cancels_the_recording() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let h2 = h.clone();
        let task = tokio::spawn(async move { h2.listen(cfg()).await });
        wait_for("recording", || port.is_active()).await;
        task.abort(); // drops the future => drop guard cancels
        let _ = task.await;
        wait_for("recorder released", || !port.is_active()).await;
        assert_eq!(port.cancels.load(Ordering::SeqCst), 1);
        assert_eq!(tr.calls.load(Ordering::SeqCst), 0);
        // the worker is still usable afterwards
        assert_eq!(
            listen_with_speech(&h, &port).await,
            ListenOutcome::Utterance("hello world".into())
        );
        h.shutdown();
    }

    #[tokio::test]
    async fn cancel_while_start_is_blocked_releases_the_late_recording() {
        let port = FakePort::new();
        let (gate_tx, gate_rx) = mpsc::channel();
        *port.start_gate.lock().unwrap() = Some(gate_rx);
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let h2 = h.clone();
        let task = tokio::spawn(async move { h2.listen(cfg()).await });
        wait_for("start entered", || port.starts.load(Ordering::SeqCst) == 1).await;
        h.cancel_all();
        tokio::time::sleep(Duration::from_millis(20)).await;
        gate_tx.send(()).unwrap(); // start() now returns Ok => recorder becomes active
        let out = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out, ListenOutcome::Aborted);
        wait_for("late recording cancelled", || !port.is_active()).await;
        assert_eq!(port.cancels.load(Ordering::SeqCst), 1);
        assert!(!h.capture_active());
        h.shutdown();
    }

    #[tokio::test]
    async fn cancel_during_transcription_discards_the_result_and_the_worker_survives() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let (gate_tx, gate_rx) = mpsc::channel();
        *tr.gate.lock().unwrap() = Some(gate_rx);
        let h = spawn(&port, &tr);
        let h2 = h.clone();
        let task = tokio::spawn(async move { h2.listen(cfg()).await });
        wait_for("recording", || port.is_active()).await;
        speak_then_silence(&h);
        wait_for("transcribe entered", || {
            tr.calls.load(Ordering::SeqCst) == 1
        })
        .await;
        task.abort();
        let _ = task.await;
        gate_tx.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!port.is_active());
        assert_eq!(
            listen_with_speech(&h, &port).await,
            ListenOutcome::Utterance("hello world".into())
        );
        h.shutdown();
    }

    #[tokio::test]
    async fn shutdown_while_recording_cancels_and_aborts_the_pending_listen() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let h2 = h.clone();
        let task = tokio::spawn(async move { h2.listen(cfg()).await });
        wait_for("recording", || port.is_active()).await;
        let h3 = h.clone();
        tokio::task::spawn_blocking(move || h3.shutdown())
            .await
            .unwrap();
        assert!(
            !port.is_active(),
            "no converse recording may outlive shutdown"
        );
        assert!(!h.capture_active());
        assert_eq!(task.await.unwrap(), ListenOutcome::Aborted);
        assert_eq!(port.cancels.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn shutdown_is_idempotent_and_listen_after_shutdown_aborts() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let h2 = h.clone();
        tokio::task::spawn_blocking(move || {
            h2.shutdown();
            h2.shutdown();
        })
        .await
        .unwrap();
        assert_eq!(h.listen(cfg()).await, ListenOutcome::Aborted);
        assert_eq!(port.starts.load(Ordering::SeqCst), 0);
        // harmless after shutdown
        h.push_levels(0, LOUD);
        h.cancel_all();
    }

    // ------------------------- watchdogs -------------------------

    #[tokio::test]
    async fn no_levels_after_a_successful_start_fails_no_microphone() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let started = Instant::now();
        assert_eq!(
            h.listen(cfg()).await,
            ListenOutcome::Failed(ListenFailure::NoMicrophone)
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!port.is_active());
        assert_eq!(port.cancels.load(Ordering::SeqCst), 1);
        h.shutdown();
    }

    #[tokio::test]
    async fn absolute_deadline_ends_a_listen_whose_clock_never_advances() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let h2 = h.clone();
        let mut c = cfg();
        c.initial_grace_ms = 100;
        c.max_utterance_ms = 100;
        // abs deadline = 100 + 100 + 150ms slack = 350ms wall clock
        let task = tokio::spawn(async move { h2.listen(c).await });
        wait_for("recording", || port.is_active()).await;
        let feeder = {
            let h = h.clone();
            tokio::spawn(async move {
                // levels keep arriving (readiness satisfied) but now_ms is frozen
                for _ in 0..200 {
                    h.push_levels(0, LOUD);
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
        };
        let out = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        feeder.abort();
        assert_eq!(out, ListenOutcome::Failed(ListenFailure::Other));
        assert!(!port.is_active());
        h.shutdown();
    }

    // ------------------------- audio-thread safety -------------------------

    #[test]
    fn push_levels_never_blocks_even_when_nobody_is_listening_or_the_queue_is_full() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        // Worker blocked inside start() so nothing drains the channel.
        let (gate_tx, gate_rx) = mpsc::channel();
        *port.start_gate.lock().unwrap() = Some(gate_rx);
        let h = CaptureWorker::spawn(port.clone(), tr.clone(), fast());
        let started = Instant::now();
        for t in 0..10_000u64 {
            h.push_levels(t, LOUD); // not armed: no-op
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let h2 = h.clone();
        let jh = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                tokio::time::timeout(Duration::from_millis(300), h2.listen(cfg()))
                    .await
                    .is_err()
            })
        });
        std::thread::sleep(Duration::from_millis(50));
        for t in 0..10_000u64 {
            h.push_levels(t, LOUD); // armed, worker blocked, queue fills: must still not block
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "push_levels blocked the caller"
        );
        gate_tx.send(()).unwrap();
        let _ = jh.join();
        drop(rt);
        h.shutdown();
    }

    #[tokio::test]
    async fn capture_flag_is_claimed_before_start_returns_and_released_on_failure() {
        // A1: the flag must already be up while start() is still blocked.
        let port = FakePort::new();
        let (gate_tx, gate_rx) = mpsc::channel();
        *port.start_gate.lock().unwrap() = Some(gate_rx);
        port.set_default(Err(StartError::NoMicrophone));
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let h2 = h.clone();
        let task = tokio::spawn(async move { h2.listen(cfg()).await });
        wait_for("start entered", || port.starts.load(Ordering::SeqCst) == 1).await;
        assert!(
            h.capture_active(),
            "claim must be visible while start() is blocked"
        );
        gate_tx.send(()).unwrap();
        assert_eq!(
            task.await.unwrap(),
            ListenOutcome::Failed(ListenFailure::NoMicrophone)
        );
        assert!(!h.capture_active(), "a failed start must release the claim");
        h.shutdown();
    }
}

// T9a: integration contract for Smart Turn and the speaker gate (red until T9b).
#[cfg(test)]
#[path = "converse_capture_voice_tests.rs"]
mod voice_tests;
