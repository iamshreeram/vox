//! S13 latency sink attachment (T9b): `ConverseController::start` hands the platform's
//! `latency_sink()` to the session reporter, so each completed turn reaches the sink once.
//! Included from converse_runtime.rs as a child module. Scripted session, no audio device.

use super::*;
use crate::managers::converse::{
    BoxFuture, Brain, Ears, Hands, ListenOutcome, Mouth, PhraseOutcome, QuietOutcome, SayOutcome,
    SessionConfig,
};
use crate::managers::converse_capture::{
    CaptureHandle, CaptureTimings, CaptureWorker, RecorderPort, StartError, TranscribeFailure,
    Transcriber,
};
use crate::managers::converse_hooks;
use crate::managers::converse_latency::{LatencyReport, LatencyStats};
use crate::managers::converse_plan::{PlanError, PlanInput, PlanKind, TurnPlan};
use crate::managers::emotion::Emotion;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

type Reports = Arc<Mutex<Vec<(LatencyReport, LatencyStats)>>>;

fn lock_hooks() -> std::sync::MutexGuard<'static, ()> {
    converse_hooks::TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn reset_hooks() {
    converse_hooks::install_capture(None);
    converse_hooks::set_session_active(false);
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

/// A sink that records every (report, stats) pair it receives.
fn recording_sink(seen: &Reports) -> LatencySink {
    let seen = Arc::clone(seen);
    Arc::new(move |report, stats| {
        seen.lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((report.clone(), stats.clone()));
    })
}

fn received(seen: &Reports) -> Vec<(LatencyReport, LatencyStats)> {
    seen.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Starts one session and waits until the controller is idle again.
fn run_one_session(c: &ConverseController) {
    c.start().expect("session starts");
    wait_until("the session to finish", || !c.is_active());
}

// ----------------------------- fakes -----------------------------

struct IdlePort {
    active: AtomicBool,
}

impl RecorderPort for IdlePort {
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

struct NoTranscribe;

impl Transcriber for NoTranscribe {
    fn transcribe(&self, _s: Vec<f32>) -> Result<String, TranscribeFailure> {
        Ok(String::new())
    }
}

fn idle_port() -> Arc<IdlePort> {
    Arc::new(IdlePort {
        active: AtomicBool::new(false),
    })
}

fn spawn_capture(port: &Arc<IdlePort>) -> CaptureHandle {
    CaptureWorker::spawn(
        port.clone(),
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

/// Hands the session one utterance, then aborts the listen loop (ends the session).
struct OneTurnEars(Mutex<VecDeque<ListenOutcome>>);

impl Ears for OneTurnEars {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        let next = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
            .unwrap_or(ListenOutcome::Aborted);
        Box::pin(async move { next })
    }
}

struct QuietMouth;

impl Mouth for QuietMouth {
    fn say<'a>(&'a self, _text: &'a str, _emotion: Emotion) -> BoxFuture<'a, SayOutcome> {
        Box::pin(async { SayOutcome::Done })
    }
    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async { QuietOutcome::Quiet })
    }
    fn stop(&self) {}
}

struct NoHands;

impl Hands for NoHands {
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

struct OneReplyBrain(Mutex<VecDeque<TurnPlan>>);

impl Brain for OneReplyBrain {
    fn plan<'a>(&'a self, _input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        let next = self.0.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
        Box::pin(async move { next.ok_or(PlanError::Backend) })
    }
}

fn answer_plan(speech: &str) -> TurnPlan {
    TurnPlan {
        job: None,
        kind: PlanKind::Answer,
        speech: speech.to_string(),
        emotion: Emotion::Neutral,
        paste_text: None,
        command: None,
    }
}

/// Parts for a session with exactly one completed turn.
fn one_turn_parts() -> SessionParts {
    SessionParts {
        ears: Box::new(OneTurnEars(Mutex::new(VecDeque::from(vec![
            ListenOutcome::Utterance("open the notes app".to_string()),
            ListenOutcome::Aborted,
        ])))),
        mouth: Box::new(QuietMouth),
        hands: Box::new(NoHands),
        brain: Box::new(OneReplyBrain(Mutex::new(VecDeque::from(vec![
            answer_plan("Done."),
        ])))),
        cfg: SessionConfig {
            post_speech_guard: Duration::from_millis(0),
            quiet_wait: Duration::from_millis(5),
            ..SessionConfig::default()
        },
        transcript: None,
        prepare: None,
        on_complete: None,
        policy: None,
        announcer: None,
    }
}

/// Platform that attaches a sink (the override under test).
struct SinkPlatform {
    sink: Option<LatencySink>,
    port: Arc<IdlePort>,
}

impl Platform for SinkPlatform {
    fn preflight(&self) -> Result<(), String> {
        Ok(())
    }
    fn capture(&self) -> CaptureHandle {
        spawn_capture(&self.port)
    }
    fn parts(&self, _capture: &CaptureHandle) -> SessionParts {
        one_turn_parts()
    }
    fn on_state(&self, _state: &'static str, _end: Option<&'static str>) {}
    fn latency_sink(&self) -> Option<LatencySink> {
        self.sink.clone()
    }
}

/// Platform that does not override `latency_sink` (the trait default).
struct DefaultSinkPlatform {
    port: Arc<IdlePort>,
    states: Mutex<Vec<&'static str>>,
}

impl Platform for DefaultSinkPlatform {
    fn preflight(&self) -> Result<(), String> {
        Ok(())
    }
    fn capture(&self) -> CaptureHandle {
        spawn_capture(&self.port)
    }
    fn parts(&self, _capture: &CaptureHandle) -> SessionParts {
        one_turn_parts()
    }
    fn on_state(&self, state: &'static str, _end: Option<&'static str>) {
        self.states
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(state);
    }
}

// ----------------------------- tests -----------------------------

#[test]
fn a_completed_turn_reaches_the_attached_sink_exactly_once() {
    let _g = lock_hooks();
    reset_hooks();
    let seen: Reports = Arc::default();
    let platform = Arc::new(SinkPlatform {
        sink: Some(recording_sink(&seen)),
        port: idle_port(),
    });
    let c = ConverseController::new(platform);
    run_one_session(&c);
    let got = received(&seen);
    assert_eq!(got.len(), 1, "one completed turn, one sink call");
    assert_eq!(got[0].0.turn, 1, "the first turn is numbered 1");
    assert_eq!(got[0].1.samples, 1, "the stats hold the one turn");
    reset_hooks();
}

#[test]
fn a_platform_with_the_default_sink_completes_the_session_without_a_sink() {
    let _g = lock_hooks();
    reset_hooks();
    let platform = Arc::new(DefaultSinkPlatform {
        port: idle_port(),
        states: Mutex::new(Vec::new()),
    });
    let c = ConverseController::new(platform.clone());
    run_one_session(&c);
    assert_eq!(c.status(), ConverseStatus::Idle);
    let states = platform
        .states
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    assert_eq!(
        states.last().copied(),
        Some("idle"),
        "the final idle is still emitted: {states:?}"
    );
    reset_hooks();
}

#[test]
fn each_session_starts_its_own_stats_so_the_second_session_is_turn_one_again() {
    let _g = lock_hooks();
    reset_hooks();
    let seen: Reports = Arc::default();
    let platform = Arc::new(SinkPlatform {
        sink: Some(recording_sink(&seen)),
        port: idle_port(),
    });
    let c = ConverseController::new(platform);
    run_one_session(&c);
    run_one_session(&c);
    let got = received(&seen);
    assert_eq!(got.len(), 2, "one completed turn per session");
    for (report, stats) in &got {
        assert_eq!(report.turn, 1, "turn numbering restarts per session");
        assert_eq!(
            stats.samples, 1,
            "the rolling stats start empty per session"
        );
    }
    reset_hooks();
}
