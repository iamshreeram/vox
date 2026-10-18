//! Queue-capable adapter over a non-queue `SpeechBackend` (the system `say` backend): lets the
//! natural-voice router fall back to the system voice WITHOUT losing the "append to a live
//! queue" behaviour that `TtsManager` relies on while the natural engine is selected.
//!
//! Spec = `tts_serial_tests.rs`. STUB: bodies are `todo!()` until step S5a. Signatures and doc
//! contracts are binding. DESIGN.md D3 describes the intent.
#![allow(dead_code)]

use crate::managers::emotion::Emotion;
use crate::managers::tts::{SpeechBackend, SpeechHandle};
use std::io;

/// Items allowed to WAIT (not counting the one being spoken); further appends are refused.
pub const SERIAL_MAX_PENDING: usize = 16;

/// One thread runs one inner utterance at a time from a FIFO.
/// * `supports_queue()` is true; `spawn`, `spawn_styled` and `enqueue_styled` ALL append to the
///   FIFO (the manager has already killed previous speech before a `spawn`) and return a handle
///   that is `queue_capable()` and shared (every handle controls the whole queue).
/// * A full FIFO (`SERIAL_MAX_PENDING` waiting) refuses with `ErrorKind::WouldBlock`.
/// * The worker pops an item, starts it with `inner.spawn_styled(text, voice, rate, emotion)`
///   (an `Err` skips that item and continues), polls the inner handle's `is_running()` every
///   <= 10 ms, and starts the next item only after the child has exited.
/// * `is_running()` is true while the FIFO is non-empty OR an item is being started/spoken.
/// * `kill()` clears the FIFO, kills the current child and returns true if there was no child
///   or the inner `kill()` confirmed; false if the inner kill failed (the child is then still
///   running and `is_running()` stays true). An item that was popped but is still being started
///   when `kill()` runs must be killed as soon as it has started (generation check) and never
///   keep speaking.
/// * Dropping the adapter stops the worker and kills a running child.
pub struct SerializedQueue {
    _private: (),
}

impl SerializedQueue {
    pub fn new(inner: Box<dyn SpeechBackend>) -> Self {
        let _ = inner;
        todo!("S5a")
    }
}

impl SpeechBackend for SerializedQueue {
    fn spawn(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        let _ = (text, voice, rate_wpm);
        todo!("S5a")
    }
    fn spawn_styled(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        let _ = (text, voice, rate_wpm, emotion);
        todo!("S5a")
    }
    fn supports_queue(&self) -> bool {
        todo!("S5a")
    }
    fn enqueue_styled(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        let _ = (text, voice, rate_wpm, emotion);
        todo!("S5a")
    }
}
