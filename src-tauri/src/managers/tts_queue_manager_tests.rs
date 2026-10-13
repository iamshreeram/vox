//! Authoritative tests for `TtsManager::speak_queued_if_current` and queue-capable backends.
//! Included from tts.rs; do not edit to make code pass.

use super::*;
use crate::managers::tts_queue::tests::Fake;
use crate::managers::tts_queue::{QueueBackend, SharedCore, SpeakerCore, StopOutcome};
use std::sync::{Arc, Mutex};

fn mgr_q() -> (TtsManager, Fake) {
    let f = Fake::default();
    let core: SharedCore<Fake> = Arc::new(Mutex::new(SpeakerCore::new(f.clone())));
    (TtsManager::new(Box::new(QueueBackend::new(core))), f)
}

fn q(m: &TtsManager, epoch: u64, text: &str) -> Result<(), TtsError> {
    m.speak_queued_if_current(epoch, text, None, Some(180), Emotion::Neutral)
}

#[test]
fn queued_sentences_accumulate_without_killing_each_other() {
    let (m, f) = mgr_q();
    assert!(m.queue_capable());
    let e = m.begin_utterance().unwrap();
    for s in ["One.", "Two.", "Three."] {
        q(&m, e, s).unwrap();
    }
    assert_eq!(f.spoken_texts(), vec!["One.", "Two.", "Three."]);
    assert_eq!(f.stop_calls(), 0, "queuing must never stop earlier speech");
    f.set_speaking(true);
    assert!(m.is_speaking());
    assert!(m.is_current(e), "queuing consumes no epoch");
}

#[test]
fn stale_epoch_is_superseded_without_touching_the_synth() {
    let (m, f) = mgr_q();
    let e1 = m.begin_utterance().unwrap();
    let _e2 = m.begin_utterance().unwrap();
    assert_eq!(q(&m, e1, "Late."), Err(TtsError::Superseded));
    assert!(f.spoken_texts().is_empty());
}

#[test]
fn empty_text_is_rejected_and_keeps_the_epoch() {
    let (m, f) = mgr_q();
    let e = m.begin_utterance().unwrap();
    assert_eq!(q(&m, e, "  \n "), Err(TtsError::Empty));
    assert!(m.is_current(e));
    assert!(f.spoken_texts().is_empty());
}

#[test]
fn text_is_sanitized_like_every_other_speech_path() {
    let (m, f) = mgr_q();
    let e = m.begin_utterance().unwrap();
    let raw = "\u{1b}[31mHello\u{1b}[0m world\u{7}";
    q(&m, e, raw).unwrap();
    assert_eq!(f.spoken_texts(), vec![sanitize_speech_text(raw).unwrap()]);
}

#[test]
fn stop_kills_the_whole_queue_bumps_the_epoch_and_refuses_stale_chunks() {
    let (m, f) = mgr_q();
    let e = m.begin_utterance().unwrap();
    q(&m, e, "One.").unwrap();
    q(&m, e, "Two.").unwrap();
    f.set_speaking(true);
    assert!(m.stop());
    assert_eq!(
        f.stop_calls(),
        1,
        "one queue-wide stop, not one per sentence"
    );
    assert!(!m.is_speaking());
    assert_eq!(q(&m, e, "Three."), Err(TtsError::Superseded));
    assert_eq!(f.spoken_texts(), vec!["One.", "Two."]);
    // a fresh intent works again
    let e2 = m.begin_utterance().unwrap();
    q(&m, e2, "Fresh.").unwrap();
    assert_eq!(f.spoken_texts(), vec!["One.", "Two.", "Fresh."]);
}

#[test]
fn recording_stops_the_queue_and_blocks_new_speech() {
    let (m, f) = mgr_q();
    let e = m.begin_utterance().unwrap();
    q(&m, e, "One.").unwrap();
    f.set_speaking(true);
    m.note_recording_started();
    assert_eq!(f.stop_calls(), 1);
    assert_eq!(m.begin_utterance(), Err(TtsError::Recording));
    assert_eq!(q(&m, e, "Two."), Err(TtsError::Superseded));
    m.note_recording_stopped();
    let e2 = m.begin_utterance().unwrap();
    q(&m, e2, "After.").unwrap();
    assert_eq!(f.spoken_texts(), vec!["One.", "After."]);
}

#[test]
fn a_failed_os_stop_leaves_speech_stuck_and_refuses_queued_chunks() {
    let (m, f) = mgr_q();
    let e = m.begin_utterance().unwrap();
    q(&m, e, "One.").unwrap();
    f.set_speaking(true);
    f.set_stop_result(StopOutcome::Failed);
    assert!(!m.stop(), "stop reports the truth");
    assert!(m.is_speaking(), "still speaking");
    let e2 = m.begin_utterance().unwrap();
    assert_eq!(q(&m, e2, "Two."), Err(TtsError::CancelFailed));
    assert_eq!(f.spoken_texts(), vec!["One."]);
}

#[test]
fn a_normal_speak_replaces_queued_speech() {
    let (m, f) = mgr_q();
    let e = m.begin_utterance().unwrap();
    q(&m, e, "One.").unwrap();
    q(&m, e, "Two.").unwrap();
    f.set_speaking(true);
    m.speak_styled_if_current(e, "Replacement.", None, Some(180), Emotion::Neutral)
        .unwrap();
    assert_eq!(f.stop_calls(), 1, "the queue was stopped once");
    assert_eq!(
        f.spoken_texts().last().map(String::as_str),
        Some("Replacement.")
    );
}

#[test]
fn is_speaking_follows_the_synth_and_goes_false_when_it_finishes() {
    let (m, f) = mgr_q();
    let e = m.begin_utterance().unwrap();
    q(&m, e, "One.").unwrap();
    f.set_speaking(true);
    assert!(m.is_speaking());
    f.set_speaking(false);
    assert!(!m.is_speaking());
    // and a later queued sentence starts a new run on the same manager
    q(&m, e, "Two.").unwrap();
    f.set_speaking(true);
    assert!(m.is_speaking());
}

#[test]
fn a_backend_without_queue_support_refuses_queued_speech() {
    struct Plain;
    struct H;
    impl SpeechHandle for H {
        fn kill(&mut self) -> bool {
            true
        }
        fn is_running(&mut self) -> bool {
            false
        }
    }
    impl SpeechBackend for Plain {
        fn spawn(
            &self,
            _t: &str,
            _v: Option<&str>,
            _r: Option<u32>,
        ) -> io::Result<Box<dyn SpeechHandle>> {
            Ok(Box::new(H))
        }
    }
    let m = TtsManager::new(Box::new(Plain));
    assert!(!m.queue_capable());
    let e = m.begin_utterance().unwrap();
    assert_eq!(q(&m, e, "Hi."), Err(TtsError::Unsupported));
    assert!(m.is_current(e), "a refused queue attempt consumes nothing");
}

#[test]
fn concurrent_queued_speech_from_many_threads_is_complete_and_panic_free() {
    let (m, f) = mgr_q();
    let m = Arc::new(m);
    let e = m.begin_utterance().unwrap();
    let mut ts = vec![];
    for i in 0..8 {
        let m = m.clone();
        ts.push(std::thread::spawn(move || {
            for j in 0..10 {
                m.speak_queued_if_current(e, &format!("t{i}-{j}"), None, None, Emotion::Neutral)
                    .unwrap();
            }
        }));
    }
    for t in ts {
        t.join().unwrap();
    }
    assert_eq!(f.spoken_texts().len(), 80);
    assert_eq!(f.stop_calls(), 0);
}

#[test]
fn stop_if_current_stops_only_for_the_matching_epoch() {
    let (m, f) = mgr_q();
    let e = m.begin_utterance().unwrap();
    q(&m, e, "One.").unwrap();
    f.set_speaking(true);
    assert!(m.stop_if_current(e));
    assert_eq!(f.stop_calls(), 1);
    assert!(!m.is_current(e), "stop bumps the epoch like stop()");
    assert!(!m.is_speaking());
}

#[test]
fn stop_if_current_with_a_stale_epoch_is_a_no_op_that_reports_true() {
    let (m, f) = mgr_q();
    let old = m.begin_utterance().unwrap();
    let new = m.begin_utterance().unwrap();
    q(&m, new, "Theirs.").unwrap();
    f.set_speaking(true);
    assert!(m.stop_if_current(old));
    assert_eq!(f.stop_calls(), 0);
    assert!(m.is_current(new));
    assert!(m.is_speaking());
}

#[test]
fn stop_if_current_reports_false_when_the_kill_is_unconfirmed() {
    let (m, f) = mgr_q();
    let e = m.begin_utterance().unwrap();
    q(&m, e, "One.").unwrap();
    f.set_speaking(true);
    f.set_stop_result(StopOutcome::Failed);
    assert!(!m.stop_if_current(e));
    assert!(m.is_speaking());
}
