//! RED tests for `TtsManager::speak_if_idle` (M3 S1b, DESIGN R2-6).
//! Uses a self-contained fake speaker; no real speech process is started.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Default)]
struct IdleLog {
    spawns: Mutex<Vec<String>>,
    kills: AtomicUsize,
    /// Set to true to make the fake process report that it has finished.
    finished: AtomicBool,
}

impl IdleLog {
    fn spawns(&self) -> Vec<String> {
        self.spawns.lock().unwrap().clone()
    }
}

struct IdleBackend(Arc<IdleLog>);

struct IdleHandle {
    log: Arc<IdleLog>,
    alive: bool,
}

impl SpeechBackend for IdleBackend {
    fn spawn(
        &self,
        text: &str,
        _voice: Option<&str>,
        _rate: Option<u32>,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.0.spawns.lock().unwrap().push(text.to_string());
        Ok(Box::new(IdleHandle {
            log: self.0.clone(),
            alive: true,
        }))
    }
}

impl SpeechHandle for IdleHandle {
    fn kill(&mut self) -> bool {
        if self.alive {
            self.alive = false;
            self.log.kills.fetch_add(1, Ordering::SeqCst);
        }
        true
    }
    fn is_running(&mut self) -> bool {
        if self.log.finished.load(Ordering::SeqCst) {
            self.alive = false;
        }
        self.alive
    }
}

fn idle_manager() -> (TtsManager, Arc<IdleLog>) {
    let log = Arc::new(IdleLog::default());
    (TtsManager::new(Box::new(IdleBackend(log.clone()))), log)
}

#[test]
fn speak_if_idle_refuses_empty_text_without_spawning() {
    let (tts, log) = idle_manager();
    assert!(!tts.speak_if_idle(""));
    assert!(!tts.speak_if_idle("   \n\t "));
    assert!(!tts.speak_if_idle("\u{0}\u{1}"));
    assert!(!tts.speak_if_idle("[[ ]]"), "markup-only text is empty");
    assert!(log.spawns().is_empty());
    assert!(!tts.is_speaking());
}

#[test]
fn speak_if_idle_starts_and_returns_true_when_idle() {
    let (tts, log) = idle_manager();
    assert!(tts.speak_if_idle("hello there"));
    assert_eq!(log.spawns(), vec!["hello there".to_string()]);
    assert!(tts.is_speaking());
}

#[test]
fn speak_if_idle_sanitizes_the_text_before_speaking() {
    let (tts, log) = idle_manager();
    assert!(tts.speak_if_idle("  hello\n\tworld  "));
    assert_eq!(log.spawns(), vec!["hello world".to_string()]);
}

#[test]
fn speak_if_idle_refuses_while_speech_is_in_progress_and_leaves_it_alone() {
    let (tts, log) = idle_manager();
    tts.speak("first", None, None).unwrap();
    assert!(tts.is_speaking());
    assert!(
        !tts.speak_if_idle("second"),
        "must not start over current speech"
    );
    assert_eq!(log.spawns(), vec!["first".to_string()], "no second spawn");
    assert_eq!(
        log.kills.load(Ordering::SeqCst),
        0,
        "speech in progress is not stopped"
    );
    assert!(tts.is_speaking(), "original speech still running");
}

#[test]
fn speak_if_idle_is_allowed_again_after_speech_finishes_naturally() {
    let (tts, log) = idle_manager();
    assert!(tts.speak_if_idle("one"));
    log.finished.store(true, Ordering::SeqCst);
    assert!(!tts.is_speaking());
    assert!(tts.speak_if_idle("two"));
    assert_eq!(log.spawns(), vec!["one".to_string(), "two".to_string()]);
}

#[test]
fn speak_if_idle_refuses_while_recording_and_spawns_nothing() {
    let (tts, log) = idle_manager();
    tts.note_recording_started();
    assert!(!tts.speak_if_idle("during recording"));
    assert!(log.spawns().is_empty());
    tts.note_recording_stopped();
    assert!(tts.speak_if_idle("after recording"));
    assert_eq!(log.spawns(), vec!["after recording".to_string()]);
}

#[test]
fn speak_if_idle_consumes_no_epoch_of_an_existing_session() {
    let (tts, _log) = idle_manager();
    let epoch = tts.begin_utterance().unwrap();
    assert!(tts.speak_if_idle("background note"));
    assert!(
        tts.is_current(epoch),
        "speak_if_idle must not bump the epoch"
    );
    assert!(
        tts.speak_styled_if_current(epoch, "session", None, None, Emotion::Neutral)
            .is_ok(),
        "a session that began before speak_if_idle can still speak"
    );
}

#[test]
fn speak_if_idle_does_not_supersede_a_pending_session_epoch() {
    let (tts, _log) = idle_manager();
    let stale = tts.begin_utterance().unwrap();
    assert!(tts.speak_if_idle("aside"));
    assert!(
        tts.is_current(stale),
        "an aside must not supersede an older session"
    );
    assert!(tts.is_speaking(), "the aside really started");
}
