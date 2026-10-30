//! Speech scopes (DESIGN Revision 3, D3): the real voice output opens one scope per speech
//! segment so the bubble of a spoken reply stays up until the audio ends.
//!
//! Covered: the queue session (`TtsQueueSession` begins on its first successful push and ends
//! exactly once), the whole-reply path (`JournalMouth` opens the segment before the line is
//! recorded and ends it when its playback future completes or is dropped), the no-signal cases,
//! wrapper delegation, and the real `ChatLog` ordering.

use super::*;
use crate::managers::chat_log::{ChatEmitter, ChatEvent, ChatEventKind, ChatLog, ChatRole};
use crate::managers::converse::SpeechQueue;
use crate::managers::converse_platform::{speech_segment, SpeechScope, SpeechSignal};
use crate::managers::converse_queue::TtsQueueSession;
use crate::managers::converse_text::SilentMouth;
use crate::managers::tts::TtsManager;
use crate::managers::tts_queue::tests::Fake;
use crate::managers::tts_queue::{QueueBackend, SharedCore, SpeakerCore};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::Context;
use std::time::Duration;

/// Ordered record shared by the fake signal, the fake chat recorder and the fake mouth.
#[derive(Default)]
struct Ledger(Mutex<Vec<String>>);

impl Ledger {
    fn push(&self, entry: String) {
        self.0.lock().unwrap().push(entry);
    }
    fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
    fn begins(&self) -> Vec<u64> {
        self.scopes("begin:")
    }
    fn ends(&self) -> Vec<u64> {
        self.scopes("end:")
    }
    fn scopes(&self, prefix: &str) -> Vec<u64> {
        self.all()
            .iter()
            .filter_map(|entry| entry.strip_prefix(prefix)?.parse().ok())
            .collect()
    }
}

/// Fake port: records every begin and end in the ledger.
struct LedgerSignal(Arc<Ledger>);

impl SpeechSignal for LedgerSignal {
    fn begin(&self, scope: u64) {
        self.0.push(format!("begin:{scope}"));
    }
    fn end(&self, scope: u64) {
        self.0.push(format!("end:{scope}"));
    }
}

/// Fake chat window: records every recorded line in the ledger.
struct LedgerChat(Arc<Ledger>);

impl Reporter for LedgerChat {
    fn state(&self, _state: SessionState) {}
    fn chat(&self, _role: ChatRole, text: &str) {
        self.0.push(format!("chat:{text}"));
    }
}

/// Controllable inner mouth: `say` hangs when asked to, and `open_segment` opens the real
/// `SpeechScope` guard on the given signal.
struct SegMouth {
    signal: Option<Arc<dyn SpeechSignal>>,
    hang: bool,
}

impl Mouth for SegMouth {
    fn say<'a>(&'a self, _text: &'a str, _emotion: Emotion) -> BoxFuture<'a, SayOutcome> {
        let hang = self.hang;
        Box::pin(async move {
            if hang {
                std::future::pending::<SayOutcome>().await
            } else {
                SayOutcome::Done
            }
        })
    }
    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async { QuietOutcome::Quiet })
    }
    fn stop(&self) {}
    fn open_segment(&self) -> Option<Box<dyn SpeechSegment>> {
        let signal = self.signal.clone()?;
        Some(Box::new(SpeechScope::open(signal)))
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

fn segment_mouth(ledger: &Arc<Ledger>, hang: bool) -> SegMouth {
    SegMouth {
        signal: Some(Arc::new(LedgerSignal(ledger.clone()))),
        hang,
    }
}

fn journal_mouth<'m>(inner: &'m dyn Mouth, chat: &'m dyn Reporter) -> JournalMouth<'m> {
    JournalMouth {
        inner,
        journal: Journal::default(),
        reporter: chat,
    }
}

fn queue_setup() -> (Arc<TtsManager>, Fake) {
    let fake = Fake::default();
    let core: SharedCore<Fake> = Arc::new(Mutex::new(SpeakerCore::new(fake.clone())));
    (
        Arc::new(TtsManager::new(Box::new(QueueBackend::new(core)))),
        fake,
    )
}

fn queue_session(ledger: &Arc<Ledger>, tts: Arc<TtsManager>) -> Arc<TtsQueueSession> {
    let signal: Arc<dyn SpeechSignal> = Arc::new(LedgerSignal(ledger.clone()));
    TtsQueueSession::open_with_signal(tts, None, None, Some(signal)).expect("session")
}

// ------------------------------------------------------------------
// T1: queue path, normal end
// ------------------------------------------------------------------

#[test]
fn queue_session_begins_on_first_push_and_ends_on_finish_with_one_scope() {
    let (tts, _fake) = queue_setup();
    let ledger = Arc::new(Ledger::default());
    let session = queue_session(&ledger, tts);
    assert!(session.push("First sentence", Emotion::Neutral));
    let begun = ledger.begins();
    assert_eq!(begun.len(), 1, "the first successful push begins the scope");
    assert!(session.push("Second sentence", Emotion::Neutral));
    assert_eq!(ledger.begins(), begun, "later pushes do not begin again");
    assert!(
        ledger.ends().is_empty(),
        "nothing ends while the reply is queued"
    );
    assert_eq!(block_on(session.finish()), SayOutcome::Done);
    assert_eq!(
        ledger.all(),
        vec![format!("begin:{}", begun[0]), format!("end:{}", begun[0])]
    );
}

#[test]
fn second_queue_session_gets_its_own_scope() {
    let (tts, _fake) = queue_setup();
    let ledger = Arc::new(Ledger::default());
    let first = queue_session(&ledger, tts.clone());
    assert!(first.push("One", Emotion::Neutral));
    assert_eq!(block_on(first.finish()), SayOutcome::Done);
    let second = queue_session(&ledger, tts);
    assert!(second.push("Two", Emotion::Neutral));
    let begun = ledger.begins();
    assert_eq!(begun.len(), 2);
    assert_ne!(begun[0], begun[1], "every session has a fresh scope id");
}

// ------------------------------------------------------------------
// T2: queue path, abnormal ends
// ------------------------------------------------------------------

#[test]
fn stop_before_finish_ends_the_scope_exactly_once() {
    let (tts, _fake) = queue_setup();
    let ledger = Arc::new(Ledger::default());
    let session = queue_session(&ledger, tts);
    assert!(session.push("One", Emotion::Neutral));
    session.abort();
    // Ended by the abort itself, not only by the Drop that follows.
    assert_eq!(ledger.ends(), ledger.begins(), "abort closes the scope");
    session.abort();
    drop(session);
    let begun = ledger.begins();
    assert_eq!(begun.len(), 1);
    assert_eq!(ledger.ends(), begun, "exactly one end");
}

#[test]
fn refused_push_after_a_success_ends_the_scope_exactly_once() {
    let (tts, _fake) = queue_setup();
    let ledger = Arc::new(Ledger::default());
    let session = queue_session(&ledger, tts.clone());
    assert!(session.push("One", Emotion::Neutral));
    let _newer = tts.begin_utterance().unwrap(); // supersedes the session
    assert!(!session.push("Two", Emotion::Neutral));
    // Ended by the refused push itself, not only by the Drop that follows.
    assert_eq!(
        ledger.ends(),
        ledger.begins(),
        "a refused push closes the scope"
    );
    assert!(!session.push("Three", Emotion::Neutral));
    drop(session);
    let begun = ledger.begins();
    assert_eq!(begun.len(), 1);
    assert_eq!(ledger.ends(), begun, "exactly one end");
}

#[test]
fn dropping_a_session_after_a_push_ends_the_scope_exactly_once() {
    let (tts, _fake) = queue_setup();
    let ledger = Arc::new(Ledger::default());
    let session = queue_session(&ledger, tts);
    assert!(session.push("One", Emotion::Neutral));
    drop(session);
    let begun = ledger.begins();
    assert_eq!(begun.len(), 1);
    assert_eq!(ledger.ends(), begun, "exactly one end");
}

#[test]
fn a_refused_first_push_begins_nothing_and_ends_nothing() {
    let (tts, _fake) = queue_setup();
    let ledger = Arc::new(Ledger::default());
    let session = queue_session(&ledger, tts.clone());
    let _newer = tts.begin_utterance().unwrap();
    assert!(!session.push("One", Emotion::Neutral));
    session.abort();
    drop(session);
    assert!(ledger.all().is_empty(), "no push succeeded, so no scope");
}

#[test]
fn blank_text_only_session_begins_nothing() {
    let (tts, _fake) = queue_setup();
    let ledger = Arc::new(Ledger::default());
    let session = queue_session(&ledger, tts);
    assert!(session.push("   ", Emotion::Neutral));
    drop(session);
    assert!(ledger.all().is_empty());
}

#[test]
fn abort_after_finish_and_drop_do_not_end_twice() {
    let (tts, _fake) = queue_setup();
    let ledger = Arc::new(Ledger::default());
    let session = queue_session(&ledger, tts);
    assert!(session.push("One", Emotion::Neutral));
    assert_eq!(block_on(session.finish()), SayOutcome::Done);
    session.abort();
    drop(session);
    let begun = ledger.begins();
    assert_eq!(begun.len(), 1);
    assert_eq!(ledger.ends(), begun, "exactly one end");
}

// ------------------------------------------------------------------
// T3: whole-reply path, ordering
// ------------------------------------------------------------------

#[test]
fn whole_reply_orders_begin_then_chat_then_end() {
    let ledger = Arc::new(Ledger::default());
    let mouth = segment_mouth(&ledger, false);
    let chat = LedgerChat(ledger.clone());
    let journal = journal_mouth(&mouth, &chat);
    assert_eq!(
        block_on(journal.say("Hello there", Emotion::Neutral)),
        SayOutcome::Done
    );
    let begun = ledger.begins();
    assert_eq!(begun.len(), 1);
    assert_eq!(
        ledger.all(),
        vec![
            format!("begin:{}", begun[0]),
            "chat:Hello there".to_string(),
            format!("end:{}", begun[0]),
        ]
    );
}

#[test]
fn consecutive_says_get_distinct_scopes_each_ending_before_the_next_begins() {
    let ledger = Arc::new(Ledger::default());
    let mouth = segment_mouth(&ledger, false);
    let chat = LedgerChat(ledger.clone());
    let journal = journal_mouth(&mouth, &chat);
    block_on(journal.say("Confirm this", Emotion::Neutral));
    block_on(journal.say("Done now", Emotion::Neutral));
    let begun = ledger.begins();
    assert_eq!(begun.len(), 2);
    assert_ne!(begun[0], begun[1]);
    assert_eq!(
        ledger.all(),
        vec![
            format!("begin:{}", begun[0]),
            "chat:Confirm this".to_string(),
            format!("end:{}", begun[0]),
            format!("begin:{}", begun[1]),
            "chat:Done now".to_string(),
            format!("end:{}", begun[1]),
        ]
    );
}

// ------------------------------------------------------------------
// T4: whole-reply path, dropped playback future
// ------------------------------------------------------------------

#[test]
fn unpolled_playback_future_dropped_ends_the_scope_once() {
    let ledger = Arc::new(Ledger::default());
    let mouth = segment_mouth(&ledger, false);
    let chat = LedgerChat(ledger.clone());
    let journal = journal_mouth(&mouth, &chat);
    let playback = journal.say("Never played", Emotion::Neutral);
    let begun = ledger.begins();
    assert_eq!(begun.len(), 1, "the scope opens when the line is said");
    assert_eq!(
        ledger.all()[1],
        "chat:Never played",
        "line recorded already"
    );
    drop(playback);
    assert_eq!(
        ledger.all(),
        vec![
            format!("begin:{}", begun[0]),
            "chat:Never played".to_string(),
            format!("end:{}", begun[0]),
        ]
    );
}

#[test]
fn playback_dropped_mid_say_ends_the_scope_once() {
    let ledger = Arc::new(Ledger::default());
    let mouth = segment_mouth(&ledger, true);
    let chat = LedgerChat(ledger.clone());
    let journal = journal_mouth(&mouth, &chat);
    let mut playback = journal.say("Interrupted", Emotion::Neutral);
    let mut cx = Context::from_waker(futures_util::task::noop_waker_ref());
    assert!(playback.as_mut().poll(&mut cx).is_pending());
    let begun = ledger.begins();
    assert_eq!(begun.len(), 1);
    assert!(ledger.ends().is_empty(), "still speaking");
    drop(playback);
    assert_eq!(ledger.ends(), begun, "exactly one end");
}

// ------------------------------------------------------------------
// T5: no-signal cases
// ------------------------------------------------------------------

#[test]
fn silent_mouth_records_the_line_and_opens_no_scope() {
    let ledger = Arc::new(Ledger::default());
    let chat = LedgerChat(ledger.clone());
    let silent = SilentMouth;
    let journal = journal_mouth(&silent, &chat);
    assert_eq!(
        block_on(journal.say("Quiet reply", Emotion::Neutral)),
        SayOutcome::Done
    );
    assert_eq!(ledger.all(), vec!["chat:Quiet reply".to_string()]);
    assert!(SilentMouth.open_segment().is_none());
}

#[test]
fn no_segment_without_tts_output_or_without_a_port() {
    let ledger = Arc::new(Ledger::default());
    let signal: Arc<dyn SpeechSignal> = Arc::new(LedgerSignal(ledger.clone()));
    assert!(
        speech_segment(false, Some(&signal)).is_none(),
        "tts disabled"
    );
    assert!(speech_segment(true, None).is_none(), "no port configured");
    let segment = speech_segment(true, Some(&signal));
    assert!(segment.is_some(), "tts on and port configured");
    drop(segment);
    assert_eq!(ledger.begins().len(), 1);
    assert_eq!(ledger.ends(), ledger.begins());
}

// ------------------------------------------------------------------
// T6: wrapper delegation
// ------------------------------------------------------------------

#[test]
fn wrappers_forward_open_segment_to_the_inner_mouth() {
    let ledger = Arc::new(Ledger::default());
    let mouth = segment_mouth(&ledger, false);
    let chat = LedgerChat(ledger.clone());

    let timed = TimedMouth::new(&mouth);
    let segment = timed.open_segment();
    assert!(segment.is_some(), "timed forwards");
    drop(segment);

    let gate = ReplyGate::new();
    let gated = GatedMouth {
        inner: &mouth,
        gate: gate.clone(),
    };
    let segment = gated.open_segment();
    assert!(segment.is_some(), "gated forwards while open");
    drop(segment);
    gate.close();
    assert!(
        gated.open_segment().is_none(),
        "a closed gate admits nothing"
    );

    let journal = journal_mouth(&mouth, &chat);
    let segment = journal.open_segment();
    assert!(segment.is_some(), "journal forwards");
    drop(segment);

    assert_eq!(ledger.begins().len(), 3);
    assert_eq!(ledger.ends(), ledger.begins(), "every scope ended once");
}

// ------------------------------------------------------------------
// T7: real ChatLog ordering for the whole-reply path
// ------------------------------------------------------------------

/// Real chat log emitter: records line entries and speech marks in the order the worker
/// applies them.
struct ChatLedger(Arc<Ledger>);

impl ChatEmitter for ChatLedger {
    fn emit(&self, event: &ChatEvent) {
        if let ChatEventKind::Entry { entry } = &event.kind {
            self.0.push(format!("chat:{}", entry.text));
        }
    }
    fn speech(&self, scope: u64, on: bool, _stamp: Option<crate::managers::chat_log::Stamp>) {
        let mark = if on { "begin" } else { "end" };
        self.0.push(format!("{mark}:{scope}"));
    }
}

/// The chat window of the real app: a recorded line goes into the real chat log.
struct ChatWindow(Arc<ChatLog>);

impl Reporter for ChatWindow {
    fn state(&self, _state: SessionState) {}
    fn chat(&self, role: ChatRole, text: &str) {
        self.0.say(role, text);
    }
}

fn wait_for_entries(ledger: &Ledger, count: usize) {
    for _ in 0..1000 {
        if ledger.all().len() >= count {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn real_chat_log_gets_the_mark_before_the_reply_line_and_its_end_after() {
    let ledger = Arc::new(Ledger::default());
    let chat = ChatLog::start(Arc::new(ChatLedger(ledger.clone())), true);
    let signal: Arc<dyn SpeechSignal> = chat.clone();
    let mouth = SegMouth {
        signal: Some(signal),
        hang: false,
    };
    let window = ChatWindow(chat.clone());
    let journal = journal_mouth(&mouth, &window);
    assert_eq!(
        block_on(journal.say("Real reply", Emotion::Neutral)),
        SayOutcome::Done
    );
    wait_for_entries(&ledger, 3);
    let begun = ledger.begins();
    assert_eq!(begun.len(), 1);
    assert_eq!(
        ledger.all(),
        vec![
            format!("begin:{}", begun[0]),
            "chat:Real reply".to_string(),
            format!("end:{}", begun[0]),
        ]
    );
}
