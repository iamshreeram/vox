//! TYPING SPEC (DESIGN-TYPE Rev 1, written by ram-ai; workers must not edit this file).
//! Pure pieces of the text chat: sanitising, the inbox, routing, the ears, the silent mouth
//! and the no-paste hands. Child module of `converse_text` (`use super::*`).

use super::*;
use crate::managers::converse::{
    BoxFuture, Ears, Hands, ListenOutcome, Mouth, PhraseOutcome, QuietOutcome, SayOutcome,
};
use crate::managers::converse_runtime::ConverseStatus;
use crate::managers::emotion::Emotion;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(f)
}

// ------------------------------------------------------------------ sanitize_typed

#[test]
fn sanitize_trims_and_collapses_whitespace_to_single_spaces() {
    assert_eq!(sanitize_typed("  hello   world \n").unwrap(), "hello world");
    assert_eq!(sanitize_typed("a\tb\r\nc\n\nd").unwrap(), "a b c d");
}

#[test]
fn sanitize_removes_control_characters_but_keeps_text() {
    assert_eq!(
        sanitize_typed("he\u{0000}llo\u{0007}\u{001b}[0m").unwrap(),
        "hello[0m"
    );
}

#[test]
fn sanitize_rejects_empty_and_whitespace_only() {
    assert_eq!(sanitize_typed(""), Err(TypedError::Empty));
    assert_eq!(sanitize_typed("   \n\t "), Err(TypedError::Empty));
    assert_eq!(sanitize_typed("\u{0000}\u{0001}"), Err(TypedError::Empty));
}

#[test]
fn sanitize_limit_is_in_characters_after_cleaning() {
    let ok: String = "é".repeat(MAX_TYPED_CHARS);
    assert_eq!(
        sanitize_typed(&ok).unwrap().chars().count(),
        MAX_TYPED_CHARS
    );
    let too_long: String = "é".repeat(MAX_TYPED_CHARS + 1);
    assert_eq!(sanitize_typed(&too_long), Err(TypedError::TooLong));
    // whitespace that collapses away does not count toward the limit
    let padded = format!("{}{}", "a ".repeat(10), " ".repeat(MAX_TYPED_CHARS * 2));
    assert_eq!(sanitize_typed(&padded).unwrap(), "a a a a a a a a a a");
}

#[test]
fn sanitize_keeps_markup_as_plain_text() {
    assert_eq!(
        sanitize_typed("<b>hi</b> & <script>x</script>").unwrap(),
        "<b>hi</b> & <script>x</script>"
    );
}

#[test]
fn typed_error_codes_are_stable_wire_strings() {
    assert_eq!(TypedError::Empty.code(), "empty");
    assert_eq!(TypedError::TooLong.code(), "too_long");
    assert_eq!(TypedError::Busy.code(), "busy");
    assert_eq!(TypedError::NotConfigured.code(), "not_configured");
    assert_eq!(TypedError::Failed.code(), "failed");
}

// ------------------------------------------------------------------ route_typed

#[test]
fn route_idle_starts_text_running_queues_stopping_is_busy() {
    assert_eq!(route_typed(ConverseStatus::Idle), TypedRoute::StartText);
    assert_eq!(route_typed(ConverseStatus::Running), TypedRoute::Queue);
    assert_eq!(route_typed(ConverseStatus::Stopping), TypedRoute::Busy);
}

// ------------------------------------------------------------------ TypedInbox

#[test]
fn inbox_is_fifo_and_next_returns_pushed_text() {
    let inbox = TypedInbox::new();
    inbox.open();
    inbox.push("one".into()).unwrap();
    inbox.push("two".into()).unwrap();
    assert_eq!(inbox.len(), 2);
    assert_eq!(block_on(inbox.next()), "one");
    assert_eq!(block_on(inbox.next()), "two");
    assert_eq!(inbox.len(), 0);
}

#[test]
fn inbox_is_bounded_and_reports_busy_when_full() {
    let inbox = TypedInbox::new();
    inbox.open();
    for i in 0..INBOX_CAPACITY {
        inbox.push(format!("m{i}")).unwrap();
    }
    assert_eq!(inbox.push("overflow".into()), Err(TypedError::Busy));
    assert_eq!(inbox.len(), INBOX_CAPACITY);
}

#[test]
fn inbox_clear_drops_everything_and_allows_new_pushes() {
    let inbox = TypedInbox::new();
    inbox.open();
    inbox.push("a".into()).unwrap();
    inbox.clear();
    assert_eq!(inbox.len(), 0);
    inbox.push("b".into()).unwrap();
    assert_eq!(block_on(inbox.next()), "b");
}

#[test]
fn inbox_next_waits_until_a_push_arrives_and_is_cancel_safe() {
    let inbox = TypedInbox::new();
    inbox.open();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        // a cancelled waiter must not swallow a later message
        let waiting = tokio::time::timeout(Duration::from_millis(30), inbox.next()).await;
        assert!(waiting.is_err(), "nothing queued: next() must pend");
        inbox.push("late".into()).unwrap();
        let got = tokio::time::timeout(Duration::from_millis(500), inbox.next())
            .await
            .expect("message must be delivered");
        assert_eq!(got, "late");
    });
}

#[test]
fn inbox_wakes_a_waiter_pushed_from_another_thread() {
    let inbox = TypedInbox::new();
    inbox.open();
    let pusher = inbox.clone();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        pusher.push("from thread".into()).unwrap();
    });
    let got = block_on(async {
        tokio::time::timeout(Duration::from_secs(2), inbox.next())
            .await
            .expect("delivered")
    });
    handle.join().unwrap();
    assert_eq!(got, "from thread");
}

// ------------------------------------------------------------------ ears

struct ScriptedInner {
    outcomes: std::sync::Mutex<Vec<ListenOutcome>>,
    calls: AtomicUsize,
}
impl Ears for ScriptedInner {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let next = self.outcomes.lock().unwrap().pop();
        Box::pin(async move {
            match next {
                Some(o) => o,
                None => std::future::pending::<ListenOutcome>().await,
            }
        })
    }
}

#[test]
fn pending_ears_never_return() {
    let ears = PendingEars;
    block_on(async {
        let r = tokio::time::timeout(Duration::from_millis(60), ears.listen()).await;
        assert!(r.is_err(), "PendingEars::listen must never complete");
    });
}

#[test]
fn with_typed_returns_typed_text_as_an_utterance_even_when_the_inner_ears_pend() {
    let inbox = TypedInbox::new();
    inbox.open();
    let ears = WithTyped::new(Box::new(PendingEars), inbox.clone());
    inbox.push("hello there".into()).unwrap();
    let out = block_on(ears.listen());
    assert_eq!(out, ListenOutcome::Utterance("hello there".into()));
}

#[test]
fn with_typed_prefers_already_queued_typed_text_over_the_inner_ears() {
    let inbox = TypedInbox::new();
    inbox.open();
    let inner = Arc::new(ScriptedInner {
        outcomes: std::sync::Mutex::new(vec![ListenOutcome::Utterance("spoken".into())]),
        calls: AtomicUsize::new(0),
    });
    struct Shared(Arc<ScriptedInner>);
    impl Ears for Shared {
        fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
            self.0.listen()
        }
    }
    let ears = WithTyped::new(Box::new(Shared(inner.clone())), inbox.clone());
    inbox.push("typed".into()).unwrap();
    assert_eq!(
        block_on(ears.listen()),
        ListenOutcome::Utterance("typed".into())
    );
}

#[test]
fn with_typed_passes_inner_outcomes_through_when_nothing_is_typed() {
    let inbox = TypedInbox::new();
    inbox.open();
    let inner = ScriptedInner {
        outcomes: std::sync::Mutex::new(vec![ListenOutcome::Utterance("spoken".into())]),
        calls: AtomicUsize::new(0),
    };
    let ears = WithTyped::new(Box::new(inner), inbox);
    assert_eq!(
        block_on(ears.listen()),
        ListenOutcome::Utterance("spoken".into())
    );
}

#[test]
fn with_typed_unblocks_a_waiting_listen_when_text_arrives_later() {
    let inbox = TypedInbox::new();
    inbox.open();
    let ears = WithTyped::new(Box::new(PendingEars), inbox.clone());
    let pusher = inbox.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        pusher.push("later".into()).unwrap();
    });
    let out = block_on(async {
        tokio::time::timeout(Duration::from_secs(2), ears.listen())
            .await
            .expect("must wake")
    });
    assert_eq!(out, ListenOutcome::Utterance("later".into()));
}

// ------------------------------------------------------------------ silent mouth

#[test]
fn silent_mouth_finishes_instantly_and_never_queues_speech() {
    let mouth = SilentMouth;
    assert_eq!(
        block_on(mouth.say("hello", Emotion::Neutral)),
        SayOutcome::Done
    );
    assert_eq!(
        block_on(mouth.wait_quiet(Duration::from_secs(5))),
        QuietOutcome::Quiet
    );
    mouth.stop();
    assert!(mouth.open_queue().is_none());
}

// ------------------------------------------------------------------ text hands

struct StubHands {
    pasted: AtomicUsize,
    phrases: AtomicUsize,
}
impl Hands for StubHands {
    fn paste_generated(&self, _t: &str) -> Result<(), String> {
        self.pasted.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn run_user_phrase<'a>(&'a self, _p: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        self.phrases.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { PhraseOutcome::Done("opened".into()) })
    }
    fn run_model_command<'a>(&'a self, _p: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        Box::pin(async { PhraseOutcome::NoMatch })
    }
}

#[test]
fn text_hands_refuse_to_paste_into_the_active_window_but_run_phrases() {
    let hands = TextHands::new(Box::new(StubHands {
        pasted: AtomicUsize::new(0),
        phrases: AtomicUsize::new(0),
    }));
    assert!(hands.paste_generated("secret draft").is_err());
    assert_eq!(
        block_on(hands.run_user_phrase("open notes")),
        PhraseOutcome::Done("opened".into())
    );
    assert_eq!(
        block_on(hands.run_model_command("x")),
        PhraseOutcome::NoMatch
    );
}

#[test]
fn text_hands_never_call_the_inner_paste() {
    let inner = Arc::new(StubHands {
        pasted: AtomicUsize::new(0),
        phrases: AtomicUsize::new(0),
    });
    struct Shared(Arc<StubHands>);
    impl Hands for Shared {
        fn paste_generated(&self, t: &str) -> Result<(), String> {
            self.0.paste_generated(t)
        }
        fn run_user_phrase<'a>(&'a self, p: &'a str) -> BoxFuture<'a, PhraseOutcome> {
            self.0.run_user_phrase(p)
        }
        fn run_model_command<'a>(&'a self, p: &'a str) -> BoxFuture<'a, PhraseOutcome> {
            self.0.run_model_command(p)
        }
    }
    let hands = TextHands::new(Box::new(Shared(inner.clone())));
    let _ = hands.paste_generated("x");
    assert_eq!(inner.pasted.load(Ordering::SeqCst), 0);
}

// ------------------------------------------------------------------ inbox lifecycle (C1)

#[test]
fn a_new_inbox_is_closed_and_refuses_pushes_with_busy() {
    let inbox = TypedInbox::new();
    assert_eq!(inbox.push("early".into()), Err(TypedError::Busy));
    assert_eq!(inbox.len(), 0);
}

#[test]
fn open_clears_leftovers_and_starts_accepting() {
    let inbox = TypedInbox::new();
    inbox.open();
    inbox.push("old session".into()).unwrap();
    inbox.open(); // a new session begins: nothing from the previous one survives
    assert_eq!(inbox.len(), 0);
    inbox.push("new session".into()).unwrap();
    assert_eq!(block_on(inbox.next()), "new session");
}

#[test]
fn close_clears_everything_and_refuses_later_pushes() {
    let inbox = TypedInbox::new();
    inbox.open();
    inbox.push("pending".into()).unwrap();
    inbox.close();
    assert_eq!(inbox.len(), 0);
    assert_eq!(inbox.push("late".into()), Err(TypedError::Busy));
    inbox.open();
    inbox.push("again".into()).unwrap();
    assert_eq!(block_on(inbox.next()), "again");
}

#[test]
fn concurrent_pushes_and_a_close_never_lose_or_duplicate_accepted_messages() {
    for _ in 0..50 {
        let inbox = TypedInbox::new();
        inbox.open();
        let accepted = Arc::new(AtomicUsize::new(0));
        let mut handles = vec![];
        for i in 0..8 {
            let inbox = inbox.clone();
            let accepted = accepted.clone();
            handles.push(std::thread::spawn(move || {
                if inbox.push(format!("m{i}")).is_ok() {
                    accepted.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // everything accepted before the close is either still queued or was cleared by it
        let queued_before_close = inbox.len();
        assert_eq!(queued_before_close, accepted.load(Ordering::SeqCst));
        inbox.close();
        assert_eq!(inbox.len(), 0);
    }
}

// ------------------------------------------------------------------ WithTyped timing (C6)

struct TimedInner {
    timing_calls: Arc<AtomicUsize>,
    listen_calls: Arc<AtomicUsize>,
    shutdowns: Arc<AtomicUsize>,
    answer: std::sync::Mutex<Option<ListenOutcome>>,
}
impl Ears for TimedInner {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        self.listen_calls.fetch_add(1, Ordering::SeqCst);
        let next = self.answer.lock().unwrap().take();
        Box::pin(async move {
            match next {
                Some(o) => o,
                None => std::future::pending::<ListenOutcome>().await,
            }
        })
    }
    fn take_timing(&self) -> Option<crate::managers::converse_latency::ListenTiming> {
        self.timing_calls.fetch_add(1, Ordering::SeqCst);
        Some(crate::managers::converse_latency::ListenTiming::default())
    }
    fn barge_shutdown(&self) {
        self.shutdowns.fetch_add(1, Ordering::SeqCst);
    }
}

fn timed(answer: Option<ListenOutcome>) -> (TimedInner, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let timing = Arc::new(AtomicUsize::new(0));
    let shutdowns = Arc::new(AtomicUsize::new(0));
    (
        TimedInner {
            timing_calls: timing.clone(),
            listen_calls: Arc::new(AtomicUsize::new(0)),
            shutdowns: shutdowns.clone(),
            answer: std::sync::Mutex::new(answer),
        },
        timing,
        shutdowns,
    )
}

#[test]
fn a_typed_result_has_no_timing_and_the_stale_inner_timing_is_discarded() {
    let inbox = TypedInbox::new();
    inbox.open();
    let (inner, timing_calls, _) = timed(None);
    let ears = WithTyped::new(Box::new(inner), inbox.clone());
    inbox.push("typed".into()).unwrap();
    assert_eq!(
        block_on(ears.listen()),
        ListenOutcome::Utterance("typed".into())
    );
    assert!(
        ears.take_timing().is_none(),
        "typed turns carry no listen timing"
    );
    assert_eq!(
        timing_calls.load(Ordering::SeqCst),
        1,
        "the cancelled inner listen's timing is consumed and dropped, once"
    );
}

#[test]
fn a_spoken_result_keeps_its_own_timing_exactly_once() {
    let inbox = TypedInbox::new();
    inbox.open();
    let (inner, timing_calls, _) = timed(Some(ListenOutcome::Utterance("spoken".into())));
    let ears = WithTyped::new(Box::new(inner), inbox);
    assert_eq!(
        block_on(ears.listen()),
        ListenOutcome::Utterance("spoken".into())
    );
    assert!(ears.take_timing().is_some());
    assert_eq!(timing_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn barge_shutdown_is_delegated_exactly_once() {
    let inbox = TypedInbox::new();
    let (inner, _, shutdowns) = timed(None);
    let ears = WithTyped::new(Box::new(inner), inbox);
    ears.barge_shutdown();
    assert_eq!(shutdowns.load(Ordering::SeqCst), 1);
}

// ------------------------------------------------------------------ typed_seen flag (C2)

#[test]
fn open_returns_a_fresh_typed_seen_flag_that_is_set_only_by_a_delivered_message() {
    let inbox = TypedInbox::new();
    let first = inbox.open();
    assert!(!first.load(Ordering::SeqCst));
    inbox.push("typed".into()).unwrap();
    assert!(!first.load(Ordering::SeqCst), "queued is not delivered yet");
    assert_eq!(block_on(inbox.next()), "typed");
    assert!(
        first.load(Ordering::SeqCst),
        "delivery marks the session as typed"
    );
    inbox.close();
    let second = inbox.open();
    assert!(
        !second.load(Ordering::SeqCst),
        "the next session starts clean"
    );
    assert!(
        first.load(Ordering::SeqCst),
        "and the finished session keeps its flag"
    );
}

// ------------------------------------------------------------------ generation binding (C1 round 3)

#[test]
fn generation_is_none_while_closed_and_increases_with_every_open() {
    let inbox = TypedInbox::new();
    assert_eq!(inbox.generation(), None);
    inbox.open();
    let a = inbox.generation().expect("open has a generation");
    inbox.close();
    assert_eq!(inbox.generation(), None);
    inbox.open();
    let b = inbox.generation().expect("open has a generation");
    assert!(b > a, "a new session gets a newer generation");
}

#[test]
fn push_for_a_stale_generation_is_busy_and_never_reaches_the_new_session() {
    let inbox = TypedInbox::new();
    inbox.open();
    let session_a = inbox.generation().unwrap();
    inbox.close();
    inbox.open(); // session B
    assert_eq!(
        inbox.push_for(session_a, "yes".into()),
        Err(TypedError::Busy)
    );
    assert_eq!(inbox.len(), 0, "the stale 'yes' must not be queued for B");
    let session_b = inbox.generation().unwrap();
    inbox.push_for(session_b, "hello".into()).unwrap();
    assert_eq!(block_on(inbox.next()), "hello");
}

#[test]
fn push_for_on_a_closed_inbox_is_busy() {
    let inbox = TypedInbox::new();
    inbox.open();
    let g = inbox.generation().unwrap();
    inbox.close();
    assert_eq!(inbox.push_for(g, "late".into()), Err(TypedError::Busy));
}
