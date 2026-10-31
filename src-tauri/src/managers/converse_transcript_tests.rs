//! Transcript contract tests (T5, DESIGN Rev 3 sections 3.5(1) and 8.2).
//! Red by design: the stubs in `converse.rs` / `converse_runtime.rs` do not emit events yet, so
//! these fail on assertions. Included from converse.rs. Do not edit to make code pass.
#![allow(dead_code)]

use super::*;
use crate::managers::converse_plan::{PlanError, PlanInput, PlanKind, TurnPlan};
use crate::managers::emotion::Emotion;
use futures_util::FutureExt;
use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::watch;

// ---- fakes (private to this file) --------------------------------------------------------

struct TEars(Mutex<VecDeque<ListenOutcome>>);
impl Ears for TEars {
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

struct TMouth;
impl Mouth for TMouth {
    fn say<'a>(&'a self, _text: &'a str, _e: Emotion) -> BoxFuture<'a, SayOutcome> {
        Box::pin(async { SayOutcome::Done })
    }
    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async { QuietOutcome::Quiet })
    }
    fn stop(&self) {}
}

struct THands {
    user: Mutex<VecDeque<PhraseOutcome>>,
    model: Mutex<VecDeque<PhraseOutcome>>,
}
impl Hands for THands {
    fn paste_generated(&self, _text: &str) -> Result<(), String> {
        Ok(())
    }
    fn run_user_phrase<'a>(&'a self, _phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        let o = self
            .user
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(PhraseOutcome::NoMatch);
        Box::pin(async move { o })
    }
    fn run_model_command<'a>(&'a self, _phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        let o = self
            .model
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(PhraseOutcome::NoMatch);
        Box::pin(async move { o })
    }
}

struct TBrain(Mutex<VecDeque<Result<TurnPlan, PlanError>>>);
impl Brain for TBrain {
    fn plan<'a>(&'a self, _input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        let next = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(PlanError::Backend));
        Box::pin(async move { next })
    }
}

/// A brain whose planning panics (used to exercise the Drop fallback).
struct PanicBrain;
impl Brain for PanicBrain {
    fn plan<'a>(&'a self, _input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        Box::pin(async move { panic!("brain exploded") })
    }
}

/// Reporter that collects transcript events: one collector per session.
struct TReporter(Mutex<Vec<TranscriptEvent>>);
impl Reporter for TReporter {
    fn state(&self, _state: SessionState) {}
    fn transcript(&self, ev: TranscriptEvent) {
        self.0.lock().unwrap().push(ev);
    }
}

// ---- helpers -----------------------------------------------------------------------------

fn heard(text: &str) -> ListenOutcome {
    ListenOutcome::Utterance(text.to_string())
}

fn cfg(context_turns: usize) -> SessionConfig {
    SessionConfig {
        post_speech_guard: Duration::from_millis(0),
        quiet_wait: Duration::from_millis(5),
        context_turns,
        ..SessionConfig::default()
    }
}

fn plan(kind: PlanKind, speech: &str) -> TurnPlan {
    TurnPlan {
        plan: None,
        state: None,
        job: None,
        role: None,
        kind,
        speech: speech.to_string(),
        emotion: Emotion::Neutral,
        paste_text: None,
        command: None,
        agent: None,
    }
}

fn hands(user: Vec<PhraseOutcome>, model: Vec<PhraseOutcome>) -> THands {
    THands {
        user: Mutex::new(VecDeque::from(user)),
        model: Mutex::new(VecDeque::from(model)),
    }
}

/// Runs one session to its end and returns the end plus the events it emitted.
async fn drive(
    utts: Vec<ListenOutcome>,
    brain: &dyn Brain,
    hands: &THands,
    cfg: &SessionConfig,
) -> (SessionEnd, Vec<TranscriptEvent>) {
    let ears = TEars(Mutex::new(VecDeque::from(utts)));
    let mouth = TMouth;
    let reporter = TReporter(Mutex::new(vec![]));
    let (tx, rx) = watch::channel(false);
    let deps = Deps {
        policy: None,
        announcer: None,
        ears: &ears,
        mouth: &mouth,
        hands,
        brain,
        reporter: &reporter,
    };
    let end = tokio::time::timeout(Duration::from_secs(10), run_session(&deps, cfg, rx))
        .await
        .expect("run_session hung");
    drop(tx);
    let events = reporter.0.lock().unwrap().clone();
    (end, events)
}

fn event(
    user: &str,
    assistant: Option<&str>,
    outcome: TurnOutcome,
    delivery: Delivery,
    detail: Option<&str>,
) -> TranscriptEvent {
    TranscriptEvent {
        user: user.to_string(),
        assistant: assistant.map(str::to_string),
        outcome,
        delivery,
        detail: detail.map(str::to_string),
    }
}

// ---- T5: one event per accepted utterance, every tier ------------------------------------

#[tokio::test]
async fn t5_local_end_records_ended_with_the_submitted_goodbye() {
    let (end, events) = drive(
        vec![heard("goodbye")],
        &TBrain(Mutex::new(VecDeque::new())),
        &hands(vec![], vec![]),
        &cfg(6),
    )
    .await;
    assert_eq!(end, SessionEnd::UserEnded);
    assert_eq!(
        events,
        vec![event(
            "goodbye",
            Some("Okay."),
            TurnOutcome::Ended,
            Delivery::Submitted,
            None
        )]
    );
}

#[tokio::test]
async fn t5_local_dictation_records_dictated_with_a_char_count_not_the_text() {
    let (_end, events) = drive(
        vec![
            heard("write this down: hello world"),
            ListenOutcome::Aborted,
        ],
        &TBrain(Mutex::new(VecDeque::new())),
        &hands(vec![], vec![]),
        &cfg(6),
    )
    .await;
    assert_eq!(events.len(), 1, "one event for the dictated utterance");
    let ev = &events[0];
    assert_eq!(ev.outcome, TurnOutcome::Dictated);
    assert_eq!(ev.delivery, Delivery::NotSpoken);
    assert_eq!(ev.assistant, None, "silent success submits no speech");
    let detail = ev.detail.as_deref().unwrap_or("");
    assert!(
        detail.contains("11"),
        "detail should carry the char count: {detail:?}"
    );
    assert!(!detail.contains("hello"), "detail must not carry the text");
}

#[tokio::test]
async fn t5_exact_action_success_records_acted_with_the_submitted_confirmation() {
    let (_end, events) = drive(
        vec![heard("open notes"), ListenOutcome::Aborted],
        &TBrain(Mutex::new(VecDeque::new())),
        &hands(vec![PhraseOutcome::Done("Opened notes".into())], vec![]),
        &cfg(6),
    )
    .await;
    assert_eq!(
        events,
        vec![event(
            "open notes",
            Some("Opened notes"),
            TurnOutcome::Acted,
            Delivery::Submitted,
            None
        )]
    );
}

#[tokio::test]
async fn t5_exact_action_failure_records_failed() {
    let (_end, events) = drive(
        vec![heard("open notes"), ListenOutcome::Aborted],
        &TBrain(Mutex::new(VecDeque::new())),
        &hands(vec![PhraseOutcome::Failed], vec![]),
        &cfg(6),
    )
    .await;
    assert_eq!(
        events,
        vec![event(
            "open notes",
            Some("That didn't work."),
            TurnOutcome::Failed,
            Delivery::Submitted,
            None
        )]
    );
}

#[tokio::test]
async fn t5_brain_answer_records_the_submitted_speech() {
    let brain = TBrain(Mutex::new(VecDeque::from(vec![Ok(plan(
        PlanKind::Answer,
        "Rust is a language.",
    ))])));
    let (_end, events) = drive(
        vec![heard("what is rust"), ListenOutcome::Aborted],
        &brain,
        &hands(vec![], vec![]),
        &cfg(6),
    )
    .await;
    assert_eq!(
        events,
        vec![event(
            "what is rust",
            Some("Rust is a language."),
            TurnOutcome::Answered,
            Delivery::Submitted,
            None
        )]
    );
}

#[tokio::test]
async fn t5_brain_paste_records_the_confirmation_and_never_the_pasted_text() {
    let brain = TBrain(Mutex::new(VecDeque::from(vec![Ok(TurnPlan {
        job: None,
        role: None,
        paste_text: Some("SECRET BODY".to_string()),
        ..plan(PlanKind::Paste, "Done, pasted.")
    })])));
    let (_end, events) = drive(
        vec![heard("draft a reply"), ListenOutcome::Aborted],
        &brain,
        &hands(vec![], vec![]),
        &cfg(6),
    )
    .await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome, TurnOutcome::Dictated);
    assert_eq!(events[0].assistant.as_deref(), Some("Done, pasted."));
    assert!(
        !format!("{events:?}").contains("SECRET BODY"),
        "pasted text must never be recorded"
    );
}

#[tokio::test]
async fn t5_brain_command_records_an_action_kind_without_the_command_text() {
    let brain = TBrain(Mutex::new(VecDeque::from(vec![Ok(TurnPlan {
        job: None,
        role: None,
        command: Some("open safari".to_string()),
        ..plan(PlanKind::Command, "")
    })])));
    let (_end, events) = drive(
        vec![heard("open safari"), ListenOutcome::Aborted],
        &brain,
        &hands(vec![], vec![PhraseOutcome::Done("Opened Safari".into())]),
        &cfg(6),
    )
    .await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome, TurnOutcome::Acted);
    let detail = events[0].detail.as_deref().unwrap_or("");
    assert!(detail.starts_with("action: "), "detail was {detail:?}");
    assert!(
        !detail.to_lowercase().contains("safari"),
        "detail must not carry the command text"
    );
}

#[tokio::test]
async fn t5_repeated_brain_failure_records_failed_with_the_spoken_failure_line() {
    let brain = TBrain(Mutex::new(VecDeque::from(vec![Err(PlanError::Backend)])));
    let (_end, events) = drive(
        vec![heard("what is go"), ListenOutcome::Aborted],
        &brain,
        &hands(vec![], vec![]),
        &cfg(6),
    )
    .await;
    assert_eq!(
        events,
        vec![event(
            "what is go",
            Some("Sorry, I couldn't work that out."),
            TurnOutcome::Failed,
            Delivery::Submitted,
            None
        )]
    );
}

#[tokio::test]
async fn t5_silence_and_empty_audio_emit_no_event() {
    let (_end, events) = drive(
        vec![
            ListenOutcome::NoSpeech,
            heard("   "),
            ListenOutcome::NoSpeech,
            ListenOutcome::Aborted,
        ],
        &TBrain(Mutex::new(VecDeque::new())),
        &hands(vec![], vec![]),
        &cfg(6),
    )
    .await;
    assert!(
        events.is_empty(),
        "unaccepted utterances must not emit: {events:?}"
    );
}

#[tokio::test]
async fn t5_context_turns_zero_still_records_every_utterance() {
    let brain = TBrain(Mutex::new(VecDeque::from(vec![Ok(plan(
        PlanKind::Answer,
        "Rust is a language.",
    ))])));
    let (_end, events) = drive(
        vec![heard("what is rust"), ListenOutcome::Aborted],
        &brain,
        &hands(vec![], vec![]),
        &cfg(0),
    )
    .await;
    assert_eq!(events.len(), 1, "context_turns=0 must not suppress records");
    assert_eq!(events[0].outcome, TurnOutcome::Answered);
}

#[tokio::test]
async fn t5_one_event_per_utterance_in_order() {
    let brain = TBrain(Mutex::new(VecDeque::from(vec![
        Ok(plan(PlanKind::Answer, "Rust is a language.")),
        Err(PlanError::Backend),
    ])));
    let (end, events) = drive(
        vec![heard("what is rust"), heard("what is go"), heard("goodbye")],
        &brain,
        &hands(vec![], vec![]),
        &cfg(6),
    )
    .await;
    assert_eq!(end, SessionEnd::UserEnded);
    let shape: Vec<(&str, TurnOutcome)> = events
        .iter()
        .map(|e| (e.user.as_str(), e.outcome))
        .collect();
    assert_eq!(
        shape,
        vec![
            ("what is rust", TurnOutcome::Answered),
            ("what is go", TurnOutcome::Failed),
            ("goodbye", TurnOutcome::Ended),
        ]
    );
}

#[tokio::test]
async fn t5_a_panicking_brain_emits_one_interrupted_or_failed_event_via_drop() {
    let ears = TEars(Mutex::new(VecDeque::from(vec![heard("what is rust")])));
    let mouth = TMouth;
    let hands = hands(vec![], vec![]);
    let brain = PanicBrain;
    let reporter = TReporter(Mutex::new(vec![]));
    let (_tx, rx) = watch::channel(false);
    let deps = Deps {
        policy: None,
        announcer: None,
        ears: &ears,
        mouth: &mouth,
        hands: &hands,
        brain: &brain,
        reporter: &reporter,
    };
    // The panic is contained at this boundary; the session future is dropped mid-turn.
    let _ = FutureExt::catch_unwind(AssertUnwindSafe(run_session(&deps, &cfg(6), rx))).await;
    let events = reporter.0.lock().unwrap().clone();
    assert_eq!(
        events.len(),
        1,
        "the dropped utterance must still be recorded"
    );
    assert_eq!(events[0].user, "what is rust");
    assert!(
        matches!(
            events[0].outcome,
            TurnOutcome::Interrupted | TurnOutcome::Failed
        ),
        "outcome was {:?}",
        events[0].outcome
    );
}

#[tokio::test]
async fn t5_two_consecutive_sessions_have_disjoint_collectors() {
    let brain = TBrain(Mutex::new(VecDeque::new()));
    let (_e1, first) = drive(
        vec![heard("goodbye")],
        &brain,
        &hands(vec![], vec![]),
        &cfg(6),
    )
    .await;
    let (_e2, second) = drive(vec![heard("bye")], &brain, &hands(vec![], vec![]), &cfg(6)).await;
    assert_eq!(first.len(), 1, "first session: {first:?}");
    assert_eq!(second.len(), 1, "second session: {second:?}");
    assert_eq!(first[0].user, "goodbye");
    assert_eq!(second[0].user, "bye");
}

// ---- barge-in and cancellation (DESIGN Rev 3, 8.2 and T5) ------------------------------

/// Queue that records each submitted sentence and fires the session cancel after the
/// `cancel_at`-th push (a barge-in observed while speech is queued).
struct BargeQueue {
    submitted: Arc<Mutex<Vec<String>>>,
    cancel: Arc<watch::Sender<bool>>,
    cancel_at: usize,
}
impl SpeechQueue for BargeQueue {
    fn push(&self, text: &str, _e: Emotion) -> bool {
        let count = {
            let mut s = self.submitted.lock().unwrap();
            s.push(text.to_string());
            s.len()
        };
        if count == self.cancel_at {
            let _ = self.cancel.send(true);
        }
        true
    }
    fn abort(&self) {}
    fn finish(&self) -> BoxFuture<'_, SayOutcome> {
        Box::pin(async { SayOutcome::Interrupted })
    }
}

/// Mouth that offers a queue, so the streaming path runs.
struct QueueMouth {
    submitted: Arc<Mutex<Vec<String>>>,
    cancel: Arc<watch::Sender<bool>>,
    cancel_at: usize,
}
impl Mouth for QueueMouth {
    fn say<'a>(&'a self, _text: &'a str, _e: Emotion) -> BoxFuture<'a, SayOutcome> {
        Box::pin(async { SayOutcome::Done })
    }
    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async { QuietOutcome::Quiet })
    }
    fn stop(&self) {}
    fn open_queue(&self) -> Option<Arc<dyn SpeechQueue>> {
        Some(Arc::new(BargeQueue {
            submitted: self.submitted.clone(),
            cancel: self.cancel.clone(),
            cancel_at: self.cancel_at,
        }))
    }
}

/// Streams the given deltas through `on_text`, then never completes (until cancelled).
struct StreamThenHang {
    deltas: Vec<String>,
}
impl Brain for StreamThenHang {
    fn plan<'a>(&'a self, _input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        Box::pin(std::future::pending())
    }
    fn plan_streaming<'a>(
        &'a self,
        _input: PlanInput<'a>,
        on_text: &'a mut (dyn FnMut(&str) + Send),
    ) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        Box::pin(async move {
            for delta in &self.deltas {
                on_text(delta);
            }
            std::future::pending::<()>().await;
            Err(PlanError::Backend)
        })
    }
}

/// Brain whose `plan` never completes.
struct NeverPlanBrain;
impl Brain for NeverPlanBrain {
    fn plan<'a>(&'a self, _input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        Box::pin(std::future::pending())
    }
}

#[tokio::test]
async fn t5_streaming_answer_with_several_submitted_sentences_then_barge_in() {
    let (tx, rx) = watch::channel(false);
    let tx = Arc::new(tx);
    let submitted: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let mouth = QueueMouth {
        submitted: submitted.clone(),
        cancel: tx.clone(),
        cancel_at: 2,
    };
    let ears = TEars(Mutex::new(VecDeque::from(vec![heard(
        "tell me about rust",
    )])));
    let brain = StreamThenHang {
        deltas: vec![
            r#"{"kind":"answer","emotion":"calm","speech":"First sentence is long enough here. "#
                .to_string(),
            "Second sentence is also long enough here. ".to_string(),
            "Third sentence must never be submitted.\"}".to_string(),
        ],
    };
    let hands = hands(vec![], vec![]);
    let reporter = TReporter(Mutex::new(vec![]));
    let deps = Deps {
        policy: None,
        announcer: None,
        ears: &ears,
        mouth: &mouth,
        hands: &hands,
        brain: &brain,
        reporter: &reporter,
    };
    let cfg = SessionConfig {
        early_speech: true,
        ..cfg(6)
    };
    let end = tokio::time::timeout(Duration::from_secs(10), run_session(&deps, &cfg, rx))
        .await
        .expect("run_session hung");
    drop(tx);

    assert_eq!(end, SessionEnd::Cancelled, "barge-in ends the session");
    let events = reporter.0.lock().unwrap().clone();
    assert_eq!(
        events.len(),
        1,
        "exactly one event, no second one: {events:?}"
    );
    let ev = &events[0];
    assert_eq!(ev.user, "tell me about rust");
    assert_eq!(ev.outcome, TurnOutcome::Interrupted);
    assert_eq!(ev.delivery, Delivery::Interrupted);
    assert_eq!(
        ev.assistant.as_deref(),
        Some("First sentence is long enough here. Second sentence is also long enough here."),
        "assistant must be exactly the submitted sentences, never a heard prefix or the 3rd sentence"
    );
}

#[tokio::test]
async fn t5_cancellation_during_planning() {
    let (tx, rx) = watch::channel(false);
    let tx = Arc::new(tx);
    let canceller = tx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = canceller.send(true);
    });
    let ears = TEars(Mutex::new(VecDeque::from(vec![heard("what is rust")])));
    let mouth = TMouth;
    let hands = hands(vec![], vec![]);
    let reporter = TReporter(Mutex::new(vec![]));
    let deps = Deps {
        policy: None,
        announcer: None,
        ears: &ears,
        mouth: &mouth,
        hands: &hands,
        brain: &NeverPlanBrain,
        reporter: &reporter,
    };
    let end = tokio::time::timeout(Duration::from_secs(10), run_session(&deps, &cfg(6), rx))
        .await
        .expect("run_session hung");
    drop(tx);

    assert_eq!(end, SessionEnd::Cancelled);
    let events = reporter.0.lock().unwrap().clone();
    assert_eq!(
        events.len(),
        1,
        "exactly one event for the accepted utterance: {events:?}"
    );
    let ev = &events[0];
    assert_eq!(ev.user, "what is rust");
    assert_eq!(ev.outcome, TurnOutcome::Interrupted);
    assert_eq!(ev.delivery, Delivery::NotSpoken);
    assert_eq!(ev.assistant, None, "nothing was spoken");
}
