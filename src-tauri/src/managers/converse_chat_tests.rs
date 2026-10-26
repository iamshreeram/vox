//! Spec tests for the conversation-loop chat recording hooks (DESIGN-CHAT Rev 6: section 2.2,
//! A1, A2, A10, A11). Written before the implementation; red by design while the hooks are
//! missing (`Reporter::chat` is a default no-op). Included from converse.rs. Black box through
//! `run_session`, reusing the fakes of converse_agent_loop_tests.rs (`pub(super)`). Every
//! scripted scenario first checks its own precondition through the mouth log (what was really
//! spoken), so a red result can only come from the recording, never from the script.
//! Do not edit to make code pass.
#![allow(dead_code)]

use super::converse_agent_loop_tests::*;
#[allow(unused_imports)]
use super::*;
use crate::managers::chat_log::ChatRole;
use crate::managers::converse_agent::{AgentEvent, AgentPoll};
use crate::managers::converse_plan::{AgentPlan, PlanKind, PlanStep, StepKind, TurnPlan};
use crate::managers::tts::{sanitize_speech_text, MAX_SPEECH_CHARS};
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

// ---- reporters ---------------------------------------------------------------------------------

/// Records every `chat(role, text)` call, in order.
#[derive(Default)]
struct ChatRep {
    lines: Mutex<Vec<(ChatRole, String)>>,
}

impl Reporter for ChatRep {
    fn state(&self, _s: SessionState) {}
    fn chat(&self, role: ChatRole, text: &str) {
        self.lines.lock().unwrap().push((role, text.to_string()));
    }
}

/// Does not override `chat`: the default no-op must leave every outcome unchanged.
struct Quiet;

impl Reporter for Quiet {
    fn state(&self, _s: SessionState) {}
}

fn lines(rep: &ChatRep) -> Vec<(ChatRole, String)> {
    rep.lines.lock().unwrap().clone()
}

fn users(chat: &[(ChatRole, String)]) -> Vec<String> {
    chat.iter()
        .filter(|(r, _)| *r == ChatRole::User)
        .map(|(_, t)| t.clone())
        .collect()
}

fn assistants(chat: &[(ChatRole, String)]) -> Vec<String> {
    chat.iter()
        .filter(|(r, _)| *r == ChatRole::Assistant)
        .map(|(_, t)| t.clone())
        .collect()
}

/// Every line the mouth actually received (`say`) or accepted (`push`), in call order, in its
/// speech-normalised form; lines that normalise to nothing are dropped.
fn said_oracle(log: &Shared) -> Vec<String> {
    log.lock()
        .unwrap()
        .iter()
        .filter_map(|e| e.strip_prefix("say:").or_else(|| e.strip_prefix("push:")))
        .filter_map(|t| sanitize_speech_text(t))
        .collect()
}

fn spoke(log: &Shared, text: &str) -> bool {
    said_oracle(log).iter().any(|t| t == text)
}

/// `expected` appears in `chat` as a subsequence (other entries may sit between them).
fn assert_subsequence(chat: &[(ChatRole, String)], expected: &[(ChatRole, &str)]) {
    let mut next = 0;
    for (role, text) in chat {
        if next < expected.len() && expected[next].0 == *role && expected[next].1 == text {
            next += 1;
        }
    }
    assert_eq!(
        next,
        expected.len(),
        "entries missing or out of order; expected {expected:?} within {chat:?}"
    );
}

fn count_log(log: &Shared, entry: &str) -> usize {
    log.lock().unwrap().iter().filter(|e| *e == entry).count()
}

// ---- rig ---------------------------------------------------------------------------------------

/// The fakes of converse_agent_loop_tests with a reporter chosen per run.
struct CRig {
    log: Shared,
    ears: LEars,
    mouth: LMouth,
    hands: AHands,
    brain: LBrain,
    barge: Arc<FakeBargeIn>,
}

fn crig(steps: Vec<Step>, turns: Vec<Turn>) -> CRig {
    let log: Shared = Arc::new(Mutex::new(vec![]));
    let captures = Arc::new(Mutex::new(VecDeque::new()));
    let barge = Arc::new(FakeBargeIn {
        log: log.clone(),
        arms: Mutex::new(vec![]),
        captures,
    });
    CRig {
        ears: LEars {
            script: Mutex::new(steps.into()),
            log: log.clone(),
            barge: barge.clone(),
            barge_on: false,
        },
        mouth: LMouth {
            log: log.clone(),
            says: Mutex::new(VecDeque::new()),
            offer: true,
            accept: None,
            finish: SayOutcome::Done,
            last_said: Mutex::new(None),
            fire_after_said: Mutex::new(None),
            barge: barge.clone(),
        },
        hands: AHands::new(log.clone()),
        brain: LBrain {
            script: Mutex::new(turns.into()),
            seen: Mutex::new(vec![]),
            log: log.clone(),
        },
        barge,
        log,
    }
}

async fn crun(r: &CRig, cfg: &SessionConfig, reporter: &dyn Reporter) -> SessionEnd {
    let deps = Deps {
        policy: None,
        announcer: None,
        ears: &r.ears,
        mouth: &r.mouth,
        hands: &r.hands,
        brain: &r.brain,
        reporter,
    };
    let (tx, rx) = watch::channel(false);
    let end = tokio::time::timeout(Duration::from_secs(10), run_session(&deps, cfg, rx))
        .await
        .expect("run_session hung");
    drop(tx);
    end
}

/// Session settings: the hand-off is on, no speech guard, and a generous idle budget.
fn ccfg() -> SessionConfig {
    SessionConfig {
        post_speech_guard: Duration::from_millis(0),
        quiet_wait: Duration::from_millis(5),
        agent_handoff: true,
        max_idle_turns: 5,
        ..SessionConfig::default()
    }
}

/// Early speech on: the turn's speech goes through the queue (`open_queue`).
fn early_cfg() -> SessionConfig {
    SessionConfig {
        early_speech: true,
        ..ccfg()
    }
}

/// Planner on (agent hand-off and the plan executor).
fn planner_cfg() -> SessionConfig {
    SessionConfig {
        agent_planner: true,
        ..ccfg()
    }
}

/// A brain turn that streams `deltas` (the early-speech path) and then resolves with an answer.
fn streamed(deltas: &[&str], speech: &str) -> Turn {
    Turn {
        deltas: deltas.iter().map(|d| d.to_string()).collect(),
        delay: 0,
        plan: Ok(tp(PlanKind::Answer, speech)),
    }
}

/// Queues one agent run with the given polls (the next Agent start gets it).
fn queue_ctl(r: &CRig, polls: Vec<AgentPoll>) {
    let ctl = RunCtl::new();
    for poll in polls {
        ctl.push(poll);
    }
    r.hands.starts.lock().unwrap().push_back(Ok(ctl));
}

fn done(text: &str) -> AgentPoll {
    AgentPoll::Event(AgentEvent::Done(text.to_string()))
}

fn st(id: u8, kind: StepKind, task: &str, needs: &[u8]) -> PlanStep {
    PlanStep {
        id,
        kind,
        task: task.to_string(),
        needs: needs.to_vec(),
    }
}

/// Steps 1..=n in order, each needing the one before it.
fn chain(items: &[(StepKind, &str)]) -> Vec<PlanStep> {
    items
        .iter()
        .enumerate()
        .map(|(index, (kind, task))| {
            let id = index as u8 + 1;
            let needs = if id == 1 { vec![] } else { vec![id - 1] };
            st(id, *kind, task, &needs)
        })
        .collect()
}

/// A planner call that returns a plan of `steps` announced with `speech`.
fn plan_turn(speech: &str, steps: Vec<PlanStep>) -> Turn {
    Turn {
        deltas: vec![],
        delay: 0,
        plan: Ok(TurnPlan {
            kind: PlanKind::Plan,
            speech: speech.to_string(),
            plan: Some(AgentPlan { steps }),
            ..tp(PlanKind::Answer, "")
        }),
    }
}

const CMD: &str = "cargo build --release";
const UTT: &str = "run the build";
const PROMPT: &str = "I am about to do this: cargo build --release. Say yes to go ahead.";
/// The action reply of an approved command is the plan's own speech (the hands' `Done` text is
/// not spoken for a planner command).
const ACTION_REPLY: &str = "Running it.";
const DECLINED: &str = "Okay, I won't.";
const PLAN_SPEECH: &str = "I will research this, then write it up.";
const PLAN_FINAL: &str = "The summary is saved. It lists three changes.";
const SECRET: &str = "INTERMEDIATE-SECRET-42";
const STREAM_DELTAS: [&str; 3] = ["Here is one sentence. ", "[[]] ", "Here is another [1]. "];
const STREAM_SPEECH: &str = "Here is one sentence. [[]] Here is another [1].";

/// A session in which an agent result has arrived (so the session is tainted), then the user
/// says `UTT` (a command effect), then `replies` are heard for the forced confirmation.
fn command_taint(replies: Vec<Step>) -> CRig {
    let mut steps = vec![heard(REQ), heard("thanks"), heard(UTT)];
    steps.extend(replies);
    steps.push(heard("hello there"));
    let turns = vec![
        agent_turn(TASK, ACK_TEXT),
        answer("You're welcome."),
        ok(cmd_plan(CMD, "Running it.")),
        answer("Hi there."),
    ];
    let r = crig(steps, turns);
    queue_ctl(&r, vec![done(RESULT)]);
    r.hands
        .model
        .lock()
        .unwrap()
        .push_back(PhraseOutcome::Done("Ran the build.".to_string()));
    r
}

// ---- 1. normal turn ----------------------------------------------------------------------------

#[tokio::test]
async fn normal_turn_records_user_then_normalised_assistant_each_once() {
    let r = crig(
        vec![heard("what is rust"), outcome(ListenOutcome::Aborted)],
        vec![answer("Rust is a language.\n\nIt   is fast [1].")],
    );
    let rep = ChatRep::default();
    crun(&r, &ccfg(), &rep).await;
    assert_eq!(
        said_oracle(&r.log),
        vec!["Rust is a language. It is fast 1 .".to_string()],
        "precondition: the reply was spoken"
    );
    assert_eq!(
        lines(&rep),
        vec![
            (ChatRole::User, "what is rust".to_string()),
            (
                ChatRole::Assistant,
                "Rust is a language. It is fast 1 .".to_string()
            ),
        ]
    );
}

// ---- 2. early-speech queue ---------------------------------------------------------------------

/// Every accepted push is recorded once, in push order, normalised; the unspeakable `[[]]`
/// push is not recorded; the chat equals what the mouth actually received.
#[tokio::test]
async fn early_speech_pushes_are_recorded_once_normalised_in_order() {
    let r = crig(
        vec![heard("explain it"), outcome(ListenOutcome::Aborted)],
        vec![streamed(&STREAM_DELTAS, STREAM_SPEECH)],
    );
    let rep = ChatRep::default();
    crun(&r, &early_cfg(), &rep).await;
    assert!(
        r.log.lock().unwrap().iter().any(|e| e.starts_with("push:")),
        "precondition: the early-speech queue was used: {:?}",
        r.log.lock().unwrap()
    );
    let chat = assistants(&lines(&rep));
    assert_eq!(
        chat,
        said_oracle(&r.log),
        "assistant lines == accepted pushes, normalised"
    );
    assert!(
        chat.iter()
            .all(|t| !t.is_empty() && !t.contains('[') && !t.contains(']')),
        "no empty or bracket text recorded: {chat:?}"
    );
}

/// A push the queue REJECTS is not recorded (the chat still equals what was accepted).
#[tokio::test]
async fn rejected_queue_push_is_not_recorded() {
    let mut r = crig(
        vec![heard("explain it"), outcome(ListenOutcome::Aborted)],
        vec![streamed(&STREAM_DELTAS, STREAM_SPEECH)],
    );
    r.mouth.accept = Some(1);
    let rep = ChatRep::default();
    crun(&r, &early_cfg(), &rep).await;
    let oracle = said_oracle(&r.log);
    assert!(!oracle.is_empty(), "precondition: one push was accepted");
    assert_eq!(
        assistants(&lines(&rep)),
        oracle,
        "rejected pushes never reach the chat"
    );
}

/// Abort and finish of the queue are unchanged by the hook: the same scenario with the
/// default no-op reporter produces the same mouth/queue log and the same end.
#[tokio::test]
async fn queue_abort_and_finish_are_unchanged_by_the_hook() {
    let a = crig(
        vec![heard("explain it"), outcome(ListenOutcome::Aborted)],
        vec![streamed(&STREAM_DELTAS, STREAM_SPEECH)],
    );
    let b = crig(
        vec![heard("explain it"), outcome(ListenOutcome::Aborted)],
        vec![streamed(&STREAM_DELTAS, STREAM_SPEECH)],
    );
    let end_a = crun(&a, &early_cfg(), &ChatRep::default()).await;
    let end_b = crun(&b, &early_cfg(), &Quiet).await;
    assert_eq!(end_a, end_b, "same end");
    assert_eq!(
        a.log.lock().unwrap().clone(),
        b.log.lock().unwrap().clone(),
        "same queue and mouth calls with and without the hook"
    );
}

// ---- 3. local intent ---------------------------------------------------------------------------

/// A stop phrase is a local reply: the utterance is User, `Okay.` is Assistant, the agent
/// (brain) is never called and no user phrase runs.
#[tokio::test]
async fn local_stop_reply_is_recorded_and_never_reaches_the_brain() {
    let r = crig(vec![heard("stop")], vec![]);
    let rep = ChatRep::default();
    let end = crun(&r, &ccfg(), &rep).await;
    assert_eq!(end, SessionEnd::UserEnded);
    assert_eq!(
        said_oracle(&r.log),
        vec!["Okay.".to_string()],
        "precondition: the local reply was spoken"
    );
    assert_eq!(
        lines(&rep),
        vec![
            (ChatRole::User, "stop".to_string()),
            (ChatRole::Assistant, "Okay.".to_string()),
        ]
    );
    assert!(r.brain.seen.lock().unwrap().is_empty(), "no planner call");
    assert_eq!(count_log(&r.log, "user:stop"), 0, "no user phrase run");
}

// ---- 4. error line -----------------------------------------------------------------------------

/// A failed plan speaks its fixed error line, which is recorded as Assistant.
#[tokio::test]
async fn error_line_is_recorded_as_assistant() {
    let r = crig(
        vec![heard("what is rust"), outcome(ListenOutcome::Aborted)],
        vec![],
    );
    let rep = ChatRep::default();
    crun(&r, &ccfg(), &rep).await;
    assert_eq!(
        said_oracle(&r.log),
        vec!["Sorry, I couldn't work that out.".to_string()],
        "precondition: the error line was spoken"
    );
    assert_subsequence(
        &lines(&rep),
        &[
            (ChatRole::User, "what is rust"),
            (ChatRole::Assistant, "Sorry, I couldn't work that out."),
        ],
    );
}

// ---- 5. confirmation answers -------------------------------------------------------------------

/// Agent-tainted effect: prompt (Assistant), then the exact `yes` (User), then the action reply
/// (Assistant), in that order; the command runs exactly once.
#[tokio::test]
async fn confirmation_prompt_yes_and_action_reply_are_recorded_in_order() {
    let r = command_taint(vec![heard("yes")]);
    let rep = ChatRep::default();
    crun(&r, &ccfg(), &rep).await;
    assert!(
        spoke(&r.log, PROMPT) && spoke(&r.log, ACTION_REPLY),
        "precondition: prompt and action reply were spoken: {:?} log {:?}",
        said_oracle(&r.log),
        r.log.lock().unwrap()
    );
    let chat = lines(&rep);
    assert_subsequence(
        &chat,
        &[
            (ChatRole::User, UTT),
            (ChatRole::Assistant, PROMPT),
            (ChatRole::User, "yes"),
            (ChatRole::Assistant, ACTION_REPLY),
        ],
    );
    assert_eq!(
        users(&chat),
        vec![REQ, "thanks", UTT, "yes", "hello there"],
        "each accepted user line exactly once"
    );
    assert_eq!(count_log(&r.log, &format!("model:{CMD}")), 1, "ran once");
}

/// `yes please` is recorded once as User and runs nothing (exact-yes rule unchanged).
#[tokio::test]
async fn yes_please_is_recorded_once_and_runs_nothing() {
    let r = command_taint(vec![heard("yes please")]);
    let rep = ChatRep::default();
    crun(&r, &ccfg(), &rep).await;
    assert!(spoke(&r.log, PROMPT), "precondition: the prompt was spoken");
    let chat = lines(&rep);
    assert_subsequence(
        &chat,
        &[
            (ChatRole::Assistant, PROMPT),
            (ChatRole::User, "yes please"),
            (ChatRole::Assistant, DECLINED),
        ],
    );
    assert_eq!(
        users(&chat),
        vec![REQ, "thanks", UTT, "yes please", "hello there"]
    );
    assert_eq!(
        count_log(&r.log, &format!("model:{CMD}")),
        0,
        "nothing executed"
    );
}

/// A refusal is recorded once as User; the decline is Assistant; nothing runs.
#[tokio::test]
async fn refusal_is_recorded_once_and_runs_nothing() {
    let r = command_taint(vec![heard("no")]);
    let rep = ChatRep::default();
    crun(&r, &ccfg(), &rep).await;
    assert!(spoke(&r.log, PROMPT), "precondition: the prompt was spoken");
    let chat = lines(&rep);
    assert_subsequence(
        &chat,
        &[
            (ChatRole::Assistant, PROMPT),
            (ChatRole::User, "no"),
            (ChatRole::Assistant, DECLINED),
        ],
    );
    assert_eq!(users(&chat).iter().filter(|t| *t == "no").count(), 1);
    assert_eq!(
        count_log(&r.log, &format!("model:{CMD}")),
        0,
        "nothing executed"
    );
}

/// An answer the speaker filter rejected is NOT recorded (A11); the question is declined.
#[tokio::test]
async fn confirmation_answer_rejected_by_speaker_filter_is_not_recorded() {
    let r = command_taint(vec![outcome(ListenOutcome::Ignored)]);
    let rep = ChatRep::default();
    crun(&r, &ccfg(), &rep).await;
    assert!(spoke(&r.log, PROMPT), "precondition: the prompt was spoken");
    let chat = lines(&rep);
    assert_eq!(
        users(&chat),
        vec![REQ, "thanks", UTT, "hello there"],
        "the rejected voice left no User line"
    );
    assert!(
        assistants(&chat).iter().any(|t| t == DECLINED),
        "declined: {chat:?}"
    );
    assert_eq!(
        count_log(&r.log, &format!("model:{CMD}")),
        0,
        "nothing executed"
    );
}

/// An empty confirmation answer (no speech) is NOT recorded.
#[tokio::test]
async fn empty_confirmation_answer_is_not_recorded() {
    let r = command_taint(vec![silent()]);
    let rep = ChatRep::default();
    crun(&r, &ccfg(), &rep).await;
    assert!(spoke(&r.log, PROMPT), "precondition: the prompt was spoken");
    let chat = lines(&rep);
    assert_eq!(users(&chat), vec![REQ, "thanks", UTT, "hello there"]);
    assert_eq!(
        count_log(&r.log, &format!("model:{CMD}")),
        0,
        "nothing executed"
    );
}

// ---- 6. main turn loop filters -----------------------------------------------------------------

/// Utterances the speaker filter rejected, and empty or whitespace captures, are NOT User lines.
#[tokio::test]
async fn ignored_empty_and_noise_captures_are_not_recorded_as_user() {
    let r = crig(
        vec![
            outcome(ListenOutcome::Ignored),
            outcome(ListenOutcome::Ignored),
            silent(),
            outcome(ListenOutcome::Utterance("   ".to_string())),
            heard("what is rust"),
            outcome(ListenOutcome::Aborted),
        ],
        vec![answer("Rust is a language.")],
    );
    let rep = ChatRep::default();
    crun(&r, &ccfg(), &rep).await;
    assert_eq!(
        said_oracle(&r.log),
        vec!["Rust is a language.".to_string()],
        "precondition: the one accepted utterance was answered"
    );
    assert_eq!(
        users(&lines(&rep)),
        vec!["what is rust".to_string()],
        "only the accepted utterance is recorded"
    );
}

// ---- 7. barge-in capture -----------------------------------------------------------------------

/// The text a barge-in captured is recorded exactly once (as the next turn's User line).
#[tokio::test]
async fn barge_in_capture_is_recorded_exactly_once() {
    let mut r = crig(
        vec![heard("first question"), outcome(ListenOutcome::Aborted)],
        vec![answer("Long answer."), answer("More detail.")],
    );
    r.ears.barge_on = true;
    *r.mouth.fire_after_said.lock().unwrap() = Some("Long answer.".to_string());
    r.barge
        .captures
        .lock()
        .unwrap()
        .push_back("tell me more".to_string());
    let rep = ChatRep::default();
    crun(&r, &ccfg(), &rep).await;
    assert_eq!(
        said_oracle(&r.log),
        vec!["Long answer.".to_string(), "More detail.".to_string()],
        "precondition: the barge cut the first reply and the captured turn was answered"
    );
    assert_eq!(
        lines(&rep),
        vec![
            (ChatRole::User, "first question".to_string()),
            (ChatRole::Assistant, "Long answer.".to_string()),
            (ChatRole::User, "tell me more".to_string()),
            (ChatRole::Assistant, "More detail.".to_string()),
        ]
    );
}

// ---- 8. planner --------------------------------------------------------------------------------

/// Plan announcement, step narration and the final result are Assistant lines; the intermediate
/// step result is never spoken and never appears in any chat call.
#[tokio::test]
async fn planner_lines_are_recorded_and_intermediate_results_never_are() {
    let steps = chain(&[
        (StepKind::Research, "gather the facts"),
        (StepKind::Code, "write it up"),
    ]);
    let r = crig(
        vec![
            heard("research and write it"),
            outcome(ListenOutcome::Aborted),
        ],
        vec![plan_turn(PLAN_SPEECH, steps)],
    );
    queue_ctl(&r, vec![done(&format!("{SECRET} is the first finding."))]);
    queue_ctl(&r, vec![done(PLAN_FINAL)]);
    let rep = ChatRep::default();
    crun(&r, &planner_cfg(), &rep).await;
    assert!(
        spoke(&r.log, PLAN_SPEECH) && spoke(&r.log, PLAN_FINAL),
        "precondition: announcement and final result were spoken"
    );
    let chat = lines(&rep);
    assert!(
        !chat.iter().any(|(_, t)| t.contains(SECRET)),
        "intermediate result must not be recorded: {chat:?}"
    );
    assert!(
        assistants(&chat).iter().any(|t| t == PLAN_SPEECH),
        "announcement recorded: {chat:?}"
    );
    assert!(
        assistants(&chat).iter().any(|t| t == PLAN_FINAL),
        "final result recorded: {chat:?}"
    );
    assert_eq!(
        assistants(&chat),
        said_oracle(&r.log),
        "every Assistant line is a line the mouth actually received"
    );
}

// ---- 9. agent final result ---------------------------------------------------------------------

/// An agent final result with control characters and very long text: the recorded lines equal
/// the speech-normalised spoken text, and no control character is recorded.
#[tokio::test]
async fn agent_result_with_control_characters_is_recorded_normalised() {
    let weird = format!(
        "Here is the first finding.\u{7}\u{1b}[31m red text\n\tline {}",
        "word ".repeat(700)
    );
    let r = crig(vec![heard(REQ)], vec![agent_turn(TASK, ACK_TEXT)]);
    queue_ctl(&r, vec![done(&weird)]);
    let rep = ChatRep::default();
    crun(&r, &ccfg(), &rep).await;
    assert!(
        !said_oracle(&r.log).is_empty(),
        "precondition: the agent result was spoken: {:?}",
        r.log.lock().unwrap()
    );
    let chat = assistants(&lines(&rep));
    for line in &chat {
        assert!(
            !line.chars().any(|c| c.is_control()),
            "no control characters recorded: {line:?}"
        );
        assert!(line.chars().count() <= MAX_SPEECH_CHARS, "bounded length");
    }
    assert_eq!(
        chat,
        said_oracle(&r.log),
        "recorded == sanitize_speech_text(spoken)"
    );
}

// ---- 10. cancel --------------------------------------------------------------------------------

/// The cancel phrase: User is recorded, then the acknowledgement as Assistant.
#[tokio::test]
async fn cancel_phrase_records_the_user_then_the_acknowledgement() {
    let r = crig(
        vec![heard("cancel the job"), outcome(ListenOutcome::Aborted)],
        vec![],
    );
    r.hands.active_job.store(true, Ordering::SeqCst);
    let rep = ChatRep::default();
    crun(&r, &ccfg(), &rep).await;
    assert_eq!(
        said_oracle(&r.log),
        vec!["Cancelled the coding job.".to_string()],
        "precondition: the cancel acknowledgement was spoken"
    );
    assert_eq!(
        lines(&rep),
        vec![
            (ChatRole::User, "cancel the job".to_string()),
            (ChatRole::Assistant, "Cancelled the coding job.".to_string()),
        ]
    );
}

// ---- 11. job announcer (source contract: the announcer has its own speech path) ---------------

/// job_announcer.rs speech is not part of the conversation: it never records chat lines.
#[test]
fn job_announcer_source_never_records_chat_lines() {
    let src = include_str!("job_announcer.rs");
    for token in [".chat(", "ChatTap", "chat_tap", "ChatRole", "ChatLog"] {
        assert!(
            !src.contains(token),
            "job_announcer.rs must not reference `{token}`"
        );
    }
}

// ---- 12. default no-op reporter leaves outcomes unchanged --------------------------------------

/// Running the same normal turn with a Reporter that keeps the default `chat` gives the same
/// mouth/brain/hands log and the same end as the recording reporter.
#[tokio::test]
async fn default_noop_chat_leaves_every_outcome_unchanged() {
    let make = || {
        crig(
            vec![heard("what is rust"), outcome(ListenOutcome::Aborted)],
            vec![answer("Rust is a language.")],
        )
    };
    let a = make();
    let b = make();
    let end_a = crun(&a, &ccfg(), &ChatRep::default()).await;
    let end_b = crun(&b, &ccfg(), &Quiet).await;
    assert_eq!(end_a, end_b);
    assert_eq!(
        a.log.lock().unwrap().clone(),
        b.log.lock().unwrap().clone(),
        "identical call log with the default no-op"
    );
}

// ---- 13. acknowledgement push site (`push_ack`) -------------------------------------------------

/// A brain turn that stays silent for `delay` ms and then answers, so the acknowledgement timer
/// (`SessionConfig::ack_after`) fires first.
fn slow_answer(delay: u64, speech: &str) -> Turn {
    Turn {
        deltas: vec![],
        delay,
        plan: Ok(tp(PlanKind::Answer, speech)),
    }
}

/// The acknowledgement timer fires, `push_ack` queues the acknowledgement, and it is recorded
/// exactly once as Assistant, between the user line and the final answer.
#[tokio::test]
async fn acknowledgement_push_is_recorded_once_between_user_and_answer() {
    let r = crig(
        vec![heard("explain it"), outcome(ListenOutcome::Aborted)],
        vec![slow_answer(300, "Rust is a language.")],
    );
    let rep = ChatRep::default();
    let cfg = SessionConfig {
        ack_after: Some(Duration::from_millis(10)),
        ..ccfg()
    };
    crun(&r, &cfg, &rep).await;
    assert_eq!(
        said_oracle(&r.log),
        vec![MSG_ACK.to_string(), "Rust is a language.".to_string()],
        "precondition: the acknowledgement was pushed before the answer"
    );
    assert_eq!(
        lines(&rep),
        vec![
            (ChatRole::User, "explain it".to_string()),
            (ChatRole::Assistant, MSG_ACK.to_string()),
            (ChatRole::Assistant, "Rust is a language.".to_string()),
        ]
    );
}

/// The queue REJECTS the acknowledgement push: the turn goes silent and nothing is recorded as
/// Assistant. Control: the same turn with the timer disabled (answer arrives first) is spoken
/// and recorded, so the silence is caused by the rejected acknowledgement push.
#[tokio::test]
async fn rejected_acknowledgement_push_is_not_recorded() {
    let control = crig(
        vec![heard("explain it"), outcome(ListenOutcome::Aborted)],
        vec![slow_answer(0, "Rust is a language.")],
    );
    let control_rep = ChatRep::default();
    let cfg = SessionConfig {
        ack_after: Some(Duration::from_secs(5)),
        ..ccfg()
    };
    crun(&control, &cfg, &control_rep).await;
    assert_eq!(
        said_oracle(&control.log),
        vec!["Rust is a language.".to_string()],
        "precondition (control): the answer was spoken without the timer"
    );

    let mut r = crig(
        vec![heard("explain it"), outcome(ListenOutcome::Aborted)],
        vec![slow_answer(300, "Rust is a language.")],
    );
    r.mouth.accept = Some(0);
    let rep = ChatRep::default();
    let cfg = SessionConfig {
        ack_after: Some(Duration::from_millis(10)),
        ..ccfg()
    };
    crun(&r, &cfg, &rep).await;
    assert!(
        count_log(&r.log, "open") >= 1,
        "precondition: the early-speech queue was opened"
    );
    assert!(
        said_oracle(&r.log).is_empty(),
        "precondition: the rejected push was never accepted: {:?}",
        r.log.lock().unwrap()
    );
    assert!(
        assistants(&lines(&rep)).is_empty(),
        "a rejected acknowledgement push is never recorded: {:?}",
        lines(&rep)
    );
    assert_eq!(
        users(&lines(&rep)),
        vec!["explain it".to_string()],
        "the user line is still recorded"
    );
}
