//! Synthesis engine core of the natural voice: a `Synth` (see `tts_queue::Synth`) that runs a
//! neural engine on ONE worker thread and plays through an `AudioSink`.
//!
//! Spec = `natural_engine_tests.rs`. STUB: bodies are `todo!()` until step S3. Signatures and
//! doc contracts are binding. DESIGN.md D2 (engine) and D7 (health) describe the intent.
#![allow(dead_code)]

use crate::managers::natural_sink::AudioSink;
use crate::managers::tts_queue::{StopOutcome, Synth};
use std::sync::Arc;
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
    _private: (),
}

impl NaturalHealth {
    pub fn new(auto_retry_cooldown: Duration) -> Arc<Self> {
        let _ = auto_retry_cooldown;
        todo!("S3")
    }
    pub fn revision(&self) -> u64 {
        todo!("S3")
    }
    pub fn state(&self) -> HealthState {
        todo!("S3")
    }
    pub fn is_ready(&self) -> bool {
        todo!("S3")
    }
    /// Applies a failure only if `revision` is current AND the state is `Ready`; returns
    /// whether it was applied. Exactly one event is emitted per applied failure.
    pub fn report_failure(&self, revision: u64, reason: FailureReason) -> bool {
        let _ = (revision, reason);
        todo!("S3")
    }
    /// Recovery trigger: new revision, state `Ready`. Returns the new revision.
    pub fn bump(&self) -> u64 {
        todo!("S3")
    }
    /// If `Failed` for at least the cooldown: bump and return true (once); otherwise false.
    pub fn maybe_auto_retry(&self, now: Instant) -> bool {
        let _ = now;
        todo!("S3")
    }
    /// Callback invoked once per applied failure (the UI event).
    pub fn set_event_sink(&self, sink: Box<dyn Fn(FailureReason) + Send + Sync>) {
        let _ = sink;
        todo!("S3")
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
    _private: (),
}

impl NaturalSynth {
    pub fn new(
        loader: Box<dyn EngineLoader>,
        sink: Arc<dyn AudioSink>,
        health: Arc<NaturalHealth>,
        config: NaturalConfig,
    ) -> Self {
        let _ = (loader, sink, health, config);
        todo!("S3")
    }

    /// Chunks dropped because of `max_pending_chunks`, since creation.
    pub fn dropped_chunks(&self) -> usize {
        todo!("S3")
    }

    /// Retry path: clears accounting and the sink error (`sink.recover()`), restarts the worker
    /// if it exited. Does NOT touch health (the caller bumps the revision).
    pub fn recover(&mut self) {
        todo!("S3")
    }
}

impl Synth for NaturalSynth {
    fn speak(&mut self, text: &str, voice: Option<&str>, rate_wpm: Option<u32>, pitch: f32) {
        let _ = (text, voice, rate_wpm, pitch);
        todo!("S3")
    }
    fn is_speaking(&mut self) -> bool {
        todo!("S3")
    }
    fn stop_immediate(&mut self) -> StopOutcome {
        todo!("S3")
    }
    fn reset(&mut self) {
        // Idle clock restarts; the worker itself drives unloading.
        todo!("S3")
    }
}
