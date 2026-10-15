//! RED tests for M4 S8 (DESIGN Revision 5, D6 TURN-BOUNDARY EXIT, finding V1): a guarded
//! summary queued during a conversation must never reach the session mouth after its guard
//! turned invalid. Announcer level: `take_pending`, `take_pending_items` and
//! `PendingSpeech::resolve`. Converse level: `speak_pending` at the turn boundary, driven
//! through `run_session` with a recording session mouth. Child module of `job_announcer`.

use super::*;
use crate::managers::converse::{
    run_session, BoxFuture, Brain, Deps, Ears, Hands, ListenOutcome, Mouth, PhraseOutcome,
    QuietOutcome, Reporter, SayOutcome, SessionConfig, SessionEnd, SessionState,
};
use crate::managers::converse_plan::{PlanError, PlanInput, TurnPlan};
use crate::managers::emotion::Emotion;
use crate::managers::job_summary::{SpeechGuard, SummaryTokens, Ticket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use tokio::sync::watch;

const SUMMARY: &str = "The agent says: summary text";
const FALLBACK: &str = "The coding job in proj finished successfully.";
const FIXED_FINISH: &str = "The coding job finished.";

// ------------------------------------------------------------------ fakes

/// Direct (announcer-level) mouth that records every text and always accepts.
#[derive(Default)]
struct RecordMouth {
    spoken: Mutex<Vec<String>>,
}

impl RecordMouth {
    fn spoken(&self) -> Vec<String> {
        self.spoken.lock().unwrap().clone()
    }
}

impl LowPriorityMouth for RecordMouth {
    fn try_speak_if_idle(&self, text: &str) -> bool {
        self.spoken.lock().unwrap().push(text.to_string());
        true
    }
}

/// Direct mouth whose first offer blocks until the test releases it, so an entry stays in
/// flight while the test queues and takes other entries.
struct BlockingMouth {
    spoken: Mutex<Vec<String>>,
    entered: Mutex<Sender<()>>,
    release: Mutex<Receiver<()>>,
    blocked_once: AtomicBool,
}

impl BlockingMouth {
    fn spoken(&self) -> Vec<String> {
        self.spoken.lock().unwrap().clone()
    }
}

impl LowPriorityMouth for BlockingMouth {
    fn try_speak_if_idle(&self, text: &str) -> bool {
        self.spoken.lock().unwrap().push(text.to_string());
        if !self.blocked_once.swap(true, Ordering::SeqCst) {
            let _ = self.entered.lock().unwrap().send(());
            let _ = self
                .release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5));
        }
        true
    }
}

/// Session mouth that records every line it is asked to say. The optional hook runs inside
/// `say`, after the text is recorded, to simulate a change happening while speech runs.
struct SessionMouth {
    spoken: Mutex<Vec<String>>,
    hook: Mutex<Option<Box<dyn FnMut(&str) + Send>>>,
}

impl SessionMouth {
    fn new(hook: Option<Box<dyn FnMut(&str) + Send>>) -> Arc<Self> {
        Arc::new(Self {
            spoken: Mutex::new(Vec::new()),
            hook: Mutex::new(hook),
        })
    }
    fn spoken(&self) -> Vec<String> {
        self.spoken.lock().unwrap().clone()
    }
}

impl Mouth for SessionMouth {
    fn say<'a>(&'a self, text: &'a str, _emotion: Emotion) -> BoxFuture<'a, SayOutcome> {
        self.spoken.lock().unwrap().push(text.to_string());
        let mut hook_slot = self.hook.lock().unwrap();
        if let Some(hook) = hook_slot.as_mut() {
            hook(text);
        }
        Box::pin(async { SayOutcome::Done })
    }
    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async { QuietOutcome::Quiet })
    }
    fn stop(&self) {}
}

/// Ears whose only listen aborts, so a session reaches exactly one turn boundary.
struct AbortEars;

impl Ears for AbortEars {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        Box::pin(async { ListenOutcome::Aborted })
    }
}

struct NoBrain;

impl Brain for NoBrain {
    fn plan<'a>(&'a self, _input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        Box::pin(async { Err(PlanError::Empty) })
    }
}

struct NoHands;

impl Hands for NoHands {
    fn paste_generated(&self, _text: &str) -> Result<(), String> {
        Ok(())
    }
    fn run_user_phrase<'a>(&'a self, _phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        Box::pin(async { PhraseOutcome::NoMatch })
    }
    fn run_model_command<'a>(&'a self, _phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        Box::pin(async { PhraseOutcome::NoMatch })
    }
}

struct QuietReporter;

impl Reporter for QuietReporter {
    fn state(&self, _state: SessionState) {}
}

// ------------------------------------------------------------------ helpers

/// Retry interval long enough that only the explicit calls in a test act on the queue.
fn quiet(mouth: Arc<dyn LowPriorityMouth>) -> Arc<Announcer> {
    Arc::new(Announcer::with_timing(
        mouth,
        Duration::from_secs(30),
        Duration::from_secs(120),
    ))
}

fn direct_announcer() -> (Arc<RecordMouth>, Arc<Announcer>) {
    let mouth = Arc::new(RecordMouth::default());
    let announcer = quiet(mouth.clone());
    (mouth, announcer)
}

/// Tokens with job 1 admitted and a summary ticket issued for it.
fn admitted_tokens() -> (Arc<SummaryTokens>, Ticket) {
    let tokens = SummaryTokens::new();
    tokens.admit(1);
    let ticket = tokens.begin(1).expect("ticket for the latest admitted job");
    (tokens, ticket)
}

fn guard(tokens: &Arc<SummaryTokens>, ticket: Ticket) -> SpeechGuard {
    SpeechGuard {
        tokens: tokens.clone(),
        ticket,
        fallback: FALLBACK.to_string(),
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn texts_of(items: &[PendingSpeech]) -> Vec<String> {
    items.iter().map(|item| item.text.clone()).collect()
}

/// Starts announcing "f1" on a worker thread. The blocking mouth holds it in flight. Returns
/// once "f1" is inside the mouth, with the announcer, the mouth, the release sender and the
/// worker handle.
fn start_in_flight() -> (
    Arc<Announcer>,
    Arc<BlockingMouth>,
    Sender<()>,
    thread::JoinHandle<()>,
) {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mouth = Arc::new(BlockingMouth {
        spoken: Mutex::new(Vec::new()),
        entered: Mutex::new(entered_tx),
        release: Mutex::new(release_rx),
        blocked_once: AtomicBool::new(false),
    });
    let announcer = quiet(mouth.clone());
    let worker = announcer.clone();
    let handle = thread::spawn(move || worker.announce(AnnounceKind::Finish, "f1".to_string()));
    entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("f1 reached the mouth");
    (announcer, mouth, release_tx, handle)
}

fn session_cfg() -> SessionConfig {
    SessionConfig {
        post_speech_guard: Duration::ZERO,
        quiet_wait: Duration::from_millis(5),
        ..SessionConfig::default()
    }
}

/// Runs one conversation session. Its only listen aborts, so it reaches exactly one turn
/// boundary (where pending speech is spoken) before it ends.
async fn run_one_boundary(announcer: &Announcer, mouth: &SessionMouth) -> SessionEnd {
    let ears = AbortEars;
    let brain = NoBrain;
    let hands = NoHands;
    let reporter = QuietReporter;
    let deps = Deps {
        ears: &ears,
        mouth,
        hands: &hands,
        brain: &brain,
        reporter: &reporter,
        policy: None,
        announcer: Some(announcer),
    };
    let (_tx, rx) = watch::channel(false);
    run_session(&deps, &session_cfg(), rx).await
}

/// Puts a guarded finish into the queue of a conversation that is still active.
fn queue_guarded_finish(announcer: &Announcer, tokens: &Arc<SummaryTokens>, ticket: Ticket) {
    announcer.set_conversation_active(true);
    announcer.announce_guarded(
        AnnounceKind::Finish,
        SUMMARY.to_string(),
        guard(tokens, ticket),
    );
}

// ------------------------------------------------------- A. announcer level

/// A1: a valid guard at take time keeps the summary text.
#[test]
fn a1_guarded_finish_taken_while_tokens_are_valid_is_the_summary_text() {
    let (mouth, announcer) = direct_announcer();
    let (tokens, ticket) = admitted_tokens();
    queue_guarded_finish(&announcer, &tokens, ticket);
    assert_eq!(
        announcer.take_pending(),
        vec![SUMMARY.to_string()],
        "a valid guard keeps the summary text"
    );
    assert!(mouth.spoken().is_empty(), "nothing spoken while queued");
}

/// A2: admit(newer), invalidate() or close() before take gives exactly the fallback.
#[test]
fn a2_guarded_finish_taken_after_admit_invalidate_or_close_is_only_the_fallback() {
    for which in ["admit", "invalidate", "close"] {
        let (_mouth, announcer) = direct_announcer();
        let (tokens, ticket) = admitted_tokens();
        queue_guarded_finish(&announcer, &tokens, ticket);
        match which {
            "admit" => tokens.admit(2),
            "invalidate" => tokens.invalidate(),
            _ => tokens.close(),
        }
        assert_eq!(
            announcer.take_pending(),
            vec![FALLBACK.to_string()],
            "fallback only, never the summary ({which})"
        );
    }
}

/// A3: unguarded entries keep the M3 order; a mixed queue resolves guarded items in order.
#[test]
fn a3_unguarded_entries_keep_take_pending_order_and_mixed_queues_resolve_in_order() {
    let (_mouth, announcer) = direct_announcer();
    announcer.set_conversation_active(true);
    announcer.announce(AnnounceKind::Progress, "p1".to_string());
    announcer.announce(AnnounceKind::Finish, "f1".to_string());
    assert_eq!(
        announcer.take_pending(),
        strings(&["p1", "f1"]),
        "unguarded entries are taken unchanged, in order"
    );

    let (tokens, ticket) = admitted_tokens();
    announcer.announce(AnnounceKind::Progress, "p2".to_string());
    announcer.announce_guarded(
        AnnounceKind::Finish,
        SUMMARY.to_string(),
        guard(&tokens, ticket),
    );
    tokens.invalidate();
    assert_eq!(
        announcer.take_pending(),
        strings(&["p2", FALLBACK]),
        "mixed queue after invalidation: progress text, then the fallback"
    );
}

/// A3b: take_pending keeps the in-flight entry, exactly as it does today.
#[test]
fn a3b_take_pending_keeps_the_in_flight_entry_and_takes_the_rest() {
    let (announcer, mouth, release, worker) = start_in_flight();
    announcer.announce(AnnounceKind::Progress, "p2".to_string());
    announcer.announce(AnnounceKind::Finish, "f3".to_string());
    assert_eq!(
        announcer.take_pending(),
        strings(&["p2", "f3"]),
        "the in-flight f1 is kept"
    );
    assert!(announcer.take_pending().is_empty(), "second take is empty");
    release.send(()).expect("release the blocked mouth");
    worker.join().expect("worker thread");
    assert_eq!(
        mouth.spoken(),
        strings(&["f1"]),
        "the in-flight entry is still spoken once"
    );
}

/// A4: take_pending_items returns items in order with their guards and removes them.
#[test]
fn a4_take_pending_items_returns_items_in_order_with_guards_and_removes_them() {
    let (_mouth, announcer) = direct_announcer();
    let (tokens, ticket) = admitted_tokens();
    announcer.set_conversation_active(true);
    announcer.announce(AnnounceKind::Progress, "p1".to_string());
    announcer.announce_guarded(
        AnnounceKind::Finish,
        SUMMARY.to_string(),
        guard(&tokens, ticket),
    );
    let items = announcer.take_pending_items();
    assert_eq!(texts_of(&items), strings(&["p1", SUMMARY]), "order kept");
    assert!(items[0].guard.is_none(), "unguarded item has no guard");
    assert!(items[1].guard.is_some(), "guarded item keeps its guard");
    assert!(
        announcer.take_pending_items().is_empty(),
        "a second call returns nothing: the items left the queue"
    );
    assert!(
        announcer.take_pending().is_empty(),
        "take_pending sees nothing too"
    );
}

/// A4b: take_pending_items keeps the in-flight entry, like take_pending.
#[test]
fn a4b_take_pending_items_keeps_the_in_flight_entry() {
    let (announcer, mouth, release, worker) = start_in_flight();
    let (tokens, ticket) = admitted_tokens();
    announcer.announce_guarded(
        AnnounceKind::Finish,
        SUMMARY.to_string(),
        guard(&tokens, ticket),
    );
    let items = announcer.take_pending_items();
    assert_eq!(
        texts_of(&items),
        strings(&[SUMMARY]),
        "the in-flight f1 is not returned"
    );
    release.send(()).expect("release the blocked mouth");
    worker.join().expect("worker thread");
    assert_eq!(mouth.spoken(), strings(&["f1"]));
}

/// A5: resolve() is evaluated at call time, so the same item follows the token state.
#[test]
fn a5_resolve_follows_the_token_state_at_each_call() {
    let (_mouth, announcer) = direct_announcer();
    let (tokens, ticket) = admitted_tokens();
    announcer.set_conversation_active(true);
    announcer.announce(AnnounceKind::Progress, "p".to_string());
    announcer.announce_guarded(
        AnnounceKind::Finish,
        SUMMARY.to_string(),
        guard(&tokens, ticket),
    );
    let items = announcer.take_pending_items();
    assert_eq!(items.len(), 2, "both items taken");
    assert_eq!(items[1].resolve(), SUMMARY, "valid at take time");
    assert_eq!(items[1].resolve(), SUMMARY, "resolving twice while valid");
    tokens.invalidate();
    assert_eq!(items[1].resolve(), FALLBACK, "same item after invalidate");
    assert_eq!(
        items[1].resolve(),
        FALLBACK,
        "resolving twice after invalidate"
    );
    assert_eq!(
        items[0].resolve(),
        "p",
        "unguarded item always keeps its text"
    );
    tokens.close();
    assert_eq!(items[0].resolve(), "p", "unguarded item after close");
}

/// A6: admit(newer) or close() after the item was taken but before resolve gives the fallback.
#[test]
fn a6_admit_or_close_between_take_and_resolve_gives_the_fallback() {
    for which in ["admit", "close"] {
        let (_mouth, announcer) = direct_announcer();
        let (tokens, ticket) = admitted_tokens();
        queue_guarded_finish(&announcer, &tokens, ticket);
        let items = announcer.take_pending_items();
        assert_eq!(items.len(), 1, "the guarded item was taken");
        match which {
            "admit" => tokens.admit(2),
            _ => tokens.close(),
        }
        assert_eq!(
            items[0].resolve(),
            FALLBACK,
            "fallback after {which} between take and resolve"
        );
    }
}

// ----------------------------------------------------- B. converse turn boundary

/// B7: a summary superseded by a newer admission after it was queued is never spoken;
/// the session mouth gets only the fallback, and the direct mouth gets nothing.
#[tokio::test]
async fn b7_guarded_finish_superseded_after_queueing_speaks_only_the_fallback() {
    let (direct, announcer) = direct_announcer();
    let (tokens, ticket) = admitted_tokens();
    queue_guarded_finish(&announcer, &tokens, ticket);
    tokens.admit(2);
    let mouth = SessionMouth::new(None);
    run_one_boundary(&announcer, &mouth).await;
    assert_eq!(
        mouth.spoken(),
        strings(&[FALLBACK]),
        "only the fallback reaches the session mouth"
    );
    assert!(direct.spoken().is_empty(), "no direct announcer speech");
}

/// B8: with the tokens still valid, the session mouth gets the summary exactly once.
#[tokio::test]
async fn b8_guarded_finish_with_valid_tokens_is_spoken_as_the_summary_exactly_once() {
    let (direct, announcer) = direct_announcer();
    let (tokens, ticket) = admitted_tokens();
    queue_guarded_finish(&announcer, &tokens, ticket);
    let mouth = SessionMouth::new(None);
    run_one_boundary(&announcer, &mouth).await;
    assert_eq!(mouth.spoken(), strings(&[SUMMARY]), "summary exactly once");
    assert!(direct.spoken().is_empty(), "no direct announcer speech");
}

/// B9: per-item recheck. A newer admission made while the first item is being spoken
/// turns the second item (the guarded summary) into the fallback.
#[tokio::test]
async fn b9_admission_during_the_first_item_turns_the_next_summary_into_the_fallback() {
    let (_direct, announcer) = direct_announcer();
    let (tokens, ticket) = admitted_tokens();
    let hook_tokens = tokens.clone();
    let mouth = SessionMouth::new(Some(Box::new(move |text: &str| {
        if text == "p1" {
            hook_tokens.admit(2);
        }
    })));
    announcer.set_conversation_active(true);
    announcer.announce(AnnounceKind::Progress, "p1".to_string());
    announcer.announce_guarded(
        AnnounceKind::Finish,
        SUMMARY.to_string(),
        guard(&tokens, ticket),
    );
    run_one_boundary(&announcer, &mouth).await;
    assert_eq!(
        mouth.spoken(),
        strings(&["p1", FALLBACK]),
        "the second spoken text is the fallback, never the summary"
    );
}

/// B10: Ask/TTS toggled (modelled by invalidate) before the turn boundary: fallback only.
#[tokio::test]
async fn b10a_invalidate_before_the_turn_boundary_speaks_only_the_fallback() {
    let (_direct, announcer) = direct_announcer();
    let (tokens, ticket) = admitted_tokens();
    queue_guarded_finish(&announcer, &tokens, ticket);
    tokens.invalidate();
    let mouth = SessionMouth::new(None);
    run_one_boundary(&announcer, &mouth).await;
    assert_eq!(mouth.spoken(), strings(&[FALLBACK]));
}

/// B10b: Ask/TTS toggled during the first item: the next guarded item is the fallback.
#[tokio::test]
async fn b10b_invalidate_during_the_first_item_speaks_the_fallback_for_the_summary() {
    let (_direct, announcer) = direct_announcer();
    let (tokens, ticket) = admitted_tokens();
    let hook_tokens = tokens.clone();
    let mouth = SessionMouth::new(Some(Box::new(move |text: &str| {
        if text == "p1" {
            hook_tokens.invalidate();
        }
    })));
    announcer.set_conversation_active(true);
    announcer.announce(AnnounceKind::Progress, "p1".to_string());
    announcer.announce_guarded(
        AnnounceKind::Finish,
        SUMMARY.to_string(),
        guard(&tokens, ticket),
    );
    run_one_boundary(&announcer, &mouth).await;
    assert_eq!(mouth.spoken(), strings(&["p1", FALLBACK]));
}

/// B11: exit (close) during the first item: the guarded item is the fallback, and the
/// summary text is never spoken after the close.
#[tokio::test]
async fn b11_exit_during_the_first_item_speaks_only_the_fallback_for_the_summary() {
    let (_direct, announcer) = direct_announcer();
    let (tokens, ticket) = admitted_tokens();
    let hook_tokens = tokens.clone();
    let mouth = SessionMouth::new(Some(Box::new(move |text: &str| {
        if text == "p1" {
            hook_tokens.close();
        }
    })));
    announcer.set_conversation_active(true);
    announcer.announce(AnnounceKind::Progress, "p1".to_string());
    announcer.announce_guarded(
        AnnounceKind::Finish,
        SUMMARY.to_string(),
        guard(&tokens, ticket),
    );
    run_one_boundary(&announcer, &mouth).await;
    let spoken = mouth.spoken();
    assert_eq!(spoken, strings(&["p1", FALLBACK]));
    assert!(
        spoken.iter().all(|text| text != SUMMARY),
        "the summary is never spoken after exit began"
    );
}

/// B12: no regression. An unguarded pending finish is spoken exactly once, as before.
#[tokio::test]
async fn b12_unguarded_pending_finish_is_spoken_exactly_once_as_before() {
    let (direct, announcer) = direct_announcer();
    announcer.set_conversation_active(true);
    announcer.announce(AnnounceKind::Finish, FIXED_FINISH.to_string());
    let mouth = SessionMouth::new(None);
    run_one_boundary(&announcer, &mouth).await;
    assert_eq!(
        mouth.spoken(),
        strings(&[FIXED_FINISH]),
        "spoken exactly once"
    );
    assert!(direct.spoken().is_empty(), "no direct announcer speech");
}
