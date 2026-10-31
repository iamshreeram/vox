//! T1a chat Stop (DESIGN Rev 4, B1/B2b). Child module of `converse_runtime` (`use super::*`).
//! Compare-and-stop on the displayed session id through `ConverseController::stop_if`, with a
//! fake platform (same shape as the text-runtime fakes; those fakes are private to their module).

use super::*;
use crate::managers::chat_log::ChatRole;
use crate::managers::converse::{BoxFuture, Hands, Mouth, PhraseOutcome, QuietOutcome, SayOutcome};
use crate::managers::converse_capture::{
    CaptureTimings, CaptureWorker, RecorderPort, StartError, TranscribeFailure, Transcriber,
};
use crate::managers::converse_hooks;
use crate::managers::converse_plan::{PlanError, PlanInput, TurnPlan};
use crate::managers::converse_text::PendingEars;
use crate::managers::emotion::Emotion;
use crate::managers::ownership::{Owner, OwnershipGate};
use std::time::{Duration, Instant};

struct SilentPort;
impl RecorderPort for SilentPort {
    fn start(&self) -> Result<(), StartError> {
        Ok(())
    }
    fn stop(&self) -> Option<Vec<f32>> {
        None
    }
    fn cancel(&self) {}
}
struct NoTranscribe;
impl Transcriber for NoTranscribe {
    fn transcribe(&self, _s: Vec<f32>) -> Result<String, TranscribeFailure> {
        Ok(String::new())
    }
}
struct QuietMouth;
impl Mouth for QuietMouth {
    fn say<'a>(&'a self, _t: &'a str, _e: Emotion) -> BoxFuture<'a, SayOutcome> {
        Box::pin(async { SayOutcome::Done })
    }
    fn wait_quiet(&self, _m: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async { QuietOutcome::Quiet })
    }
    fn stop(&self) {}
}
struct NoHands;
impl Hands for NoHands {
    fn paste_generated(&self, _t: &str) -> Result<(), String> {
        Ok(())
    }
    fn run_user_phrase<'a>(&'a self, _p: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        Box::pin(async { PhraseOutcome::NoMatch })
    }
    fn run_model_command<'a>(&'a self, _p: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        Box::pin(async { PhraseOutcome::NoMatch })
    }
}
struct NoBrain;
impl Brain for NoBrain {
    fn plan<'a>(&'a self, _i: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        Box::pin(async { Err(PlanError::Backend) })
    }
}

struct StopPlatform {
    events: Mutex<Vec<(&'static str, Option<&'static str>)>>,
    chat: Mutex<Vec<(ChatRole, String)>>,
}
impl StopPlatform {
    fn new() -> Arc<Self> {
        Arc::new(StopPlatform {
            events: Mutex::new(vec![]),
            chat: Mutex::new(vec![]),
        })
    }
    fn parts(&self) -> SessionParts {
        SessionParts {
            ears: Box::new(PendingEars),
            mouth: Box::new(QuietMouth),
            hands: Box::new(NoHands),
            brain: Box::new(NoBrain),
            cfg: SessionConfig {
                post_speech_guard: Duration::from_millis(0),
                quiet_wait: Duration::from_millis(5),
                ..SessionConfig::default()
            },
            transcript: None,
            prepare: None,
            policy: None,
            announcer: None,
            on_complete: None,
        }
    }
    fn ended(&self, end: &'static str) -> bool {
        self.events.lock().unwrap().contains(&("idle", Some(end)))
    }
}
impl Platform for StopPlatform {
    fn admit(&self) -> Result<Box<dyn AdmissionGuard>, String> {
        Ok(Box::new(NoopGuard))
    }
    fn preflight(&self) -> Result<(), String> {
        Ok(())
    }
    fn preflight_text(&self) -> Result<(), String> {
        Ok(())
    }
    fn capture(&self) -> CaptureHandle {
        CaptureWorker::spawn(
            Arc::new(SilentPort),
            Arc::new(NoTranscribe),
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
        self.parts()
    }
    fn parts_text(&self, _capture: &CaptureHandle) -> SessionParts {
        self.parts()
    }
    fn on_state(&self, state: &'static str, end: Option<&'static str>) {
        self.events.lock().unwrap().push((state, end));
    }
    fn on_state_text(&self, state: &'static str, end: Option<&'static str>) {
        self.events.lock().unwrap().push((state, end));
    }
    fn on_chat(&self, role: ChatRole, text: &str) {
        self.chat.lock().unwrap().push((role, text.to_string()));
    }
}

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

fn controller(p: &Arc<StopPlatform>) -> ConverseController {
    ConverseController::new(p.clone())
}

/// True once the registry itself reports Running (`status()` also reports Starting as Running).
fn registry_running(c: &ConverseController) -> bool {
    matches!(c.registry.state(), RegistryState::Running(_))
}

/// Starts a text session and waits until the registry reports Running; returns its displayed id.
fn start_running_text(c: &ConverseController) -> SessionId {
    c.start_text().expect("text session starts");
    wait_until("registry running", || registry_running(c));
    SessionId(c.active_session_id().expect("an active id while running"))
}

#[test]
fn stop_if_the_active_session_stops_it_and_the_id_is_reported_only_while_active() {
    let _g = lock();
    reset_hooks();
    let p = StopPlatform::new();
    let c = controller(&p);
    assert_eq!(c.active_session_id(), None, "idle reports no session");
    let a = start_running_text(&c);
    assert!(c.stop_if(a), "stop of the active session is accepted");
    wait_until("idle", || c.status() == ConverseStatus::Idle);
    wait_until("cancelled end", || p.ended("cancelled"));
    assert_eq!(c.active_session_id(), None, "ended session reports no id");
    reset_hooks();
}

#[test]
fn a_delayed_stop_for_an_ended_session_does_not_touch_the_next_one() {
    let _g = lock();
    reset_hooks();
    let p = StopPlatform::new();
    let c = controller(&p);
    let a = start_running_text(&c);
    assert!(c.stop_if(a));
    wait_until("idle", || c.status() == ConverseStatus::Idle);
    let b = start_running_text(&c);
    assert_ne!(a, b, "a new session gets a new id");
    assert!(!c.stop_if(a), "a delayed stop for A must not stop B");
    assert!(registry_running(&c), "B keeps running");
    assert!(c.stop_if(b));
    wait_until("idle", || c.status() == ConverseStatus::Idle);
    reset_hooks();
}

#[test]
fn duplicate_presses_second_is_a_no_op() {
    let _g = lock();
    reset_hooks();
    let p = StopPlatform::new();
    let c = controller(&p);
    let a = start_running_text(&c);
    assert!(c.stop_if(a));
    assert!(!c.stop_if(a), "second press for the same session is false");
    wait_until("idle", || c.status() == ConverseStatus::Idle);
    assert!(!c.stop_if(a), "press after the end is false");
    reset_hooks();
}

#[test]
fn stop_on_an_idle_controller_is_false() {
    let _g = lock();
    reset_hooks();
    let p = StopPlatform::new();
    let c = controller(&p);
    assert!(!c.stop_if(SessionId(1)));
    assert!(!c.stop_if(SessionId(0)));
    assert_eq!(c.status(), ConverseStatus::Idle);
    reset_hooks();
}

#[test]
fn voice_and_text_sessions_are_both_stoppable() {
    let _g = lock();
    reset_hooks();
    let p = StopPlatform::new();
    let c = controller(&p);
    c.start().expect("voice session starts");
    wait_until("voice registry running", || registry_running(&c));
    let voice = SessionId(c.active_session_id().unwrap());
    assert!(c.stop_if(voice), "voice session stops by id");
    wait_until("idle after voice", || c.status() == ConverseStatus::Idle);
    let text = start_running_text(&c);
    assert!(c.stop_if(text), "text session stops by id");
    wait_until("idle after text", || c.status() == ConverseStatus::Idle);
    // Settle: the finalizer releases the admission guard after the registry reports idle.
    std::thread::sleep(Duration::from_millis(50));
    reset_hooks();
}

#[test]
fn stop_leaves_an_unrelated_active_dictation_untouched() {
    let _g = lock();
    reset_hooks();
    let gate = OwnershipGate::new();
    let lease = gate
        .try_acquire(Owner::Dictation)
        .expect("dictation owns the gate");
    let p = StopPlatform::new();
    let c = controller(&p);
    let a = start_running_text(&c);
    assert!(c.stop_if(a));
    wait_until("idle", || c.status() == ConverseStatus::Idle);
    assert!(
        matches!(gate.current(), Some(Owner::Dictation)),
        "dictation ownership unchanged by the conversation stop"
    );
    drop(lease);
    reset_hooks();
}
