//! CaptureWorker: the ONLY starter/stopper/canceller of the session's
//! `converse` recording. One thread owns the recorder port; the audio
//! callback only enqueues levels (never blocks, never stops or transcribes).
//! No tauri types: hardware is behind ports. Contract: DESIGN.md 3.4 and the
//! AMENDMENTS section (Revision 4).

use crate::managers::converse::{ListenFailure, ListenOutcome};
use crate::managers::converse_audio::{EndpointConfig, EndpointEvent, Endpointer};
use crate::managers::converse_latency::ListenTiming;
use crate::managers::smart_turn::TurnDetector;
use crate::managers::speaker_id::SpeakerGate;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
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
    fn set_raw_tap(&self, _tap: Option<Arc<dyn Fn(&[f32]) + Send + Sync>>) {}
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
    capture_active: Arc<AtomicBool>,
    /// Highest cancelled listen id. Raised by the caller before the Cancel is
    /// queued, so the worker sees it even while `start()` is blocked.
    floor: Arc<AtomicU64>,
    join: Mutex<Option<JoinHandle<()>>>,
    /// T9b test-only counters (never written by the stub).
    #[cfg_attr(not(test), allow(dead_code))]
    tap_installed: AtomicU64,
    #[cfg_attr(not(test), allow(dead_code))]
    ring_len: AtomicU64,
    #[cfg_attr(not(test), allow(dead_code))]
    ring_allocated: AtomicBool,
    #[cfg_attr(not(test), allow(dead_code))]
    decider_spawned: AtomicU64,
    #[cfg_attr(not(test), allow(dead_code))]
    decider_live: AtomicU64,
    /// T9b (A7): timing of the last listen that ended, keyed by listen id.
    #[cfg_attr(not(test), allow(dead_code))]
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

struct Recording {
    id: u64,
    reply: oneshot::Sender<ListenOutcome>,
    started_at: Instant,
    abs_deadline: Duration,
    endpointer: Endpointer,
    heard: bool,
    /// Latest VAD speech frame time for this listen (None until one arrives).
    last_speech_ms: Option<u64>,
    /// Number of VAD speech frames accepted for this listen (diagnostic).
    speech_frames: u32,
}

enum State {
    Idle,
    Waiting(Pending),
    Recording(Recording),
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
}

pub struct CaptureWorker;

impl CaptureWorker {
    /// Spawn the worker thread.
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
        let worker = Worker {
            port,
            transcriber,
            timings,
            capture_active: Arc::clone(&capture_active),
            floor: Arc::clone(&floor),
            state: State::Idle,
            turn_detector: options.turn_detector,
            speaker_gate: options.speaker_gate,
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
                capture_active,
                floor,
                join: Mutex::new(join),
                tap_installed: AtomicU64::new(0),
                ring_len: AtomicU64::new(0),
                ring_allocated: AtomicBool::new(false),
                decider_spawned: AtomicU64::new(0),
                decider_live: AtomicU64::new(0),
                timing_slot: Mutex::new(None),
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
                if listen_id <= self.floor.load(Ordering::SeqCst) {
                    let _ = reply.send(ListenOutcome::Aborted);
                } else {
                    if self.turn_detector.is_some() {
                        todo!("T9b: EndpointConfig.smart_turn = Some(default_policy) when a detector is configured; tap + decider");
                    }
                    self.drop_state();
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
                    self.state = State::Recording(Recording {
                        id: p.id,
                        reply: p.reply,
                        started_at: Instant::now(),
                        abs_deadline: Duration::from_millis(
                            p.cfg
                                .initial_grace_ms
                                .saturating_add(p.cfg.max_utterance_ms),
                        ) + self.timings.abs_slack,
                        endpointer: Endpointer::new(p.cfg),
                        heard: false,
                        last_speech_ms: None,
                        speech_frames: 0,
                    });
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

    /// Timer work: busy retries and the readiness / absolute watchdogs.
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
            State::Recording(r) => {
                let elapsed = r.started_at.elapsed();
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
            if r.id == listen_id {
                r.last_speech_ms = Some(match r.last_speech_ms {
                    Some(prev) => prev.max(now_ms),
                    None => now_ms,
                });
                r.speech_frames = r.speech_frames.saturating_add(1);
            }
        }
    }

    /// The endpointer is driven by VAD speech activity, not by level values:
    /// room tone on the visualizer scale is always above the level floor, so
    /// level-driven endpointing never detects the end of speech.
    fn on_levels(&mut self, listen_id: u64, now_ms: u64, _levels: &[f32]) {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Recording(mut r) if r.id == listen_id => {
                r.heard = true;
                let speech_active = r
                    .last_speech_ms
                    .is_some_and(|t| now_ms.saturating_sub(t) <= SPEECH_GAP_MS);
                match r.endpointer.feed_activity(now_ms, speech_active) {
                    None => self.state = State::Recording(r),
                    Some(EndpointEvent::NoSpeech) => {
                        self.end_with_cancel(r, ListenOutcome::NoSpeech)
                    }
                    Some(EndpointEvent::MaxLength) => {
                        self.end_with_cancel(r, ListenOutcome::TooLong)
                    }
                    Some(EndpointEvent::SpeechEnded) => self.finish_speech(r),
                    // Smart Turn is not wired into capture yet (policy is None
                    // everywhere), so a pending decision keeps listening.
                    Some(EndpointEvent::NeedTurnDecision { .. }) => {
                        self.state = State::Recording(r)
                    }
                }
            }
            other => self.state = other,
        }
    }

    fn finish_speech(&mut self, r: Recording) {
        let samples = self.port.stop();
        self.set_inactive();
        let outcome = match samples {
            None => ListenOutcome::Aborted,
            Some(samples) => {
                if self.speaker_gate.is_some() {
                    todo!(
                        "T9b: speaker gate after stop(), before transcription (Reject => Ignored)"
                    );
                }
                self.transcribe(samples)
            }
        };
        log_listen_end(end_reason(&outcome), &r);
        let _ = r.reply.send(outcome);
    }

    fn transcribe(&self, samples: Vec<f32>) -> ListenOutcome {
        match self.transcriber.transcribe(samples) {
            Ok(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    ListenOutcome::NoSpeech
                } else {
                    ListenOutcome::Utterance(trimmed.to_owned())
                }
            }
            Err(_) => ListenOutcome::Failed(ListenFailure::ModelUnavailable),
        }
    }

    fn cancel_upto(&mut self, upto: u64) {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Waiting(p) if p.id <= upto => {
                let _ = p.reply.send(ListenOutcome::Aborted);
            }
            State::Recording(r) if r.id <= upto => self.end_with_cancel(r, ListenOutcome::Aborted),
            other => self.state = other,
        }
    }

    /// Ends whatever is pending or recording, used on shutdown and new listens.
    fn drop_state(&mut self) {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Idle => {}
            State::Waiting(p) => {
                let _ = p.reply.send(ListenOutcome::Aborted);
            }
            State::Recording(r) => self.end_with_cancel(r, ListenOutcome::Aborted),
        }
    }

    /// Cancels the recording this worker started and answers its listen.
    fn end_with_cancel(&mut self, r: Recording, outcome: ListenOutcome) {
        self.port.cancel();
        self.set_inactive();
        log_listen_end(end_reason(&outcome), &r);
        let _ = r.reply.send(outcome);
    }

    fn set_inactive(&self) {
        self.capture_active.store(false, Ordering::SeqCst);
    }
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
fn log_listen_end(reason: &str, r: &Recording) {
    log::info!(
        "converse capture ended: reason={} dur_ms={} speech_frames={}",
        reason,
        r.started_at.elapsed().as_millis(),
        r.speech_frames
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
    pub fn capture_active(&self) -> bool {
        self.inner.capture_active.load(Ordering::SeqCst)
    }

    /// T9b test-only: number of pre-VAD taps the worker installed (cumulative).
    #[cfg(test)]
    pub fn tap_installed_count(&self) -> usize {
        self.inner.tap_installed.load(Ordering::SeqCst) as usize
    }

    /// T9b test-only: samples currently held by the pre-VAD ring.
    #[cfg(test)]
    pub fn ring_len(&self) -> usize {
        self.inner.ring_len.load(Ordering::SeqCst) as usize
    }

    /// T9b test-only: whether the pre-VAD ring buffer is allocated.
    #[cfg(test)]
    pub fn ring_allocated(&self) -> bool {
        self.inner.ring_allocated.load(Ordering::SeqCst)
    }

    /// T9b test-only: decider (helper) threads created so far (cumulative).
    #[cfg(test)]
    pub fn decider_threads_spawned(&self) -> usize {
        self.inner.decider_spawned.load(Ordering::SeqCst) as usize
    }

    /// T9b test-only: decider threads that have not finished yet.
    #[cfg(test)]
    pub fn decider_threads_live(&self) -> usize {
        self.inner.decider_live.load(Ordering::SeqCst) as usize
    }

    /// T9b (A7): the timing of listen `listen_id` if it is the last listen that ended.
    /// Returns it only for the matching id and clears the slot (a second take is None).
    #[allow(dead_code)]
    pub fn take_timing(&self, _listen_id: u64) -> Option<ListenTiming> {
        todo!("T9b: timing slot (A7): clear at listen start, write before ListenOutcome, match id, take once")
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
