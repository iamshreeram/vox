//! Answer-only boundary tests (T10, DESIGN Rev 3 sections 3.6 and 8.1).
//! Red by design: `SessionConfig::answer_only` is not enforced yet, so these fail on assertions.
//! Included from converse.rs. Do not edit to make code pass.
#![allow(dead_code)]

use super::*;
use crate::managers::converse_plan::{PlanError, PlanInput, PlanKind, TurnPlan};
use crate::managers::emotion::Emotion;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::watch;

const REFUSAL: &str =
    "Vault context is on, so I can only answer questions. Turn it off to dictate text or open things.";

/// Everything the mouth/queue was asked to speak, in order ("say:..." / "push:...").
type Log = Arc<Mutex<Vec<String>>>;

struct AQueue {
    log: Log,
}
impl SpeechQueue for AQueue {
    fn push(&self, text: &str, _e: Emotion) -> bool {
        self.log.lock().unwrap().push(format!("push:{text}"));
        true
    }
    fn abort(&self) {}
    fn finish(&self) -> BoxFuture<'_, SayOutcome> {
        Box::pin(async { SayOutcome::Done })
    }
}

struct AMouth {
    log: Log,
}
impl Mouth for AMouth {
    fn say<'a>(&'a self, text: &'a str, _e: Emotion) -> BoxFuture<'a, SayOutcome> {
        self.log.lock().unwrap().push(format!("say:{text}"));
        Box::pin(async { SayOutcome::Done })
    }
    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async { QuietOutcome::Quiet })
    }
    fn stop(&self) {}
    fn open_queue(&self) -> Option<Arc<dyn SpeechQueue>> {
        Some(Arc::new(AQueue {
            log: self.log.clone(),
        }))
    }
}

#[derive(Default)]
struct AHands {
    paste_calls: AtomicUsize,
    model_calls: AtomicUsize,
    user_calls: Mutex<Vec<String>>,
    user: Mutex<VecDeque<PhraseOutcome>>,
}
impl Hands for AHands {
    fn paste_generated(&self, _text: &str) -> Result<(), String> {
        self.paste_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn run_user_phrase<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        self.user_calls.lock().unwrap().push(phrase.to_string());
        let o = self
            .user
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(PhraseOutcome::NoMatch);
        Box::pin(async move { o })
    }
    fn run_model_command<'a>(&'a self, _phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        self.model_calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { PhraseOutcome::Done("ran".into()) })
    }
}

/// Brain that counts both entry points. `streamed` is what `plan_streaming` emits (its
/// deltas, then its plan); `plans` is what `plan` returns, one per call.
struct ABrain {
    plan_calls: AtomicUsize,
    streaming_calls: AtomicUsize,
    plans: Mutex<VecDeque<Result<TurnPlan, PlanError>>>,
    streamed: Mutex<Option<(String, Result<TurnPlan, PlanError>)>>,
}
impl ABrain {
    fn new(
        plans: Vec<Result<TurnPlan, PlanError>>,
        streamed: Option<(String, Result<TurnPlan, PlanError>)>,
    ) -> Self {
        ABrain {
            plan_calls: AtomicUsize::new(0),
            streaming_calls: AtomicUsize::new(0),
            plans: Mutex::new(VecDeque::from(plans)),
            streamed: Mutex::new(streamed),
        }
    }
}
impl Brain for ABrain {
    fn plan<'a>(&'a self, _input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        self.plan_calls.fetch_add(1, Ordering::SeqCst);
        let next = self
            .plans
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(PlanError::Backend));
        Box::pin(async move { next })
    }
    fn plan_streaming<'a>(
        &'a self,
        _input: PlanInput<'a>,
        on_text: &'a mut (dyn FnMut(&str) + Send),
    ) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        self.streaming_calls.fetch_add(1, Ordering::SeqCst);
        let streamed = self.streamed.lock().unwrap().take();
        Box::pin(async move {
            match streamed {
                Some((delta, result)) => {
                    on_text(&delta);
                    result
                }
                None => Err(PlanError::Backend),
            }
        })
    }
}

fn plan(kind: PlanKind, speech: &str) -> TurnPlan {
    TurnPlan {
        kind,
        speech: speech.to_string(),
        emotion: Emotion::Neutral,
        paste_text: None,
        command: None,
    }
}

fn paste_plan() -> TurnPlan {
    TurnPlan {
        paste_text: Some("model-written body".to_string()),
        ..plan(PlanKind::Paste, "Pasted it.")
    }
}

fn command_plan() -> TurnPlan {
    TurnPlan {
        command: Some("open safari".to_string()),
        ..plan(PlanKind::Command, "Opening Safari.")
    }
}

fn cfg(answer_only: bool, streaming: bool) -> SessionConfig {
    SessionConfig {
        post_speech_guard: Duration::from_millis(0),
        quiet_wait: Duration::from_millis(5),
        early_speech: streaming,
        answer_only,
        ..SessionConfig::default()
    }
}

/// Runs one session over the given utterances. Returns the end, every spoken or queued
/// string, the brain and hands counters, and the transcript events.
async fn drive(
    utts: Vec<&str>,
    brain: &ABrain,
    hands: &AHands,
    cfg: &SessionConfig,
) -> (SessionEnd, Vec<String>, Vec<TranscriptEvent>) {
    let log: Log = Arc::new(Mutex::new(vec![]));
    let ears = Ears2(Mutex::new(VecDeque::from(
        utts.iter()
            .map(|u| ListenOutcome::Utterance((*u).to_string()))
            .collect::<Vec<_>>(),
    )));
    let mouth = AMouth { log: log.clone() };
    let reporter = Rep(Mutex::new(vec![]));
    let (_tx, rx) = watch::channel(false);
    let deps = Deps {
        ears: &ears,
        mouth: &mouth,
        hands,
        brain,
        reporter: &reporter,
    };
    let end = tokio::time::timeout(Duration::from_secs(10), run_session(&deps, cfg, rx))
        .await
        .expect("run_session hung");
    let spoken = log.lock().unwrap().clone();
    let events = reporter.0.lock().unwrap().clone();
    (end, spoken, events)
}

struct Ears2(Mutex<VecDeque<ListenOutcome>>);
impl Ears for Ears2 {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        let next = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(ListenOutcome::Aborted);
        Box::pin(async move { next })
    }
}

struct Rep(Mutex<Vec<TranscriptEvent>>);
impl Reporter for Rep {
    fn state(&self, _s: SessionState) {}
    fn transcript(&self, ev: TranscriptEvent) {
        self.0.lock().unwrap().push(ev);
    }
}

// ---- T10: answer-only blocks model paste and command, before any speech -----------------

#[tokio::test]
async fn t10_answer_only_never_calls_plan_streaming_even_when_streaming_is_on() {
    let brain = ABrain::new(vec![Ok(plan(PlanKind::Answer, "Fine."))], None);
    let (_end, _spoken, _ev) = drive(
        vec!["how are you", "stop"],
        &brain,
        &AHands::default(),
        &cfg(true, true),
    )
    .await;
    assert_eq!(
        brain.streaming_calls.load(Ordering::SeqCst),
        0,
        "plan_streaming must never run in an answer-only session"
    );
    assert_eq!(
        brain.plan_calls.load(Ordering::SeqCst),
        1,
        "plan is used instead"
    );
}

#[tokio::test]
async fn t10_answer_only_streamed_paste_is_blocked_with_no_model_speech_or_effects() {
    // plan_streaming would speak a complete reply first, then request a paste. The same
    // actionable plan is ALSO available from the non-streaming `plan()` (the only path an
    // answer-only session may use), so the Rust boundary, not the fake, must block it.
    let brain = ABrain::new(
        vec![Ok(paste_plan())],
        Some((
            "Here is the draft that I wrote for you. ".to_string(),
            Ok(paste_plan()),
        )),
    );
    let hands = AHands::default();
    let (_end, spoken, events) = drive(
        vec!["draft me something", "stop"],
        &brain,
        &hands,
        &cfg(true, true),
    )
    .await;
    assert_eq!(
        hands.paste_calls.load(Ordering::SeqCst),
        0,
        "no paste_generated"
    );
    assert_eq!(
        hands.model_calls.load(Ordering::SeqCst),
        0,
        "no run_model_command"
    );
    assert!(
        spoken
            .iter()
            .all(|s| !s.contains("draft that I wrote") && !s.contains("model-written")),
        "no model-derived text may reach the mouth or queue: {spoken:?}"
    );
    assert!(
        spoken
            .iter()
            .any(|s| s == &format!("say:{REFUSAL}") || s == &format!("push:{REFUSAL}")),
        "the fixed refusal must be spoken: {spoken:?}"
    );
    assert_eq!(
        events.first().map(|e| e.outcome),
        Some(TurnOutcome::Blocked)
    );
}

#[tokio::test]
async fn t10_answer_only_plan_paste_is_blocked_and_speaks_only_the_refusal() {
    let brain = ABrain::new(vec![Ok(paste_plan())], None);
    let hands = AHands::default();
    let (_end, spoken, events) = drive(
        vec!["draft me something", "stop"],
        &brain,
        &hands,
        &cfg(true, false),
    )
    .await;
    assert_eq!(hands.paste_calls.load(Ordering::SeqCst), 0);
    assert!(
        !spoken.iter().any(|s| s.contains("Pasted it.")),
        "the model's paste speech must not be spoken: {spoken:?}"
    );
    assert!(
        spoken.iter().any(|s| s == &format!("say:{REFUSAL}")),
        "exact refusal line expected: {spoken:?}"
    );
    assert_eq!(
        events.first().map(|e| e.outcome),
        Some(TurnOutcome::Blocked)
    );
    assert_eq!(
        events.first().and_then(|e| e.assistant.clone()).as_deref(),
        Some(REFUSAL),
        "the event must carry the refusal actually spoken"
    );
}

#[tokio::test]
async fn t10_answer_only_plan_command_is_blocked_with_zero_model_command_calls() {
    let brain = ABrain::new(vec![Ok(command_plan())], None);
    let hands = AHands::default();
    let (_end, spoken, events) = drive(
        vec!["open safari", "stop"],
        &brain,
        &hands,
        &cfg(true, false),
    )
    .await;
    assert_eq!(
        hands.model_calls.load(Ordering::SeqCst),
        0,
        "no run_model_command"
    );
    assert!(
        !spoken.iter().any(|s| s.contains("Opening Safari")),
        "the model's command speech must not be spoken: {spoken:?}"
    );
    assert!(spoken.iter().any(|s| s == &format!("say:{REFUSAL}")));
    assert_eq!(
        events.first().map(|e| e.outcome),
        Some(TurnOutcome::Blocked)
    );
}

#[tokio::test]
async fn t10_answer_only_streamed_command_is_blocked_with_zero_model_command_calls() {
    // The actionable plan is also available from `plan()`: an answer-only session uses only
    // that path and must still refuse to execute it.
    let brain = ABrain::new(
        vec![Ok(command_plan())],
        Some(("Opening Safari now. ".to_string(), Ok(command_plan()))),
    );
    let hands = AHands::default();
    let (_end, spoken, _ev) = drive(
        vec!["open safari", "stop"],
        &brain,
        &hands,
        &cfg(true, true),
    )
    .await;
    assert_eq!(hands.model_calls.load(Ordering::SeqCst), 0);
    assert!(
        spoken.iter().all(|s| !s.contains("Opening Safari")),
        "no model-derived text may be queued or spoken: {spoken:?}"
    );
}

// ---- T10: allowed plans are delivered normally ------------------------------------------

#[tokio::test]
async fn t10_answer_only_answer_clarify_and_end_plans_are_delivered() {
    let brain = ABrain::new(
        vec![
            Ok(plan(PlanKind::Answer, "Rust is a language.")),
            Ok(plan(PlanKind::Clarify, "Which Rust do you mean?")),
            Ok(plan(PlanKind::End, "See you.")),
        ],
        None,
    );
    let (end, spoken, _ev) = drive(
        vec!["what is rust", "which one", "bye for now"],
        &brain,
        &AHands::default(),
        &cfg(true, false),
    )
    .await;
    assert_eq!(end, SessionEnd::UserEnded);
    for expected in ["Rust is a language.", "Which Rust do you mean?", "See you."] {
        assert!(
            spoken.iter().any(|s| s == &format!("say:{expected}")),
            "{expected:?} must be delivered: {spoken:?}"
        );
    }
}

#[tokio::test]
async fn t10_answer_only_still_runs_the_users_own_exact_phrase() {
    let brain = ABrain::new(vec![], None);
    let hands = AHands {
        user: Mutex::new(VecDeque::from(vec![PhraseOutcome::Done(
            "Opened notes".into(),
        )])),
        ..AHands::default()
    };
    let (_end, _spoken, _ev) = drive(
        vec!["open notes", "stop"],
        &brain,
        &hands,
        &cfg(true, false),
    )
    .await;
    assert_eq!(
        *hands.user_calls.lock().unwrap(),
        vec!["open notes".to_string()],
        "the user's own exact phrase must still reach run_user_phrase"
    );
}

// ---- T10: answer_only = false keeps today's behaviour -----------------------------------

#[tokio::test]
async fn t10_answer_only_off_streaming_plan_executes_and_streams_as_today() {
    let brain = ABrain::new(
        vec![],
        Some((
            "Here is the draft that I wrote for you. ".to_string(),
            Ok(paste_plan()),
        )),
    );
    let hands = AHands::default();
    let (_end, _spoken, _ev) = drive(
        vec!["draft me something", "stop"],
        &brain,
        &hands,
        &cfg(false, true),
    )
    .await;
    assert_eq!(
        brain.streaming_calls.load(Ordering::SeqCst),
        1,
        "streaming stays allowed when answer_only is false"
    );
    assert_eq!(
        hands.paste_calls.load(Ordering::SeqCst),
        1,
        "the paste plan must execute when answer_only is false"
    );
}

#[tokio::test]
async fn t10_answer_only_off_plan_command_executes_as_today() {
    let brain = ABrain::new(vec![Ok(command_plan())], None);
    let hands = AHands::default();
    let (_end, _spoken, _ev) = drive(
        vec!["open safari", "stop"],
        &brain,
        &hands,
        &cfg(false, false),
    )
    .await;
    assert_eq!(
        hands.model_calls.load(Ordering::SeqCst),
        1,
        "the command plan must execute when answer_only is false"
    );
}
