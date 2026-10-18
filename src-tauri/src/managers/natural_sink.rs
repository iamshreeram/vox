//! Playback side of the natural voice: the shared playback state machine (`PlaybackCtl`), the
//! sink abstraction the engine talks to (`AudioSink`) and the cpal-backed stream manager.
//!
//! Spec = `natural_sink_tests.rs` (+ `natural_test_support.rs`). The signatures and the doc
//! contracts are binding.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, TryLockError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

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

/// `base + d`, saturating to a far-but-representable instant instead of panicking.
fn instant_after(base: Instant, d: Duration) -> Instant {
    base.checked_add(d)
        .or_else(|| base.checked_add(Duration::from_secs(86_400)))
        .unwrap_or(base)
}

/// Duration of `frames` samples at `rate` Hz (`rate == 0` is treated as 1 Hz).
fn frames_duration(frames: usize, rate: u32) -> Duration {
    let secs = frames as f64 / f64::from(rate.max(1));
    if secs.is_finite() && secs >= 0.0 {
        Duration::try_from_secs_f64(secs).unwrap_or(Duration::MAX)
    } else {
        Duration::ZERO
    }
}

/// Everything behind the `PlaybackCtl` lock.
struct Inner {
    generation: u64,
    errored: bool,
    /// Committed chunks. Chunks before `head_chunk` are fully consumed; they are released by
    /// the non-callback side (`trim`) so that `render` never frees memory.
    queue: VecDeque<Vec<f32>>,
    head_chunk: usize,
    head_offset: usize,
    /// Unconsumed samples in `queue`.
    queued_samples: usize,
    /// When audio was queued into an empty queue (stall reference).
    queued_since: Option<Instant>,
    callback_seen_gen: u64,
    last_callback: Option<Instant>,
    audible_until: Option<Instant>,
    tail_bound: Duration,
    /// Threads currently waiting in `flush` (the callback only notifies when > 0).
    flush_waiters: usize,
}

impl Inner {
    fn new(generation: u64) -> Self {
        Self {
            generation,
            errored: false,
            queue: VecDeque::new(),
            head_chunk: 0,
            head_offset: 0,
            queued_samples: 0,
            queued_since: None,
            callback_seen_gen: 0,
            last_callback: None,
            audible_until: None,
            tail_bound: Duration::ZERO,
            flush_waiters: 0,
        }
    }

    fn clear_queue(&mut self) {
        self.queue.clear();
        self.head_chunk = 0;
        self.head_offset = 0;
        self.queued_samples = 0;
        self.queued_since = None;
    }

    /// Releases fully consumed chunks (never called from the device callback).
    fn trim(&mut self) {
        let consumed = self.head_chunk.min(self.queue.len());
        self.queue.drain(..consumed);
        self.head_chunk = 0;
    }

    fn invalidate(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.clear_queue();
        self.generation
    }

    fn next_sample(&mut self) -> Option<f32> {
        if self.queued_samples == 0 {
            return None;
        }
        while let Some(chunk) = self.queue.get(self.head_chunk) {
            if let Some(sample) = chunk.get(self.head_offset) {
                self.head_offset += 1;
                self.queued_samples = self.queued_samples.saturating_sub(1);
                return Some(*sample);
            }
            self.head_chunk += 1;
            self.head_offset = 0;
        }
        None
    }

    fn audible(&self, now: Instant) -> bool {
        self.queued_samples > 0 || self.audible_until.is_some_and(|until| now < until)
    }
}

/// The ONLY state shared between the synthesis worker, `stop` and the audio callback.
/// Mono f32 at the DEVICE rate. Every method takes the internal lock for O(1) work only and
/// is never held across synthesis, resampling, sleeps or backpressure waits.
pub struct PlaybackCtl {
    sample_rate: u32,
    inner: Mutex<Inner>,
    acked: Condvar,
}

impl PlaybackCtl {
    /// New control for a device running at `sample_rate` Hz. Generation starts at 0,
    /// tail bound at zero.
    pub fn new(sample_rate: u32) -> Self {
        Self::with_generation(sample_rate, 0)
    }

    fn with_generation(sample_rate: u32, generation: u64) -> Self {
        Self {
            sample_rate,
            inner: Mutex::new(Inner::new(generation)),
            acked: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Current generation. Monotonic.
    pub fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// Atomically (one lock): if errored -> `Errored`; if `generation != current` -> `Stale`;
    /// if `queued_secs + samples/rate > NATURAL_MAX_QUEUED_SECS` -> `Full`; else enqueue ->
    /// `Accepted`. The generation check and the enqueue can never be separated by an
    /// `invalidate` (C1).
    pub fn commit(&self, generation: u64, samples: Vec<f32>) -> CommitResult {
        let rate = f64::from(self.sample_rate.max(1));
        let mut g = self.lock();
        if g.errored {
            return CommitResult::Errored;
        }
        if generation != g.generation {
            return CommitResult::Stale;
        }
        let total = g.queued_samples.saturating_add(samples.len());
        if total as f64 / rate > f64::from(NATURAL_MAX_QUEUED_SECS) {
            return CommitResult::Full;
        }
        g.trim();
        if samples.is_empty() {
            return CommitResult::Accepted;
        }
        if g.queued_samples == 0 {
            g.queued_since = Some(Instant::now());
        }
        g.queued_samples = total;
        g.queue.push_back(samples);
        CommitResult::Accepted
    }

    /// Stop request: bumps the generation, clears every queued chunk. Returns the new
    /// generation. O(1), never waits for the callback.
    pub fn invalidate(&self) -> u64 {
        self.lock().invalidate()
    }

    /// Seconds of PCM currently queued.
    pub fn queued_secs(&self) -> f32 {
        let queued = self.lock().queued_samples;
        (queued as f64 / f64::from(self.sample_rate.max(1))) as f32
    }

    /// Device-callback side. Fills `out` (mono, device rate) with queued samples in order and
    /// zeros for the rest. Uses `try_lock`: if the lock is contended it writes zeros and
    /// returns without blocking. On success it records the acknowledged generation and the
    /// callback time. If it emitted any queued sample, `audible_until` becomes
    /// `now + emitted_frames/rate + tail_bound`.
    pub fn render(&self, out: &mut [f32], now: Instant) {
        self.render_frames(out, 1, now);
    }

    /// Shared callback body: every frame of `channels` samples gets the next mono sample.
    /// Samples beyond the last whole frame are zeroed. No allocation, no blocking.
    fn render_frames(&self, out: &mut [f32], channels: usize, now: Instant) {
        if channels == 0 {
            return;
        }
        let mut g = match self.inner.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => {
                out.fill(0.0);
                return;
            }
        };
        let newly_acked = g.callback_seen_gen != g.generation;
        g.callback_seen_gen = g.generation;
        g.last_callback = Some(now);
        let mut emitted = 0usize;
        let mut frames = out.chunks_exact_mut(channels);
        for frame in &mut frames {
            let sample = match g.next_sample() {
                Some(s) => {
                    emitted += 1;
                    s
                }
                None => 0.0,
            };
            frame.fill(sample);
        }
        frames.into_remainder().fill(0.0);
        if emitted > 0 {
            let ahead = frames_duration(emitted, self.sample_rate).saturating_add(g.tail_bound);
            g.audible_until = Some(instant_after(now, ahead));
        }
        let notify = newly_acked && g.flush_waiters > 0;
        drop(g);
        if notify {
            self.acked.notify_all();
        }
    }

    /// True while PCM is queued, or `now < audible_until` (the acoustic tail).
    pub fn audible(&self, now: Instant) -> bool {
        self.lock().audible(now)
    }

    /// Upper bound of the device's output latency (measured by the stream manager).
    pub fn set_tail_bound(&self, tail: Duration) {
        self.lock().tail_bound = tail;
    }

    /// Non-blocking variant for the device callback. Returns false when the lock was busy.
    fn try_set_tail_bound(&self, tail: Duration) -> bool {
        match self.inner.try_lock() {
            Ok(mut g) => {
                g.tail_bound = tail;
                true
            }
            Err(TryLockError::Poisoned(e)) => {
                e.into_inner().tail_bound = tail;
                true
            }
            Err(TryLockError::WouldBlock) => false,
        }
    }

    pub fn tail_bound(&self) -> Duration {
        self.lock().tail_bound
    }

    /// Truthful stop. `invalidate()`, then:
    /// * if `!audible(now)` -> `Confirmed` immediately WITHOUT waiting for any callback
    ///   (nothing can replay: queue empty, generation bumped; tail drained) (C3 round 3);
    /// * otherwise wait for the callback to acknowledge the new generation, up to `budget`;
    /// * after the ack, if the remaining tail does not fit in the remaining budget ->
    ///   `Unconfirmed` immediately; if it fits, sleep it out -> `Confirmed`;
    /// * if the budget ends without an ack: `Confirmed` only if `!audible(now)` by then,
    ///   else `Unconfirmed`.
    ///
    /// Never reports `Confirmed` while old audio may still be audible.
    pub fn flush(&self, budget: Duration) -> FlushOutcome {
        let deadline = Instant::now().checked_add(budget);
        let mut g = self.lock();
        let new_gen = g.invalidate();
        if !g.audible(Instant::now()) {
            return FlushOutcome::Confirmed;
        }
        g.flush_waiters += 1;
        while g.callback_seen_gen < new_gen {
            match deadline {
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    g = match self.acked.wait_timeout(g, deadline - now) {
                        Ok((guard, _)) => guard,
                        Err(e) => e.into_inner().0,
                    };
                }
                None => {
                    g = self.acked.wait(g).unwrap_or_else(|e| e.into_inner());
                }
            }
        }
        g.flush_waiters = g.flush_waiters.saturating_sub(1);
        let acked = g.callback_seen_gen >= new_gen;
        let audible_until = g.audible_until;
        drop(g);

        let now = Instant::now();
        if !acked {
            return if self.audible(now) {
                FlushOutcome::Unconfirmed
            } else {
                FlushOutcome::Confirmed
            };
        }
        let remaining_tail =
            audible_until.map_or(Duration::ZERO, |u| u.saturating_duration_since(now));
        let remaining_budget = deadline.map_or(Duration::MAX, |d| d.saturating_duration_since(now));
        if remaining_tail > remaining_budget {
            return FlushOutcome::Unconfirmed;
        }
        if !remaining_tail.is_zero() {
            std::thread::sleep(remaining_tail);
        }
        FlushOutcome::Confirmed
    }

    /// Playback failure (device error / stall): set `errored`, bump the generation, clear all
    /// queued PCM, reject further commits. The acoustic tail already in flight is kept
    /// (`audible` stays true until it elapses, at most `tail_bound`).
    pub fn fail(&self) {
        let mut g = self.lock();
        g.errored = true;
        g.invalidate();
    }

    /// Clears `errored` (the generation is NOT reset).
    pub fn recover(&self) {
        self.lock().errored = false;
    }

    pub fn is_errored(&self) -> bool {
        self.lock().errored
    }

    /// True when PCM is queued and no callback has run for `STALL_AFTER` (measured from the
    /// later of the last callback and the moment audio was first queued after it).
    pub fn stalled(&self, now: Instant) -> bool {
        let g = self.lock();
        if g.queued_samples == 0 {
            return false;
        }
        let reference = match (g.last_callback, g.queued_since) {
            (Some(a), Some(b)) => a.max(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => return false,
        };
        now.saturating_duration_since(reference) >= STALL_AFTER
    }

    /// Test hook: hold the internal lock (to simulate contention with the callback).
    #[cfg(test)]
    pub(crate) fn lock_for_test(&self) -> impl Drop + '_ {
        self.lock()
    }
}

/// Callback helper: fills an interleaved f32 buffer with `channels` copies of the mono stream.
/// `channels == 0` is a no-op. Frames = `out.len() / channels`.
pub fn fill_interleaved_f32(ctl: &PlaybackCtl, out: &mut [f32], channels: usize, now: Instant) {
    ctl.render_frames(out, channels, now);
}

/// Clamped conversions for non-f32 device formats. NaN maps to silence.
/// `f32_to_i16(1.0) == 32767`; `f32_to_u16(0.0) == 32768`, `1.0 -> 65535`, `-1.0 -> 0`.
pub fn f32_to_i16(x: f32) -> i16 {
    if x.is_nan() {
        return 0;
    }
    (x.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

pub fn f32_to_u16(x: f32) -> u16 {
    if x.is_nan() {
        return 32768;
    }
    ((x.clamp(-1.0, 1.0) + 1.0) * 32767.5).round() as u16
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

/// The control (created lazily) and the generation to answer before it exists.
struct CtlSlot {
    ctl: Option<Arc<PlaybackCtl>>,
    generation_floor: u64,
}

/// The open stream (if any) and the idle bookkeeping.
struct StreamSlot {
    stream: Option<Box<dyn Send>>,
    idle_since: Option<Instant>,
}

/// `AudioSink` over a `StreamFactory`: lazy open, idle close, stall detection, recovery.
///
/// Lock order: `stream` before `ctl`. The `ctl` slot is only held for O(1) work, so
/// `invalidate`/`generation` never wait for a stream being opened or closed.
pub struct ManagedSink {
    factory: Arc<dyn StreamFactory>,
    idle_close: Duration,
    ctl: Mutex<CtlSlot>,
    stream: Mutex<StreamSlot>,
    open_failed: AtomicBool,
}

impl ManagedSink {
    pub fn new(factory: Arc<dyn StreamFactory>, idle_close: Duration) -> Self {
        Self {
            factory,
            idle_close,
            ctl: Mutex::new(CtlSlot {
                ctl: None,
                generation_floor: 0,
            }),
            stream: Mutex::new(StreamSlot {
                stream: None,
                idle_since: None,
            }),
            open_failed: AtomicBool::new(false),
        }
    }

    fn lock_ctl(&self) -> MutexGuard<'_, CtlSlot> {
        self.ctl.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_stream(&self) -> MutexGuard<'_, StreamSlot> {
        self.stream.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn current_ctl(&self) -> Option<Arc<PlaybackCtl>> {
        self.lock_ctl().ctl.clone()
    }

    /// The control to open a stream with: the existing one when the rate still matches,
    /// otherwise a fresh one at `rate` that continues the generation sequence.
    fn ctl_for_rate(&self, rate: u32) -> Arc<PlaybackCtl> {
        let mut slot = self.lock_ctl();
        let generation = match &slot.ctl {
            Some(ctl) if ctl.sample_rate() == rate => return ctl.clone(),
            Some(ctl) => ctl.generation(),
            None => slot.generation_floor,
        };
        let ctl = Arc::new(PlaybackCtl::with_generation(rate, generation));
        slot.ctl = Some(ctl.clone());
        ctl
    }

    /// Opens a stream if none is open. `Err(())` means the sink is now errored.
    fn ensure_stream(&self, slot: &mut StreamSlot) -> Result<(), ()> {
        if slot.stream.is_some() {
            return Ok(());
        }
        let rate = match self.factory.device_rate() {
            Ok(rate) => rate,
            Err(e) => {
                log::warn!("natural voice: no output device rate: {e}");
                self.open_failed.store(true, Ordering::SeqCst);
                return Err(());
            }
        };
        let ctl = self.ctl_for_rate(rate);
        if ctl.is_errored() {
            return Err(());
        }
        match self.factory.open(ctl) {
            Ok(stream) => {
                slot.stream = Some(stream);
                slot.idle_since = None;
                Ok(())
            }
            Err(e) => {
                log::warn!("natural voice: cannot open output stream: {e}");
                self.open_failed.store(true, Ordering::SeqCst);
                Err(())
            }
        }
    }
}

impl AudioSink for ManagedSink {
    fn device_rate(&self) -> Result<u32, String> {
        self.factory.device_rate()
    }

    fn generation(&self) -> u64 {
        let slot = self.lock_ctl();
        match &slot.ctl {
            Some(ctl) => ctl.generation(),
            None => slot.generation_floor,
        }
    }

    fn commit(&self, generation: u64, samples: Vec<f32>) -> CommitResult {
        if self.is_errored() {
            return CommitResult::Errored;
        }
        // Do not open a stream for PCM that can only be rejected (re-checked atomically
        // by `PlaybackCtl::commit` below).
        if generation != self.generation() {
            return CommitResult::Stale;
        }
        let mut slot = self.lock_stream();
        if self.ensure_stream(&mut slot).is_err() {
            return CommitResult::Errored;
        }
        let Some(ctl) = self.current_ctl() else {
            return CommitResult::Errored;
        };
        let result = ctl.commit(generation, samples);
        if result == CommitResult::Accepted {
            slot.idle_since = None;
        }
        result
    }

    fn invalidate(&self) -> u64 {
        let mut slot = self.lock_ctl();
        match &slot.ctl {
            Some(ctl) => ctl.invalidate(),
            None => {
                slot.generation_floor = slot.generation_floor.wrapping_add(1);
                slot.generation_floor
            }
        }
    }

    fn flush(&self, budget: Duration) -> FlushOutcome {
        match self.current_ctl() {
            Some(ctl) => ctl.flush(budget),
            None => {
                self.invalidate();
                FlushOutcome::Confirmed
            }
        }
    }

    fn audible(&self) -> bool {
        self.current_ctl()
            .is_some_and(|ctl| ctl.audible(Instant::now()))
    }

    fn queued_secs(&self) -> f32 {
        self.current_ctl().map_or(0.0, |ctl| ctl.queued_secs())
    }

    fn is_errored(&self) -> bool {
        self.open_failed.load(Ordering::SeqCst)
            || self.current_ctl().is_some_and(|ctl| ctl.is_errored())
    }

    fn recover(&self) {
        let mut slot = self.lock_stream();
        // Drop the (possibly dead) stream; the next commit re-opens through the factory.
        slot.stream = None;
        slot.idle_since = None;
        // Always bump: PCM committed before the failure can never play afterwards.
        self.invalidate();
        if let Some(ctl) = self.current_ctl() {
            ctl.recover();
        }
        self.open_failed.store(false, Ordering::SeqCst);
    }

    fn maintain(&self, now: Instant) {
        let Some(ctl) = self.current_ctl() else {
            return;
        };
        if ctl.stalled(now) {
            log::warn!("natural voice: output stream stalled");
            ctl.fail();
        }
        let mut slot = self.lock_stream();
        if slot.stream.is_none() || ctl.audible(now) {
            slot.idle_since = None;
            return;
        }
        let since = *slot.idle_since.get_or_insert(now);
        if now.saturating_duration_since(since) >= self.idle_close {
            slot.stream = None;
            slot.idle_since = None;
        }
    }
}

// ---- real output device (cpal) --------------------------------------------------------------

/// Preferred fixed buffer size (frames) when the device supports it.
const PREFERRED_BUFFER_FRAMES: u32 = 256;
/// Latency window: `LATENCY_BUCKETS` buckets of `LATENCY_BUCKET` each.
const LATENCY_BUCKETS: usize = 10;
const LATENCY_BUCKET: Duration = Duration::from_secs(1);
/// `set_tail_bound` is called at most this often from the data callback.
const TAIL_PUBLISH_EVERY: Duration = Duration::from_secs(1);

/// `StreamFactory` over the real cpal output device chosen by the `selected_output_device`
/// setting (`None` / `"Default"` / unknown name -> the default output device).
/// The device is resolved every time a stream is built, so a settings change takes effect on
/// the next stream.
pub struct CpalStreamFactory {
    device_source: DeviceSource,
}

/// Returns the currently selected output device name (`None` / `"Default"` -> default device).
pub type DeviceSource = Arc<dyn Fn() -> Option<String> + Send + Sync>;

impl CpalStreamFactory {
    /// Fixed device name.
    pub fn new(device_name: Option<String>) -> Self {
        Self::with_device_source(Arc::new(move || device_name.clone()))
    }

    /// Device name read from `source` each time a stream is built.
    pub fn with_device_source(source: DeviceSource) -> Self {
        Self {
            device_source: source,
        }
    }

    /// Same resolution as `audio_feedback::play_audio_file`.
    fn resolve_device(&self) -> Result<cpal::Device, String> {
        let host = crate::audio_toolkit::get_cpal_host();
        let device_name = (self.device_source)();
        if let Some(name) = device_name.as_deref().filter(|n| *n != "Default") {
            match host.output_devices() {
                Ok(devices) => {
                    for device in devices {
                        if device.name().is_ok_and(|n| n == name) {
                            return Ok(device);
                        }
                    }
                    log::warn!("Device '{}' not found, using default device", name);
                }
                Err(e) => log::warn!("Cannot list output devices ({e}), using default device"),
            }
        }
        host.default_output_device()
            .ok_or_else(|| "no default output device".to_string())
    }
}

impl StreamFactory for CpalStreamFactory {
    fn device_rate(&self) -> Result<u32, String> {
        let device = self.resolve_device()?;
        device
            .default_output_config()
            .map(|config| config.sample_rate().0)
            .map_err(|e| format!("cannot query output config: {e}"))
    }

    fn open(&self, ctl: Arc<PlaybackCtl>) -> Result<Box<dyn Send>, String> {
        let device_source = self.device_source.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let handle = std::thread::Builder::new()
            .name("natural-voice-out".into())
            .spawn(move || {
                // The cpal `Stream` lives and dies on this thread (not `Send` everywhere).
                let factory = CpalStreamFactory::with_device_source(device_source);
                let stream = match factory.build_stream(ctl) {
                    Ok(stream) => stream,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                if let Err(e) = stream.play() {
                    let _ = ready_tx.send(Err(format!("cannot start output stream: {e}")));
                    return;
                }
                let _ = ready_tx.send(Ok(()));
                // Park until the guard signals shutdown (message or dropped sender).
                let _ = stop_rx.recv();
                drop(stream);
            })
            .map_err(|e| format!("cannot spawn audio thread: {e}"))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Box::new(StreamGuard {
                stop: Some(stop_tx),
                handle: Some(handle),
            })),
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => {
                let _ = handle.join();
                Err("audio thread exited before the stream started".to_string())
            }
        }
    }
}

impl CpalStreamFactory {
    /// Builds (does not start) the output stream. Runs on the dedicated audio thread.
    fn build_stream(&self, ctl: Arc<PlaybackCtl>) -> Result<cpal::Stream, String> {
        let device = self.resolve_device()?;
        let supported = device
            .default_output_config()
            .map_err(|e| format!("cannot query output config: {e}"))?;
        let rate = supported.sample_rate().0;
        if rate != ctl.sample_rate() {
            return Err(format!(
                "output device rate changed ({} Hz, expected {} Hz)",
                rate,
                ctl.sample_rate()
            ));
        }
        let channels = usize::from(supported.channels());
        let (buffer_size, scratch_frames) = match supported.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max }
                if *min <= PREFERRED_BUFFER_FRAMES && PREFERRED_BUFFER_FRAMES <= *max =>
            {
                (
                    cpal::BufferSize::Fixed(PREFERRED_BUFFER_FRAMES),
                    PREFERRED_BUFFER_FRAMES as usize,
                )
            }
            _ => (cpal::BufferSize::Default, 8192),
        };
        let config = cpal::StreamConfig {
            channels: supported.channels(),
            sample_rate: supported.sample_rate(),
            buffer_size,
        };
        let err_ctl = ctl.clone();
        let on_error = move |e: cpal::StreamError| {
            log::error!("natural voice output stream error: {e}");
            err_ctl.fail();
        };
        let mut latency = LatencyWindow::new(rate);
        let result = match supported.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config,
                move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                    let now = Instant::now();
                    latency.observe(info, data.len() / channels.max(1), now, &ctl);
                    fill_interleaved_f32(&ctl, data, channels, now);
                },
                on_error,
                None,
            ),
            cpal::SampleFormat::I16 => {
                let mut scratch = vec![0.0f32; scratch_frames * channels.max(1)];
                device.build_output_stream(
                    &config,
                    move |data: &mut [i16], info: &cpal::OutputCallbackInfo| {
                        let now = Instant::now();
                        latency.observe(info, data.len() / channels.max(1), now, &ctl);
                        if scratch.len() < data.len() {
                            scratch.resize(data.len(), 0.0);
                        }
                        let Some(buf) = scratch.get_mut(..data.len()) else {
                            data.fill(0);
                            return;
                        };
                        fill_interleaved_f32(&ctl, buf, channels, now);
                        for (dst, src) in data.iter_mut().zip(buf.iter()) {
                            *dst = f32_to_i16(*src);
                        }
                    },
                    on_error,
                    None,
                )
            }
            cpal::SampleFormat::U16 => {
                let mut scratch = vec![0.0f32; scratch_frames * channels.max(1)];
                device.build_output_stream(
                    &config,
                    move |data: &mut [u16], info: &cpal::OutputCallbackInfo| {
                        let now = Instant::now();
                        latency.observe(info, data.len() / channels.max(1), now, &ctl);
                        if scratch.len() < data.len() {
                            scratch.resize(data.len(), 0.0);
                        }
                        let Some(buf) = scratch.get_mut(..data.len()) else {
                            data.fill(32768);
                            return;
                        };
                        fill_interleaved_f32(&ctl, buf, channels, now);
                        for (dst, src) in data.iter_mut().zip(buf.iter()) {
                            *dst = f32_to_u16(*src);
                        }
                    },
                    on_error,
                    None,
                )
            }
            other => return Err(format!("unsupported output sample format {other:?}")),
        };
        result.map_err(|e| format!("cannot build output stream: {e}"))
    }
}

/// Closes the stream: signals the audio thread and joins it.
struct StreamGuard {
    stop: Option<mpsc::Sender<()>>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Output-latency tracker owned by the data callback: maximum of `playback - callback` over
/// the last ~10 s (1 s buckets), never below the buffer duration. Allocation-free.
struct LatencyWindow {
    rate: u32,
    buckets: [Duration; LATENCY_BUCKETS],
    next: usize,
    current: Duration,
    bucket_started: Option<Instant>,
    last_publish: Option<Instant>,
}

impl LatencyWindow {
    fn new(rate: u32) -> Self {
        Self {
            rate,
            buckets: [Duration::ZERO; LATENCY_BUCKETS],
            next: 0,
            current: Duration::ZERO,
            bucket_started: None,
            last_publish: None,
        }
    }

    fn observe(
        &mut self,
        info: &cpal::OutputCallbackInfo,
        frames: usize,
        now: Instant,
        ctl: &PlaybackCtl,
    ) {
        let ts = info.timestamp();
        if let Some(latency) = ts.playback.duration_since(&ts.callback) {
            self.current = self.current.max(latency);
        }
        let started = *self.bucket_started.get_or_insert(now);
        if now.saturating_duration_since(started) >= LATENCY_BUCKET {
            if let Some(slot) = self.buckets.get_mut(self.next) {
                *slot = self.current;
            }
            self.next = (self.next + 1) % LATENCY_BUCKETS;
            self.current = Duration::ZERO;
            self.bucket_started = Some(now);
        }
        let due = self
            .last_publish
            .is_none_or(|t| now.saturating_duration_since(t) >= TAIL_PUBLISH_EVERY);
        if due {
            let window_max = self.buckets.iter().fold(self.current, |acc, d| acc.max(*d));
            let bound = window_max.max(frames_duration(frames, self.rate));
            if ctl.try_set_tail_bound(bound) {
                self.last_publish = Some(now);
            }
        }
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Plays a 1 s 440 Hz beep on the real default output device. Run by hand:
    /// `cargo test --lib natural_live_beep -- --ignored`.
    #[test]
    #[ignore]
    fn natural_live_beep() {
        let sink = ManagedSink::new(
            Arc::new(CpalStreamFactory::new(None)),
            Duration::from_secs(3),
        );
        let rate = sink.device_rate().expect("output device rate");
        let samples: Vec<f32> = (0..rate)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / rate as f32).sin() * 0.2)
            .collect();
        assert_eq!(
            sink.commit(sink.generation(), samples),
            CommitResult::Accepted
        );
        let started = Instant::now();
        while sink.audible() && started.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!sink.audible(), "beep did not finish within 3 s");
    }
}
