//! Resident, queue-capable speech (design R5-R9, stage 2; simplified by measurement: a
//! resident AVSpeechSynthesizer needs no delegate - polling `isSpeaking` works and queued
//! utterances play gaplessly).
//!
//! * `Synth` abstracts the platform synthesizer (real impl: `tts_av.rs`, macOS only).
//! * `SpeakerCore` is the queue state machine; all time is injected (`now`), no threads.
//!   `busy` = something was enqueued and the synth has not yet been seen to start AND finish
//!   (or the start watchdog has not expired). Stop is queue-wide and reports a TRUTHFUL
//!   outcome; a failed stop leaves the core busy and refuses new speech (`Stuck`) until the
//!   synth really goes quiet.
//! * `QueueBackend<S>` adapts a shared core to `tts::SpeechBackend`; `QueueHandle<S>` is the
//!   `SpeechHandle` onto the shared queue.

use std::io;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::managers::emotion::{effective_rate, Emotion};
use crate::managers::tts::{clamp_rate, sanitize_voice, SpeechBackend, SpeechHandle};

/// A synth that never reports speaking within this long after an enqueue is declared failed
/// (the core goes idle and counts a start failure).
pub const START_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    /// It was speaking and is now silent.
    Stopped,
    /// It was not speaking.
    AlreadyIdle,
    /// Still speaking after the stop request.
    Failed,
}

pub trait Synth: Send {
    /// Appends one utterance to the synthesizer's own queue (returns immediately).
    fn speak(&mut self, text: &str, voice: Option<&str>, rate_wpm: Option<u32>, pitch: f32);
    /// True while anything is playing or queued inside the synthesizer.
    fn is_speaking(&mut self) -> bool;
    /// Cancels everything immediately and reports the truthful result (re-checks speaking).
    fn stop_immediate(&mut self) -> StopOutcome;
    /// Called by the core every time it transitions to idle (natural completion, start
    /// watchdog expiry, confirmed stop, or recovery from a failed stop). The real macOS
    /// synthesizer drops its instance here: a stopped/reused AVSpeechSynthesizer is not
    /// reliable, a fresh one per burst is. Never called while the synth may be speaking.
    /// Default: no-op.
    fn reset(&mut self) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    /// Empty/whitespace-only text; nothing was spoken.
    Empty,
    /// A previous stop failed and the synth is still speaking.
    Stuck,
}

pub struct SpeakerCore<S: Synth> {
    synth: S,
    busy: bool,
    pending: usize,
    started_seen: bool,
    origin: Instant,
    stuck: bool,
    start_failures: usize,
}

impl<S: Synth> SpeakerCore<S> {
    pub fn new(synth: S) -> Self {
        Self {
            synth,
            busy: false,
            pending: 0,
            started_seen: false,
            origin: Instant::now(),
            stuck: false,
            start_failures: 0,
        }
    }

    /// Returns the core to idle (nothing enqueued, no stop failure outstanding).
    fn go_idle(&mut self) {
        self.busy = false;
        self.pending = 0;
        self.stuck = false;
        self.started_seen = false;
    }

    /// Clears a `Stuck` condition once the synth is genuinely quiet.
    fn refresh_stuck(&mut self) {
        if self.stuck && !self.synth.is_speaking() {
            self.go_idle();
        }
    }

    /// Appends `text` (trimmed; empty => `Err(Empty)`). `Err(Stuck)` while a failed stop is
    /// unresolved. Forwards voice/rate/pitch verbatim. Marks the core busy and records `now`
    /// as the start-watchdog origin when the core was idle.
    pub fn enqueue(
        &mut self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        pitch: f32,
        now: Instant,
    ) -> Result<(), QueueError> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(QueueError::Empty);
        }
        self.refresh_stuck();
        if self.stuck {
            return Err(QueueError::Stuck);
        }
        if !self.busy {
            self.busy = true;
            self.started_seen = false;
            self.origin = now;
            self.pending = 0;
        }
        self.pending += 1;
        self.synth.speak(trimmed, voice, rate_wpm, pitch);
        Ok(())
    }

    /// Refreshes state from the synth and returns whether the core is busy. Becomes idle
    /// when the synth was seen speaking and has stopped, or when it never started within
    /// `START_TIMEOUT` of the watchdog origin (counts a start failure). Also clears a
    /// `Stuck` condition once the synth is genuinely quiet.
    pub fn poll(&mut self, now: Instant) -> bool {
        self.refresh_stuck();
        if !self.busy {
            return false;
        }
        if self.synth.is_speaking() {
            self.started_seen = true;
            return true;
        }
        if self.started_seen {
            self.go_idle();
            return false;
        }
        if now >= self.origin + START_TIMEOUT {
            self.go_idle();
            self.start_failures += 1;
            return false;
        }
        true
    }

    /// Sentences enqueued since the core was last idle.
    pub fn pending(&self) -> usize {
        self.pending
    }

    /// Returns and clears the number of start-watchdog failures.
    pub fn take_start_failures(&mut self) -> usize {
        std::mem::take(&mut self.start_failures)
    }

    /// Queue-wide stop. Not busy and the synth quiet => `AlreadyIdle` WITHOUT calling the
    /// synth. Otherwise `synth.stop_immediate()`: Stopped/AlreadyIdle => idle (pending 0);
    /// Failed => remains busy and `enqueue` returns `Stuck` until a later `poll` sees the
    /// synth quiet.
    pub fn stop(&mut self, _now: Instant) -> StopOutcome {
        if !self.busy && !self.synth.is_speaking() {
            return StopOutcome::AlreadyIdle;
        }
        match self.synth.stop_immediate() {
            StopOutcome::Failed => {
                self.stuck = true;
                self.busy = true;
                StopOutcome::Failed
            }
            outcome => {
                self.go_idle();
                outcome
            }
        }
    }
}

/// Shared core handle used by the backend, its handles and (on macOS) the real synth.
pub type SharedCore<S> = Arc<Mutex<SpeakerCore<S>>>;

/// Locks the shared core, recovering from poisoning instead of panicking.
fn lock_core<S: Synth>(core: &SharedCore<S>) -> MutexGuard<'_, SpeakerCore<S>> {
    core.lock().unwrap_or_else(|e| e.into_inner())
}

/// The `SpeechHandle` onto the shared queue: `is_running` = core busy (polls), `kill` =
/// queue-wide stop (true for Stopped/AlreadyIdle, false for Failed), `queue_capable` = true.
pub struct QueueHandle<S: Synth> {
    core: SharedCore<S>,
}

impl<S: Synth + 'static> SpeechHandle for QueueHandle<S> {
    fn kill(&mut self) -> bool {
        lock_core(&self.core).stop(Instant::now()) != StopOutcome::Failed
    }
    fn is_running(&mut self) -> bool {
        lock_core(&self.core).poll(Instant::now())
    }
    fn queue_capable(&self) -> bool {
        true
    }
}

/// Adapts a shared core to `SpeechBackend`. `spawn`/`spawn_styled` enqueue one utterance and
/// return a handle (the manager has already killed any previous handle); `enqueue_styled`
/// appends without any replacement; `supports_queue` is true. Emotion maps to pitch via
/// `pitch_for` and to rate via the same multiplier the `say` backend uses.
pub struct QueueBackend<S: Synth> {
    core: SharedCore<S>,
}

impl<S: Synth + 'static> QueueBackend<S> {
    pub fn new(core: SharedCore<S>) -> Self {
        Self { core }
    }
}

/// Enqueues one utterance on `core` and returns a handle onto the shared queue.
fn enqueue_on<S: Synth + 'static>(
    core: &SharedCore<S>,
    text: &str,
    voice: Option<&str>,
    rate_wpm: Option<u32>,
    emotion: Emotion,
) -> io::Result<Box<dyn SpeechHandle>> {
    let voice = voice.and_then(sanitize_voice);
    let rate = effective_rate(rate_wpm.map(clamp_rate), emotion);
    let pitch = pitch_for(emotion);
    let result = lock_core(core).enqueue(text, voice.as_deref(), rate, pitch, Instant::now());
    match result {
        Ok(()) => Ok(Box::new(QueueHandle { core: core.clone() })),
        Err(QueueError::Empty) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "nothing speakable",
        )),
        Err(QueueError::Stuck) => Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "previous speech could not be stopped",
        )),
    }
}

impl<S: Synth + 'static> SpeechBackend for QueueBackend<S> {
    fn spawn(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        enqueue_on(&self.core, text, voice, rate_wpm, Emotion::Neutral)
    }
    fn spawn_styled(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        enqueue_on(&self.core, text, voice, rate_wpm, emotion)
    }
    fn supports_queue(&self) -> bool {
        true
    }
    fn enqueue_styled(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        enqueue_on(&self.core, text, voice, rate_wpm, emotion)
    }
}

/// Chooses between two backends at CALL time (so a settings toggle takes effect without a
/// restart): `pick_fast() == true` routes everything to `fast`, otherwise to `slow`.
/// `supports_queue` and `enqueue_styled` follow the same choice; the slow side never queues.
/// `pick_fast` is evaluated once per call.
pub struct SwitchBackend {
    slow: Box<dyn SpeechBackend>,
    fast: Box<dyn SpeechBackend>,
    pick_fast: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl SwitchBackend {
    pub fn new(
        slow: Box<dyn SpeechBackend>,
        fast: Box<dyn SpeechBackend>,
        pick_fast: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        Self {
            slow,
            fast,
            pick_fast,
        }
    }
}

impl SpeechBackend for SwitchBackend {
    fn spawn(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        if (self.pick_fast)() {
            self.fast.spawn(text, voice, rate_wpm)
        } else {
            self.slow.spawn(text, voice, rate_wpm)
        }
    }
    fn spawn_styled(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        if (self.pick_fast)() {
            self.fast.spawn_styled(text, voice, rate_wpm, emotion)
        } else {
            self.slow.spawn_styled(text, voice, rate_wpm, emotion)
        }
    }
    fn supports_queue(&self) -> bool {
        (self.pick_fast)() && self.fast.supports_queue()
    }
    fn enqueue_styled(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        if (self.pick_fast)() {
            self.fast.enqueue_styled(text, voice, rate_wpm, emotion)
        } else {
            Err(io::Error::from(io::ErrorKind::Unsupported))
        }
    }
}

/// Pitch multiplier (AVSpeechUtterance range 0.5..=2.0) for an emotion; Neutral is 1.0.
/// Uses the same semitone delta as the `say` pitch prefix, at 2% per semitone.
pub fn pitch_for(emotion: Emotion) -> f32 {
    let semitones = f32::from(emotion.prosody().pitch_delta);
    (1.0 + semitones * 0.02).clamp(0.5, 2.0)
}

#[cfg(test)]
#[path = "tts_queue_tests.rs"]
pub(crate) mod tests;
