//! Playback side of the natural voice: the shared playback state machine (`PlaybackCtl`), the
//! sink abstraction the engine talks to (`AudioSink`) and the cpal-backed stream manager.
//!
//! Spec = `natural_sink_tests.rs` (+ `natural_test_support.rs`). STUB: bodies are `todo!()`
//! until step S2 implements them; the signatures and the doc contracts are binding.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

/// Queued PCM above this many seconds makes `commit` return `Full` (backpressure bound).
pub const NATURAL_MAX_QUEUED_SECS: f32 = 20.0;
/// No device callback for this long while PCM is queued counts as a stalled stream.
pub const STALL_AFTER: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitResult {
    /// Samples were queued for the CURRENT generation.
    Accepted,
    /// The generation is no longer current (a stop happened): samples dropped.
    Stale,
    /// Queue bound exceeded (`queued + incoming > NATURAL_MAX_QUEUED_SECS`): caller waits and retries.
    Full,
    /// The playback path has failed (`fail()`): commits are rejected until `recover()`.
    Errored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushOutcome {
    /// Old audio can no longer be heard (software silence acknowledged AND tail elapsed).
    Confirmed,
    /// Silence could not be confirmed within the budget.
    Unconfirmed,
}

/// The ONLY state shared between the synthesis worker, `stop` and the audio callback.
/// Mono f32 at the DEVICE rate. Every method takes the internal lock for O(1) work only and
/// is never held across synthesis, resampling, sleeps or backpressure waits.
pub struct PlaybackCtl {
    _private: (),
}

impl PlaybackCtl {
    /// New control for a device running at `sample_rate` Hz. Generation starts at 0,
    /// tail bound at zero.
    pub fn new(sample_rate: u32) -> Self {
        let _ = sample_rate;
        todo!("S2")
    }

    pub fn sample_rate(&self) -> u32 {
        todo!("S2")
    }

    /// Current generation. Monotonic.
    pub fn generation(&self) -> u64 {
        todo!("S2")
    }

    /// Atomically (one lock): if errored -> `Errored`; if `generation != current` -> `Stale`;
    /// if `queued_secs + samples/rate > NATURAL_MAX_QUEUED_SECS` -> `Full`; else enqueue ->
    /// `Accepted`. The generation check and the enqueue can never be separated by an
    /// `invalidate` (C1).
    pub fn commit(&self, generation: u64, samples: Vec<f32>) -> CommitResult {
        let _ = (generation, samples);
        todo!("S2")
    }

    /// Stop request: bumps the generation, clears every queued chunk. Returns the new
    /// generation. O(1), never waits for the callback.
    pub fn invalidate(&self) -> u64 {
        todo!("S2")
    }

    /// Seconds of PCM currently queued.
    pub fn queued_secs(&self) -> f32 {
        todo!("S2")
    }

    /// Device-callback side. Fills `out` (mono, device rate) with queued samples in order and
    /// zeros for the rest. Uses `try_lock`: if the lock is contended it writes zeros and
    /// returns without blocking. On success it records the acknowledged generation and the
    /// callback time. If it emitted any queued sample, `audible_until` becomes
    /// `now + emitted_frames/rate + tail_bound`.
    pub fn render(&self, out: &mut [f32], now: Instant) {
        let _ = (out, now);
        todo!("S2")
    }

    /// True while PCM is queued, or `now < audible_until` (the acoustic tail).
    pub fn audible(&self, now: Instant) -> bool {
        let _ = now;
        todo!("S2")
    }

    /// Upper bound of the device's output latency (measured by the stream manager).
    pub fn set_tail_bound(&self, tail: Duration) {
        let _ = tail;
        todo!("S2")
    }

    pub fn tail_bound(&self) -> Duration {
        todo!("S2")
    }

    /// Truthful stop. `invalidate()`, then:
    /// * if `!audible(now)` -> `Confirmed` immediately WITHOUT waiting for any callback
    ///   (nothing can replay: queue empty, generation bumped; tail drained) (C3 round 3);
    /// * otherwise wait for the callback to acknowledge the new generation, up to `budget`;
    /// * after the ack, if the remaining tail does not fit in the remaining budget ->
    ///   `Unconfirmed` immediately; if it fits, sleep it out -> `Confirmed`;
    /// * if the budget ends without an ack: `Confirmed` only if `!audible(now)` by then,
    ///   else `Unconfirmed`.
    /// Never reports `Confirmed` while old audio may still be audible.
    pub fn flush(&self, budget: Duration) -> FlushOutcome {
        let _ = budget;
        todo!("S2")
    }

    /// Playback failure (device error / stall): set `errored`, bump the generation, clear all
    /// queued PCM, reject further commits. The acoustic tail already in flight is kept
    /// (`audible` stays true until it elapses, at most `tail_bound`).
    pub fn fail(&self) {
        todo!("S2")
    }

    /// Clears `errored` (the generation is NOT reset).
    pub fn recover(&self) {
        todo!("S2")
    }

    pub fn is_errored(&self) -> bool {
        todo!("S2")
    }

    /// True when PCM is queued and no callback has run for `STALL_AFTER` (measured from the
    /// later of the last callback and the moment audio was first queued after it).
    pub fn stalled(&self, now: Instant) -> bool {
        let _ = now;
        todo!("S2")
    }

    /// Test hook: hold the internal lock (to simulate contention with the callback).
    #[cfg(test)]
    pub(crate) fn lock_for_test(&self) -> impl Drop + '_ {
        todo!("S2");
        #[allow(unreachable_code)]
        Box::new(())
    }
}

/// Callback helper: fills an interleaved f32 buffer with `channels` copies of the mono stream.
/// `channels == 0` is a no-op. Frames = `out.len() / channels`.
pub fn fill_interleaved_f32(ctl: &PlaybackCtl, out: &mut [f32], channels: usize, now: Instant) {
    let _ = (ctl, out, channels, now);
    todo!("S2")
}

/// Clamped conversions for non-f32 device formats. NaN maps to silence.
/// `f32_to_i16(1.0) == 32767`; `f32_to_u16(0.0) == 32768`, `1.0 -> 65535`, `-1.0 -> 0`.
pub fn f32_to_i16(x: f32) -> i16 {
    let _ = x;
    todo!("S2")
}

pub fn f32_to_u16(x: f32) -> u16 {
    let _ = x;
    todo!("S2")
}

/// Opens (and closes, by dropping) real or fake output streams.
pub trait StreamFactory: Send + Sync {
    /// Sample rate of the output device (probed on demand; may change between calls).
    fn device_rate(&self) -> Result<u32, String>;
    /// Opens an output stream that pulls audio through `ctl.render`/`fill_interleaved_f32`
    /// and calls `ctl.fail()` from its error callback. Dropping the returned box closes it.
    fn open(&self, ctl: Arc<PlaybackCtl>) -> Result<Box<dyn Send>, String>;
}

/// What the synthesis engine talks to. Samples are mono f32 at `device_rate()`.
pub trait AudioSink: Send + Sync {
    fn device_rate(&self) -> Result<u32, String>;
    /// Current generation; monotonic, also across stream re-creation.
    fn generation(&self) -> u64;
    /// Ensures a stream is open (lazily), then `PlaybackCtl::commit`. Open failure ->
    /// `Errored` (and the sink is errored until `recover()`).
    fn commit(&self, generation: u64, samples: Vec<f32>) -> CommitResult;
    fn invalidate(&self) -> u64;
    fn flush(&self, budget: Duration) -> FlushOutcome;
    /// `PlaybackCtl::audible(Instant::now())`; false when no stream was ever opened.
    fn audible(&self) -> bool;
    fn queued_secs(&self) -> f32;
    fn is_errored(&self) -> bool;
    /// Clears the error, drops the (possibly dead) stream and bumps the generation if needed
    /// so stale PCM can never play; the next commit re-opens a stream.
    fn recover(&self);
    /// Housekeeping, called about once a second by the engine worker: closes the stream after
    /// it has been idle (nothing queued, not audible, no commit) for `idle_close`; marks the
    /// sink errored (`ctl.fail()`) when `stalled(now)`.
    fn maintain(&self, now: Instant);
}

/// `AudioSink` over a `StreamFactory`: lazy open, idle close, stall detection, recovery.
pub struct ManagedSink {
    _private: (),
}

impl ManagedSink {
    pub fn new(factory: Arc<dyn StreamFactory>, idle_close: Duration) -> Self {
        let _ = (factory, idle_close);
        todo!("S2")
    }
}

impl AudioSink for ManagedSink {
    fn device_rate(&self) -> Result<u32, String> {
        todo!("S2")
    }
    fn generation(&self) -> u64 {
        todo!("S2")
    }
    fn commit(&self, generation: u64, samples: Vec<f32>) -> CommitResult {
        let _ = (generation, samples);
        todo!("S2")
    }
    fn invalidate(&self) -> u64 {
        todo!("S2")
    }
    fn flush(&self, budget: Duration) -> FlushOutcome {
        let _ = budget;
        todo!("S2")
    }
    fn audible(&self) -> bool {
        todo!("S2")
    }
    fn queued_secs(&self) -> f32 {
        todo!("S2")
    }
    fn is_errored(&self) -> bool {
        todo!("S2")
    }
    fn recover(&self) {
        todo!("S2")
    }
    fn maintain(&self, now: Instant) {
        let _ = now;
        todo!("S2")
    }
}
