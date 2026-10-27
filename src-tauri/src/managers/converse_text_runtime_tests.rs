//! TYPING SPEC (DESIGN-TYPE Rev 1, written by ram-ai; workers must not edit this file).
//! Controller-level behaviour of the text chat with a fake platform. Child module of
//! `converse_runtime` (`use super::*`). Contract used here:
//!   ConverseController::send_typed(&self, raw: &str) -> Result<(), TypedError>
//!   ConverseController::start_text(&self) -> Result<u64, String>
//!   ConverseController::typed_inbox(&self) -> Arc<TypedInbox>
//!   Platform::preflight_text(&self) -> Result<(), String>          (default: self.preflight())
//!   Platform::parts_text(&self, &CaptureHandle) -> SessionParts    (default: self.parts(..))
//!   pub const TEXT_ENDING: &str  (returned by a voice `start()` while a text session runs)

use super::*;
use crate::managers::chat_log::ChatRole;
use crate::managers::converse::{BoxFuture, Hands, Mouth, PhraseOutcome, QuietOutcome, SayOutcome};
use crate::managers::converse_capture::{
    CaptureTimings, CaptureWorker, RecorderPort, StartError, TranscribeFailure, Transcriber,
};
use crate::managers::converse_hooks;
use crate::managers::converse_plan::{PlanError, PlanInput, TurnPlan};
use crate::managers::converse_text::{PendingEars, TypedError};
use crate::managers::emotion::Emotion;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct CountingPort {
    starts: AtomicUsize,
}
impl RecorderPort for CountingPort {
    fn start(&self) -> Result<(), StartError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
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

struct TextPlatform {
    admit_err: Mutex<Option<String>>,
    preflight: Mutex<Result<(), String>>,
    text_preflight: Mutex<Result<(), String>>,
    port: Arc<CountingPort>,
    events: Mutex<Vec<(&'static str, Option<&'static str>)>>,
    chat: Mutex<Vec<(ChatRole, String)>>,
    voice_parts: AtomicUsize,
    text_parts: AtomicUsize,
    completes: Arc<AtomicUsize>,
}
impl TextPlatform {
    fn new() -> Arc<Self> {
        Arc::new(TextPlatform {
            admit_err: Mutex::new(None),
            preflight: Mutex::new(Ok(())),
            text_preflight: Mutex::new(Ok(())),
            port: Arc::new(CountingPort {
                starts: AtomicUsize::new(0),
            }),
            events: Mutex::new(vec![]),
            chat: Mutex::new(vec![]),
            voice_parts: AtomicUsize::new(0),
            text_parts: AtomicUsize::new(0),
            completes: Arc::new(AtomicUsize::new(0)),
        })
    }
    fn session_parts(&self) -> SessionParts {
        SessionParts {
            // The inner ears never hear anything: typed text is the only input.
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
            // Stands for the transcript save: it must not run for a session that saw typing.
            on_complete: Some({
                let done = self.completes.clone();
                Box::new(move |_end| {
                    done.fetch_add(1, Ordering::SeqCst);
                })
            }),
        }
    }
    fn user_lines(&self) -> Vec<String> {
        self.chat
            .lock()
            .unwrap()
            .iter()
            .filter(|(r, _)| *r == ChatRole::User)
            .map(|(_, t)| t.clone())
            .collect()
    }
}
impl Platform for TextPlatform {
    fn admit(&self) -> Result<Box<dyn AdmissionGuard>, String> {
        match self.admit_err.lock().unwrap().clone() {
            Some(e) => Err(e),
            None => Ok(Box::new(NoopGuard)),
        }
    }
    fn preflight(&self) -> Result<(), String> {
        self.preflight.lock().unwrap().clone()
    }
    fn preflight_text(&self) -> Result<(), String> {
        self.text_preflight.lock().unwrap().clone()
    }
    fn capture(&self) -> CaptureHandle {
        CaptureWorker::spawn(
            self.port.clone(),
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
        self.voice_parts.fetch_add(1, Ordering::SeqCst);
        self.session_parts()
    }
    fn parts_text(&self, _capture: &CaptureHandle) -> SessionParts {
        self.text_parts.fetch_add(1, Ordering::SeqCst);
        self.session_parts()
    }
    fn on_state(&self, state: &'static str, end: Option<&'static str>) {
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

fn controller(p: &Arc<TextPlatform>) -> ConverseController {
    ConverseController::new(p.clone())
}

#[test]
fn typing_on_an_idle_controller_starts_a_text_session_and_the_line_reaches_the_loop() {
    let _g = lock();
    reset_hooks();
    let p = TextPlatform::new();
    let c = controller(&p);
    c.send_typed("hello there").expect("accepted");
    wait_until("user line recorded", || {
        p.user_lines() == vec!["hello there".to_string()]
    });
    assert_eq!(c.status(), ConverseStatus::Running);
    assert_eq!(p.text_parts.load(Ordering::SeqCst), 1);
    assert_eq!(
        p.voice_parts.load(Ordering::SeqCst),
        0,
        "voice parts unused"
    );
    c.stop();
    wait_until("idle", || c.status() == ConverseStatus::Idle);
    reset_hooks();
}

#[test]
fn a_text_session_never_opens_the_microphone_or_shows_listening() {
    let _g = lock();
    reset_hooks();
    let p = TextPlatform::new();
    let c = controller(&p);
    c.send_typed("anything").unwrap();
    wait_until("user line", || !p.user_lines().is_empty());
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(p.port.starts.load(Ordering::SeqCst), 0, "mic never armed");
    let events = p.events.lock().unwrap().clone();
    assert!(
        events.iter().all(|(s, _)| *s != "listening"),
        "text mode must not emit the listening state: {events:?}"
    );
    c.stop();
    wait_until("idle", || c.status() == ConverseStatus::Idle);
    reset_hooks();
}

#[test]
fn typing_bye_ends_the_text_session_as_user_ended_and_drops_leftovers() {
    let _g = lock();
    reset_hooks();
    let p = TextPlatform::new();
    let c = controller(&p);
    c.send_typed("bye").unwrap();
    wait_until("session finished", || c.status() == ConverseStatus::Idle);
    wait_until("final idle event", || {
        p.events
            .lock()
            .unwrap()
            .contains(&("idle", Some("user_ended")))
    });
    assert_eq!(c.typed_inbox().len(), 0);
    reset_hooks();
}

#[test]
fn stop_unblocks_a_text_session_that_is_waiting_for_typing() {
    let _g = lock();
    reset_hooks();
    let p = TextPlatform::new();
    let c = controller(&p);
    c.start_text().expect("text session starts");
    wait_until("running", || c.status() == ConverseStatus::Running);
    assert!(c.stop());
    wait_until("idle after stop", || c.status() == ConverseStatus::Idle);
    assert!(p
        .events
        .lock()
        .unwrap()
        .contains(&("idle", Some("cancelled"))));
    reset_hooks();
}

#[test]
fn a_failed_text_preflight_is_reported_as_not_configured_and_starts_nothing() {
    let _g = lock();
    reset_hooks();
    let p = TextPlatform::new();
    *p.text_preflight.lock().unwrap() = Err("no reply model".into());
    let c = controller(&p);
    assert_eq!(c.send_typed("hello"), Err(TypedError::NotConfigured));
    assert_eq!(c.status(), ConverseStatus::Idle);
    assert_eq!(p.text_parts.load(Ordering::SeqCst), 0);
    assert_eq!(
        c.typed_inbox().len(),
        0,
        "nothing stays queued after a failed start"
    );
    reset_hooks();
}

#[test]
fn empty_and_oversized_text_is_rejected_without_starting_a_session() {
    let _g = lock();
    reset_hooks();
    let p = TextPlatform::new();
    let c = controller(&p);
    assert_eq!(c.send_typed("   "), Err(TypedError::Empty));
    let big = "x".repeat(crate::managers::converse_text::MAX_TYPED_CHARS + 1);
    assert_eq!(c.send_typed(&big), Err(TypedError::TooLong));
    assert_eq!(c.status(), ConverseStatus::Idle);
    assert_eq!(p.text_parts.load(Ordering::SeqCst), 0);
    reset_hooks();
}

#[test]
fn typing_into_a_running_voice_session_queues_through_the_same_loop() {
    let _g = lock();
    reset_hooks();
    let p = TextPlatform::new();
    let c = controller(&p);
    c.start().expect("voice session starts");
    wait_until("running", || c.status() == ConverseStatus::Running);
    c.send_typed("typed during voice").expect("queued");
    wait_until("user line", || {
        p.user_lines() == vec!["typed during voice".to_string()]
    });
    assert_eq!(p.voice_parts.load(Ordering::SeqCst), 1);
    assert_eq!(p.text_parts.load(Ordering::SeqCst), 0, "no second session");
    c.stop();
    wait_until("idle", || c.status() == ConverseStatus::Idle);
    reset_hooks();
}

#[test]
fn a_voice_start_while_a_text_session_runs_ends_it_and_reports_why() {
    let _g = lock();
    reset_hooks();
    let p = TextPlatform::new();
    let c = controller(&p);
    c.start_text().unwrap();
    wait_until("running", || c.status() == ConverseStatus::Running);
    let err = c
        .start()
        .expect_err("voice cannot start over a text session");
    assert_eq!(err, TEXT_ENDING);
    wait_until("text session ended", || c.status() == ConverseStatus::Idle);
    // the next voice start works
    c.start().expect("voice start after the text session ended");
    c.stop();
    wait_until("idle", || c.status() == ConverseStatus::Idle);
    reset_hooks();
}

#[test]
fn many_threads_typing_at_once_start_exactly_one_session_and_deliver_every_accepted_message() {
    let _g = lock();
    reset_hooks();
    let p = TextPlatform::new();
    let c = Arc::new(controller(&p));
    let mut handles = vec![];
    for i in 0..20 {
        let c = c.clone();
        handles.push(std::thread::spawn(move || {
            c.send_typed(&format!("msg {i}"))
        }));
    }
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let accepted = results.iter().filter(|r| r.is_ok()).count();
    assert!(accepted >= 1, "someone got through");
    for r in &results {
        assert!(
            matches!(r, Ok(()) | Err(TypedError::Busy)),
            "only Ok or Busy are acceptable, got {r:?}"
        );
    }
    assert_eq!(p.text_parts.load(Ordering::SeqCst), 1, "one session only");
    // exactly-once delivery of everything that was accepted (no stop was requested)
    wait_until("every accepted message reached the loop", || {
        p.user_lines().len() == accepted
    });
    let mut lines = p.user_lines();
    lines.sort();
    lines.dedup();
    assert_eq!(lines.len(), accepted, "no duplicates");
    c.stop();
    wait_until("idle", || c.status() == ConverseStatus::Idle);
    reset_hooks();
}

#[test]
fn an_admission_conflict_is_busy_not_not_configured() {
    let _g = lock();
    reset_hooks();
    let p = TextPlatform::new();
    *p.admit_err.lock().unwrap() = Some("microphone is in use".into());
    let c = controller(&p);
    assert_eq!(c.send_typed("hello"), Err(TypedError::Busy));
    assert_eq!(c.status(), ConverseStatus::Idle);
    assert_eq!(p.text_parts.load(Ordering::SeqCst), 0);
    assert_eq!(c.typed_inbox().len(), 0);
    reset_hooks();
}

#[test]
fn typing_right_after_stop_is_busy_or_starts_a_fresh_session_and_never_leaks() {
    let _g = lock();
    reset_hooks();
    let p = TextPlatform::new();
    let c = controller(&p);
    c.start_text().unwrap();
    wait_until("running", || c.status() == ConverseStatus::Running);
    assert!(c.stop());
    // Between stop() and the finalizer the controller is Stopping (Busy); if the finalizer
    // already finished it is Idle and the message legitimately starts a new text session.
    match c.send_typed("too late") {
        Ok(()) => {
            assert!(c.stop(), "the freshly started session must be stoppable");
        }
        Err(e) => assert_eq!(e, TypedError::Busy),
    }
    wait_until("idle", || c.status() == ConverseStatus::Idle);
    assert_eq!(
        c.typed_inbox().len(),
        0,
        "nothing survives a finished session"
    );
    reset_hooks();
}

#[test]
fn a_session_that_saw_typing_never_runs_the_persistence_callback() {
    let _g = lock();
    reset_hooks();
    let p = TextPlatform::new();
    let c = controller(&p);
    c.start().expect("voice session");
    wait_until("running", || c.status() == ConverseStatus::Running);
    c.send_typed("private sentence").unwrap();
    wait_until("line delivered", || !p.user_lines().is_empty());
    c.stop();
    wait_until("idle", || c.status() == ConverseStatus::Idle);
    std::thread::sleep(Duration::from_millis(300)); // on_complete runs after the final idle
    assert_eq!(
        p.completes.load(Ordering::SeqCst),
        0,
        "a session with typed turns must not reach the transcript save"
    );
    reset_hooks();
}

#[test]
fn a_voice_session_without_typing_still_runs_the_persistence_callback() {
    let _g = lock();
    reset_hooks();
    let p = TextPlatform::new();
    let c = controller(&p);
    c.start().expect("voice session");
    wait_until("running", || c.status() == ConverseStatus::Running);
    c.stop();
    wait_until("idle", || c.status() == ConverseStatus::Idle);
    wait_until("on_complete ran once", || {
        p.completes.load(Ordering::SeqCst) == 1
    });
    reset_hooks();
}

#[test]
fn the_inbox_is_cleared_when_a_session_ends() {
    let _g = lock();
    reset_hooks();
    let p = TextPlatform::new();
    let c = controller(&p);
    c.start_text().unwrap();
    wait_until("running", || c.status() == ConverseStatus::Running);
    c.typed_inbox().push("stale".into()).ok();
    c.stop();
    wait_until("idle", || c.status() == ConverseStatus::Idle);
    assert_eq!(c.typed_inbox().len(), 0);
    reset_hooks();
}

#[test]
fn a_stale_sender_bound_to_the_previous_session_cannot_feed_the_next_one() {
    let _g = lock();
    reset_hooks();
    let p = TextPlatform::new();
    let c = controller(&p);
    c.start_text().unwrap();
    wait_until("running", || c.status() == ConverseStatus::Running);
    let session_a = c.typed_inbox().generation().expect("open while running");
    c.stop();
    wait_until("idle", || c.status() == ConverseStatus::Idle);
    c.start_text().unwrap(); // session B
    wait_until("running again", || c.status() == ConverseStatus::Running);
    assert_eq!(
        c.typed_inbox().push_for(session_a, "yes".into()),
        Err(TypedError::Busy)
    );
    std::thread::sleep(Duration::from_millis(100));
    assert!(p.user_lines().is_empty(), "B never saw the stale 'yes'");
    c.stop();
    wait_until("idle", || c.status() == ConverseStatus::Idle);
    reset_hooks();
}
