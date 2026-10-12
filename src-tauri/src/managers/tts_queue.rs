//! Resident, queue-capable speech (design R5-R9, stage 2; simplified by measurement: a
//! resident AVSpeechSynthesizer needs no delegate - polling `isSpeaking` works and queued
//! utterances play gaplessly).
//!
//! SKELETON: bodies are `todo!("WP-queue")`; tests in `tts_queue_tests.rs` are the spec.
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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::managers::emotion::Emotion;
use crate::managers::tts::{SpeechBackend, SpeechHandle};

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    /// Empty/whitespace-only text; nothing was spoken.
    Empty,
    /// A previous stop failed and the synth is still speaking.
    Stuck,
}

pub struct SpeakerCore<S: Synth> {
    _synth: S,
}

impl<S: Synth> SpeakerCore<S> {
    pub fn new(_synth: S) -> Self {
        todo!("WP-queue")
    }

    /// Appends `text` (trimmed; empty => `Err(Empty)`). `Err(Stuck)` while a failed stop is
    /// unresolved. Forwards voice/rate/pitch verbatim. Marks the core busy and records `now`
    /// as the start-watchdog origin when the core was idle.
    pub fn enqueue(
        &mut self,
        _text: &str,
        _voice: Option<&str>,
        _rate_wpm: Option<u32>,
        _pitch: f32,
        _now: Instant,
    ) -> Result<(), QueueError> {
        todo!("WP-queue")
    }

    /// Refreshes state from the synth and returns whether the core is busy. Becomes idle
    /// when the synth was seen speaking and has stopped, or when it never started within
    /// `START_TIMEOUT` of the watchdog origin (counts a start failure). Also clears a
    /// `Stuck` condition once the synth is genuinely quiet.
    pub fn poll(&mut self, _now: Instant) -> bool {
        todo!("WP-queue")
    }

    /// Sentences enqueued since the core was last idle.
    pub fn pending(&self) -> usize {
        todo!("WP-queue")
    }

    /// Returns and clears the number of start-watchdog failures.
    pub fn take_start_failures(&mut self) -> usize {
        todo!("WP-queue")
    }

    /// Queue-wide stop. Not busy and the synth quiet => `AlreadyIdle` WITHOUT calling the
    /// synth. Otherwise `synth.stop_immediate()`: Stopped/AlreadyIdle => idle (pending 0);
    /// Failed => remains busy and `enqueue` returns `Stuck` until a later `poll` sees the
    /// synth quiet.
    pub fn stop(&mut self, _now: Instant) -> StopOutcome {
        todo!("WP-queue")
    }
}

/// Shared core handle used by the backend, its handles and (on macOS) the real synth.
pub type SharedCore<S> = Arc<Mutex<SpeakerCore<S>>>;

/// The `SpeechHandle` onto the shared queue: `is_running` = core busy (polls), `kill` =
/// queue-wide stop (true for Stopped/AlreadyIdle, false for Failed), `queue_capable` = true.
pub struct QueueHandle<S: Synth> {
    _core: SharedCore<S>,
}

impl<S: Synth + 'static> SpeechHandle for QueueHandle<S> {
    fn kill(&mut self) -> bool {
        todo!("WP-queue")
    }
    fn is_running(&mut self) -> bool {
        todo!("WP-queue")
    }
    fn queue_capable(&self) -> bool {
        todo!("WP-queue")
    }
}

/// Adapts a shared core to `SpeechBackend`. `spawn`/`spawn_styled` enqueue one utterance and
/// return a handle (the manager has already killed any previous handle); `enqueue_styled`
/// appends without any replacement; `supports_queue` is true. Emotion maps to pitch via
/// `pitch_for` and to rate via the same multiplier the `say` backend uses.
pub struct QueueBackend<S: Synth> {
    _core: SharedCore<S>,
}

impl<S: Synth + 'static> QueueBackend<S> {
    pub fn new(_core: SharedCore<S>) -> Self {
        todo!("WP-queue")
    }
}

impl<S: Synth + 'static> SpeechBackend for QueueBackend<S> {
    fn spawn(
        &self,
        _text: &str,
        _voice: Option<&str>,
        _rate_wpm: Option<u32>,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        todo!("WP-queue")
    }
    fn spawn_styled(
        &self,
        _text: &str,
        _voice: Option<&str>,
        _rate_wpm: Option<u32>,
        _emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        todo!("WP-queue")
    }
    fn supports_queue(&self) -> bool {
        todo!("WP-queue")
    }
    fn enqueue_styled(
        &self,
        _text: &str,
        _voice: Option<&str>,
        _rate_wpm: Option<u32>,
        _emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        todo!("WP-queue")
    }
}

/// Chooses between two backends at CALL time (so a settings toggle takes effect without a
/// restart): `pick_fast() == true` routes everything to `fast`, otherwise to `slow`.
/// `supports_queue` and `enqueue_styled` follow the same choice; the slow side never queues.
/// `pick_fast` is evaluated once per call.
pub struct SwitchBackend {
    _slow: Box<dyn SpeechBackend>,
    _fast: Box<dyn SpeechBackend>,
    _pick_fast: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl SwitchBackend {
    pub fn new(
        _slow: Box<dyn SpeechBackend>,
        _fast: Box<dyn SpeechBackend>,
        _pick_fast: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        todo!("WP-queue")
    }
}

impl SpeechBackend for SwitchBackend {
    fn spawn(
        &self,
        _text: &str,
        _voice: Option<&str>,
        _rate_wpm: Option<u32>,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        todo!("WP-queue")
    }
    fn spawn_styled(
        &self,
        _text: &str,
        _voice: Option<&str>,
        _rate_wpm: Option<u32>,
        _emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        todo!("WP-queue")
    }
    fn supports_queue(&self) -> bool {
        todo!("WP-queue")
    }
    fn enqueue_styled(
        &self,
        _text: &str,
        _voice: Option<&str>,
        _rate_wpm: Option<u32>,
        _emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        todo!("WP-queue")
    }
}

/// Pitch multiplier (AVSpeechUtterance range 0.5..=2.0) for an emotion; Neutral is 1.0.
pub fn pitch_for(_emotion: Emotion) -> f32 {
    todo!("WP-queue")
}

#[cfg(test)]
#[path = "tts_queue_tests.rs"]
pub(crate) mod tests;
