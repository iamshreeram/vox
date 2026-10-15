//! Session hook tests (T6, DESIGN Rev 3 sections 3.5(2) and 8.3): `SessionParts::prepare`,
//! `SessionParts::on_complete`, panic containment and the finalizer order.
//! Red by design: the stubs ignore `prepare` and never call `on_complete`, so these fail on
//! assertions. Included from converse_runtime.rs. Do not edit to make code pass.
#![allow(dead_code)]

use super::*;
use crate::managers::converse::{
    BoxFuture, Brain, Ears, Hands, ListenFailure, ListenOutcome, Mouth, PhraseOutcome,
    QuietOutcome, SayOutcome, SessionConfig, SessionEnd,
};
use crate::managers::converse_capture::{
    CaptureTimings, CaptureWorker, RecorderPort, StartError, TranscribeFailure, Transcriber,
};
use crate::managers::converse_hooks;
use crate::managers::converse_plan::{PlanError, PlanInput, TurnPlan};
use crate::managers::emotion::Emotion;
use crate::managers::ownership::{Owner, OwnershipGate};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ---- fakes -------------------------------------------------------------------------------

struct HPort {
    active: AtomicBool,
}
impl RecorderPort for HPort {
    fn start(&self) -> Result<(), StartError> {
        self.active.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn stop(&self) -> Option<Vec<f32>> {
        self.active
            .swap(false, Ordering::SeqCst)
            .then(|| vec![0.0; 16])
    }
    fn cancel(&self) {
        self.active.store(false, Ordering::SeqCst);
    }
}

struct HTranscribe;
impl Transcriber for HTranscribe {
    fn transcribe(&self, _s: Vec<f32>) -> Result<String, TranscribeFailure> {
        Ok(String::new())
    }
}

/// Scripted listens; when the script is empty it either reports Aborted or waits forever
/// (so only `stop()` ends the session).
struct HEars {
    script: Mutex<VecDeque<ListenOutcome>>,
    pending_after: bool,
    listens: Arc<Mutex<Vec<Instant>>>,
}
impl Ears for HEars {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        self.listens.lock().unwrap().push(Instant::now());
        let next = self.script.lock().unwrap().pop_front();
        match next {
            Some(outcome) => Box::pin(async move { outcome }),
            None if self.pending_after => {
                let fut: BoxFuture<'_, ListenOutcome> = Box::pin(std::future::pending());
                fut
            }
            None => Box::pin(async { ListenOutcome::Aborted }),
        }
    }
}

struct HMouth {
    stuck: bool,
}
impl Mouth for HMouth {
    fn say<'a>(&'a self, _text: &'a str, _e: Emotion) -> BoxFuture<'a, SayOutcome> {
        Box::pin(async { SayOutcome::Done })
    }
    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        let quiet = if self.stuck {
            QuietOutcome::StillSpeaking
        } else {
            QuietOutcome::Quiet
        };
        Box::pin(async move { quiet })
    }
    fn stop(&self) {}
}

struct HHands;
impl Hands for HHands {
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

struct HBrain {
    err: Option<PlanError>,
}
impl Brain for HBrain {
    fn plan<'a>(&'a self, _input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        let err = self.err.unwrap_or(PlanError::Backend);
        Box::pin(async move { Err(err) })
    }
}

#[derive(Clone)]
enum PrepareMode {
    Off,
    /// Never completes.
    Stall,
    /// Completes after this many milliseconds.
    SleepMs(u64),
    Panic,
}

#[derive(Clone)]
struct Scenario {
    utterances: Vec<ListenOutcome>,
    pending_after: bool,
    brain_err: Option<PlanError>,
    stuck: bool,
    max_turns: usize,
    prepare: PrepareMode,
    panic_on_complete: bool,
    spawn_slow_complete: bool,
}

impl Scenario {
    fn base() -> Self {
        Scenario {
            utterances: vec![],
            pending_after: false,
            brain_err: None,
            stuck: false,
            max_turns: 50,
            prepare: PrepareMode::Off,
            panic_on_complete: false,
            spawn_slow_complete: false,
        }
    }

    /// The smallest scenario that ends the session with `end`.
    fn ending(end: SessionEnd) -> Self {
        match end {
            SessionEnd::UserEnded => Scenario {
                utterances: vec![ListenOutcome::Utterance("goodbye".to_string())],
                ..Scenario::base()
            },
            SessionEnd::Idle => Scenario {
                utterances: vec![ListenOutcome::NoSpeech, ListenOutcome::NoSpeech],
                ..Scenario::base()
            },
            SessionEnd::Cancelled => Scenario {
                pending_after: true,
                ..Scenario::base()
            },
            SessionEnd::NotConfigured => Scenario {
                utterances: vec![ListenOutcome::Utterance("what is rust".to_string())],
                brain_err: Some(PlanError::NotConfigured),
                ..Scenario::base()
            },
            SessionEnd::AudioUnavailable => Scenario {
                utterances: vec![ListenOutcome::Failed(ListenFailure::Other)],
                ..Scenario::base()
            },
            SessionEnd::SpeechStuck => Scenario {
                utterances: vec![ListenOutcome::Utterance("goodbye".to_string())],
                stuck: true,
                ..Scenario::base()
            },
            SessionEnd::TurnLimit => Scenario {
                max_turns: 0,
                ..Scenario::base()
            },
        }
    }
}

/// Fake platform. The scenario can be swapped between sessions on the same controller.
struct HPlatform {
    scenario: Mutex<Scenario>,
    gate: Arc<OwnershipGate>,
    port: Arc<HPort>,
    listens: Arc<Mutex<Vec<Instant>>>,
    /// Ordered finalizer log: `idle:...` from `on_state`, `complete:...` from `on_complete`.
    trace: Arc<Mutex<Vec<String>>>,
    completions: Arc<Mutex<Vec<SessionEnd>>>,
}

impl HPlatform {
    fn new(scenario: Scenario) -> Arc<Self> {
        Arc::new(HPlatform {
            scenario: Mutex::new(scenario),
            gate: OwnershipGate::new(),
            port: Arc::new(HPort {
                active: AtomicBool::new(false),
            }),
            listens: Arc::new(Mutex::new(vec![])),
            trace: Arc::new(Mutex::new(vec![])),
            completions: Arc::new(Mutex::new(vec![])),
        })
    }
}

impl Platform for HPlatform {
    fn preflight(&self) -> Result<(), String> {
        Ok(())
    }
    fn admit(&self) -> Result<Box<dyn AdmissionGuard>, String> {
        match self.gate.try_acquire(Owner::Conversation) {
            Ok(lease) => Ok(Box::new(lease)),
            Err(busy) => Err(format!("busy:{busy:?}")),
        }
    }
    fn capture(&self) -> CaptureHandle {
        CaptureWorker::spawn(
            self.port.clone(),
            Arc::new(HTranscribe),
            CaptureTimings {
                busy_retry: Duration::from_millis(10),
                busy_deadline: Duration::from_millis(100),
                readiness_deadline: Duration::from_secs(30),
                abs_slack: Duration::from_secs(30),
                tick: Duration::from_millis(5),
            },
        )
    }
    fn parts(&self, _capture: &CaptureHandle) -> SessionParts {
        let s = self.scenario.lock().unwrap().clone();
        let prepare: Option<BoxFuture<'static, ()>> = match s.prepare {
            PrepareMode::Off => None,
            PrepareMode::Stall => Some(Box::pin(std::future::pending::<()>())),
            PrepareMode::SleepMs(ms) => Some(Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(ms)).await;
            })),
            PrepareMode::Panic => Some(Box::pin(async {
                panic!("prepare exploded");
            })),
        };
        let trace = self.trace.clone();
        let completions = self.completions.clone();
        let slow = s.spawn_slow_complete;
        let panic_now = s.panic_on_complete;
        SessionParts {
            ears: Box::new(HEars {
                script: Mutex::new(VecDeque::from(s.utterances.clone())),
                pending_after: s.pending_after,
                listens: self.listens.clone(),
            }),
            mouth: Box::new(HMouth { stuck: s.stuck }),
            hands: Box::new(HHands),
            brain: Box::new(HBrain { err: s.brain_err }),
            cfg: SessionConfig {
                post_speech_guard: Duration::from_millis(0),
                quiet_wait: Duration::from_millis(5),
                max_turns: s.max_turns,
                ..SessionConfig::default()
            },
            transcript: None,
            policy: None,
            announcer: None,
            prepare,
            on_complete: Some(Box::new(move |end: SessionEnd| {
                trace
                    .lock()
                    .unwrap()
                    .push(format!("complete:{}", end_label(end)));
                completions.lock().unwrap().push(end);
                if slow {
                    // The callback itself only spawns: a slow task must not hold the finalizer.
                    std::thread::spawn(|| std::thread::sleep(Duration::from_millis(300)));
                }
                if panic_now {
                    panic!("complete exploded");
                }
            })),
        }
    }
    fn on_state(&self, state: &'static str, _end: Option<&'static str>) {
        if state == "idle" {
            self.trace.lock().unwrap().push(format!(
                "idle:session={} port={}",
                converse_hooks::session_active(),
                self.port.active.load(Ordering::SeqCst)
            ));
        }
    }
}

// ---- helpers -----------------------------------------------------------------------------

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    converse_hooks::TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn reset_hooks() {
    converse_hooks::install_capture(None);
    converse_hooks::set_session_active(false);
}

fn drain(c: &ConverseController) {
    wait_until("session to drain", || !c.is_active());
}

// ---- T6 tests ----------------------------------------------------------------------------

#[test]
fn start_returns_immediately_and_listening_waits_while_prepare_stalls() {
    let _g = lock();
    reset_hooks();
    let p = HPlatform::new(Scenario {
        prepare: PrepareMode::Stall,
        ..Scenario::ending(SessionEnd::Cancelled)
    });
    let c = ConverseController::new(p.clone());
    let t = Instant::now();
    c.start().expect("start must succeed");
    assert!(
        t.elapsed() < Duration::from_millis(500),
        "start must return at once, took {:?}",
        t.elapsed()
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        p.listens.lock().unwrap().is_empty(),
        "listening must not start while prepare is pending"
    );
    assert!(c.stop());
    drain(&c);
    reset_hooks();
}

#[test]
fn cancel_during_prepare_ends_cancelled_releases_admission_and_calls_on_complete_once() {
    let _g = lock();
    reset_hooks();
    let p = HPlatform::new(Scenario {
        prepare: PrepareMode::Stall,
        ..Scenario::ending(SessionEnd::Cancelled)
    });
    let c = ConverseController::new(p.clone());
    c.start().expect("start");
    std::thread::sleep(Duration::from_millis(100));
    assert!(c.stop());
    drain(&c);
    assert_eq!(
        *p.completions.lock().unwrap(),
        vec![SessionEnd::Cancelled],
        "cancel during prepare must end Cancelled and call on_complete once"
    );
    assert_eq!(p.gate.current(), None, "admission must be released");
    assert!(!converse_hooks::session_active());

    // A following session starts and completes normally.
    *p.scenario.lock().unwrap() = Scenario::ending(SessionEnd::UserEnded);
    c.start().expect("next session starts");
    drain(&c);
    let done = p.completions.lock().unwrap().clone();
    assert_eq!(done.len(), 2, "one more on_complete: {done:?}");
    assert_eq!(done[1], SessionEnd::UserEnded);
    reset_hooks();
}

#[test]
fn a_prepare_that_stalls_past_the_two_second_bound_proceeds_without_error() {
    let _g = lock();
    reset_hooks();
    let p = HPlatform::new(Scenario {
        prepare: PrepareMode::Stall,
        ..Scenario::ending(SessionEnd::UserEnded)
    });
    let c = ConverseController::new(p.clone());
    let t = Instant::now();
    c.start().expect("start");
    wait_until("the session to listen after the bound", || {
        !p.listens.lock().unwrap().is_empty()
    });
    assert!(
        t.elapsed() >= Duration::from_millis(1800),
        "listening began before the 2 s prepare bound: {:?}",
        t.elapsed()
    );
    drain(&c);
    assert_eq!(*p.completions.lock().unwrap(), vec![SessionEnd::UserEnded]);
    reset_hooks();
}

#[test]
fn a_prepare_that_finishes_early_lets_the_session_listen_after_it() {
    let _g = lock();
    reset_hooks();
    let p = HPlatform::new(Scenario {
        prepare: PrepareMode::SleepMs(500),
        ..Scenario::ending(SessionEnd::UserEnded)
    });
    let c = ConverseController::new(p.clone());
    let t = Instant::now();
    c.start().expect("start");
    wait_until("the session to listen", || {
        !p.listens.lock().unwrap().is_empty()
    });
    assert!(
        t.elapsed() >= Duration::from_millis(400),
        "listening began before prepare finished: {:?}",
        t.elapsed()
    );
    drain(&c);
    assert_eq!(*p.completions.lock().unwrap(), vec![SessionEnd::UserEnded]);
    reset_hooks();
}

#[test]
fn on_complete_is_called_exactly_once_for_every_session_end() {
    let _g = lock();
    reset_hooks();
    let p = HPlatform::new(Scenario::ending(SessionEnd::UserEnded));
    let c = ConverseController::new(p.clone());
    let cases = [
        SessionEnd::UserEnded,
        SessionEnd::Idle,
        SessionEnd::Cancelled,
        SessionEnd::NotConfigured,
        SessionEnd::AudioUnavailable,
        SessionEnd::SpeechStuck,
        SessionEnd::TurnLimit,
    ];
    for (i, end) in cases.into_iter().enumerate() {
        *p.scenario.lock().unwrap() = Scenario::ending(end);
        c.start().expect("start");
        if end == SessionEnd::Cancelled {
            std::thread::sleep(Duration::from_millis(50));
            assert!(c.stop());
        }
        drain(&c);
        let got = p.completions.lock().unwrap().clone();
        assert_eq!(got.len(), i + 1, "after {end:?}: completions {got:?}");
        assert_eq!(got[i], end, "on_complete must receive the real end");
    }
    reset_hooks();
}

#[test]
fn on_complete_runs_after_capture_shutdown_the_final_idle_and_registry_finish() {
    let _g = lock();
    reset_hooks();
    let p = HPlatform::new(Scenario::ending(SessionEnd::UserEnded));
    let c = ConverseController::new(p.clone());
    c.start().expect("start");
    drain(&c);
    assert_eq!(
        *p.trace.lock().unwrap(),
        vec![
            "idle:session=false port=false".to_string(),
            "complete:user_ended".to_string(),
        ],
        "idle must follow capture shutdown with hooks reset, then on_complete"
    );
    reset_hooks();
}

#[test]
fn a_panicking_on_complete_does_not_break_finalization_or_the_next_session() {
    let _g = lock();
    reset_hooks();
    let p = HPlatform::new(Scenario {
        panic_on_complete: true,
        ..Scenario::ending(SessionEnd::UserEnded)
    });
    let c = ConverseController::new(p.clone());
    c.start().expect("start");
    drain(&c);
    assert_eq!(
        *p.completions.lock().unwrap(),
        vec![SessionEnd::UserEnded],
        "on_complete must have been called"
    );
    let idles = p
        .trace
        .lock()
        .unwrap()
        .iter()
        .filter(|t| t.starts_with("idle:"))
        .count();
    assert_eq!(idles, 1, "exactly one idle event");
    assert!(!converse_hooks::session_active(), "hooks must be reset");
    assert!(
        !converse_hooks::capture_active(),
        "capture must be released"
    );
    assert_eq!(p.gate.current(), None, "ownership must be released");

    *p.scenario.lock().unwrap() = Scenario::ending(SessionEnd::UserEnded);
    c.start().expect("next session starts");
    drain(&c);
    let done = p.completions.lock().unwrap().clone();
    assert_eq!(done.len(), 2, "next session completes: {done:?}");
    reset_hooks();
}

#[test]
fn a_panicking_prepare_ends_cancelled_through_the_normal_finalizer() {
    let _g = lock();
    reset_hooks();
    let p = HPlatform::new(Scenario {
        prepare: PrepareMode::Panic,
        ..Scenario::ending(SessionEnd::UserEnded)
    });
    let c = ConverseController::new(p.clone());
    c.start().expect("start");
    drain(&c);
    assert_eq!(
        *p.completions.lock().unwrap(),
        vec![SessionEnd::Cancelled],
        "a panic in prepare maps to Cancelled and on_complete is called once"
    );
    let idles = p
        .trace
        .lock()
        .unwrap()
        .iter()
        .filter(|t| t.starts_with("idle:"))
        .count();
    assert_eq!(idles, 1, "exactly one idle event");
    assert!(!converse_hooks::session_active());
    assert!(!converse_hooks::capture_active());
    assert_eq!(p.gate.current(), None, "ownership must be released");

    *p.scenario.lock().unwrap() = Scenario::ending(SessionEnd::UserEnded);
    c.start().expect("next session starts");
    drain(&c);
    assert_eq!(p.completions.lock().unwrap().len(), 2);
    reset_hooks();
}

#[test]
fn a_slow_spawned_completion_task_does_not_delay_registry_or_admission_release() {
    let _g = lock();
    reset_hooks();
    let p = HPlatform::new(Scenario {
        spawn_slow_complete: true,
        ..Scenario::ending(SessionEnd::UserEnded)
    });
    let c = ConverseController::new(p.clone());
    c.start().expect("start");
    let t = Instant::now();
    drain(&c);
    assert!(
        t.elapsed() < Duration::from_millis(250),
        "a detached 300 ms task must not delay the finalizer: {:?}",
        t.elapsed()
    );
    assert_eq!(
        p.gate.current(),
        None,
        "admission must be released promptly"
    );
    assert_eq!(*p.completions.lock().unwrap(), vec![SessionEnd::UserEnded]);
    reset_hooks();
}
