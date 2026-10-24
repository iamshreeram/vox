//! Stage 1 roles and lessons: loop-level behaviour (DESIGN-PR2-STAGE1 Revision 3, sections 2.1-2.7
//! and 4). Authoritative spec, written before the loop implementation (step S3). Fakes are copied
//! from converse_context_loop_tests.rs and converse_barge_loop_tests.rs; the barge mechanism is the
//! same one: a `BargeIn` watch armed per reply, fired by the mouth, its capture delivered on fire.
//! Included from converse.rs. Do not edit to make code pass.
#![allow(dead_code)]

#[allow(unused_imports)]
use super::*;
use crate::managers::converse_barge::{BargeCapture, BargeIn, BargeWatch, ReplyGate};
use crate::managers::converse_plan::{PlanError, PlanInput, PlanKind, RoleDirective, TurnPlan};
use crate::managers::emotion::Emotion;
use std::collections::VecDeque;
use std::future::pending;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{oneshot, watch};

type Shared = Arc<Mutex<Vec<String>>>;

fn note(log: &Shared, entry: impl Into<String>) {
    log.lock().unwrap().push(entry.into());
}

// ---- barge-in fake (same mechanism as converse_barge_loop_tests.rs) ---------------------------

struct WatchCore {
    log: Shared,
    gate: Arc<ReplyGate>,
    reply: watch::Sender<bool>,
    latched: AtomicBool,
    /// False once the watch is dropped: a disarmed watch cannot act.
    active: AtomicBool,
    captures: Arc<Mutex<VecDeque<String>>>,
    capture_tx: Mutex<Option<oneshot::Sender<BargeCapture>>>,
    capture_rx: Mutex<Option<oneshot::Receiver<BargeCapture>>>,
}

impl WatchCore {
    /// The worker's sustained-speech sequence: close the reply gate, latch, deliver the capture the
    /// barge hears (`NoSpeech` when none is scripted), then set the reply token.
    fn fire(&self) -> bool {
        if !self.active.load(Ordering::SeqCst) {
            return false;
        }
        self.gate.close();
        self.latched.store(true, Ordering::SeqCst);
        let capture = match self.captures.lock().unwrap().pop_front() {
            Some(text) => BargeCapture::Utterance(text),
            None => BargeCapture::NoSpeech,
        };
        if let Some(tx) = self.capture_tx.lock().unwrap().take() {
            let _ = tx.send(capture);
        }
        self.reply.send_replace(true);
        note(&self.log, "worker-stop");
        true
    }
}

struct FakeBargeIn {
    log: Shared,
    arms: Mutex<Vec<Arc<WatchCore>>>,
    /// The utterance each fired barge hears, in order.
    captures: Arc<Mutex<VecDeque<String>>>,
}

impl FakeBargeIn {
    fn latest(&self) -> Arc<WatchCore> {
        self.arms
            .lock()
            .unwrap()
            .last()
            .expect("no watch armed yet")
            .clone()
    }
}

impl BargeIn for FakeBargeIn {
    fn arm(&self) -> Option<Box<dyn BargeWatch>> {
        let (capture_tx, capture_rx) = oneshot::channel();
        let core = Arc::new(WatchCore {
            log: self.log.clone(),
            gate: ReplyGate::new(),
            reply: watch::channel(false).0,
            latched: AtomicBool::new(false),
            active: AtomicBool::new(true),
            captures: self.captures.clone(),
            capture_tx: Mutex::new(Some(capture_tx)),
            capture_rx: Mutex::new(Some(capture_rx)),
        });
        note(
            &self.log,
            format!("arm:{}", self.arms.lock().unwrap().len() + 1),
        );
        self.arms.lock().unwrap().push(core.clone());
        Some(Box::new(FakeWatch { core }))
    }
}

struct FakeWatch {
    core: Arc<WatchCore>,
}

impl BargeWatch for FakeWatch {
    fn reply_token(&self) -> watch::Receiver<bool> {
        self.core.reply.subscribe()
    }

    fn cancel_latched(&self) -> bool {
        self.core.latched.load(Ordering::SeqCst)
    }

    fn gate(&self) -> Arc<ReplyGate> {
        self.core.gate.clone()
    }

    fn capture(&mut self) -> BoxFuture<'_, BargeCapture> {
        let rx = self.core.capture_rx.lock().unwrap().take();
        Box::pin(async move {
            match rx {
                Some(rx) => rx.await.unwrap_or(BargeCapture::NoSpeech),
                None => pending::<BargeCapture>().await,
            }
        })
    }
}

impl Drop for FakeWatch {
    fn drop(&mut self) {
        self.core.active.store(false, Ordering::SeqCst);
    }
}

// ---- ears, mouth, queue, brain, hands, reporter ------------------------------------------------

struct LEars {
    script: Mutex<VecDeque<String>>,
    log: Shared,
    barge: Arc<FakeBargeIn>,
    /// When true the loop arms a barge watch per reply (needed for latch scenarios).
    barge_on: bool,
}

impl Ears for LEars {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        note(&self.log, "listen");
        let next = self.script.lock().unwrap().pop_front();
        Box::pin(async move {
            match next {
                Some(u) => ListenOutcome::Utterance(u),
                None => ListenOutcome::Aborted,
            }
        })
    }

    fn barge_in(&self) -> Option<&dyn BargeIn> {
        let barge: &dyn BargeIn = self.barge.as_ref();
        self.barge_on.then_some(barge)
    }
}

struct LQueue {
    log: Shared,
    accept: Option<usize>,
    finish: SayOutcome,
    pushes: Mutex<usize>,
}

impl SpeechQueue for LQueue {
    fn push(&self, text: &str, _e: Emotion) -> bool {
        let mut n = self.pushes.lock().unwrap();
        if self.accept.is_some_and(|max| *n >= max) {
            return false;
        }
        *n += 1;
        note(&self.log, format!("push:{text}"));
        true
    }

    fn abort(&self) {
        note(&self.log, "abort");
    }

    fn finish(&self) -> BoxFuture<'_, SayOutcome> {
        note(&self.log, "finish");
        let o = self.finish;
        Box::pin(async move { o })
    }
}

struct LMouth {
    log: Shared,
    /// Scripted outcomes for `say`, one per call; empty => `Done`.
    says: Mutex<VecDeque<SayOutcome>>,
    /// Whether `open_queue` offers a queue (false => legacy mouth).
    offer: bool,
    accept: Option<usize>,
    finish: SayOutcome,
    /// The text of the most recent `say`.
    last_said: Mutex<Option<String>>,
    /// When the last `say` was this text, the next `wait_quiet` fires the barge (once).
    fire_after_said: Mutex<Option<String>>,
    barge: Arc<FakeBargeIn>,
}

impl Mouth for LMouth {
    fn say<'a>(&'a self, text: &'a str, _e: Emotion) -> BoxFuture<'a, SayOutcome> {
        note(&self.log, format!("say:{text}"));
        *self.last_said.lock().unwrap() = Some(text.to_string());
        let o = self
            .says
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(SayOutcome::Done);
        Box::pin(async move { o })
    }

    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        let last = self.last_said.lock().unwrap().clone();
        let mut fire = self.fire_after_said.lock().unwrap();
        if fire.is_some() && *fire == last {
            *fire = None;
            drop(fire);
            self.barge.latest().fire();
        }
        Box::pin(async { QuietOutcome::Quiet })
    }

    fn stop(&self) {
        note(&self.log, "stop");
    }

    fn open_queue(&self) -> Option<Arc<dyn SpeechQueue>> {
        if !self.offer {
            return None;
        }
        note(&self.log, "open");
        Some(Arc::new(LQueue {
            log: self.log.clone(),
            accept: self.accept,
            finish: self.finish,
            pushes: Mutex::new(0),
        }))
    }
}

struct LHands {
    log: Shared,
    paste: Mutex<VecDeque<Result<(), String>>>,
    user: Mutex<VecDeque<PhraseOutcome>>,
    model: Mutex<VecDeque<PhraseOutcome>>,
    /// Makes `cancel_job` reachable from the local tier (`classify_local_with` gate).
    active_job: bool,
}

impl Hands for LHands {
    fn paste_generated(&self, text: &str) -> Result<(), String> {
        note(&self.log, format!("paste:{text}"));
        self.paste.lock().unwrap().pop_front().unwrap_or(Ok(()))
    }

    fn run_user_phrase<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        note(&self.log, format!("user:{phrase}"));
        let o = self
            .user
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(PhraseOutcome::NoMatch);
        Box::pin(async move { o })
    }

    fn run_model_command<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        note(&self.log, format!("model:{phrase}"));
        let o = self
            .model
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(PhraseOutcome::NoMatch);
        Box::pin(async move { o })
    }

    fn cancel_job(&self) -> bool {
        note(&self.log, "cancel_job");
        true
    }

    fn has_active_job(&self) -> bool {
        self.active_job
    }
}

/// One scripted brain call: streamed deltas (for the streaming path), a delay, then the plan.
struct Turn {
    deltas: Vec<String>,
    delay: u64,
    plan: Result<TurnPlan, PlanError>,
}

/// What the loop gave the brain on one call.
#[derive(Debug, Clone)]
struct Seen {
    utterance: String,
    role: Option<String>,
    lesson_paused: bool,
    roles_enabled: bool,
    history: Vec<(String, String)>,
}

struct LBrain {
    script: Mutex<VecDeque<Turn>>,
    seen: Mutex<Vec<Seen>>,
}

impl LBrain {
    fn record(&self, input: &PlanInput<'_>) {
        self.seen.lock().unwrap().push(Seen {
            utterance: input.utterance.to_string(),
            role: input.role.map(str::to_string),
            lesson_paused: input.lesson_paused,
            roles_enabled: input.roles_enabled,
            history: input.history.to_vec(),
        });
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
                Some(t) => {
                    if t.delay > 0 {
                        tokio::time::sleep(Duration::from_millis(t.delay)).await;
                    }
                    t.plan
                }
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
        Box::pin(async move {
            let Some(t) = t else {
                return Err(PlanError::Backend);
            };
            for delta in &t.deltas {
                on_text(delta);
            }
            if t.delay > 0 {
                tokio::time::sleep(Duration::from_millis(t.delay)).await;
            }
            t.plan
        })
    }
}

struct LReporter;

impl Reporter for LReporter {
    fn state(&self, _s: SessionState) {}
}

struct Rig {
    log: Shared,
    cancel: Arc<watch::Sender<bool>>,
    ears: LEars,
    mouth: LMouth,
    hands: LHands,
    brain: LBrain,
    reporter: LReporter,
    barge: Arc<FakeBargeIn>,
}

/// A rig whose ears hear `utts` in order (then Aborted), whose brain follows `turns`, and whose
/// mouth offers a queue. Tests tweak the rig (it is owned) before running it.
fn rig(utts: &[impl AsRef<str>], turns: Vec<Turn>) -> Rig {
    let log: Shared = Arc::new(Mutex::new(vec![]));
    let captures = Arc::new(Mutex::new(VecDeque::new()));
    let barge = Arc::new(FakeBargeIn {
        log: log.clone(),
        arms: Mutex::new(vec![]),
        captures: captures.clone(),
    });
    let (tx, _rx) = watch::channel(false);
    Rig {
        ears: LEars {
            script: Mutex::new(utts.iter().map(|s| s.as_ref().to_string()).collect()),
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
        hands: LHands {
            log: log.clone(),
            paste: Mutex::new(VecDeque::new()),
            user: Mutex::new(VecDeque::new()),
            model: Mutex::new(VecDeque::new()),
            active_job: false,
        },
        brain: LBrain {
            script: Mutex::new(turns.into()),
            seen: Mutex::new(vec![]),
        },
        reporter: LReporter,
        cancel: Arc::new(tx),
        barge,
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

async fn run_for(r: &Rig, cfg: &SessionConfig, secs: u64) -> SessionEnd {
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
    tokio::time::timeout(Duration::from_secs(secs), run_session(&deps, cfg, token))
        .await
        .expect("run_session hung")
}

async fn run(r: &Rig, cfg: &SessionConfig) -> SessionEnd {
    run_for(r, cfg, 10).await
}

// ---- observation helpers -----------------------------------------------------------------------

fn entries(r: &Rig) -> Vec<String> {
    r.log.lock().unwrap().clone()
}

/// Texts passed to `Mouth::say`, in order.
fn says(r: &Rig) -> Vec<String> {
    entries(r)
        .into_iter()
        .filter_map(|e| e.strip_prefix("say:").map(str::to_string))
        .collect()
}

/// Texts pushed to any speech queue, in order.
fn pushes(r: &Rig) -> Vec<String> {
    entries(r)
        .into_iter()
        .filter_map(|e| e.strip_prefix("push:").map(str::to_string))
        .collect()
}

/// Paste, restricted command and cancel calls made to hands.
fn effect_calls(r: &Rig) -> usize {
    entries(r)
        .iter()
        .filter(|e| {
            e.starts_with("paste:") || e.starts_with("model:") || e.as_str() == "cancel_job"
        })
        .count()
}

fn seen(r: &Rig) -> Vec<Seen> {
    r.brain.seen.lock().unwrap().clone()
}

fn seen_n(r: &Rig, n: usize) -> Seen {
    let all = seen(r);
    assert!(
        n < all.len(),
        "brain call {n} missing; calls: {:?}",
        all.iter().map(|s| &s.utterance).collect::<Vec<_>>()
    );
    all[n].clone()
}

fn script_says(r: &Rig, outcomes: &[SayOutcome]) {
    r.mouth
        .says
        .lock()
        .unwrap()
        .extend(outcomes.iter().copied());
}

// ---- plan builders -----------------------------------------------------------------------------

const ROLE: &str = "a patient teacher for six year olds";
const TEACH: &str = "teach fractions to six year olds";
const ACK: &str = "Okay, teacher.";
const CONTINUE: &str = "Let's continue.";

/// A lesson answer: three chunks of at most three short sentences each.
const LESSON_PARTS: [&str; 3] = [
    "One is first. Two follows it. Three comes next.",
    "Four is here. Five keeps going. Six is almost done.",
    "Seven ends it.",
];

/// A question answer of three chunks, distinct from the lesson.
const ANSWER_PARTS: [&str; 3] = [
    "Alpha is one. Beta is two. Gamma is three.",
    "Delta is four. Epsilon is five. Zeta is six.",
    "Eta is seven.",
];

fn lesson_text() -> String {
    LESSON_PARTS.join(" ")
}

fn answer_text() -> String {
    ANSWER_PARTS.join(" ")
}

fn lesson_chunks() -> Vec<String> {
    LESSON_PARTS.iter().map(|s| s.to_string()).collect()
}

fn tp(kind: PlanKind, speech: &str) -> TurnPlan {
    TurnPlan {
        job: None,
        role: None,
        kind,
        speech: speech.to_string(),
        emotion: Emotion::Neutral,
        paste_text: None,
        command: None,
    }
}

fn set_role(role: &str, task: Option<&str>, resume: bool, speech: &str) -> TurnPlan {
    TurnPlan {
        role: Some(RoleDirective {
            role: role.to_string(),
            task: task.map(str::to_string),
            resume,
        }),
        ..tp(PlanKind::SetRole, speech)
    }
}

fn clear_role(speech: &str) -> TurnPlan {
    tp(PlanKind::ClearRole, speech)
}

fn resume_plan(speech: &str) -> TurnPlan {
    tp(PlanKind::Resume, speech)
}

fn paste_plan(text: &str, speech: &str) -> TurnPlan {
    TurnPlan {
        paste_text: Some(text.to_string()),
        ..tp(PlanKind::Paste, speech)
    }
}

fn cmd_plan(command: &str, speech: &str) -> TurnPlan {
    TurnPlan {
        command: Some(command.to_string()),
        ..tp(PlanKind::Command, speech)
    }
}

/// A brain call with no streamed deltas resolving to `p`.
fn ok(p: TurnPlan) -> Turn {
    Turn {
        deltas: vec![],
        delay: 0,
        plan: Ok(p),
    }
}

/// A brain call with no streamed deltas failing with `e`.
fn fail(e: PlanError) -> Turn {
    Turn {
        deltas: vec![],
        delay: 0,
        plan: Err(e),
    }
}

/// A plain Answer call.
fn answer(speech: &str) -> Turn {
    ok(tp(PlanKind::Answer, speech))
}

/// An Answer call that streams its JSON in 8-char deltas and resolves after `delay` ms.
fn streamed_answer(speech: &str, delay: u64) -> Turn {
    let json = format!(r#"{{"kind":"answer","speech":"{speech}"}}"#);
    let chars: Vec<char> = json.chars().collect();
    Turn {
        deltas: chars.chunks(8).map(|c| c.iter().collect()).collect(),
        delay,
        plan: Ok(tp(PlanKind::Answer, speech)),
    }
}

/// `ACK` set as a role-only plan (no task), as the first turn of most scenarios.
fn role_only() -> Turn {
    ok(set_role(ROLE, None, false, ACK))
}

/// Compound start: the role with a first request (lesson) and a resume flag.
fn compound(resume: bool) -> Turn {
    ok(set_role(ROLE, Some(TEACH), resume, ACK))
}

// ---- B1 ----------------------------------------------------------------------------------------

#[tokio::test]
async fn role_b1_every_plan_input_has_roles_enabled() {
    let r = rig(
        &["hello there", "and again"],
        vec![answer("Hi."), answer("Again.")],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    let calls = seen(&r);
    assert_eq!(calls.len(), 2, "two brain calls expected");
    let flags: Vec<bool> = calls.iter().map(|s| s.roles_enabled).collect();
    assert!(
        flags.iter().all(|f| *f),
        "roles_enabled must be true on every PlanInput, got {flags:?}"
    );
}

// ---- B2 ----------------------------------------------------------------------------------------

#[tokio::test]
async fn role_b2_set_role_persists_for_more_than_context_turns() {
    let mut utts = vec!["act like a teacher".to_string()];
    let mut turns = vec![role_only()];
    for i in 1..=9 {
        utts.push(format!("question {i}"));
        turns.push(answer(&format!("answer {i}")));
    }
    let r = rig(&utts, turns);
    let _ = run(&r, &lcfg(false, None)).await;

    let mut expected = vec![ACK.to_string()];
    expected.extend((1..=9).map(|i| format!("answer {i}")));
    assert_eq!(says(&r), expected, "speech for every turn");

    let calls = seen(&r);
    assert_eq!(calls.len(), 10, "one brain call per utterance");
    for (i, call) in calls.iter().enumerate().skip(1) {
        assert_eq!(
            call.role.as_deref(),
            Some(ROLE),
            "call {i} must see the active role"
        );
    }
    assert_eq!(
        calls[9].history.len(),
        6,
        "history window is context_turns=6"
    );
    for call in &calls {
        for (u, a) in &call.history {
            assert!(
                !u.contains(ROLE) && !a.contains(ROLE),
                "role text must not enter history: {u:?} / {a:?}"
            );
        }
    }
}

#[tokio::test]
async fn role_b2_second_set_role_replaces_the_first() {
    let r = rig(
        &["act like a teacher", "now act like a pirate", "what now"],
        vec![
            ok(set_role(ROLE, None, false, ACK)),
            ok(set_role("a grumpy pirate", None, false, "Arr.")),
            answer("Aye."),
        ],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(seen_n(&r, 1).role.as_deref(), Some(ROLE));
    assert_eq!(seen_n(&r, 2).role.as_deref(), Some("a grumpy pirate"));
    assert_eq!(says(&r), vec![ACK, "Arr.", "Aye."]);
}

#[tokio::test]
async fn role_b2_clear_role_plan_clears_the_role_and_speaks() {
    let r = rig(
        &["act like a teacher", "forget the act", "what now"],
        vec![
            role_only(),
            ok(clear_role("Okay, I'm myself again.")),
            answer("Hi."),
        ],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(seen_n(&r, 2).role, None, "a cleared role is not sent");
    assert_eq!(says(&r), vec![ACK, "Okay, I'm myself again.", "Hi."]);
}

// ---- B3 ----------------------------------------------------------------------------------------

#[tokio::test]
async fn role_b3_local_clear_phrases_clear_without_the_brain() {
    for phrase in [
        "back to normal",
        "Stop acting.",
        "stop the role",
        "drop the role",
        "End the role!",
    ] {
        let r = rig(
            &["act like a teacher", phrase, "what now"],
            vec![role_only(), answer("Two.")],
        );
        let _ = run(&r, &lcfg(false, None)).await;
        assert_eq!(
            says(&r),
            vec![
                ACK.to_string(),
                "Okay, back to normal.".to_string(),
                "Two.".to_string()
            ],
            "phrase {phrase:?}"
        );
        let calls = seen(&r);
        assert_eq!(
            calls.len(),
            2,
            "phrase {phrase:?}: brain must not be called"
        );
        assert_eq!(calls[1].utterance, "what now");
        assert_eq!(calls[1].role, None, "phrase {phrase:?}: role cleared");
    }
}

#[tokio::test]
async fn role_b3_local_clear_drops_a_paused_lesson() {
    let r = rig(
        &["act like a teacher", "back to normal", "next thing"],
        vec![compound(false), answer(&lesson_text()), answer("Next.")],
    );
    script_says(
        &r,
        &[SayOutcome::Done, SayOutcome::Done, SayOutcome::Interrupted],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        says(&r),
        [
            vec![ACK.to_string()],
            lesson_chunks(),
            vec!["Okay, back to normal.".to_string(), "Next.".to_string()],
        ]
        .concat(),
    );
    let next = seen_n(&r, 2);
    assert!(
        !next.lesson_paused,
        "clear phrase must drop the paused lesson"
    );
    assert_eq!(next.role, None);
}

#[tokio::test]
async fn role_b3_phrases_that_only_contain_the_clear_words_reach_the_brain() {
    let r = rig(
        &[
            "act like a teacher",
            "I am back to normal",
            "back to normal please",
        ],
        vec![role_only(), answer("Fine."), answer("Also fine.")],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    let calls = seen(&r);
    assert_eq!(calls.len(), 3, "both phrases must reach the brain");
    assert_eq!(calls[1].utterance, "I am back to normal");
    assert_eq!(calls[2].utterance, "back to normal please");
    assert!(calls[1].role.is_some() && calls[2].role.is_some());
    assert!(
        !says(&r).contains(&"Okay, back to normal.".to_string()),
        "not local: {:?}",
        says(&r)
    );
}

#[tokio::test]
async fn role_b3_local_end_still_ends_the_session_while_a_role_is_active() {
    let r = rig(&["act like a teacher", "bye"], vec![role_only()]);
    let end = run(&r, &lcfg(false, None)).await;
    assert_eq!(end, SessionEnd::UserEnded);
    assert_eq!(seen(&r).len(), 1, "brain uncalled for the local end");
}

#[tokio::test]
async fn role_b3_local_dictation_still_pastes_while_a_role_is_active() {
    let r = rig(
        &["act like a teacher", "write this down hello there"],
        vec![role_only()],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(seen(&r).len(), 1, "brain uncalled for the dictation");
    assert!(
        entries(&r).contains(&"paste:hello there".to_string()),
        "dictation must paste: {:?}",
        entries(&r)
    );
}

#[tokio::test]
async fn role_b3_local_cancel_job_still_cancels_while_a_role_is_active() {
    let mut r = rig(&["act like a teacher", "cancel the job"], vec![role_only()]);
    r.hands.active_job = true;
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(seen(&r).len(), 1, "brain uncalled for the cancel");
    assert!(
        entries(&r).contains(&"cancel_job".to_string()),
        "cancel must reach hands: {:?}",
        entries(&r)
    );
}

// ---- B4 ----------------------------------------------------------------------------------------

#[tokio::test]
async fn role_b4_chunks_are_spoken_in_order_on_the_legacy_mouth() {
    let mut r = rig(
        &["act like a teacher", "tell me about it", "what next"],
        vec![role_only(), answer(&lesson_text()), answer("Next.")],
    );
    r.mouth.offer = false;
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        says(&r),
        [
            vec![ACK.to_string()],
            lesson_chunks(),
            vec!["Next.".to_string()],
        ]
        .concat(),
    );
    assert!(
        !seen_n(&r, 2).lesson_paused,
        "an ordinary long answer creates no paused lesson"
    );
}

#[tokio::test]
async fn role_b4_chunks_are_spoken_in_order_on_the_queue_mouth() {
    let r = rig(
        &["act like a teacher", "tell me about it", "what next"],
        vec![role_only(), answer(&lesson_text()), answer("Next.")],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        says(&r),
        [
            vec![ACK.to_string()],
            lesson_chunks(),
            vec!["Next.".to_string()],
        ]
        .concat(),
    );
}

#[tokio::test]
async fn role_b4_early_speech_and_acknowledgement_stay_off_for_role_turns() {
    let r = rig(
        &["act like a teacher", "tell me about it"],
        vec![role_only(), streamed_answer(&lesson_text(), 80)],
    );
    let _ = run(&r, &lcfg(true, Some(20))).await;
    let lesson_pushes: Vec<String> = pushes(&r)
        .into_iter()
        .filter(|p| {
            LESSON_PARTS
                .iter()
                .any(|part| part.contains(p.as_str()) || p.contains(part))
        })
        .collect();
    assert!(
        lesson_pushes.is_empty(),
        "no early partial push of the lesson: {lesson_pushes:?}"
    );
    let log = entries(&r);
    for ack in [
        "One moment.",
        "Okay.",
        "Sure, writing that.",
        "Okay, drafting that.",
        "On it.",
        "Let me think.",
        "Good question.",
        "Let me check.",
        "Sure, revising that.",
        "Okay, reworking it.",
        "Okay, working on that.",
    ] {
        assert!(
            !log.contains(&format!("push:{ack}")) && !log.contains(&format!("say:{ack}")),
            "no acknowledgement may fire on a role turn: {ack:?} in {log:?}"
        );
    }
    let lesson_says: Vec<String> = says(&r)
        .into_iter()
        .filter(|s| LESSON_PARTS.contains(&s.as_str()))
        .collect();
    assert_eq!(
        lesson_says,
        lesson_chunks(),
        "chunks spoken with one say each"
    );
}

#[tokio::test]
async fn role_b4_a_single_chunk_answer_is_one_say_and_creates_no_lesson() {
    let r = rig(
        &["act like a teacher", "short one", "next"],
        vec![
            role_only(),
            answer("Two short sentences. Done here."),
            answer("Next."),
        ],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        says(&r),
        vec![ACK, "Two short sentences. Done here.", "Next."]
    );
    assert!(!seen_n(&r, 2).lesson_paused);
}

#[tokio::test]
async fn role_b4_a_long_single_sentence_splits_at_the_last_space_before_300() {
    let long = format!("{} {}.", "a".repeat(250), "b".repeat(100));
    let r = rig(
        &["act like a teacher", "long one"],
        vec![role_only(), answer(&long)],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        says(&r),
        vec![
            ACK.to_string(),
            "a".repeat(250),
            format!("{}.", "b".repeat(100))
        ],
    );
}

#[tokio::test]
async fn role_b4_a_long_word_with_no_space_is_hard_cut_at_300() {
    let long = "c".repeat(350);
    let r = rig(
        &["act like a teacher", "long one"],
        vec![role_only(), answer(&long)],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        says(&r),
        vec![ACK.to_string(), "c".repeat(300), "c".repeat(50)],
    );
}

#[tokio::test]
async fn role_b4_without_a_role_nothing_is_chunked() {
    let r = rig(&["just talk"], vec![answer(&lesson_text())]);
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(says(&r), vec![lesson_text()]);
}

// ---- B5 ----------------------------------------------------------------------------------------

#[tokio::test]
async fn role_b5_compound_start_speaks_the_ack_then_one_lesson_call_and_pauses_the_rest() {
    let r = rig(
        &["act like a teacher", "what now"],
        vec![compound(false), answer(&lesson_text()), answer("Next.")],
    );
    script_says(
        &r,
        &[SayOutcome::Done, SayOutcome::Done, SayOutcome::Interrupted],
    );
    let _ = run(&r, &lcfg(false, None)).await;

    let calls = seen(&r);
    assert_eq!(calls.len(), 3, "exactly one extra call for the task");
    assert_eq!(calls[1].utterance, TEACH);
    assert_eq!(
        calls[1].role.as_deref(),
        Some(ROLE),
        "role set before the task call"
    );
    assert_eq!(calls[2].utterance, "what now");
    assert!(
        calls[2].lesson_paused,
        "the unspoken remainder is the paused lesson"
    );
    assert_eq!(
        says(&r),
        [
            vec![
                ACK.to_string(),
                LESSON_PARTS[0].to_string(),
                LESSON_PARTS[1].to_string()
            ],
            vec!["Next.".to_string()],
        ]
        .concat(),
    );
}

#[tokio::test]
async fn role_b5_compound_start_fully_spoken_leaves_no_lesson() {
    let r = rig(
        &["act like a teacher", "what now"],
        vec![compound(false), answer(&lesson_text()), answer("Next.")],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert!(
        !seen_n(&r, 2).lesson_paused,
        "a fully spoken lesson leaves no remainder"
    );
}

#[tokio::test]
async fn role_b5_a_non_answer_task_plan_is_ignored_with_no_effect() {
    let extras = vec![
        ("SetRole", ok(set_role("a pirate", None, false, "Arr."))),
        ("ClearRole", ok(clear_role("Cleared."))),
        ("Resume", ok(resume_plan("Resuming."))),
        ("Command", ok(cmd_plan("open safari", "Opening."))),
        ("Paste", ok(paste_plan("secret text", "Pasting."))),
        ("Job", ok(tp(PlanKind::Job, "Starting a job."))),
        ("End", ok(tp(PlanKind::End, "Goodbye."))),
        ("Clarify", ok(tp(PlanKind::Clarify, "Which one?"))),
    ];
    for (name, extra) in extras {
        let r = rig(
            &["act like a teacher", "what now"],
            vec![compound(false), extra, answer("Next.")],
        );
        let _ = run(&r, &lcfg(false, None)).await;
        assert_eq!(
            says(&r),
            vec![ACK, "Next."],
            "{name}: only the ack and the next answer are spoken"
        );
        assert_eq!(effect_calls(&r), 0, "{name}: no hands effect");
        let calls = seen(&r);
        assert_eq!(calls.len(), 3, "{name}: exactly one extra call");
        assert!(!calls[2].lesson_paused, "{name}: no lesson exists");
        assert_eq!(
            calls[2].role.as_deref(),
            Some(ROLE),
            "{name}: role stays set"
        );
    }
}

#[tokio::test]
async fn role_b5_a_failed_task_call_speaks_the_fixed_line_and_keeps_the_role() {
    for e in [PlanError::Backend, PlanError::Timeout] {
        let r = rig(
            &["act like a teacher", "what now"],
            vec![compound(false), fail(e), answer("Next.")],
        );
        let _ = run(&r, &lcfg(false, None)).await;
        assert_eq!(
            says(&r),
            vec![ACK, "I couldn't start the lesson.", "Next."],
            "{e:?}"
        );
        assert_eq!(
            seen_n(&r, 2).role.as_deref(),
            Some(ROLE),
            "{e:?}: role kept"
        );
    }
}

#[tokio::test]
async fn role_b5_an_interrupted_ack_keeps_the_role_and_makes_no_task_call() {
    let r = rig(
        &["act like a teacher", "what now"],
        vec![compound(false), answer("Fine.")],
    );
    script_says(&r, &[SayOutcome::Interrupted]);
    let _ = run(&r, &lcfg(false, None)).await;
    let calls = seen(&r);
    assert_eq!(calls.len(), 2, "zero extra calls");
    assert_eq!(
        calls[1].utterance, "what now",
        "the next call is the next utterance"
    );
    assert_eq!(calls[1].role.as_deref(), Some(ROLE));
    assert_eq!(says(&r), vec![ACK.to_string(), "Fine.".to_string()]);
}

// ---- B6 ----------------------------------------------------------------------------------------

#[tokio::test]
async fn role_b6_an_interrupted_question_answer_leaves_the_paused_lesson_untouched() {
    let r = rig(
        &["act like a teacher", "tell me a story", "continue"],
        vec![
            compound(false),
            answer(&lesson_text()),
            answer(&answer_text()),
            ok(resume_plan("")),
        ],
    );
    script_says(
        &r,
        &[
            SayOutcome::Done,
            SayOutcome::Done,
            SayOutcome::Interrupted,
            SayOutcome::Interrupted,
        ],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        says(&r),
        vec![
            ACK.to_string(),
            LESSON_PARTS[0].to_string(),
            LESSON_PARTS[1].to_string(),
            ANSWER_PARTS[0].to_string(),
            LESSON_PARTS[1].to_string(),
            LESSON_PARTS[2].to_string(),
        ],
        "the question's remainder is dropped; the lesson resumes from its front"
    );
    assert!(
        seen_n(&r, 2).lesson_paused,
        "the question sees the lesson paused"
    );
    assert!(
        seen_n(&r, 3).lesson_paused,
        "the resume still finds the lesson"
    );
}

// ---- B7 ----------------------------------------------------------------------------------------

#[tokio::test]
async fn role_b7_resume_replays_the_interrupted_front_chunk_and_skips_done_chunks() {
    let r = rig(
        &["act like a teacher", "continue", "continue"],
        vec![
            compound(false),
            answer(&lesson_text()),
            ok(resume_plan("")),
            ok(resume_plan("")),
        ],
    );
    script_says(
        &r,
        &[
            SayOutcome::Done,
            SayOutcome::Done,
            SayOutcome::Interrupted,
            SayOutcome::Interrupted,
        ],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        says(&r),
        vec![
            ACK.to_string(),
            LESSON_PARTS[0].to_string(),
            LESSON_PARTS[1].to_string(),
            LESSON_PARTS[1].to_string(),
            LESSON_PARTS[1].to_string(),
            LESSON_PARTS[2].to_string(),
        ],
        "an interrupted chunk stays at the front; Done chunks are not replayed"
    );
}

#[tokio::test]
async fn role_b7_a_failed_chunk_stops_the_turn_and_keeps_the_queue() {
    let r = rig(
        &["act like a teacher", "continue", "continue"],
        vec![
            compound(false),
            answer(&lesson_text()),
            ok(resume_plan("")),
            ok(resume_plan("")),
        ],
    );
    script_says(
        &r,
        &[
            SayOutcome::Done,
            SayOutcome::Done,
            SayOutcome::Interrupted,
            SayOutcome::Failed,
        ],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        says(&r),
        vec![
            ACK.to_string(),
            LESSON_PARTS[0].to_string(),
            LESSON_PARTS[1].to_string(),
            LESSON_PARTS[1].to_string(),
            LESSON_PARTS[1].to_string(),
            LESSON_PARTS[2].to_string(),
        ],
        "a Failed say speaks nothing further in that turn and keeps the chunk"
    );
}

#[tokio::test]
async fn role_b7_resume_without_a_lesson_behaves_like_an_answer() {
    let r = rig(
        &["act like a teacher", "continue", "continue"],
        vec![role_only(), ok(resume_plan("Sure.")), ok(resume_plan(""))],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(says(&r), vec![ACK.to_string(), "Sure.".to_string()]);
    assert_eq!(seen(&r).len(), 3);
}

// ---- B8 ----------------------------------------------------------------------------------------

#[tokio::test]
async fn role_b8_resume_auto_continues_after_an_answered_question() {
    let r = rig(
        &["act like a teacher", "question"],
        vec![
            compound(true),
            answer(&lesson_text()),
            answer("Here is my answer."),
        ],
    );
    script_says(
        &r,
        &[SayOutcome::Done, SayOutcome::Done, SayOutcome::Interrupted],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        says(&r),
        vec![
            ACK.to_string(),
            LESSON_PARTS[0].to_string(),
            LESSON_PARTS[1].to_string(),
            "Here is my answer.".to_string(),
            CONTINUE.to_string(),
            LESSON_PARTS[1].to_string(),
            LESSON_PARTS[2].to_string(),
        ],
    );
}

#[tokio::test]
async fn role_b8_continued_speech_is_interruptible_again() {
    let r = rig(
        &["act like a teacher", "question one", "question two"],
        vec![
            compound(true),
            answer(&lesson_text()),
            answer("Here is one."),
            answer("Here is two."),
        ],
    );
    script_says(
        &r,
        &[
            SayOutcome::Done,
            SayOutcome::Done,
            SayOutcome::Interrupted,
            SayOutcome::Done,
            SayOutcome::Done,
            SayOutcome::Interrupted,
            SayOutcome::Done,
            SayOutcome::Done,
        ],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        says(&r),
        vec![
            ACK.to_string(),
            LESSON_PARTS[0].to_string(),
            LESSON_PARTS[1].to_string(),
            "Here is one.".to_string(),
            CONTINUE.to_string(),
            LESSON_PARTS[1].to_string(),
            "Here is two.".to_string(),
            CONTINUE.to_string(),
            LESSON_PARTS[1].to_string(),
            LESSON_PARTS[2].to_string(),
        ],
    );
    assert!(seen_n(&r, 3).lesson_paused);
}

#[tokio::test]
async fn role_b8_a_barge_latched_after_the_answer_blocks_the_continuation() {
    let mut r = rig(
        &["act like a teacher", "question"],
        vec![
            compound(true),
            answer(&lesson_text()),
            answer("Here is my answer."),
            answer("Shorter."),
        ],
    );
    r.ears.barge_on = true;
    r.barge
        .captures
        .lock()
        .unwrap()
        .push_back("make it shorter".to_string());
    *r.mouth.fire_after_said.lock().unwrap() = Some("Here is my answer.".to_string());
    script_says(
        &r,
        &[SayOutcome::Done, SayOutcome::Done, SayOutcome::Interrupted],
    );
    let _ = run(&r, &lcfg(false, None)).await;

    let said = says(&r);
    assert_eq!(
        said[..5].to_vec(),
        vec![
            ACK.to_string(),
            LESSON_PARTS[0].to_string(),
            LESSON_PARTS[1].to_string(),
            "Here is my answer.".to_string(),
            "Shorter.".to_string(),
        ],
        "the latched barge stops the continuation before it starts: {said:?}"
    );
    let continue_before_shorter = said
        .iter()
        .take_while(|s| s.as_str() != "Shorter.")
        .any(|s| s == CONTINUE);
    assert!(
        !continue_before_shorter,
        "no continuation before the barge text is answered"
    );
    assert_eq!(
        seen_n(&r, 3).utterance,
        "make it shorter",
        "the captured barge utterance is the next turn"
    );
    assert!(seen_n(&r, 3).lesson_paused, "the lesson stays paused");
}

#[tokio::test]
async fn role_b8_without_resume_auto_the_remainder_waits_for_a_resume_plan() {
    let r = rig(
        &["act like a teacher", "question"],
        vec![
            compound(false),
            answer(&lesson_text()),
            answer("Here is my answer."),
        ],
    );
    script_says(
        &r,
        &[SayOutcome::Done, SayOutcome::Done, SayOutcome::Interrupted],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        says(&r),
        vec![
            ACK.to_string(),
            LESSON_PARTS[0].to_string(),
            LESSON_PARTS[1].to_string(),
            "Here is my answer.".to_string(),
        ],
    );
}

#[tokio::test]
async fn role_b8_a_non_answer_interrupting_turn_skips_the_continuation() {
    // A plan error after an interruption: no continuation.
    let r = rig(
        &["act like a teacher", "question"],
        vec![
            compound(true),
            answer(&lesson_text()),
            fail(PlanError::Backend),
        ],
    );
    script_says(
        &r,
        &[SayOutcome::Done, SayOutcome::Done, SayOutcome::Interrupted],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    let said = says(&r);
    assert!(!said.contains(&CONTINUE.to_string()), "{said:?}");
    assert_eq!(
        said.iter()
            .filter(|s| s.as_str() == LESSON_PARTS[1])
            .count(),
        1
    );

    // A local command after an interruption: no continuation.
    let mut r = rig(
        &["act like a teacher", "open safari for me"],
        vec![compound(true), answer(&lesson_text())],
    );
    r.hands
        .user
        .lock()
        .unwrap()
        .push_back(PhraseOutcome::Done("Opened Safari".into()));
    script_says(
        &r,
        &[SayOutcome::Done, SayOutcome::Done, SayOutcome::Interrupted],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    let said = says(&r);
    assert!(!said.contains(&CONTINUE.to_string()), "{said:?}");
    assert!(said.contains(&"Opened Safari".to_string()), "{said:?}");
}

#[tokio::test]
async fn role_b8_end_after_an_interruption_drops_role_and_lesson() {
    let r = rig(
        &["act like a teacher", "question"],
        vec![
            compound(true),
            answer(&lesson_text()),
            ok(tp(PlanKind::End, "Goodbye.")),
        ],
    );
    script_says(
        &r,
        &[SayOutcome::Done, SayOutcome::Done, SayOutcome::Interrupted],
    );
    let end = run(&r, &lcfg(false, None)).await;
    assert_eq!(end, SessionEnd::UserEnded);
    assert!(!says(&r).contains(&CONTINUE.to_string()));
}

// ---- B9 ----------------------------------------------------------------------------------------

#[tokio::test]
async fn role_b9_nested_interruption_drops_the_answer_remainder_then_resumes_the_lesson() {
    let r = rig(
        &["act like a teacher", "question one", "question two"],
        vec![
            compound(true),
            answer(&lesson_text()),
            answer(&answer_text()),
            answer("Second answer."),
        ],
    );
    script_says(
        &r,
        &[
            SayOutcome::Done,
            SayOutcome::Done,
            SayOutcome::Interrupted,
            SayOutcome::Interrupted,
            SayOutcome::Done,
            SayOutcome::Done,
        ],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(
        says(&r),
        vec![
            ACK.to_string(),
            LESSON_PARTS[0].to_string(),
            LESSON_PARTS[1].to_string(),
            ANSWER_PARTS[0].to_string(),
            "Second answer.".to_string(),
            CONTINUE.to_string(),
            LESSON_PARTS[1].to_string(),
            LESSON_PARTS[2].to_string(),
        ],
        "A1's remainder is dropped; the lesson resumes from its front chunk"
    );
}

// ---- B10 ---------------------------------------------------------------------------------------

#[tokio::test]
async fn role_b10_role_and_lesson_do_not_survive_a_new_session() {
    let mut r = rig(&["act like a teacher"], vec![role_only()]);
    let _ = run(&r, &lcfg(false, None)).await;
    assert_eq!(seen_n(&r, 0).role, None);

    r.ears.script = Mutex::new(VecDeque::from(vec!["hello again".to_string()]));
    r.brain.script.lock().unwrap().push_back(answer("Hi."));
    let _ = run(&r, &lcfg(false, None)).await;
    let second = seen_n(&r, 1);
    assert_eq!(second.utterance, "hello again");
    assert_eq!(second.role, None, "the second session starts with no role");
    assert!(!second.lesson_paused);
}

#[tokio::test]
async fn role_b10_a_set_role_while_a_lesson_exists_drops_the_old_lesson() {
    let r = rig(
        &["act like a teacher", "pirate time", "what now"],
        vec![
            compound(false),
            answer(&lesson_text()),
            ok(set_role("a pirate", None, false, "Arr.")),
            answer("Next."),
        ],
    );
    script_says(
        &r,
        &[SayOutcome::Done, SayOutcome::Done, SayOutcome::Interrupted],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert!(seen_n(&r, 2).lesson_paused, "premise: a lesson was paused");
    let next = seen_n(&r, 3);
    assert!(!next.lesson_paused, "the new role drops the old lesson");
    assert_eq!(next.role.as_deref(), Some("a pirate"));
    assert_eq!(
        says(&r),
        vec![ACK, LESSON_PARTS[0], LESSON_PARTS[1], "Arr.", "Next."]
    );
}

// ---- B11 ---------------------------------------------------------------------------------------

#[tokio::test]
async fn role_b11_answer_only_allows_role_plans_and_blocks_effects() {
    let mut cfg = lcfg(false, None);
    cfg.answer_only = true;
    let r = rig(
        &["act like a teacher", "clear it", "paste it", "run it"],
        vec![
            role_only(),
            ok(clear_role("Cleared.")),
            ok(paste_plan("draft text", "Pasting.")),
            ok(cmd_plan("open safari", "Opening.")),
        ],
    );
    let _ = run(&r, &cfg).await;
    assert_eq!(
        says(&r),
        vec![
            ACK.to_string(),
            "Cleared.".to_string(),
            MSG_ANSWER_ONLY.to_string(),
            MSG_ANSWER_ONLY.to_string(),
        ],
    );
    assert_eq!(effect_calls(&r), 0, "answer-only: no paste or command");
    assert_eq!(seen_n(&r, 2).role, None);
}

#[tokio::test]
async fn role_b11_answer_only_allows_resume() {
    let mut cfg = lcfg(false, None);
    cfg.answer_only = true;
    let r = rig(
        &["act like a teacher", "carry on"],
        vec![role_only(), ok(resume_plan("Carrying on."))],
    );
    let _ = run(&r, &cfg).await;
    assert_eq!(says(&r), vec![ACK.to_string(), "Carrying on.".to_string()]);
}

// ---- END ----
