//! Loop-level context history tests (written by WP-looptests).
//! Authoritative spec for what the session loop records in its history (DESIGN Rev 3, "Design A").
//! Included from converse.rs. Do not edit to make code pass.
#![allow(dead_code)]

#[allow(unused_imports)]
use super::*;
use crate::managers::converse_context::{
    compose, neutralize, note_command, note_delivery_failed, note_failed, note_incomplete,
    note_interrupted, note_paste, note_paste_failed, CommandNote, CLOSE, OPEN,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::watch;

type Shared = Arc<Mutex<Vec<String>>>;

fn log_of(l: &Shared) -> Vec<String> {
    l.lock().unwrap().clone()
}

// ---- fakes -------------------------------------------------------------------------------

struct LQueue {
    log: Shared,
    pushes: Mutex<Vec<(String, Emotion)>>,
    accept: Option<usize>,
    finish: SayOutcome,
}
impl SpeechQueue for LQueue {
    fn push(&self, text: &str, emotion: Emotion) -> bool {
        let mut p = self.pushes.lock().unwrap();
        if self.accept.is_some_and(|n| p.len() >= n) {
            self.log.lock().unwrap().push(format!("refused:{text}"));
            return false;
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
        let o = self.finish;
        Box::pin(async move { o })
    }
}

struct LMouth {
    log: Shared,
    /// Scripted outcomes for `say` (one per call); empty => `Done`.
    says: Mutex<VecDeque<SayOutcome>>,
    /// Whether `open_queue` offers a queue (false => legacy mouth).
    offer: bool,
    /// Accepted pushes per queue before refusing (None = accept all).
    accept: Option<usize>,
    /// What every queue's `finish()` resolves to.
    finish: SayOutcome,
    queues: Mutex<Vec<Arc<LQueue>>>,
}
impl Mouth for LMouth {
    fn say<'a>(&'a self, text: &'a str, _e: Emotion) -> BoxFuture<'a, SayOutcome> {
        self.log.lock().unwrap().push(format!("say:{text}"));
        let o = self
            .says
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(SayOutcome::Done);
        Box::pin(async move { o })
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
        self.log.lock().unwrap().push("open".into());
        let q = Arc::new(LQueue {
            log: self.log.clone(),
            pushes: Mutex::new(vec![]),
            accept: self.accept,
            finish: self.finish,
        });
        self.queues.lock().unwrap().push(q.clone());
        Some(q)
    }
}

struct LHands {
    log: Shared,
    /// One result per paste call; empty => Ok.
    paste: Mutex<VecDeque<Result<(), String>>>,
    /// One outcome per exact-tier call; empty => NoMatch.
    user: Mutex<VecDeque<PhraseOutcome>>,
    /// One outcome per model-command call; empty => NoMatch.
    model: Mutex<VecDeque<PhraseOutcome>>,
}
impl Hands for LHands {
    fn paste_generated(&self, text: &str) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("paste:{text}"));
        self.paste.lock().unwrap().pop_front().unwrap_or(Ok(()))
    }
    fn run_user_phrase<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        self.log.lock().unwrap().push(format!("user:{phrase}"));
        let o = self
            .user
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(PhraseOutcome::NoMatch);
        Box::pin(async move { o })
    }
    fn run_model_command<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        self.log.lock().unwrap().push(format!("model:{phrase}"));
        let o = self
            .model
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(PhraseOutcome::NoMatch);
        Box::pin(async move { o })
    }
}

struct LEars {
    script: Mutex<VecDeque<String>>,
    log: Shared,
}
impl Ears for LEars {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        self.log.lock().unwrap().push("listen".into());
        let next = self.script.lock().unwrap().pop_front();
        Box::pin(async move {
            match next {
                Some(u) => ListenOutcome::Utterance(u),
                None => ListenOutcome::Aborted,
            }
        })
    }
}

/// One scripted brain call: streamed deltas (for the streaming path), then the final plan.
struct Turn {
    /// (delay before delivering, raw delta)
    deltas: Vec<(u64, String)>,
    /// Delay after the last delta before the plan resolves.
    final_delay: u64,
    plan: Result<TurnPlan, PlanError>,
}

struct LBrain {
    log: Shared,
    script: Mutex<VecDeque<Turn>>,
    /// The history every brain call SAW, in call order.
    histories: Mutex<Vec<Vec<(String, String)>>>,
    /// The `last_draft` every brain call SAW, in call order.
    drafts: Mutex<Vec<Option<String>>>,
}
impl LBrain {
    fn record(&self, input: &PlanInput<'_>) {
        self.histories.lock().unwrap().push(input.history.to_vec());
        self.drafts
            .lock()
            .unwrap()
            .push(input.last_draft.map(str::to_string));
    }
    fn next(&self) -> Option<Turn> {
        self.script.lock().unwrap().pop_front()
    }
}
impl Brain for LBrain {
    fn plan<'a>(&'a self, input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        self.record(&input);
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
        self.record(&input);
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

struct LReporter(Mutex<Vec<SessionState>>);
impl Reporter for LReporter {
    fn state(&self, s: SessionState) {
        self.0.lock().unwrap().push(s);
    }
}

struct Rig {
    log: Shared,
    cancel: Arc<watch::Sender<bool>>,
    ears: LEars,
    mouth: LMouth,
    hands: LHands,
    brain: LBrain,
    reporter: LReporter,
}

/// A rig whose ears hear `utts` in order (then Aborted), whose brain follows `turns`, and whose
/// mouth offers a queue. Tests tweak `r.mouth` / scripts before running (rig is owned).
fn rig(utts: &[&str], turns: Vec<Turn>) -> Rig {
    let log: Shared = Arc::new(Mutex::new(vec![]));
    let (tx, _rx) = watch::channel(false);
    Rig {
        ears: LEars {
            script: Mutex::new(utts.iter().map(|s| s.to_string()).collect()),
            log: log.clone(),
        },
        mouth: LMouth {
            log: log.clone(),
            says: Mutex::new(VecDeque::new()),
            offer: true,
            accept: None,
            finish: SayOutcome::Done,
            queues: Mutex::new(vec![]),
        },
        hands: LHands {
            log: log.clone(),
            paste: Mutex::new(VecDeque::new()),
            user: Mutex::new(VecDeque::new()),
            model: Mutex::new(VecDeque::new()),
        },
        brain: LBrain {
            log: log.clone(),
            script: Mutex::new(turns.into()),
            histories: Mutex::new(vec![]),
            drafts: Mutex::new(vec![]),
        },
        reporter: LReporter(Mutex::new(vec![])),
        cancel: Arc::new(tx),
        log,
    }
}

fn lcfg(early: bool, ack: Option<u64>) -> SessionConfig {
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

fn tp(kind: PlanKind, speech: &str) -> TurnPlan {
    TurnPlan {
        plan: None,
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

fn paste_plan(text: &str, speech: &str) -> TurnPlan {
    TurnPlan {
        job: None,
        role: None,
        paste_text: Some(text.to_string()),
        ..tp(PlanKind::Paste, speech)
    }
}

fn cmd_plan(command: &str, speech: &str) -> TurnPlan {
    TurnPlan {
        job: None,
        role: None,
        command: Some(command.to_string()),
        ..tp(PlanKind::Command, speech)
    }
}

/// A brain call with no streamed deltas resolving to `p`.
fn ok(p: TurnPlan) -> Turn {
    Turn {
        deltas: vec![],
        final_delay: 0,
        plan: Ok(p),
    }
}

/// A brain call with no streamed deltas failing with `e`.
fn fail(e: PlanError) -> Turn {
    Turn {
        deltas: vec![],
        final_delay: 0,
        plan: Err(e),
    }
}

/// Streams `json` in slices of `slice` chars, `gap` ms apart, then resolves after `final_delay`.
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

/// The closing turn every test uses so the next brain call's history is observable.
fn end_turn() -> Turn {
    ok(tp(PlanKind::End, "Goodbye."))
}

const THREE: &str = "First sentence is here. Second sentence follows now. Third one ends.";

fn three_json() -> String {
    format!(r#"{{"kind":"answer","emotion":"calm","speech":"{THREE}"}}"#)
}

/// The user side of a history entry for utterance `u`.
fn user(u: &str) -> String {
    neutralize(u)
}

/// A history entry `(neutralize(utterance), assistant_side)`.
fn entry(u: &str, assistant: String) -> (String, String) {
    (user(u), assistant)
}

/// The history the brain saw on call `n` (0-based).
fn seen(r: &Rig, n: usize) -> Vec<(String, String)> {
    r.brain.histories.lock().unwrap()[n].clone()
}

/// The last_draft the brain saw on call `n` (0-based).
fn draft_seen(r: &Rig, n: usize) -> Option<String> {
    r.brain.drafts.lock().unwrap()[n].clone()
}

/// Texts spoken through `Mouth::say`, in order.
fn says(r: &Rig) -> Vec<String> {
    log_of(&r.log)
        .into_iter()
        .filter_map(|e| e.strip_prefix("say:").map(str::to_string))
        .collect()
}

/// Texts pushed to the first opened queue, in order.
fn first_queue_pushes(r: &Rig) -> Vec<String> {
    let queues = r.mouth.queues.lock().unwrap();
    let q = &queues[0];
    let pushes = q.pushes.lock().unwrap();
    pushes.iter().map(|(t, _)| t.clone()).collect()
}

/// Number of `model:` (restricted-tier command) calls made to hands.
fn count_prefix_model(r: &Rig) -> usize {
    log_of(&r.log)
        .iter()
        .filter(|e| e.starts_with("model:"))
        .count()
}

// ---- tests: paste and command effects ----

#[tokio::test]
async fn cl1_paste_ok_records_the_pasted_note_with_the_speech() {
    let r = rig(
        &["draft the team note", "anything else"],
        vec![
            ok(paste_plan("Hi team, the release is delayed.", "Done.")),
            end_turn(),
        ],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        seen(&r, 1)[0],
        entry(
            "draft the team note",
            compose("Done.", &note_paste("Hi team, the release is delayed.")),
        )
    );
}

#[tokio::test]
async fn cl2_paste_ok_with_empty_speech_records_done_as_the_text_part() {
    let r = rig(
        &["draft the team note", "anything else"],
        vec![
            ok(paste_plan("Hi team, the release is delayed.", "")),
            end_turn(),
        ],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        seen(&r, 1)[0],
        entry(
            "draft the team note",
            compose("Done.", &note_paste("Hi team, the release is delayed.")),
        )
    );
}

#[tokio::test]
async fn cl3_failed_paste_records_paste_failed_and_keeps_the_previous_draft() {
    let r = rig(
        &["draft the team note", "try pasting again", "anything else"],
        vec![
            ok(paste_plan("Draft A text.", "Done.")),
            ok(paste_plan("Draft B text.", "Pasting.")),
            end_turn(),
        ],
    );
    r.hands
        .paste
        .lock()
        .unwrap()
        .extend([Ok(()), Err("no window".to_string()), Ok(())]);
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        seen(&r, 2)[1],
        entry("try pasting again", compose("", &note_paste_failed())),
    );
    assert_eq!(
        draft_seen(&r, 2).as_deref(),
        Some("Draft A text."),
        "a failed paste must not replace the last successful draft"
    );
}

#[tokio::test]
async fn cl4_paste_ok_is_recorded_even_when_the_confirmation_speech_fails() {
    let r = rig(
        &["draft the team note", "anything else"],
        vec![
            ok(paste_plan(
                "Hi team, the release is delayed.",
                "Pasting now.",
            )),
            end_turn(),
        ],
    );
    r.mouth.says.lock().unwrap().push_back(SayOutcome::Failed);
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        seen(&r, 1)[0],
        entry(
            "draft the team note",
            compose(
                "Pasting now.",
                &note_paste("Hi team, the release is delayed.")
            ),
        )
    );
}

#[tokio::test]
async fn cl5_command_done_records_the_command_and_its_result_with_the_speech() {
    let r = rig(
        &["open safari for me", "anything else"],
        vec![ok(cmd_plan("open safari", "Opening it.")), end_turn()],
    );
    r.hands
        .model
        .lock()
        .unwrap()
        .push_back(PhraseOutcome::Done("Opened Safari".into()));
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        seen(&r, 1)[0],
        entry(
            "open safari for me",
            compose(
                "Opening it.",
                &note_command("open safari", &CommandNote::Done("Opened Safari".into())),
            ),
        )
    );
}

#[tokio::test]
async fn cl6_command_done_with_empty_speech_records_the_note_with_an_empty_text_part() {
    let r = rig(
        &["open safari for me", "anything else"],
        vec![ok(cmd_plan("open safari", "")), end_turn()],
    );
    r.hands
        .model
        .lock()
        .unwrap()
        .push_back(PhraseOutcome::Done("Opened Safari".into()));
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        seen(&r, 1)[0],
        entry(
            "open safari for me",
            compose(
                "",
                &note_command("open safari", &CommandNote::Done("Opened Safari".into())),
            ),
        )
    );
}

#[tokio::test]
async fn cl7_command_no_match_refused_or_failed_speaks_the_fixed_message_and_records_the_note() {
    let cases = [
        (
            PhraseOutcome::NoMatch,
            "I can't do that yet.",
            CommandNote::NoMatch,
        ),
        (
            PhraseOutcome::Refused,
            "I can't do that.",
            CommandNote::Refused,
        ),
        (
            PhraseOutcome::Failed,
            "That didn't work.",
            CommandNote::Failed,
        ),
    ];
    for (outcome, spoken, note) in cases {
        let r = rig(
            &["do the thing", "anything else"],
            vec![ok(cmd_plan("open broken", "Working on it.")), end_turn()],
        );
        r.hands.model.lock().unwrap().push_back(outcome.clone());
        let _ = run(&r, &lcfg(false, None)).await;
        assert_eq!(says(&r)[0], spoken, "fixed message for {outcome:?}");
        assert_eq!(
            seen(&r, 1)[0],
            entry(
                "do the thing",
                compose("", &note_command("open broken", &note))
            ),
            "note for {outcome:?}"
        );
    }
}

#[tokio::test]
async fn cl8_command_done_is_recorded_even_when_the_confirmation_speech_fails() {
    let r = rig(
        &["open safari for me", "anything else"],
        vec![ok(cmd_plan("open safari", "Opening it.")), end_turn()],
    );
    r.hands
        .model
        .lock()
        .unwrap()
        .push_back(PhraseOutcome::Done("Opened Safari".into()));
    r.mouth.says.lock().unwrap().push_back(SayOutcome::Failed);
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        seen(&r, 1)[0],
        entry(
            "open safari for me",
            compose(
                "Opening it.",
                &note_command("open safari", &CommandNote::Done("Opened Safari".into())),
            ),
        )
    );
}

#[tokio::test]
async fn cl9_answer_said_ok_records_the_neutralized_speech_with_no_note() {
    let speech = "Line one\nline \u{27E6}two\u{27E7} here.";
    let r = rig(
        &["what is it", "anything else"],
        vec![ok(tp(PlanKind::Answer, speech)), end_turn()],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    let summary = seen(&r, 1)[0].1.clone();
    assert_eq!(summary, neutralize(speech));
    assert!(!summary.contains(OPEN) && !summary.contains(CLOSE));
    assert!(!summary.contains('\n'));
    assert!(
        !summary.contains("note"),
        "no note on a delivered answer: {summary:?}"
    );
}

#[tokio::test]
async fn cl10_answer_interrupted_on_the_legacy_say_records_the_interrupted_note() {
    let r = rig(
        &["what is it", "anything else"],
        vec![ok(tp(PlanKind::Answer, "It is a tool.")), end_turn()],
    );
    r.mouth
        .says
        .lock()
        .unwrap()
        .push_back(SayOutcome::Interrupted);
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        seen(&r, 1)[0],
        entry("what is it", compose("It is a tool.", &note_interrupted())),
    );
}

#[tokio::test]
async fn cl11_answer_whose_legacy_say_failed_records_the_delivery_failed_note() {
    let r = rig(
        &["what is it", "anything else"],
        vec![ok(tp(PlanKind::Answer, "It is a tool.")), end_turn()],
    );
    r.mouth.says.lock().unwrap().push_back(SayOutcome::Failed);
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        seen(&r, 1)[0],
        entry(
            "what is it",
            compose("It is a tool.", &note_delivery_failed())
        ),
    );
}

#[tokio::test]
async fn cl12_streamed_finish_interrupted_or_failed_records_the_matching_note() {
    for (outcome, note) in [
        (SayOutcome::Interrupted, note_interrupted()),
        (SayOutcome::Failed, note_delivery_failed()),
    ] {
        let r = rig(
            &["tell me about it", "anything else"],
            vec![
                streamed(&three_json(), 8, 1, 5, Ok(tp(PlanKind::Answer, THREE))),
                end_turn(),
            ],
        );
        let mut r = r;
        r.mouth.finish = outcome;
        let _ = run(&r, &lcfg(true, None)).await;
        assert_eq!(
            seen(&r, 1)[0],
            entry("tell me about it", compose(THREE, &note)),
            "finish {outcome:?}"
        );
    }
}

#[tokio::test]
async fn cl13_plan_error_before_anything_was_spoken_records_the_failed_note() {
    for e in [PlanError::Backend, PlanError::Unparseable] {
        let r = rig(&["what is it", "anything else"], vec![fail(e), end_turn()]);
        let _ = run(&r, &lcfg(false, None)).await;
        assert_eq!(
            seen(&r, 1)[0],
            entry("what is it", compose("", &note_failed())),
            "plan error {e:?}"
        );
    }
}

#[tokio::test]
async fn cl14_streamed_error_after_a_queued_sentence_records_the_prefix_and_incomplete_note() {
    // The stream completes exactly one sentence ("First sentence is here."); "Second" is a
    // partial sentence that is never completed before the brain fails.
    let partial = r#"{"kind":"answer","speech":"First sentence is here. Second"#;
    let r = rig(
        &["tell me about it", "anything else"],
        vec![
            streamed(partial, 8, 1, 5, Err(PlanError::Backend)),
            end_turn(),
        ],
    );
    let _ = run(&r, &lcfg(true, None)).await;
    let pushed = first_queue_pushes(&r);
    assert!(
        !pushed.is_empty(),
        "at least one sentence must have been queued"
    );
    let prefix = pushed.join(" ");
    assert_eq!(
        seen(&r, 1)[0],
        entry("tell me about it", compose(&prefix, &note_incomplete())),
    );
}

#[tokio::test]
async fn cl15_reconcile_abort_after_early_speech_records_the_prefix_and_executes_nothing() {
    // The stream speaks sentences as an answer, then the final plan is a command: Abort.
    let cmd = TurnPlan {
        job: None,
        role: None,
        command: Some("delete everything".into()),
        ..tp(PlanKind::Command, "Opening.")
    };
    let r = rig(
        &["tell me about it", "anything else"],
        vec![streamed(&three_json(), 8, 1, 5, Ok(cmd)), end_turn()],
    );
    let _ = run(&r, &lcfg(true, None)).await;
    assert_eq!(count_prefix_model(&r), 0, "no command may run");
    let pushed = first_queue_pushes(&r);
    assert!(!pushed.is_empty(), "early speech must have happened");
    assert_eq!(
        seen(&r, 1)[0],
        entry(
            "tell me about it",
            compose(&pushed.join(" "), &note_incomplete())
        ),
    );
}

#[tokio::test]
async fn cl16a_a_superseded_plan_records_nothing() {
    let r = rig(
        &["first try", "second try"],
        vec![
            fail(PlanError::Superseded),
            ok(tp(PlanKind::Answer, "Second.")),
        ],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert!(
        seen(&r, 1).is_empty(),
        "superseded turn must not be in history: {:?}",
        seen(&r, 1)
    );
}

#[tokio::test]
async fn cl16b_a_dead_queue_takeover_records_nothing_for_that_turn() {
    let mut r = rig(
        &["first try", "second try"],
        vec![
            streamed(&three_json(), 8, 1, 5, Ok(tp(PlanKind::Answer, THREE))),
            end_turn(),
        ],
    );
    r.mouth.accept = Some(0); // every push refused: the user took the speaker
    let _ = run(&r, &lcfg(true, None)).await;
    assert!(
        seen(&r, 1).is_empty(),
        "a turn taken over by the user must not be in history: {:?}",
        seen(&r, 1)
    );
}

#[tokio::test]
async fn cl16c_not_configured_and_cancelled_end_the_session_with_no_note_spoken() {
    // These two end the session, so no later history exists to inspect: the observable part is
    // that nothing containing a runtime note is ever spoken or pushed.
    let r = rig(&["first try"], vec![fail(PlanError::NotConfigured)]);
    assert_eq!(run(&r, &lcfg(false, None)).await, SessionEnd::NotConfigured);
    assert!(says(&r).iter().all(|s| !s.contains(OPEN)));

    // streaming path: the fake honours final_delay there, so the cancel lands mid-request
    let r = rig(
        &["first try"],
        vec![streamed(
            &three_json(),
            4,
            20,
            2000,
            Ok(tp(PlanKind::Answer, THREE)),
        )],
    );
    let tx = r.cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        tx.send_replace(true);
    });
    assert_eq!(run(&r, &lcfg(true, None)).await, SessionEnd::Cancelled);
    assert!(says(&r).iter().all(|s| !s.contains(OPEN)));
    assert!(first_queue_pushes(&r).iter().all(|s| !s.contains(OPEN)));
}

#[tokio::test]
async fn cl17_repeated_failures_are_deduplicated_on_the_full_utterance() {
    let failed = compose("", &note_failed());

    // same failing utterance twice in a row => ONE entry
    let r = rig(
        &["flaky alpha", "flaky alpha", "fine"],
        vec![
            fail(PlanError::Backend),
            fail(PlanError::Backend),
            ok(tp(PlanKind::Answer, "Fine.")),
        ],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(seen(&r, 2), vec![entry("flaky alpha", failed.clone())]);

    // two 900-char failures equal for the first 700 chars, different after => TWO entries
    let base = "w".repeat(700);
    let a1 = format!("{base}{}", "a".repeat(200));
    let a2 = format!("{base}{}", "b".repeat(200));
    let r = rig(
        &[&a1, &a2, "fine"],
        vec![
            fail(PlanError::Backend),
            fail(PlanError::Backend),
            ok(tp(PlanKind::Answer, "Fine.")),
        ],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        seen(&r, 2).len(),
        2,
        "distinct long failures must both be recorded"
    );

    // fail A, succeed B, fail A again => entries A, B, A
    let r = rig(
        &["alpha", "bravo", "alpha", "charlie"],
        vec![
            fail(PlanError::Backend),
            ok(tp(PlanKind::Answer, "Bee.")),
            fail(PlanError::Backend),
            ok(tp(PlanKind::Answer, "Sea.")),
        ],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        seen(&r, 3),
        vec![
            entry("alpha", failed.clone()),
            entry("bravo", "Bee.".to_string()),
            entry("alpha", failed.clone()),
        ]
    );
}

#[tokio::test]
async fn cl18_local_dictate_records_nothing() {
    let r = rig(
        &["write this down hello there", "what is it"],
        vec![ok(tp(PlanKind::Answer, "It is a tool."))],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    // the dictation never reaches the brain; the next brain call sees an empty history
    assert_eq!(r.brain.histories.lock().unwrap().len(), 1);
    assert!(seen(&r, 0).is_empty(), "{:?}", seen(&r, 0));
    assert_eq!(count_prefix_model(&r), 0);
}

#[tokio::test]
async fn cl18b_local_end_records_nothing_and_never_reaches_the_brain() {
    // "bye" is a local end phrase: the session ends without a brain call and without a note
    let r = rig(&["bye"], vec![ok(tp(PlanKind::Answer, "Unused."))]);
    assert_eq!(run(&r, &lcfg(false, None)).await, SessionEnd::UserEnded);
    assert!(r.brain.histories.lock().unwrap().is_empty());
    assert!(says(&r).iter().all(|s| !s.contains(OPEN)));
}

#[tokio::test]
async fn cl19_capped_answer_and_paste_keep_a_whole_note_within_the_side_limit() {
    let long_speech: String = "s".repeat(1000);
    let long_text: String = "p".repeat(300);
    let r = rig(
        &["explain everything", "now paste it", "anything else"],
        vec![
            ok(tp(PlanKind::Answer, &long_speech)),
            ok(paste_plan(&long_text, "Pasting.")),
            end_turn(),
        ],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    let h = seen(&r, 2);
    assert_eq!(h.len(), 2);
    for (_, side) in &h {
        assert!(
            side.chars().count() <= 600,
            "side is {} chars",
            side.chars().count()
        );
    }
    let paste_side = &h[1].1;
    assert_eq!(paste_side.chars().filter(|c| *c == OPEN).count(), 1);
    assert!(paste_side.ends_with(CLOSE), "{paste_side:?}");
}

#[tokio::test]
async fn cl20_history_window_drops_the_oldest_entries_and_keeps_the_rest_intact() {
    let mut cfg = lcfg(false, None);
    cfg.context_turns = 2;
    let r = rig(
        &["q1", "q2", "q3", "q4"],
        vec![
            ok(tp(PlanKind::Answer, "a1")),
            ok(tp(PlanKind::Answer, "a2")),
            ok(tp(PlanKind::Answer, "a3")),
            end_turn(),
        ],
    );
    let _ = run(&r, &cfg).await;
    assert_eq!(
        seen(&r, 3),
        vec![entry("q2", "a2".into()), entry("q3", "a3".into())],
    );
}

#[tokio::test]
async fn cl21_a_user_utterance_containing_the_open_marker_is_stored_without_it() {
    let utt = format!("what is {OPEN}this{CLOSE} thing");
    let r = rig(
        &[&utt, "anything else"],
        vec![ok(tp(PlanKind::Answer, "A thing.")), end_turn()],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    let stored = seen(&r, 1)[0].0.clone();
    assert!(
        !stored.contains(OPEN) && !stored.contains(CLOSE),
        "{stored:?}"
    );
    assert_eq!(stored, "what is this thing");
}

// ---- END ----
