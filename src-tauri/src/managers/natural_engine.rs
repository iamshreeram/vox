//! Synthesis engine core of the natural voice: a `Synth` (see `tts_queue::Synth`) that runs a
//! neural engine on ONE worker thread and plays through an `AudioSink`.
//!
//! Spec = `natural_engine_tests.rs`. STUB: bodies are `todo!()` until step S3. Signatures and
//! doc contracts are binding. DESIGN.md D2 (engine) and D7 (health) describe the intent.
#![allow(dead_code)]

use crate::managers::natural_audio::{resample_mono, trim_silence, wpm_to_speed};
use crate::managers::natural_chunk::{
    split_for_synthesis, synth_with_overflow_retry, RetryOutcome, NATURAL_MAX_CHARS,
};
use crate::managers::natural_sink::{AudioSink, CommitResult, FlushOutcome};
use crate::managers::tts_queue::{StopOutcome, Synth};
use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Kokoro produces 24 kHz mono audio.
pub const NATURAL_SAMPLE_RATE: u32 = 24_000;
/// Chunks accepted but not yet synthesised across all submissions; the excess is dropped.
pub const NATURAL_MAX_PENDING_CHUNKS: usize = 16;
/// The worker drops the engine after this long without work (about 700 MB of RAM).
pub const NATURAL_IDLE_UNLOAD: Duration = Duration::from_secs(300);
pub const DEFAULT_NATURAL_VOICE: &str = "af_heart";
/// After a failure the router may try once more after this cooldown.
pub const DEFAULT_AUTO_RETRY_COOLDOWN: Duration = Duration::from_secs(300);

/// A loaded neural engine. `synth` returns 24 kHz mono samples.
pub trait SynthEngine: Send {
    fn synth(&mut self, text: &str, voice: &str, speed: f32) -> Result<Vec<f32>, String>;
}

/// Loads the engine (reads and verifies the model). Called on the worker thread, lazily on
/// the first submission and again after an idle unload.
pub trait EngineLoader: Send + 'static {
    fn load(&mut self) -> Result<Box<dyn SynthEngine>, String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureReason {
    Load,
    Synth,
    Sink,
    WorkerExit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthState {
    Ready,
    Failed(FailureReason),
}

/// Shared health of the natural voice (D7). Failures carry the revision they started under
/// and are IGNORED when stale.
pub struct NaturalHealth {
    inner: Mutex<HealthInner>,
    auto_retry_cooldown: Duration,
}

type EventSink = Arc<dyn Fn(FailureReason) + Send + Sync>;

struct HealthInner {
    revision: u64,
    state: HealthState,
    failed_at: Option<Instant>,
    /// Stored as `Arc` so it can be cloned out and invoked after the lock is released.
    event_sink: Option<EventSink>,
}

impl HealthInner {
    fn bump(&mut self) -> u64 {
        self.revision = self.revision.wrapping_add(1);
        self.state = HealthState::Ready;
        self.failed_at = None;
        self.revision
    }
}

impl NaturalHealth {
    pub fn new(auto_retry_cooldown: Duration) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(HealthInner {
                revision: 0,
                state: HealthState::Ready,
                failed_at: None,
                event_sink: None,
            }),
            auto_retry_cooldown,
        })
    }
    fn lock(&self) -> MutexGuard<'_, HealthInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn revision(&self) -> u64 {
        self.lock().revision
    }
    pub fn state(&self) -> HealthState {
        self.lock().state
    }
    pub fn is_ready(&self) -> bool {
        self.lock().state == HealthState::Ready
    }
    /// Applies a failure only if `revision` is current AND the state is `Ready`; returns
    /// whether it was applied. Exactly one event is emitted per applied failure.
    pub fn report_failure(&self, revision: u64, reason: FailureReason) -> bool {
        let sink = {
            let mut inner = self.lock();
            if inner.revision != revision || inner.state != HealthState::Ready {
                return false;
            }
            inner.state = HealthState::Failed(reason);
            inner.failed_at = Some(Instant::now());
            inner.event_sink.clone()
        };
        if let Some(sink) = sink {
            sink(reason);
        }
        true
    }
    /// Recovery trigger: new revision, state `Ready`. Returns the new revision.
    pub fn bump(&self) -> u64 {
        self.lock().bump()
    }
    /// If `Failed` for at least the cooldown: bump and return true (once); otherwise false.
    pub fn maybe_auto_retry(&self, now: Instant) -> bool {
        let mut inner = self.lock();
        if !matches!(inner.state, HealthState::Failed(_)) {
            return false;
        }
        let due = match inner.failed_at {
            Some(at) => match at.checked_add(self.auto_retry_cooldown) {
                Some(deadline) => now >= deadline,
                None => false,
            },
            None => true,
        };
        if due {
            inner.bump();
        }
        due
    }
    /// Callback invoked once per applied failure (the UI event).
    pub fn set_event_sink(&self, sink: Box<dyn Fn(FailureReason) + Send + Sync>) {
        self.lock().event_sink = Some(Arc::from(sink));
    }
}

#[derive(Debug, Clone)]
pub struct NaturalConfig {
    pub idle_unload: Duration,
    /// Worker housekeeping period: sink `maintain`, sink-error detection, idle unload check.
    pub tick: Duration,
    pub max_pending_chunks: usize,
    /// Budget handed to `AudioSink::flush` by `stop_immediate` (design: 180 ms).
    pub flush_budget: Duration,
}

impl Default for NaturalConfig {
    fn default() -> Self {
        Self {
            idle_unload: NATURAL_IDLE_UNLOAD,
            tick: Duration::from_secs(1),
            max_pending_chunks: NATURAL_MAX_PENDING_CHUNKS,
            flush_budget: Duration::from_millis(180),
        }
    }
}

/// The natural voice as a `Synth`. Contract (DESIGN.md D2 engine, Rev 4/5):
/// * `speak` never blocks: sanitise + chunk (`split_for_synthesis`), accept at most
///   `max_pending_chunks` chunks in total (the rest counted by `dropped_chunks`), tag the
///   submission with the CURRENT generation and wake the worker. Text without anything to
///   say (or if the engine is Failed beyond recovery) is ignored. Voice `None` ->
///   `DEFAULT_NATURAL_VOICE`; speed = `wpm_to_speed(rate_wpm)`; pitch is ignored.
/// * `is_speaking` = chunks pending for the CURRENT generation, or a synthesis of the current
///   generation in flight, or `sink.audible()`. Stale-generation work never counts and never
///   clears a newer submission's state.
/// * `stop_immediate`: bump generation, drop pending chunks, `sink.invalidate()`/`flush(budget)`.
///   `AlreadyIdle` (without waiting for any callback) when nothing was speaking and the sink
///   is not audible; `Stopped` when it was speaking and the flush was Confirmed; `Failed`
///   ONLY while silence is unconfirmed (flush Unconfirmed).
/// * Worker (one thread): lazily `load`s (failure -> health Failed(Load)); per chunk calls
///   `synth_with_overflow_retry` (with an `is_current` predicate bound to the submission's
///   generation) behind `catch_unwind`; trims silence, resamples 24 kHz -> `sink.device_rate()`,
///   `commit`s (Stale -> discard, Full -> cancel-aware wait, Errored -> health Failed(Sink));
///   synthesises chunk i+1 while chunk i plays. Chunk 0 of a submission that ends Dropped ->
///   Failed(Synth); a later chunk that ends Dropped drops the rest of that submission and
///   counts; 3 consecutive such later-chunk failures -> Failed(Synth) (a later-chunk success
///   resets the count). Each tick: `sink.maintain(now)`, detect `sink.is_errored()` ->
///   Failed(Sink) and drain accounting; unload the engine after `idle_unload` of no
///   submissions, no synthesis and `!sink.audible()` (independent of `is_speaking` polling).
///   If the worker exits for any reason: health Failed(WorkerExit) and accounting drained.
/// * Failures are reported with the health revision the worker read when it started the work.
pub struct NaturalSynth {
    inner: Arc<Inner>,
    worker: Option<JoinHandle<()>>,
}

/// Bounded wait for the worker when the synth is dropped.
const DROP_JOIN_BUDGET: Duration = Duration::from_millis(200);
/// Poll period while the sink reports `Full`.
const FULL_RETRY: Duration = Duration::from_millis(10);

/// One chunk of one submission, tagged with the generation it was accepted under.
struct Chunk {
    gen: u64,
    sub_id: u64,
    index: usize,
    text: String,
    voice: String,
    speed: f32,
}

/// Accounting shared by `speak`/`stop`/`is_speaking` and the worker.
struct Acct {
    /// Current generation (mirrors the sink generation).
    generation: u64,
    /// Only ever holds chunks of the CURRENT generation (stop/sync clear it).
    queue: VecDeque<Chunk>,
    /// Generation of the chunk being synthesised/committed, if any.
    synthesizing: Option<u64>,
    next_sub: u64,
    shutdown: bool,
    worker_alive: bool,
    last_activity: Instant,
}

struct Inner {
    acct: Mutex<Acct>,
    cv: Condvar,
    gen_mirror: AtomicU64,
    dropped: AtomicUsize,
    sink: Arc<dyn AudioSink>,
    health: Arc<NaturalHealth>,
    config: NaturalConfig,
    loader: Mutex<Box<dyn EngineLoader>>,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, Acct> {
        self.acct.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Drops all pending work and the in-flight marker.
    fn drain(&self) {
        let mut acct = self.lock();
        acct.queue.clear();
        acct.synthesizing = None;
    }

    /// Clears the in-flight marker if it still belongs to `gen`, and records activity.
    fn finish_chunk(&self, gen: u64) {
        let mut acct = self.lock();
        if acct.synthesizing == Some(gen) {
            acct.synthesizing = None;
        }
        acct.last_activity = Instant::now();
    }

    fn is_current(&self, gen: u64) -> bool {
        self.gen_mirror.load(Ordering::SeqCst) == gen
    }

    fn is_speaking(&self) -> bool {
        let busy = {
            let acct = self.lock();
            !acct.queue.is_empty() || acct.synthesizing == Some(acct.generation)
        };
        busy || self.sink.audible()
    }
}

/// Marks the worker dead on any exit (including a panic) and drains the accounting.
struct WorkerGuard {
    inner: Arc<Inner>,
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        let shutdown = {
            let mut acct = self.inner.lock();
            acct.worker_alive = false;
            acct.queue.clear();
            acct.synthesizing = None;
            acct.shutdown
        };
        if !shutdown {
            log::warn!("natural voice: synthesis worker exited");
            let rev = self.inner.health.revision();
            self.inner
                .health
                .report_failure(rev, FailureReason::WorkerExit);
        }
    }
}

struct Worker {
    inner: Arc<Inner>,
    engine: Option<Box<dyn SynthEngine>>,
    later_failures: u32,
}

impl Worker {
    fn run(mut self) {
        let _guard = WorkerGuard {
            inner: self.inner.clone(),
        };
        while let Some(chunk) = self.next_chunk() {
            self.process(chunk);
        }
    }

    /// Blocks until a chunk is available (running housekeeping every tick) or shutdown.
    fn next_chunk(&mut self) -> Option<Chunk> {
        let inner = self.inner.clone();
        let mut acct = inner.lock();
        loop {
            if acct.shutdown {
                return None;
            }
            if let Some(chunk) = acct.queue.pop_front() {
                acct.synthesizing = Some(chunk.gen);
                return Some(chunk);
            }
            let (guard, timeout) = inner
                .cv
                .wait_timeout(acct, inner.config.tick)
                .unwrap_or_else(|e| e.into_inner());
            acct = guard;
            if timeout.timed_out() {
                drop(acct);
                self.housekeeping();
                acct = inner.lock();
            }
        }
    }

    fn housekeeping(&mut self) {
        let inner = &self.inner;
        let now = Instant::now();
        inner.sink.maintain(now);
        if inner.sink.is_errored() {
            let rev = inner.health.revision();
            if inner.health.report_failure(rev, FailureReason::Sink) {
                log::warn!("natural voice: output device failed");
            }
            inner.drain();
        }
        if self.engine.is_none() {
            return;
        }
        let audible = inner.sink.audible();
        let idle_for = {
            let mut acct = inner.lock();
            if !acct.queue.is_empty() || acct.synthesizing.is_some() || audible {
                acct.last_activity = now;
            }
            now.saturating_duration_since(acct.last_activity)
        };
        if idle_for >= inner.config.idle_unload {
            log::info!("natural voice: unloading idle engine");
            self.engine = None;
        }
    }

    fn process(&mut self, chunk: Chunk) {
        let inner = self.inner.clone();
        let rev = inner.health.revision();
        if self.engine.is_none() {
            let loaded = {
                let mut loader = inner.loader.lock().unwrap_or_else(|e| e.into_inner());
                catch_unwind(AssertUnwindSafe(|| loader.load()))
            };
            match loaded {
                Ok(Ok(engine)) => {
                    log::info!("natural voice: engine loaded");
                    self.engine = Some(engine);
                }
                Ok(Err(e)) => {
                    log::warn!("natural voice: engine load failed: {e}");
                    inner.health.report_failure(rev, FailureReason::Load);
                    inner.drain();
                    return;
                }
                Err(_) => {
                    log::warn!("natural voice: engine load panicked");
                    inner.health.report_failure(rev, FailureReason::Load);
                    inner.drain();
                    return;
                }
            }
        }
        let Some(engine) = self.engine.as_mut() else {
            inner.drain();
            return;
        };
        let gen = chunk.gen;
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            synth_with_overflow_retry(
                &mut |t: &str| engine.synth(t, &chunk.voice, chunk.speed),
                &chunk.text,
                NATURAL_SAMPLE_RATE,
                &|| inner.is_current(gen),
            )
        }))
        .unwrap_or_else(|_| {
            log::warn!("natural voice: synthesis panicked");
            RetryOutcome::Dropped
        });
        // A result of a superseded generation is never played nor counted as a failure.
        if !inner.is_current(gen) {
            inner.finish_chunk(gen);
            return;
        }
        match outcome {
            RetryOutcome::Cancelled => {}
            RetryOutcome::Dropped => self.on_dropped(&chunk, rev),
            RetryOutcome::Audio {
                samples,
                dropped_pieces,
            } => {
                if dropped_pieces > 0 {
                    log::debug!("natural voice: {dropped_pieces} piece(s) dropped");
                }
                if chunk.index > 0 {
                    self.later_failures = 0;
                }
                self.play(gen, &samples, rev);
            }
        }
        inner.finish_chunk(gen);
    }

    fn on_dropped(&mut self, chunk: &Chunk, rev: u64) {
        let inner = &self.inner;
        if chunk.index == 0 {
            log::warn!("natural voice: first chunk could not be synthesised");
            inner.health.report_failure(rev, FailureReason::Synth);
            inner.drain();
            return;
        }
        let removed = {
            let mut acct = inner.lock();
            let before = acct.queue.len();
            acct.queue.retain(|q| q.sub_id != chunk.sub_id);
            before - acct.queue.len()
        };
        self.later_failures += 1;
        log::warn!(
            "natural voice: later chunk dropped ({removed} more skipped, {} in a row)",
            self.later_failures
        );
        if self.later_failures >= 3 {
            inner.health.report_failure(rev, FailureReason::Synth);
            inner.drain();
        }
    }

    /// Trims, resamples and commits one chunk's audio (cancel-aware backpressure wait).
    fn play(&mut self, gen: u64, samples: &[f32], rev: u64) {
        let inner = &self.inner;
        let rate = match inner.sink.device_rate() {
            Ok(rate) => rate,
            Err(e) => {
                log::warn!("natural voice: no device rate: {e}");
                inner.health.report_failure(rev, FailureReason::Sink);
                inner.drain();
                return;
            }
        };
        let trimmed = trim_silence(samples, NATURAL_SAMPLE_RATE);
        if trimmed.is_empty() {
            return;
        }
        let pcm = match resample_mono(trimmed, NATURAL_SAMPLE_RATE, rate) {
            Ok(pcm) => pcm,
            Err(e) => {
                log::warn!("natural voice: resampling failed: {e}");
                inner.health.report_failure(rev, FailureReason::Sink);
                inner.drain();
                return;
            }
        };
        loop {
            match inner.sink.commit(gen, pcm.clone()) {
                CommitResult::Accepted | CommitResult::Stale => return,
                CommitResult::Errored => {
                    log::warn!("natural voice: commit failed (sink errored)");
                    inner.health.report_failure(rev, FailureReason::Sink);
                    inner.drain();
                    return;
                }
                CommitResult::Full => {
                    std::thread::sleep(FULL_RETRY);
                    if !inner.is_current(gen) || inner.lock().shutdown {
                        return;
                    }
                }
            }
        }
    }
}

impl NaturalSynth {
    pub fn new(
        loader: Box<dyn EngineLoader>,
        sink: Arc<dyn AudioSink>,
        health: Arc<NaturalHealth>,
        config: NaturalConfig,
    ) -> Self {
        let generation = sink.generation();
        let inner = Arc::new(Inner {
            acct: Mutex::new(Acct {
                generation,
                queue: VecDeque::new(),
                synthesizing: None,
                next_sub: 0,
                shutdown: false,
                worker_alive: false,
                last_activity: Instant::now(),
            }),
            cv: Condvar::new(),
            gen_mirror: AtomicU64::new(generation),
            dropped: AtomicUsize::new(0),
            sink,
            health,
            config,
            loader: Mutex::new(loader),
        });
        Self {
            inner,
            worker: None,
        }
    }

    /// Chunks dropped because of `max_pending_chunks`, since creation.
    pub fn dropped_chunks(&self) -> usize {
        self.inner.dropped.load(Ordering::SeqCst)
    }

    /// Retry path: clears accounting and the sink error (`sink.recover()`), restarts the worker
    /// if it exited. Does NOT touch health (the caller bumps the revision).
    pub fn recover(&mut self) {
        self.inner.sink.recover();
        let needs_worker = {
            let mut acct = self.inner.lock();
            acct.queue.clear();
            acct.synthesizing = None;
            acct.last_activity = Instant::now();
            let generation = self.inner.sink.generation();
            acct.generation = generation;
            self.inner.gen_mirror.store(generation, Ordering::SeqCst);
            !acct.worker_alive
        };
        if needs_worker {
            self.ensure_worker();
        }
    }

    /// Spawns the worker unless one is alive.
    fn ensure_worker(&mut self) {
        {
            let mut acct = self.inner.lock();
            if acct.worker_alive || acct.shutdown {
                return;
            }
            acct.worker_alive = true;
        }
        if let Some(old) = self.worker.take() {
            if old.is_finished() {
                let _ = old.join();
            }
            // Otherwise the old thread is past its exit guard: detach it.
        }
        let worker = Worker {
            inner: self.inner.clone(),
            engine: None,
            later_failures: 0,
        };
        match std::thread::Builder::new()
            .name("natural-voice".into())
            .spawn(move || worker.run())
        {
            Ok(handle) => self.worker = Some(handle),
            Err(e) => {
                log::warn!("natural voice: cannot spawn worker: {e}");
                {
                    let mut acct = self.inner.lock();
                    acct.worker_alive = false;
                    acct.queue.clear();
                    acct.synthesizing = None;
                }
                let rev = self.inner.health.revision();
                self.inner
                    .health
                    .report_failure(rev, FailureReason::WorkerExit);
            }
        }
    }
}

impl Synth for NaturalSynth {
    fn speak(&mut self, text: &str, voice: Option<&str>, rate_wpm: Option<u32>, pitch: f32) {
        let _ = pitch;
        let chunks = split_for_synthesis(text, NATURAL_MAX_CHARS);
        if chunks.is_empty() {
            return;
        }
        let voice = voice.unwrap_or(DEFAULT_NATURAL_VOICE).to_string();
        let speed = wpm_to_speed(rate_wpm);
        let total = chunks.len();
        let accepted = {
            let mut acct = self.inner.lock();
            // The sink generation may have moved (device failure / recovery): resync.
            let sink_gen = self.inner.sink.generation();
            if sink_gen != acct.generation {
                acct.generation = sink_gen;
                self.inner.gen_mirror.store(sink_gen, Ordering::SeqCst);
                acct.queue.clear();
            }
            let room = self
                .inner
                .config
                .max_pending_chunks
                .saturating_sub(acct.queue.len());
            let accepted = total.min(room);
            let sub_id = acct.next_sub;
            acct.next_sub = acct.next_sub.wrapping_add(1);
            let gen = acct.generation;
            for (index, text) in chunks.into_iter().take(accepted).enumerate() {
                acct.queue.push_back(Chunk {
                    gen,
                    sub_id,
                    index,
                    text,
                    voice: voice.clone(),
                    speed,
                });
            }
            acct.last_activity = Instant::now();
            accepted
        };
        let excess = total - accepted;
        if excess > 0 {
            self.inner.dropped.fetch_add(excess, Ordering::SeqCst);
            log::warn!("natural voice: {excess} chunk(s) over the pending cap dropped");
        }
        if accepted == 0 {
            return;
        }
        self.ensure_worker();
        self.inner.cv.notify_one();
    }

    fn is_speaking(&mut self) -> bool {
        self.inner.is_speaking()
    }

    fn stop_immediate(&mut self) -> StopOutcome {
        let was = self.inner.is_speaking();
        {
            let mut acct = self.inner.lock();
            acct.queue.clear();
            let generation = self.inner.sink.invalidate();
            acct.generation = generation;
            self.inner.gen_mirror.store(generation, Ordering::SeqCst);
        }
        self.inner.cv.notify_one();
        if !was && !self.inner.sink.audible() {
            return StopOutcome::AlreadyIdle;
        }
        match self.inner.sink.flush(self.inner.config.flush_budget) {
            FlushOutcome::Confirmed if was => StopOutcome::Stopped,
            FlushOutcome::Confirmed => StopOutcome::AlreadyIdle,
            FlushOutcome::Unconfirmed => StopOutcome::Failed,
        }
    }

    fn reset(&mut self) {
        // Idle clock restarts; the worker itself drives unloading.
        self.inner.lock().last_activity = Instant::now();
    }
}

impl Drop for NaturalSynth {
    fn drop(&mut self) {
        self.inner.lock().shutdown = true;
        self.inner.cv.notify_all();
        let Some(handle) = self.worker.take() else {
            return;
        };
        let deadline = Instant::now() + DROP_JOIN_BUDGET;
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if handle.is_finished() {
            let _ = handle.join();
        }
        // Otherwise detach: the worker exits on its own once its current call returns.
    }
}
