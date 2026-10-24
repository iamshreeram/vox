//! Stage 1 roles and lessons, real barge-in (DESIGN-PR2-STAGE1 Revision 3, sections 2.4-2.6).
//!
//! The other lesson tests simulate an interruption by scripting `SayOutcome::Interrupted`. In
//! production a real barge-in latches the reply token instead: the in-flight `say` future is
//! dropped by the race, the turn ends cancelled, and the captured utterance becomes the next turn.
//! These tests drive that path. The fakes are copied from converse_role_loop_tests.rs and
//! converse_barge_loop_tests.rs (same `BargeIn` watch, fired by the mouth or the brain, its capture
//! delivered on fire). Here the fire happens while a `say` or a planner call is in flight, and the
//! fired future never resolves, so the race is what ends it.
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

// ---- barge-in fake (same mechanism as converse_role_loop_tests.rs) ----------------------------

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

// ---- ears, mouth, hands, brain, reporter -------------------------------------------------------

struct LEars {
    script: Mutex<VecDeque<String>>,
    log: Shared,
    barge: Arc<FakeBargeIn>,
    /// When true the loop arms a barge watch per reply.
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

struct LMouth {
    log: Shared,
    /// Scripted outcomes for `say`, one per call that is not fired; empty => `Done`.
    says: Mutex<VecDeque<SayOutcome>>,
    /// Every text handed to `say`, in order (used to count occurrences).
    said: Mutex<Vec<String>>,
    /// `(text, n)`: the n-th (1-based) `say` of `text` fires the barge while it is in flight and
    /// then never resolves, so the race drops it as a real barge does.
    fire_in_flight: Mutex<Option<(String, usize)>>,
    barge: Arc<FakeBargeIn>,
}

impl Mouth for LMouth {
    fn say<'a>(&'a self, text: &'a str, _e: Emotion) -> BoxFuture<'a, SayOutcome> {
        note(&self.log, format!("say:{text}"));
        let nth = {
            let mut said = self.said.lock().unwrap();
            said.push(text.to_string());
            said.iter().filter(|s| s.as_str() == text).count()
        };
        let hit = {
            let mut fire = self.fire_in_flight.lock().unwrap();
            let hit = matches!(fire.as_ref(), Some((t, n)) if t.as_str() == text && *n == nth);
            if hit {
                *fire = None;
            }
            hit
        };
        if hit {
            // Fire on the second poll, not inside this call: `GatedMouth::say` runs the first poll
            // while holding the reply gate, and the fire closes that same gate. The second poll
            // happens in the race, outside the admission, like a real barge. It never resolves,
            // so the race drops this in-flight `say`.
            let barge = self.barge.clone();
            let mut polled = false;
            return Box::pin(std::future::poll_fn::<SayOutcome, _>(move |_cx| {
                if polled {
                    barge.latest().fire();
                } else {
                    polled = true;
                }
                std::task::Poll::Pending
            }));
        }
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
        note(&self.log, "stop");
    }

    fn open_queue(&self) -> Option<Arc<dyn SpeechQueue>> {
        None
    }
}

struct LHands {
    log: Shared,
}

impl Hands for LHands {
    fn paste_generated(&self, text: &str) -> Result<(), String> {
        note(&self.log, format!("paste:{text}"));
        Ok(())
    }

    fn run_user_phrase<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        note(&self.log, format!("user:{phrase}"));
        Box::pin(async { PhraseOutcome::NoMatch })
    }

    fn run_model_command<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        note(&self.log, format!("model:{phrase}"));
        Box::pin(async { PhraseOutcome::NoMatch })
    }

    fn cancel_job(&self) -> bool {
        note(&self.log, "cancel_job");
        true
    }

    fn has_active_job(&self) -> bool {
        false
    }
}

/// What the loop gave the brain on one call.
#[derive(Debug, Clone)]
struct Seen {
    utterance: String,
    role: Option<String>,
    lesson_paused: bool,
}

struct LBrain {
    script: Mutex<VecDeque<Result<TurnPlan, PlanError>>>,
    seen: Mutex<Vec<Seen>>,
    /// When the planner is called with this utterance, the barge fires into that call and the call
    /// never resolves (one-shot). Used for the lesson's follow-up planner call.
    fire_on_utterance: Mutex<Option<String>>,
    barge: Arc<FakeBargeIn>,
}

impl LBrain {
    /// Records the call and pops its scripted result. When this call is the one the barge fires
    /// into, the barge fires now and the call hangs (`true`).
    fn start(&self, input: &PlanInput<'_>) -> (Option<Result<TurnPlan, PlanError>>, bool) {
        self.seen.lock().unwrap().push(Seen {
            utterance: input.utterance.to_string(),
            role: input.role.map(str::to_string),
            lesson_paused: input.lesson_paused,
        });
        let turn = self.script.lock().unwrap().pop_front();
        let hit = {
            let mut fire = self.fire_on_utterance.lock().unwrap();
            let hit = fire.as_deref() == Some(input.utterance);
            if hit {
                *fire = None;
            }
            hit
        };
        if hit {
            self.barge.latest().fire();
            return (None, true);
        }
        (turn, false)
    }
}

impl Brain for LBrain {
    fn plan<'a>(&'a self, input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        let (turn, hang) = self.start(&input);
        Box::pin(async move {
            if hang {
                return pending().await;
            }
            turn.unwrap_or(Err(PlanError::Backend))
        })
    }

    fn plan_streaming<'a>(
        &'a self,
        input: PlanInput<'a>,
        _on_text: &'a mut (dyn FnMut(&str) + Send),
    ) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        let (turn, hang) = self.start(&input);
        Box::pin(async move {
            if hang {
                return pending().await;
            }
            turn.unwrap_or(Err(PlanError::Backend))
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
/// mouth is the legacy one (no queue). Tests tweak the rig (it is owned) before running it.
fn rig(utts: &[&str], turns: Vec<Result<TurnPlan, PlanError>>) -> Rig {
    let log: Shared = Arc::new(Mutex::new(vec![]));
    let barge = Arc::new(FakeBargeIn {
        log: log.clone(),
        arms: Mutex::new(vec![]),
        captures: Arc::new(Mutex::new(VecDeque::new())),
    });
    let (tx, _rx) = watch::channel(false);
    Rig {
        ears: LEars {
            script: Mutex::new(utts.iter().map(|s| s.to_string()).collect()),
            log: log.clone(),
            barge: barge.clone(),
            barge_on: false,
        },
        mouth: LMouth {
            log: log.clone(),
            says: Mutex::new(VecDeque::new()),
            said: Mutex::new(vec![]),
            fire_in_flight: Mutex::new(None),
            barge: barge.clone(),
        },
        hands: LHands { log: log.clone() },
        brain: LBrain {
            script: Mutex::new(turns.into()),
            seen: Mutex::new(vec![]),
            fire_on_utterance: Mutex::new(None),
            barge: barge.clone(),
        },
        reporter: LReporter,
        cancel: Arc::new(tx),
        barge,
        log,
    }
}

fn lcfg() -> SessionConfig {
    SessionConfig {
        post_speech_guard: Duration::from_millis(0),
        quiet_wait: Duration::from_millis(5),
        early_speech: false,
        ack_after: None,
        ..SessionConfig::default()
    }
}

/// Runs the session under a 10 s bound; a hang fails the test instead of blocking it.
async fn run(r: &Rig) -> SessionEnd {
    let deps = Deps {
        policy: None,
        announcer: None,
        ears: &r.ears,
        mouth: &r.mouth,
        hands: &r.hands,
        brain: &r.brain,
        reporter: &r.reporter,
    };
    let cfg = lcfg();
    let token = r.cancel.subscribe();
    tokio::time::timeout(Duration::from_secs(10), run_session(&deps, &cfg, token))
        .await
        .expect("run_session hung")
}

// ---- observation helpers -----------------------------------------------------------------------

/// Texts passed to `Mouth::say`, in order (a cut `say` is included: it was started).
fn says(r: &Rig) -> Vec<String> {
    r.log
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| e.strip_prefix("say:").map(str::to_string))
        .collect()
}

fn seen(r: &Rig) -> Vec<Seen> {
    r.brain.seen.lock().unwrap().clone()
}

fn texts(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

// ---- plan builders -----------------------------------------------------------------------------

const ROLE: &str = "a patient teacher for six year olds";
const TEACH: &str = "teach fractions to six year olds";
const ACK: &str = "Okay, teacher.";
const CONTINUE: &str = "Let's continue.";

/// A lesson answer of three chunks, each a separate `say`.
const LESSON_PARTS: [&str; 3] = [
    "One is first. Two follows it. Three comes next.",
    "Four is here. Five keeps going. Six is almost done.",
    "Seven ends it.",
];

fn lesson_text() -> String {
    LESSON_PARTS.join(" ")
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

fn resume_plan(speech: &str) -> Result<TurnPlan, PlanError> {
    Ok(tp(PlanKind::Resume, speech))
}

/// A plain Answer call.
fn answer(speech: &str) -> Result<TurnPlan, PlanError> {
    Ok(tp(PlanKind::Answer, speech))
}

/// Compound start: the role with a first request (the lesson) and a resume flag.
fn compound(resume: bool) -> Result<TurnPlan, PlanError> {
    Ok(set_role(ROLE, Some(TEACH), resume, ACK))
}

// ---- tests -------------------------------------------------------------------------------------

/// (a) A real barge fired while chunk 2 of 3 of a compound lesson is in flight. The in-flight say
/// is dropped, the barge utterance is the next turn with the lesson paused, and a later Resume
/// replays chunk 2 from its start without repeating chunk 1.
#[tokio::test]
async fn role_s5a_real_barge_cuts_an_in_flight_lesson_chunk_and_resume_replays_it() {
    let mut r = rig(
        &["act like a teacher", "continue"],
        vec![
            compound(false),
            answer(&lesson_text()),
            answer("Simpler."),
            resume_plan(""),
        ],
    );
    r.ears.barge_on = true;
    r.barge
        .captures
        .lock()
        .unwrap()
        .push_back("make it simpler".to_string());
    *r.mouth.fire_in_flight.lock().unwrap() = Some((LESSON_PARTS[1].to_string(), 1));

    let _ = run(&r).await;

    assert_eq!(
        says(&r),
        texts(&[
            ACK,
            LESSON_PARTS[0],
            LESSON_PARTS[1],
            "Simpler.",
            LESSON_PARTS[1],
            LESSON_PARTS[2],
        ]),
        "chunk 1 is spoken once; the cut chunk 2 is started again from its start after the barge turn"
    );
    let calls = seen(&r);
    assert_eq!(calls.len(), 4, "calls: {calls:?}");
    assert_eq!(
        calls[2].utterance, "make it simpler",
        "the captured barge utterance is the next turn"
    );
    assert!(
        calls[2].lesson_paused,
        "the cut lesson is paused on the barge turn"
    );
    assert_eq!(calls[2].role.as_deref(), Some(ROLE), "the role is kept");
}

/// (b) A real barge fired during the first resumed chunk of an auto-continuation (`Let's continue.`
/// after an answer). The next question is answered, the continuation runs again and resumes from
/// the cut chunk.
#[tokio::test]
async fn role_s5b_real_barge_during_a_resumed_chunk_of_an_auto_continuation_resumes_from_the_cut_chunk(
) {
    let mut r = rig(
        &["act like a teacher", "question one"],
        vec![
            compound(true),
            answer(&lesson_text()),
            answer("Here is one."),
            answer("Here is two."),
        ],
    );
    r.ears.barge_on = true;
    r.barge
        .captures
        .lock()
        .unwrap()
        .push_back("question two".to_string());
    // Chunk 2 is interrupted (scripted) at the lesson start, so one lesson chunk stays paused.
    script(
        &r,
        &[SayOutcome::Done, SayOutcome::Done, SayOutcome::Interrupted],
    );
    // The second `say` of chunk 2 is the first resumed chunk of the auto-continuation.
    *r.mouth.fire_in_flight.lock().unwrap() = Some((LESSON_PARTS[1].to_string(), 2));

    let _ = run(&r).await;

    assert_eq!(
        says(&r),
        texts(&[
            ACK,
            LESSON_PARTS[0],
            LESSON_PARTS[1],
            "Here is one.",
            CONTINUE,
            LESSON_PARTS[1],
            "Here is two.",
            CONTINUE,
            LESSON_PARTS[1],
            LESSON_PARTS[2],
        ]),
        "the barge cuts the continuation at chunk 2; the next answer's continuation resumes from it"
    );
    let calls = seen(&r);
    assert_eq!(calls.len(), 4, "calls: {calls:?}");
    assert!(
        calls[2].lesson_paused,
        "the question sees the lesson paused"
    );
    assert_eq!(
        calls[3].utterance, "question two",
        "the captured barge utterance is the next turn"
    );
    assert!(
        calls[3].lesson_paused,
        "the lesson stays paused on the barge turn"
    );
}

/// (c) A real barge fired while the lesson-start follow-up planner call is pending. The follow-up
/// is abandoned: no lesson exists, no lesson speech follows, the role stays set, and the barge
/// utterance is the next turn.
#[tokio::test]
async fn role_s5c_real_barge_during_the_pending_lesson_start_call_abandons_the_lesson() {
    let mut r = rig(
        &["act like a teacher"],
        vec![
            compound(false),
            // Consumed by the hung follow-up call; it must never be spoken.
            answer(&lesson_text()),
            answer("Simpler."),
        ],
    );
    r.ears.barge_on = true;
    r.barge
        .captures
        .lock()
        .unwrap()
        .push_back("make it simpler".to_string());
    *r.brain.fire_on_utterance.lock().unwrap() = Some(TEACH.to_string());

    let _ = run(&r).await;

    assert_eq!(
        says(&r),
        texts(&[ACK, "Simpler."]),
        "no lesson speech follows the abandoned follow-up"
    );
    let calls = seen(&r);
    assert_eq!(calls.len(), 3, "calls: {calls:?}");
    assert_eq!(calls[1].utterance, TEACH, "the follow-up call was made");
    assert!(
        !calls[1].lesson_paused,
        "no lesson exists during the follow-up"
    );
    assert_eq!(
        calls[2].utterance, "make it simpler",
        "the captured barge utterance is the next turn"
    );
    assert!(
        !calls[2].lesson_paused,
        "the abandoned follow-up leaves no lesson"
    );
    assert_eq!(calls[2].role.as_deref(), Some(ROLE), "the role stays set");
}

/// Scripts the outcomes of the mouth's `say` calls that are not fired, in order.
fn script(r: &Rig, outcomes: &[SayOutcome]) {
    r.mouth
        .says
        .lock()
        .unwrap()
        .extend(outcomes.iter().copied());
}
