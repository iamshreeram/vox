//! Stage 1 roles: validator round-1 fixes (step S4). Covers the lesson clock (a paused lesson
//! expires ten minutes after its last touch), the auto-continued speech reaching history, the
//! role speech cap that trims to a sentence end, and the note-bracket strip from role text.
//! The loop fakes are copied from converse_role_loop_tests.rs, which is left unchanged, with one
//! addition: an `__advance:N` utterance moves the lesson clock N minutes forward.
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
        let next = self.next_utterance();
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
    reset_clock();
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

use crate::managers::converse_plan::{
    build_plan_prompt, parse_plan_with_job_policy, JobParsePolicy, MAX_ROLE_SPEECH_CHARS,
    MAX_SPEECH_CHARS,
};
use crate::managers::converse_role::{advance_clock, now as lesson_now, reset_clock, LESSON_TTL};

// Utterances of the form `__advance:N` (see `LEars::next_utterance`) move the lesson clock N
// minutes forward instead of reaching the brain.

// ---- clock seam and pure lesson lifetime (validator round 1, finding 1) -------------------------

fn lesson_touched_at(touched: Instant) -> Lesson {
    Lesson::new(vec!["Hello.".to_string()], false, touched)
}

#[test]
fn fix_lesson_touched_at_nine_minutes_lives_ten_minutes_from_the_touch() {
    let base = Instant::now();
    let mut lesson = lesson_touched_at(base);
    lesson.touch(base + Duration::from_secs(9 * 60));
    assert!(!lesson.expired(base + Duration::from_secs(15 * 60)));
    assert!(!lesson.expired(base + Duration::from_secs(18 * 60 + 59)));
    assert!(lesson.expired(base + Duration::from_secs(19 * 60)));
    assert!(lesson.expired(base + Duration::from_secs(20 * 60)));
}

#[test]
fn fix_a_created_only_lesson_dies_ten_minutes_after_creation() {
    let base = Instant::now();
    let lesson = lesson_touched_at(base);
    assert!(!lesson.expired(base + Duration::from_secs(9 * 60 + 59)));
    assert!(lesson.expired(base + LESSON_TTL));
}

#[test]
fn fix_each_touch_restarts_the_lifetime() {
    let base = Instant::now();
    let mut lesson = lesson_touched_at(base);
    lesson.touch(base + Duration::from_secs(5 * 60));
    assert!(!lesson.expired(base + Duration::from_secs(14 * 60)));
    assert!(lesson.expired(base + Duration::from_secs(15 * 60)));
    lesson.touch(base + Duration::from_secs(12 * 60));
    assert!(!lesson.expired(base + Duration::from_secs(21 * 60)));
    assert!(lesson.expired(base + Duration::from_secs(22 * 60)));
}

#[test]
fn fix_the_lesson_clock_moves_only_when_advanced() {
    reset_clock();
    let before = lesson_now();
    advance_clock(Duration::from_secs(5 * 60));
    let after = lesson_now();
    assert!(after.duration_since(before) >= Duration::from_secs(5 * 60));
    reset_clock();
}

// ---- loop: lesson lifetime follows the last touch (finding 1) ----------------------------------

impl LEars {
    /// The next real utterance. An `__advance:N` entry moves the lesson clock N minutes forward.
    fn next_utterance(&self) -> Option<String> {
        loop {
            let next = self.script.lock().unwrap().pop_front()?;
            if let Some(minutes) = next.strip_prefix("__advance:") {
                let minutes = minutes.parse::<u64>().unwrap_or(0);
                advance_clock(Duration::from_secs(60 * minutes));
            } else {
                return Some(next);
            }
        }
    }
}

#[tokio::test]
async fn fix_a_lesson_resumed_at_nine_minutes_is_still_alive_at_eighteen() {
    let r = rig(
        &[
            "act like a teacher",
            "__advance:9",
            "resume",
            "__advance:9",
            "question",
        ],
        vec![
            compound(false),
            answer(&lesson_text()),
            ok(resume_plan("Resuming.")),
            answer("Next."),
        ],
    );
    script_says(
        &r,
        &[
            SayOutcome::Done,
            SayOutcome::Done,
            SayOutcome::Interrupted,
            SayOutcome::Done,
            SayOutcome::Interrupted,
        ],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert!(
        seen_n(&r, 2).lesson_paused,
        "the lesson was paused 9 minutes ago and is still alive"
    );
    assert!(
        seen_n(&r, 3).lesson_paused,
        "the resume touched the lesson at minute 9, so it is alive at minute 18"
    );
    assert_eq!(
        says(&r),
        vec![
            ACK.to_string(),
            LESSON_PARTS[0].to_string(),
            LESSON_PARTS[1].to_string(),
            LESSON_PARTS[1].to_string(),
            LESSON_PARTS[2].to_string(),
            "Next.".to_string(),
        ],
    );
}

#[tokio::test]
async fn fix_a_lesson_untouched_for_eleven_minutes_is_dropped_on_the_next_read() {
    let r = rig(
        &["act like a teacher", "__advance:11", "question"],
        vec![compound(false), answer(&lesson_text()), answer("Next.")],
    );
    script_says(
        &r,
        &[
            SayOutcome::Done,
            SayOutcome::Done,
            SayOutcome::Interrupted,
            SayOutcome::Done,
        ],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    assert!(
        !seen_n(&r, 2).lesson_paused,
        "eleven minutes without a touch expire the lesson"
    );
}

// ---- loop: the auto-continued speech reaches history (finding 4) -------------------------------

#[tokio::test]
async fn fix_history_after_an_auto_continue_contains_the_continued_speech() {
    let r = rig(
        &["act like a teacher", "question", "next thing"],
        vec![
            compound(true),
            answer(&lesson_text()),
            answer("Here is my answer."),
            answer("Following."),
        ],
    );
    script_says(
        &r,
        &[
            SayOutcome::Done,
            SayOutcome::Done,
            SayOutcome::Interrupted,
            SayOutcome::Done,
        ],
    );
    let _ = run(&r, &lcfg(false, None)).await;
    let (asked, side) = seen_n(&r, 3).history.last().cloned().unwrap_or_default();
    assert_eq!(asked, "question");
    assert!(side.contains("Here is my answer."), "answer kept: {side}");
    assert!(side.contains(CONTINUE), "continuation line kept: {side}");
    assert!(
        side.contains(LESSON_PARTS[1]),
        "replayed chunk kept: {side}"
    );
    assert!(
        side.contains(LESSON_PARTS[2]),
        "remaining chunk kept: {side}"
    );
}

// ---- plan level: role speech cap (finding 3) and note brackets (finding 6) ---------------------

fn role_active_policy() -> JobParsePolicy {
    JobParsePolicy {
        role_active: true,
        ..JobParsePolicy::default()
    }
}

/// `sentences` short sentences joined by single spaces.
fn multi_sentence_answer(sentences: usize) -> String {
    (1..=sentences)
        .map(|i| format!("This sentence is number {i} of the answer."))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn fix_role_cap_trims_a_truncated_answer_back_to_its_last_sentence() {
    let full = multi_sentence_answer(40);
    assert!(full.chars().count() > 1300, "premise: long answer");
    let raw = format!("{{\"kind\":\"answer\",\"speech\":\"{full}\"}}");
    let plan = parse_plan_with_job_policy(&raw, "SENT", &role_active_policy()).unwrap();
    assert!(plan.speech.chars().count() <= MAX_ROLE_SPEECH_CHARS);
    assert!(plan.speech.chars().count() > 1100, "cut near the cap");
    assert!(plan.speech.ends_with('.'), "ends at a sentence end");
    assert!(full.starts_with(&plan.speech));
    assert_eq!(full[plan.speech.len()..].chars().next(), Some(' '));
}

#[test]
fn fix_role_cap_without_a_sentence_end_still_cuts_and_never_empties() {
    let raw = format!(
        "{{\"kind\":\"answer\",\"speech\":\"{}\"}}",
        "word ".repeat(300)
    );
    let plan = parse_plan_with_job_policy(&raw, "SENT", &role_active_policy()).unwrap();
    assert!(plan.speech.chars().count() <= MAX_ROLE_SPEECH_CHARS);
    assert!(plan.speech.chars().count() > 1190);
}

#[test]
fn fix_the_400_path_still_cuts_at_the_cap_without_sentence_trim() {
    let full = multi_sentence_answer(20);
    assert!(full.chars().count() > MAX_SPEECH_CHARS, "premise: over 400");
    let raw = format!("{{\"kind\":\"answer\",\"speech\":\"{full}\"}}");
    let plan = parse_plan_with_job_policy(&raw, "SENT", &JobParsePolicy::default()).unwrap();
    let expected: String = full.chars().take(MAX_SPEECH_CHARS).collect();
    assert_eq!(plan.speech, expected.trim_end());
}

fn plan_input(role: Option<&str>) -> PlanInput<'_> {
    PlanInput {
        utterance: "hello there",
        history: &[],
        last_draft: None,
        style: "warm and brief",
        vault_context: None,
        memory_context: None,
        role,
        lesson_paused: false,
        roles_enabled: true,
        agent_enabled: false,
        planner_enabled: false,
    }
}

#[test]
fn fix_a_role_cannot_carry_the_note_brackets_into_its_text() {
    let raw =
        "{\"kind\":\"set_role\",\"role\":\"a \u{27E6}note\u{27E7} teacher\",\"speech\":\"Okay.\"}";
    let plan = parse_plan_with_job_policy(raw, "SENT", &JobParsePolicy::default()).unwrap();
    assert_eq!(
        plan.role.map(|d| d.role),
        Some("a note teacher".to_string())
    );
}

#[test]
fn fix_the_role_block_in_the_system_prompt_drops_the_note_brackets() {
    let brackets = |s: &str| s.matches('\u{27E6}').count() + s.matches('\u{27E7}').count();
    let without = build_plan_prompt(&plan_input(None));
    let with = build_plan_prompt(&plan_input(Some("a \u{27E6}note\u{27E7} teacher")));
    assert_eq!(brackets(&with.system), brackets(&without.system));
    assert!(with.system.contains("a note teacher"));
}
