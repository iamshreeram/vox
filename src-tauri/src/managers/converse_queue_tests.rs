//! Authoritative tests for `converse_queue` (TtsQueueSession). Do not edit to make code pass.

use super::*;
use crate::managers::tts_queue::tests::Fake;
use crate::managers::tts_queue::{QueueBackend, SharedCore, SpeakerCore, StopOutcome};
use std::sync::Mutex;

fn setup() -> (Arc<TtsManager>, Fake) {
    let f = Fake::default();
    let core: SharedCore<Fake> = Arc::new(Mutex::new(SpeakerCore::new(f.clone())));
    (
        Arc::new(TtsManager::new(Box::new(QueueBackend::new(core)))),
        f,
    )
}

#[test]
fn open_requires_a_queue_capable_backend_and_no_active_recording() {
    let (tts, _f) = setup();
    assert!(TtsQueueSession::open(tts.clone(), None, None).is_some());
    tts.note_recording_started();
    assert!(
        TtsQueueSession::open(tts.clone(), None, None).is_none(),
        "recording active"
    );
    tts.note_recording_stopped();
    assert!(TtsQueueSession::open(tts, None, None).is_some());

    // a backend without queue support => None
    struct Plain;
    struct H;
    impl crate::managers::tts::SpeechHandle for H {
        fn kill(&mut self) -> bool {
            true
        }
        fn is_running(&mut self) -> bool {
            false
        }
    }
    impl crate::managers::tts::SpeechBackend for Plain {
        fn spawn(
            &self,
            _t: &str,
            _v: Option<&str>,
            _r: Option<u32>,
        ) -> std::io::Result<Box<dyn crate::managers::tts::SpeechHandle>> {
            Ok(Box::new(H))
        }
    }
    let plain = Arc::new(TtsManager::new(Box::new(Plain)));
    assert!(TtsQueueSession::open(plain, None, None).is_none());
}

#[test]
fn push_queues_sentences_in_order_with_voice_rate_and_emotion() {
    let (tts, f) = setup();
    let s = TtsQueueSession::open(tts, Some("Samantha".into()), Some(190)).unwrap();
    assert!(s.push("First sentence.", Emotion::Neutral));
    assert!(s.push("Second sentence.", Emotion::Cheerful));
    assert_eq!(
        f.spoken_texts(),
        vec!["First sentence.", "Second sentence."]
    );
    let spoken = f.0.lock().unwrap().spoken.clone();
    assert_eq!(spoken[0].1.as_deref(), Some("Samantha"));
    assert!(spoken[0].2.is_some());
    assert!(
        spoken[1].3 > spoken[0].3,
        "cheerful is higher pitched than neutral"
    );
    assert_eq!(f.stop_calls(), 0);
}

#[test]
fn empty_text_is_a_harmless_success() {
    let (tts, f) = setup();
    let s = TtsQueueSession::open(tts, None, None).unwrap();
    assert!(s.push("   ", Emotion::Neutral));
    assert!(f.spoken_texts().is_empty());
    assert!(
        s.push("Real sentence.", Emotion::Neutral),
        "session still alive"
    );
}

#[test]
fn a_refused_push_kills_the_session_for_good() {
    let (tts, f) = setup();
    let s = TtsQueueSession::open(tts.clone(), None, None).unwrap();
    assert!(s.push("One.", Emotion::Neutral));
    let _newer = tts.begin_utterance().unwrap(); // supersedes the session's epoch
    assert!(!s.push("Two.", Emotion::Neutral));
    assert!(!s.push("Three.", Emotion::Neutral));
    assert_eq!(
        f.spoken_texts(),
        vec!["One."],
        "nothing reached the speaker after the refusal"
    );
}

#[test]
fn abort_stops_this_sessions_speech_and_blocks_further_pushes() {
    let (tts, f) = setup();
    let s = TtsQueueSession::open(tts, None, None).unwrap();
    s.push("One.", Emotion::Neutral);
    f.set_speaking(true);
    s.abort();
    assert_eq!(f.stop_calls(), 1);
    assert!(!s.push("Two.", Emotion::Neutral));
}

#[test]
fn abort_never_disturbs_a_newer_owner_of_the_speaker() {
    let (tts, f) = setup();
    let s = TtsQueueSession::open(tts.clone(), None, None).unwrap();
    s.push("One.", Emotion::Neutral);
    let newer = tts.begin_utterance().unwrap(); // someone else now owns speech
    tts.speak_queued_if_current(newer, "Theirs.", None, None, Emotion::Neutral)
        .unwrap();
    f.set_speaking(true);
    let stops_before = f.stop_calls();
    s.abort();
    assert_eq!(
        f.stop_calls(),
        stops_before,
        "stale session must not stop newer speech"
    );
    assert!(tts.is_current(newer));
}

#[tokio::test]
async fn finish_is_done_immediately_when_nothing_was_queued() {
    let (tts, _f) = setup();
    let s = TtsQueueSession::open(tts, None, None).unwrap();
    assert_eq!(
        s.finish_within(Duration::from_secs(2)).await,
        SayOutcome::Done
    );
}

#[tokio::test]
async fn finish_waits_for_the_queue_to_drain() {
    let (tts, f) = setup();
    let s = TtsQueueSession::open(tts, None, None).unwrap();
    s.push("One.", Emotion::Neutral);
    s.push("Two.", Emotion::Neutral);
    f.set_speaking(true);
    let f2 = f.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        f2.set_speaking(false);
    });
    let t = std::time::Instant::now();
    assert_eq!(
        s.finish_within(Duration::from_secs(5)).await,
        SayOutcome::Done
    );
    assert!(
        t.elapsed() >= Duration::from_millis(100),
        "returned too early: {:?}",
        t.elapsed()
    );
}

#[tokio::test]
async fn finish_reports_interrupted_when_the_epoch_changes() {
    let (tts, f) = setup();
    let s = TtsQueueSession::open(tts.clone(), None, None).unwrap();
    s.push("One.", Emotion::Neutral);
    f.set_speaking(true);
    let t2 = tts.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        t2.invalidate();
    });
    assert_eq!(
        s.finish_within(Duration::from_secs(5)).await,
        SayOutcome::Interrupted
    );
}

#[tokio::test]
async fn finish_times_out_as_failed_and_stops_its_own_speech() {
    let (tts, f) = setup();
    let s = TtsQueueSession::open(tts, None, None).unwrap();
    s.push("One.", Emotion::Neutral);
    f.set_speaking(true); // never ends
    assert_eq!(
        s.finish_within(Duration::from_millis(200)).await,
        SayOutcome::Failed
    );
    assert!(f.stop_calls() >= 1, "a timed-out session stops its speech");
}

#[test]
fn a_failed_os_stop_is_not_hidden_by_abort() {
    let (tts, f) = setup();
    let s = TtsQueueSession::open(tts.clone(), None, None).unwrap();
    s.push("One.", Emotion::Neutral);
    f.set_speaking(true);
    f.set_stop_result(StopOutcome::Failed);
    s.abort();
    assert!(
        tts.is_speaking(),
        "still speaking: the truth is visible to the loop's settle step"
    );
}
