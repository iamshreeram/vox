//! Epoch-scoped speech queue session over `TtsManager` (design R9 stage 3).
//!
//! SKELETON: `todo!("WP-session")`; tests in `converse_queue_tests.rs` are the spec.
//!
//! One session = one conversational turn's speech. It owns ONE epoch obtained from
//! `TtsManager::begin_utterance`, so every sentence pushed through it is admitted only while
//! that epoch is still current (a newer utterance, a stop, or a recording invalidates it).
//! Once any push is refused the session is DEAD: later pushes return false without touching
//! the speaker, and `abort` becomes a no-op for the owner of any newer epoch.

use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;

use crate::managers::converse::{SayOutcome, SpeechQueue};
use crate::managers::emotion::Emotion;
use crate::managers::tts::TtsManager;

pub struct TtsQueueSession {
    _tts: Arc<TtsManager>,
    _epoch: u64,
    _voice: Option<String>,
    _rate_wpm: Option<u32>,
    _dead: AtomicBool,
    _chars: AtomicUsize,
}

impl TtsQueueSession {
    /// `None` unless the manager's backend is queue-capable AND `begin_utterance` succeeds
    /// (it fails while a recording is active). Consumes exactly one epoch on success.
    pub fn open(
        _tts: Arc<TtsManager>,
        _voice: Option<String>,
        _rate_wpm: Option<u32>,
    ) -> Option<Arc<TtsQueueSession>> {
        todo!("WP-session")
    }

    /// Like `SpeechQueue::finish` with an explicit overall deadline (tests use short ones).
    pub async fn finish_within(&self, _deadline: Duration) -> SayOutcome {
        todo!("WP-session")
    }
}

impl SpeechQueue for TtsQueueSession {
    /// Empty/unspeakable text => `true` with no effect. Otherwise
    /// `tts.speak_queued_if_current(epoch, ..)`: Ok => true (adds to the char total used for
    /// the finish deadline); any error => the session becomes dead and false is returned.
    /// A dead session returns false immediately.
    fn push(&self, _text: &str, _emotion: Emotion) -> bool {
        todo!("WP-session")
    }

    /// Marks the session dead and `tts.stop_if_current(epoch)` (never disturbs the speech of a
    /// newer epoch's owner).
    fn abort(&self) {
        todo!("WP-session")
    }

    /// Deadline = `converse_platform::speech_deadline` of a string with the same word count
    /// as everything pushed (10 s floor). Equivalent to `finish_within(that)`.
    fn finish(&self) -> BoxFuture<'_, SayOutcome> {
        todo!("WP-session")
    }
}

#[cfg(test)]
#[path = "converse_queue_tests.rs"]
mod tests;
