//! Epoch-scoped speech queue session over `TtsManager` (design R9 stage 3).
//!
//! SKELETON: `todo!("WP-session")`; tests in `converse_queue_tests.rs` are the spec.
//!
//! One session = one conversational turn's speech. It owns ONE epoch obtained from
//! `TtsManager::begin_utterance`, so every sentence pushed through it is admitted only while
//! that epoch is still current (a newer utterance, a stop, or a recording invalidates it).
//! Once any push is refused the session is DEAD: later pushes return false without touching
//! the speaker, and `abort` becomes a no-op for the owner of any newer epoch.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;

use crate::managers::converse::{SayOutcome, SpeechQueue};
use crate::managers::converse_audio::{wait_for_speech_end, SpeechProbe, WaitOutcome};
use crate::managers::converse_platform::{speech_deadline, SpeechScope, SpeechSignal};
use crate::managers::emotion::Emotion;
use crate::managers::tts::{sanitize_speech_text, TtsManager};

/// Adapter so `wait_for_speech_end` can poll the shared TTS manager.
struct Probe<'a>(&'a TtsManager);

impl SpeechProbe for Probe<'_> {
    fn is_current(&self, epoch: u64) -> bool {
        self.0.is_current(epoch)
    }
    fn is_speaking(&self) -> bool {
        self.0.is_speaking()
    }
}

pub struct TtsQueueSession {
    tts: Arc<TtsManager>,
    epoch: u64,
    voice: Option<String>,
    rate_wpm: Option<u32>,
    dead: AtomicBool,
    chars: AtomicUsize,
    /// Speech-scope port (DESIGN D3). `None`: no scope is ever opened.
    signal: Option<Arc<dyn SpeechSignal>>,
    /// The session's one speech scope. Dropping the `Open` state (on `Closed`, or when the
    /// session itself is dropped) ends the scope, so it ends exactly once.
    scope: Mutex<ScopeState>,
}

/// Where the session's speech scope is in its life.
enum ScopeState {
    /// No push has succeeded yet, so no scope is open.
    Idle,
    /// The scope began on the first successful push and has not ended yet. The guard is only
    /// held for its `Drop`, which ends the scope.
    Open(#[allow(dead_code)] SpeechScope),
    /// The scope ended (or never began); nothing begins again.
    Closed,
}

impl TtsQueueSession {
    /// `None` unless the manager's backend is queue-capable AND `begin_utterance` succeeds
    /// (it fails while a recording is active). Consumes exactly one epoch on success.
    #[cfg_attr(not(test), allow(dead_code))] // Production opens sessions with a port.
    pub fn open(
        tts: Arc<TtsManager>,
        voice: Option<String>,
        rate_wpm: Option<u32>,
    ) -> Option<Arc<TtsQueueSession>> {
        Self::open_with_signal(tts, voice, rate_wpm, None)
    }

    /// Like `open`, and the session reports its speech scope to `signal` (DESIGN D3): begun on
    /// the first successful push, ended on finish, abort, a refused push, or drop.
    pub fn open_with_signal(
        tts: Arc<TtsManager>,
        voice: Option<String>,
        rate_wpm: Option<u32>,
        signal: Option<Arc<dyn SpeechSignal>>,
    ) -> Option<Arc<TtsQueueSession>> {
        if !tts.queue_capable() {
            return None;
        }
        let epoch = tts.begin_utterance().ok()?;
        Some(Arc::new(TtsQueueSession {
            tts,
            epoch,
            voice,
            rate_wpm,
            dead: AtomicBool::new(false),
            chars: AtomicUsize::new(0),
            signal,
            scope: Mutex::new(ScopeState::Idle),
        }))
    }

    /// Begins the speech scope on the first successful push only (never again after it ended).
    fn begin_scope(&self) {
        let Some(signal) = self.signal.as_ref() else {
            return;
        };
        let mut scope = self.lock_scope();
        if matches!(*scope, ScopeState::Idle) {
            *scope = ScopeState::Open(SpeechScope::open(signal.clone()));
        }
    }

    /// Ends the speech scope, if one is open. Idempotent.
    fn end_scope(&self) {
        *self.lock_scope() = ScopeState::Closed;
    }

    fn lock_scope(&self) -> std::sync::MutexGuard<'_, ScopeState> {
        self.scope
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Like `SpeechQueue::finish` with an explicit overall deadline (tests use short ones).
    /// The speech scope ends when this returns, whatever the outcome.
    pub async fn finish_within(&self, deadline: Duration) -> SayOutcome {
        let probe = Probe(&self.tts);
        let outcome = match wait_for_speech_end(
            &probe,
            self.epoch,
            deadline,
            Duration::from_millis(25),
        )
        .await
        {
            WaitOutcome::Done => SayOutcome::Done,
            WaitOutcome::Interrupted => SayOutcome::Interrupted,
            WaitOutcome::TimedOut => {
                let _ = self.tts.stop_if_current(self.epoch);
                SayOutcome::Failed
            }
        };
        self.end_scope();
        outcome
    }
}

impl SpeechQueue for TtsQueueSession {
    /// Empty/unspeakable text => `true` with no effect. Otherwise
    /// `tts.speak_queued_if_current(epoch, ..)`: Ok => true (adds to the char total used for
    /// the finish deadline); any error => the session becomes dead and false is returned.
    /// A dead session returns false immediately.
    fn push(&self, text: &str, emotion: Emotion) -> bool {
        if self.dead.load(Ordering::Relaxed) {
            return false;
        }
        let Some(sanitized) = sanitize_speech_text(text) else {
            return true;
        };
        match self.tts.speak_queued_if_current(
            self.epoch,
            text,
            self.voice.as_deref(),
            self.rate_wpm,
            emotion,
        ) {
            Ok(()) => {
                self.chars
                    .fetch_add(sanitized.chars().count(), Ordering::Relaxed);
                self.begin_scope();
                true
            }
            Err(_) => {
                self.dead.store(true, Ordering::Relaxed);
                self.end_scope();
                false
            }
        }
    }

    /// Marks the session dead, ends its speech scope and `tts.stop_if_current(epoch)` (never
    /// disturbs the speech of a newer epoch's owner).
    fn abort(&self) {
        self.dead.store(true, Ordering::Relaxed);
        self.end_scope();
        let _ = self.tts.stop_if_current(self.epoch);
    }

    /// Deadline = `converse_platform::speech_deadline` of a string with the same word count
    /// as everything pushed (10 s floor). Equivalent to `finish_within(that)`.
    fn finish(&self) -> BoxFuture<'_, SayOutcome> {
        // Word estimate: about 6 characters per word, plus one for the first word.
        let words = self.chars.load(Ordering::Relaxed) / 6 + 1;
        let deadline = speech_deadline(&"w ".repeat(words));
        Box::pin(async move { self.finish_within(deadline).await })
    }
}

#[cfg(test)]
#[path = "converse_queue_tests.rs"]
mod tests;
