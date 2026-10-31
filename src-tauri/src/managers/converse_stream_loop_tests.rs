//! Authoritative loop-level tests for realtime early speech / acknowledgement
//! (`SessionConfig::early_speech`, `ack_after`; `Mouth::open_queue`; `Brain::plan_streaming`).
//! Included from converse.rs. Do not edit to make code pass.

use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::watch;

type Shared = Arc<Mutex<Vec<String>>>;

fn log_of(l: &Shared) -> Vec<String> {
    l.lock().unwrap().clone()
}
fn has(l: &Shared, e: &str) -> bool {
    log_of(l).iter().any(|x| x == e)
}
fn count_prefix(l: &Shared, p: &str) -> usize {
    log_of(l).iter().filter(|x| x.starts_with(p)).count()
}
fn pos(l: &Shared, e: &str) -> Option<usize> {
    log_of(l).iter().position(|x| x == e)
}

// ---- fakes -------------------------------------------------------------------------------

struct FakeQueue {
    log: Shared,
    pushes: Mutex<Vec<(String, Emotion)>>,
    /// Refuse every push after this many accepted ones.
    accept_limit: Option<usize>,
    finish_outcome: SayOutcome,
}
impl SpeechQueue for FakeQueue {
    fn push(&self, text: &str, emotion: Emotion) -> bool {
        let mut p = self.pushes.lock().unwrap();
        if let Some(n) = self.accept_limit {
            if p.len() >= n {
                self.log.lock().unwrap().push(format!("refused:{text}"));
                return false;
            }
        }
        p.push((text.to_string(), emotion));
        self.log.lock().unwrap().push(format!("push:{text}"));
        true
    }
    fn abort(&self) {
        self.log.lock().unwrap().push("abort".into());
    }
    fn finish(&self) -> BoxFuture<'_, SayOutcome> {
        self.log.lock().unwrap().push("finish".into());
        let o = self.finish_outcome;
        Box::pin(async move { o })
    }
}

struct QMouth {
    log: Shared,
    offer: bool,
    opens: AtomicUsize,
    queues: Mutex<Vec<Arc<FakeQueue>>>,
    accept_limit: Option<usize>,
    finish_outcome: SayOutcome,
}
impl Mouth for QMouth {
    fn say<'a>(&'a self, text: &'a str, _e: Emotion) -> BoxFuture<'a, SayOutcome> {
        self.log.lock().unwrap().push(format!("say:{text}"));
        Box::pin(async { SayOutcome::Done })
    }
    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async { QuietOutcome::Quiet })
    }
    fn stop(&self) {
        self.log.lock().unwrap().push("stop".into());
    }
    fn open_queue(&self) -> Option<Arc<dyn SpeechQueue>> {
        if !self.offer {
            return None;
        }
        self.opens.fetch_add(1, Ordering::SeqCst);
        self.log.lock().unwrap().push("open".into());
        let q = Arc::new(FakeQueue {
            log: self.log.clone(),
            pushes: Mutex::new(vec![]),
            accept_limit: self.accept_limit,
            finish_outcome: self.finish_outcome,
        });
        self.queues.lock().unwrap().push(q.clone());
        Some(q)
    }
}

struct Turn {
    /// (delay before delivering, raw delta)
    deltas: Vec<(u64, String)>,
    /// Delay after the last delta before the plan resolves.
    final_delay: u64,
    plan: Result<TurnPlan, PlanError>,
}

struct SBrain {
    log: Shared,
    script: Mutex<VecDeque<Turn>>,
    plain_calls: AtomicUsize,
    streaming_calls: AtomicUsize,
    histories: Mutex<Vec<Vec<(String, String)>>>,
}
impl SBrain {
    fn next(&self) -> Option<Turn> {
        self.script.lock().unwrap().pop_front()
    }
}
impl Brain for SBrain {
    fn plan<'a>(&'a self, input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        self.plain_calls.fetch_add(1, Ordering::SeqCst);
        self.histories.lock().unwrap().push(input.history.to_vec());
        let t = self.next();
        Box::pin(async move {
            match t {
                Some(t) => t.plan,
                None => Err(PlanError::Backend),
            }
        })
    }
    fn plan_streaming<'a>(
        &'a self,
        input: PlanInput<'a>,
        on_text: &'a mut (dyn FnMut(&str) + Send),
    ) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        self.streaming_calls.fetch_add(1, Ordering::SeqCst);
        self.histories.lock().unwrap().push(input.history.to_vec());
        let t = self.next();
        let log = self.log.clone();
        Box::pin(async move {
            let Some(t) = t else {
                return Err(PlanError::Backend);
            };
            for (delay, delta) in t.deltas {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                on_text(&delta);
            }
            tokio::time::sleep(Duration::from_millis(t.final_delay)).await;
            log.lock().unwrap().push("plan_done".into());
            t.plan
        })
    }
}

struct SEars {
    script: Mutex<VecDeque<String>>,
    log: Shared,
}
impl Ears for SEars {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        self.log.lock().unwrap().push("listen".into());
        let n = self.script.lock().unwrap().pop_front();
        Box::pin(async move {
            match n {
                Some(u) => ListenOutcome::Utterance(u),
                None => ListenOutcome::Aborted,
            }
        })
    }
}

struct SHands {
    log: Shared,
}
impl Hands for SHands {
    fn paste_generated(&self, text: &str) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("paste:{text}"));
        Ok(())
    }
    fn run_user_phrase<'a>(&'a self, _p: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        Box::pin(async { PhraseOutcome::NoMatch })
    }
    fn run_model_command<'a>(&'a self, p: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        self.log.lock().unwrap().push(format!("model:{p}"));
        Box::pin(async { PhraseOutcome::Done("ok".into()) })
    }
}

struct SReporter(Mutex<Vec<SessionState>>);
impl Reporter for SReporter {
    fn state(&self, s: SessionState) {
        self.0.lock().unwrap().push(s);
    }
}

struct Rig {
    log: Shared,
    cancel: Arc<watch::Sender<bool>>,
    ears: SEars,
    mouth: QMouth,
    hands: SHands,
    brain: SBrain,
    reporter: SReporter,
}

struct RigOpts {
    offer: bool,
    accept_limit: Option<usize>,
    finish_outcome: SayOutcome,
}
impl Default for RigOpts {
    fn default() -> Self {
        RigOpts {
            offer: true,
            accept_limit: None,
            finish_outcome: SayOutcome::Done,
        }
    }
}

fn rig(utterances: &[&str], turns: Vec<Turn>, o: RigOpts) -> Rig {
    let log: Shared = Arc::new(Mutex::new(vec![]));
    let (tx, _rx) = watch::channel(false);
    Rig {
        ears: SEars {
            script: Mutex::new(utterances.iter().map(|s| s.to_string()).collect()),
            log: log.clone(),
        },
        mouth: QMouth {
            log: log.clone(),
            offer: o.offer,
            opens: AtomicUsize::new(0),
            queues: Mutex::new(vec![]),
            accept_limit: o.accept_limit,
            finish_outcome: o.finish_outcome,
        },
        hands: SHands { log: log.clone() },
        brain: SBrain {
            log: log.clone(),
            script: Mutex::new(turns.into()),
            plain_calls: AtomicUsize::new(0),
            streaming_calls: AtomicUsize::new(0),
            histories: Mutex::new(vec![]),
        },
        reporter: SReporter(Mutex::new(vec![])),
        cancel: Arc::new(tx),
        log,
    }
}

fn cfg(early: bool, ack: Option<u64>) -> SessionConfig {
    SessionConfig {
        post_speech_guard: Duration::from_millis(0),
        quiet_wait: Duration::from_millis(5),
        early_speech: early,
        ack_after: ack.map(Duration::from_millis),
        ..SessionConfig::default()
    }
}

async fn run(r: &Rig, cfg: &SessionConfig) -> SessionEnd {
    let deps = Deps {
        policy: None,
        announcer: None,
        ears: &r.ears,
        mouth: &r.mouth,
        hands: &r.hands,
        brain: &r.brain,
        reporter: &r.reporter,
    };
    let token = r.cancel.subscribe();
    tokio::time::timeout(Duration::from_secs(10), run_session(&deps, cfg, token))
        .await
        .expect("run_session hung")
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

/// A turn that streams `json` in slices of `slice` chars, `gap` ms apart, then resolves with
/// `plan` after `final_delay` ms.
fn streamed(
    json: &str,
    slice: usize,
    gap: u64,
    final_delay: u64,
    p: Result<TurnPlan, PlanError>,
) -> Turn {
    let chars: Vec<char> = json.chars().collect();
    Turn {
        deltas: chars
            .chunks(slice)
            .map(|c| (gap, c.iter().collect()))
            .collect(),
        final_delay,
        plan: p,
    }
}

fn end_turn() -> Turn {
    Turn {
        deltas: vec![],
        final_delay: 0,
        plan: Ok(plan(PlanKind::End, "Goodbye.")),
    }
}

const THREE: &str = "First sentence is here. Second sentence follows now. Third one ends.";
fn three_json() -> String {
    format!(r#"{{"kind":"answer","emotion":"calm","speech":"{THREE}"}}"#)
}

// ---- tests ---------------------------------------------------------------------------------

#[tokio::test]
async fn disabled_early_speech_uses_the_legacy_say_path_and_never_opens_a_queue() {
    let r = rig(
        &["hi"],
        vec![Turn {
            deltas: vec![],
            final_delay: 0,
            plan: Ok(plan(PlanKind::Answer, "Hello there.")),
        }],
        RigOpts::default(),
    );
    let _ = run(&r, &cfg(false, None)).await;
    assert!(has(&r.log, "say:Hello there."));
    assert_eq!(
        r.mouth.opens.load(Ordering::SeqCst),
        0,
        "neither early speech nor ack => no queue"
    );
    assert_eq!(r.brain.streaming_calls.load(Ordering::SeqCst), 0);
    assert_eq!(r.brain.plain_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_mouth_without_a_queue_falls_back_to_the_legacy_path() {
    let r = rig(
        &["hi"],
        vec![Turn {
            deltas: vec![],
            final_delay: 0,
            plan: Ok(plan(PlanKind::Answer, "Hello there.")),
        }],
        RigOpts {
            offer: false,
            ..Default::default()
        },
    );
    let _ = run(&r, &cfg(true, Some(10))).await;
    assert!(has(&r.log, "say:Hello there."));
    assert_eq!(count_prefix(&r.log, "push:"), 0);
}

#[tokio::test]
async fn sentences_are_spoken_while_the_reply_streams_then_the_remainder_follows() {
    let r = rig(
        &["hi", "what about tomorrow"],
        vec![
            streamed(&three_json(), 7, 2, 40, Ok(plan(PlanKind::Answer, THREE))),
            end_turn(),
        ],
        RigOpts::default(),
    );
    let end = run(&r, &cfg(true, None)).await;
    assert_eq!(end, SessionEnd::UserEnded);
    let log = log_of(&r.log);
    let interesting: Vec<&str> = log
        .iter()
        .map(String::as_str)
        .filter(|e| {
            e.starts_with("push:") || *e == "plan_done" || *e == "finish" || e.starts_with("say:")
        })
        .collect();
    assert_eq!(
        interesting[..5],
        [
            "push:First sentence is here.",
            "push:Second sentence follows now.",
            "plan_done",
            "push:Third one ends.",
            "finish"
        ],
        "{log:?}"
    );
    assert_eq!(
        count_prefix(&r.log, "say:"),
        0,
        "no legacy say for a queued turn"
    );
    // the full spoken answer is remembered as context for the next turn
    let h = r.brain.histories.lock().unwrap().clone();
    assert_eq!(h[1].len(), 1);
    assert_eq!(h[1][0].0, "hi");
    assert_eq!(h[1][0].1, THREE);
}

#[tokio::test]
async fn the_first_sentence_is_spoken_before_the_plan_resolves() {
    let r = rig(
        &["hi"],
        vec![streamed(
            &three_json(),
            5,
            3,
            120,
            Ok(plan(PlanKind::Answer, THREE)),
        )],
        RigOpts::default(),
    );
    let _ = run(&r, &cfg(true, None)).await;
    let first = pos(&r.log, "push:First sentence is here.").unwrap();
    let done = pos(&r.log, "plan_done").unwrap();
    assert!(first < done);
}

#[tokio::test]
async fn early_sentences_carry_the_streamed_emotion_and_the_remainder_the_plans() {
    let mut p = plan(PlanKind::Answer, THREE);
    p.emotion = Emotion::Serious;
    let r = rig(
        &["hi"],
        vec![streamed(&three_json(), 9, 1, 5, Ok(p))],
        RigOpts::default(),
    );
    let _ = run(&r, &cfg(true, None)).await;
    let pushes = r.mouth.queues.lock().unwrap()[0]
        .pushes
        .lock()
        .unwrap()
        .clone();
    assert_eq!(pushes[0].1, Emotion::Calm);
    assert_eq!(pushes[1].1, Emotion::Calm);
    assert_eq!(pushes[2], ("Third one ends.".to_string(), Emotion::Serious));
}

#[tokio::test]
async fn clarify_turns_are_spoken_like_answers() {
    let json = r#"{"kind":"clarify","speech":"Which contact do you mean exactly? The one from work or home."}"#;
    let r = rig(
        &["call sam"],
        vec![streamed(
            json,
            6,
            1,
            5,
            Ok(plan(
                PlanKind::Clarify,
                "Which contact do you mean exactly? The one from work or home.",
            )),
        )],
        RigOpts::default(),
    );
    let _ = run(&r, &cfg(true, None)).await;
    assert!(has(&r.log, "push:Which contact do you mean exactly?"));
    assert!(has(&r.log, "push:The one from work or home."));
    assert_eq!(count_prefix(&r.log, "say:"), 0);
}

#[tokio::test]
async fn an_end_plan_is_spoken_through_the_queue_and_ends_the_session() {
    let r = rig(
        &["wrap up the session please"],
        vec![end_turn()],
        RigOpts::default(),
    );
    let end = run(&r, &cfg(true, None)).await;
    assert_eq!(end, SessionEnd::UserEnded);
    assert!(has(&r.log, "push:Goodbye."));
    assert!(has(&r.log, "finish"));
    assert_eq!(count_prefix(&r.log, "say:"), 0);
}

#[tokio::test]
async fn a_final_plan_that_contradicts_early_speech_aborts_and_executes_nothing() {
    // streams an "answer" sentence, then the final plan turns out to be a command
    let cmd = TurnPlan {
        job: None,
        role: None,
        command: Some("delete everything".into()),
        ..plan(PlanKind::Command, "Opening.")
    };
    let r = rig(
        &["hi", "what about tomorrow"],
        vec![streamed(&three_json(), 8, 1, 5, Ok(cmd)), end_turn()],
        RigOpts::default(),
    );
    let _ = run(&r, &cfg(true, None)).await;
    assert!(has(&r.log, "abort"));
    assert_eq!(count_prefix(&r.log, "model:"), 0, "no command may run");
    assert!(has(&r.log, &format!("say:{MSG_PLAN_FAILED}")));
    let h = r.brain.histories.lock().unwrap().clone();
    // deliberate spec change (DESIGN Revision 3, critic C2): an aborted turn after early speech
    // records the spoken prefix with an incomplete-reply note (never a false "no reply").
    assert_eq!(h[1].len(), 1, "{:?}", h[1]);
    assert_eq!(h[1][0].0, "hi");
    assert!(
        h[1][0]
            .1
            .ends_with(&crate::managers::converse_context::note_incomplete()),
        "{:?}",
        h[1][0].1
    );
    assert!(
        !h[1][0].1.contains("\"event\":\"failed\""),
        "{:?}",
        h[1][0].1
    );
}

#[tokio::test]
async fn final_speech_that_does_not_extend_the_spoken_prefix_aborts() {
    let r = rig(
        &["hi", "what about tomorrow"],
        vec![
            streamed(
                &three_json(),
                8,
                1,
                5,
                Ok(plan(PlanKind::Answer, "Something entirely different now.")),
            ),
            end_turn(),
        ],
        RigOpts::default(),
    );
    let _ = run(&r, &cfg(true, None)).await;
    assert!(has(&r.log, "abort"));
    assert!(has(&r.log, &format!("say:{MSG_PLAN_FAILED}")));
    assert_eq!(count_prefix(&r.log, "push:Something"), 0);
}

#[tokio::test]
async fn a_plan_error_after_early_speech_stops_the_speech_and_apologizes() {
    let r = rig(
        &["hi", "what about tomorrow"],
        vec![
            streamed(&three_json(), 8, 1, 5, Err(PlanError::Backend)),
            end_turn(),
        ],
        RigOpts::default(),
    );
    let _ = run(&r, &cfg(true, None)).await;
    assert!(has(&r.log, "abort"));
    assert!(has(&r.log, &format!("say:{MSG_PLAN_FAILED}")));
}

#[tokio::test]
async fn action_plans_without_early_speech_use_the_legacy_path() {
    let paste = TurnPlan {
        job: None,
        role: None,
        paste_text: Some("hello".into()),
        ..plan(PlanKind::Paste, "")
    };
    let r = rig(
        &["write hello", "bye"],
        vec![
            Turn {
                deltas: vec![(1, r#"{"kind":"paste","paste_text":"hello"}"#.into())],
                final_delay: 1,
                plan: Ok(paste),
            },
            end_turn(),
        ],
        RigOpts::default(),
    );
    let _ = run(&r, &cfg(true, None)).await;
    assert!(has(&r.log, "paste:hello"));
    assert!(has(&r.log, &format!("say:{MSG_DONE}")));
    assert!(!has(&r.log, "abort"), "an unused queue is not an error");
}

#[tokio::test]
async fn a_refused_push_ends_the_turns_speech_without_further_pushes_or_say() {
    let r = rig(
        &["hi", "what about tomorrow"],
        vec![
            streamed(&three_json(), 8, 1, 5, Ok(plan(PlanKind::Answer, THREE))),
            end_turn(),
        ],
        RigOpts {
            accept_limit: Some(1),
            ..Default::default()
        },
    );
    let _ = run(&r, &cfg(true, None)).await;
    assert!(has(&r.log, "push:First sentence is here."));
    assert!(has(&r.log, "refused:Second sentence follows now."));
    assert_eq!(
        count_prefix(&r.log, "push:"),
        1 + 1,
        "first accepted turn-1 push plus turn 2's goodbye, nothing else: {:?}",
        log_of(&r.log)
    );
    assert_eq!(
        count_prefix(&r.log, "say:"),
        0,
        "the user took over the speaker: do not resume speaking"
    );
    assert!(
        count_prefix(&r.log, "listen") >= 2,
        "the loop keeps listening"
    );
}

#[tokio::test]
async fn an_interrupted_finish_returns_to_listening_without_a_fallback_say() {
    let r = rig(
        &["hi", "what about tomorrow"],
        vec![
            streamed(&three_json(), 8, 1, 5, Ok(plan(PlanKind::Answer, THREE))),
            end_turn(),
        ],
        RigOpts {
            finish_outcome: SayOutcome::Interrupted,
            ..Default::default()
        },
    );
    let end = run(&r, &cfg(true, None)).await;
    assert_eq!(end, SessionEnd::UserEnded);
    assert_eq!(count_prefix(&r.log, "say:"), 0);
    assert!(count_prefix(&r.log, "listen") >= 2);
}

#[tokio::test]
async fn a_slow_brain_gets_exactly_one_acknowledgement_before_the_answer() {
    let json = r#"{"kind":"answer","speech":"This is the only sentence here."}"#;
    let r = rig(
        &["hi"],
        vec![Turn {
            deltas: vec![(400, json.to_string())],
            final_delay: 0,
            plan: Ok(plan(PlanKind::Answer, "This is the only sentence here.")),
        }],
        RigOpts::default(),
    );
    let _ = run(&r, &cfg(true, Some(60))).await;
    let log = log_of(&r.log);
    let interesting: Vec<&str> = log
        .iter()
        .map(String::as_str)
        .filter(|e| e.starts_with("push:") || *e == "plan_done" || *e == "finish")
        .collect();
    assert_eq!(
        interesting[..4],
        [
            "push:One moment.",
            "plan_done",
            "push:This is the only sentence here.",
            "finish"
        ],
        "{log:?}"
    );
    assert_eq!(count_prefix(&r.log, "push:One moment."), 1);
}

#[tokio::test]
async fn a_fast_brain_gets_no_acknowledgement() {
    let r = rig(
        &["hi"],
        vec![Turn {
            deltas: vec![(
                1,
                r#"{"kind":"answer","speech":"Quick answer here."}"#.into(),
            )],
            final_delay: 5,
            plan: Ok(plan(PlanKind::Answer, "Quick answer here.")),
        }],
        RigOpts::default(),
    );
    let _ = run(&r, &cfg(true, Some(400))).await;
    assert_eq!(count_prefix(&r.log, "push:One moment."), 0);
    assert!(has(&r.log, "push:Quick answer here."));
}

#[tokio::test]
async fn no_acknowledgement_once_early_speech_has_started() {
    let r = rig(
        &["hi"],
        vec![streamed(
            &three_json(),
            12,
            1,
            300,
            Ok(plan(PlanKind::Answer, THREE)),
        )],
        RigOpts::default(),
    );
    let _ = run(&r, &cfg(true, Some(120))).await;
    assert_eq!(
        count_prefix(&r.log, "push:One moment."),
        0,
        "{:?}",
        log_of(&r.log)
    );
}

#[tokio::test]
async fn an_acknowledgement_never_counts_as_spoken_answer_text() {
    // ack spoken, then the plan is a plain answer: the whole answer is still spoken (the ack
    // is not part of the prefix ledger) and nothing is aborted
    let r = rig(
        &["hi"],
        vec![Turn {
            deltas: vec![(
                300,
                r#"{"kind":"answer","speech":"Here is your answer."}"#.into(),
            )],
            final_delay: 0,
            plan: Ok(plan(PlanKind::Answer, "Here is your answer.")),
        }],
        RigOpts::default(),
    );
    let _ = run(&r, &cfg(true, Some(50))).await;
    assert!(has(&r.log, "push:One moment."));
    assert!(has(&r.log, "push:Here is your answer."));
    assert!(!has(&r.log, "abort"));
}

#[tokio::test]
async fn ack_alone_works_without_early_speech_and_streams_no_sentences() {
    let r = rig(
        &["hi"],
        vec![Turn {
            deltas: vec![(300, three_json())],
            final_delay: 0,
            plan: Ok(plan(PlanKind::Answer, THREE)),
        }],
        RigOpts::default(),
    );
    let _ = run(&r, &cfg(false, Some(50))).await;
    let log = log_of(&r.log);
    let pushes: Vec<&str> = log
        .iter()
        .map(String::as_str)
        .filter(|e| e.starts_with("push:"))
        .collect();
    assert_eq!(
        pushes,
        vec!["push:One moment.", &format!("push:{THREE}")[..]],
        "{log:?}"
    );
    assert!(!has(&r.log, "abort"));
}

#[tokio::test]
async fn ack_alone_with_a_fast_brain_uses_the_legacy_say() {
    let r = rig(
        &["hi"],
        vec![Turn {
            deltas: vec![],
            final_delay: 0,
            plan: Ok(plan(PlanKind::Answer, "Quick answer here.")),
        }],
        RigOpts::default(),
    );
    let _ = run(&r, &cfg(false, Some(400))).await;
    assert_eq!(count_prefix(&r.log, "push:One moment."), 0);
    // no ack was needed: either path may speak the answer, but exactly once
    assert_eq!(
        count_prefix(&r.log, "say:Quick answer here.")
            + count_prefix(&r.log, "push:Quick answer here."),
        1
    );
}

#[tokio::test]
async fn cancellation_during_streaming_ends_the_session_and_stops_the_mouth() {
    let r = rig(
        &["hi"],
        vec![streamed(
            &three_json(),
            4,
            20,
            2000,
            Ok(plan(PlanKind::Answer, THREE)),
        )],
        RigOpts::default(),
    );
    let tx = r.cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        tx.send_replace(true);
    });
    let end = run(&r, &cfg(true, Some(5000))).await;
    assert_eq!(end, SessionEnd::Cancelled);
    assert!(has(&r.log, "stop"));
}
