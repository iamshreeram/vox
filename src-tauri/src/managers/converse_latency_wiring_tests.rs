//! Authoritative T9b tests for the latency wiring (design D4, A7, G9): `LatencySink`,
//! `make_latency_sink`, `ForwardReporter::latency`, the text-leak guard and the
//! production-shaped loop scenario. Included from converse_runtime.rs (child module).
//! Do not edit to make code pass.

use super::*;
use crate::managers::converse::{
    run_session, BoxFuture, Brain, CancelToken, Deps, Ears, Hands, ListenOutcome, Mouth,
    PhraseOutcome, QuietOutcome, Reporter, SayOutcome, SessionConfig, SessionEnd, SessionState,
};
use crate::managers::converse_latency::{
    EndpointMode, LatencyEvent, LatencyReport, LatencyStats, ListenTiming, RollingStats,
};
use crate::managers::converse_plan::{PlanError, PlanInput, PlanKind, TurnPlan};
use crate::managers::emotion::Emotion;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::watch;

const SECRET: &str = "zebra-quartz-742";

fn report_for(turn: u32, endpoint_ms: u32) -> LatencyReport {
    let timing = ListenTiming {
        endpoint_wait_ms: Some(endpoint_ms),
        transcribe_ms: Some(50),
        verify_ms: None,
        mode: EndpointMode::Fixed,
    };
    LatencyReport::build(turn, Some(&timing), Some(120), Some(7))
}

fn stats_for(reports: &[LatencyReport]) -> LatencyStats {
    let mut rolling = RollingStats::new();
    for r in reports {
        rolling.push(r);
    }
    rolling.stats()
}

/// Collects every emitted payload.
#[derive(Clone, Default)]
struct Emitted(Arc<Mutex<Vec<LatencyEvent>>>);

impl Emitted {
    fn emit_fn(&self) -> Arc<dyn Fn(LatencyEvent) + Send + Sync> {
        let sink = self.0.clone();
        Arc::new(move |event| sink.lock().unwrap().push(event))
    }
    fn all(&self) -> Vec<LatencyEvent> {
        self.0.lock().unwrap().clone()
    }
}

/// Switchable panel setting, read per call.
#[derive(Clone)]
struct Panel(Arc<AtomicBool>);

impl Panel {
    fn new(on: bool) -> Self {
        Panel(Arc::new(AtomicBool::new(on)))
    }
    fn set(&self, on: bool) {
        self.0.store(on, Ordering::SeqCst);
    }
    fn fn_(&self) -> Arc<dyn Fn() -> bool + Send + Sync> {
        let flag = self.0.clone();
        Arc::new(move || flag.load(Ordering::SeqCst))
    }
}

// ---- ForwardReporter + sink ---------------------------------------------------------------

struct NullPlatform;

impl Platform for NullPlatform {
    fn preflight(&self) -> Result<(), String> {
        Ok(())
    }
    fn capture(&self) -> CaptureHandle {
        unreachable!("not used by these tests")
    }
    fn parts(&self, _capture: &CaptureHandle) -> SessionParts {
        unreachable!("not used by these tests")
    }
    fn on_state(&self, _state: &'static str, _end: Option<&'static str>) {}
}

#[test]
fn forward_reporter_without_a_sink_is_a_no_op() {
    let reporter = ForwardReporter {
        platform: Arc::new(NullPlatform),
        transcript: None,
        latency_sink: None,
    };
    // Must not panic and must not need any sink.
    reporter.latency(&report_for(1, 300), &stats_for(&[report_for(1, 300)]));
}

#[test]
fn forward_reporter_forwards_the_report_and_stats_to_its_sink() {
    let seen: Arc<Mutex<Vec<(LatencyReport, LatencyStats)>>> = Arc::default();
    let sink_seen = seen.clone();
    let sink: LatencySink = Arc::new(move |r, s| {
        sink_seen.lock().unwrap().push((r.clone(), s.clone()));
    });
    let reporter = ForwardReporter {
        platform: Arc::new(NullPlatform),
        transcript: None,
        latency_sink: Some(sink),
    };
    let report = report_for(3, 222);
    let stats = stats_for(std::slice::from_ref(&report));
    reporter.latency(&report, &stats);
    let got = seen.lock().unwrap().clone();
    assert_eq!(got, vec![(report, stats)]);
}

#[test]
fn sink_emits_when_the_panel_is_on() {
    let emitted = Emitted::default();
    let panel = Panel::new(true);
    let sink = make_latency_sink(emitted.emit_fn(), panel.fn_());
    let report = report_for(1, 300);
    let stats = stats_for(std::slice::from_ref(&report));
    sink(&report, &stats);
    assert_eq!(emitted.all().len(), 1);
}

#[test]
fn sink_is_silent_when_the_panel_is_off() {
    let emitted = Emitted::default();
    let panel = Panel::new(false);
    let sink = make_latency_sink(emitted.emit_fn(), panel.fn_());
    let report = report_for(1, 300);
    let stats = stats_for(std::slice::from_ref(&report));
    sink(&report, &stats);
    sink(&report, &stats);
    assert!(emitted.all().is_empty());
}

#[test]
fn toggling_the_panel_mid_session_takes_effect_on_the_next_report() {
    let emitted = Emitted::default();
    let panel = Panel::new(true);
    let sink = make_latency_sink(emitted.emit_fn(), panel.fn_());
    let r1 = report_for(1, 100);
    let r2 = report_for(2, 200);
    let r3 = report_for(3, 300);
    let stats = stats_for(&[r1.clone(), r2.clone(), r3.clone()]);
    sink(&r1, &stats);
    panel.set(false);
    sink(&r2, &stats);
    panel.set(true);
    sink(&r3, &stats);
    let turns: Vec<u32> = emitted.all().iter().map(|e| e.report.turn).collect();
    assert_eq!(turns, vec![1, 3], "the off-period report is not emitted");
}

#[test]
fn emitted_payload_equals_the_report_and_stats() {
    let emitted = Emitted::default();
    let panel = Panel::new(true);
    let sink = make_latency_sink(emitted.emit_fn(), panel.fn_());
    let report = report_for(4, 410);
    let stats = stats_for(std::slice::from_ref(&report));
    sink(&report, &stats);
    let got = emitted.all();
    assert_eq!(
        got,
        vec![LatencyEvent {
            report: report.clone(),
            stats: stats.clone(),
        }]
    );
}

/// Walks a JSON value and collects every string leaf.
fn string_leaves(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(items) => items.iter().for_each(|v| string_leaves(v, out)),
        serde_json::Value::Object(map) => map.values().for_each(|v| string_leaves(v, out)),
        _ => {}
    }
}

#[test]
fn payload_serializes_numbers_and_enum_tags_only() {
    let report = report_for(1, 300);
    let stats = stats_for(std::slice::from_ref(&report));
    let event = LatencyEvent { report, stats };
    let json = serde_json::to_value(&event).expect("serializes");
    let mut strings = Vec::new();
    string_leaves(&json, &mut strings);
    for s in &strings {
        assert!(
            ["fixed", "smart_turn", "smart_turn_fallback"].contains(&s.as_str()),
            "unexpected string in latency payload: {s:?}"
        );
    }
}

// ---- full loop through the sink: text never leaks ----------------------------------------

struct Ears1 {
    script: Mutex<VecDeque<ListenOutcome>>,
    pending: Mutex<Option<ListenTiming>>,
}

impl Ears for Ears1 {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        let next = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(ListenOutcome::Aborted);
        *self.pending.lock().unwrap() = Some(ListenTiming {
            endpoint_wait_ms: Some(250),
            transcribe_ms: Some(60),
            verify_ms: None,
            mode: EndpointMode::Fixed,
        });
        Box::pin(async move { next })
    }
    fn take_timing(&self) -> Option<ListenTiming> {
        self.pending.lock().unwrap().take()
    }
}

struct Mouth1;

impl Mouth for Mouth1 {
    fn say<'a>(&'a self, _text: &'a str, _emotion: Emotion) -> BoxFuture<'a, SayOutcome> {
        Box::pin(async { SayOutcome::Done })
    }
    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async { QuietOutcome::Quiet })
    }
    fn stop(&self) {}
}

struct Hands1;

impl Hands for Hands1 {
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

struct Brain1 {
    replies: Mutex<VecDeque<TurnPlan>>,
}

impl Brain for Brain1 {
    fn plan<'a>(&'a self, _input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        let next = self.replies.lock().unwrap().pop_front();
        Box::pin(async move { next.ok_or(PlanError::Backend) })
    }
}

fn answer_plan(speech: &str) -> TurnPlan {
    TurnPlan {
        plan: None,
        job: None,
        role: None,
        kind: PlanKind::Answer,
        speech: speech.to_string(),
        emotion: Emotion::Neutral,
        paste_text: None,
        command: None,
        agent: None,
    }
}

/// Records timing lines and forwards latency to a sink (the production path shape).
struct SinkReporter {
    sink: LatencySink,
    timing: Mutex<Vec<String>>,
}

impl Reporter for SinkReporter {
    fn state(&self, _state: SessionState) {}
    fn timing_line(&self, line: &str) {
        self.timing.lock().unwrap().push(line.to_string());
    }
    fn latency(&self, report: &LatencyReport, stats: &LatencyStats) {
        (self.sink)(report, stats);
    }
}

async fn run_loop(
    ears: &Ears1,
    brain: &Brain1,
    reporter: &SinkReporter,
    cfg: &SessionConfig,
) -> SessionEnd {
    let (tx, rx) = watch::channel(false);
    let _keep = tx;
    let mouth = Mouth1;
    let hands = Hands1;
    let deps = Deps {
        policy: None,
        announcer: None,
        ears,
        mouth: &mouth,
        hands: &hands,
        brain,
        reporter,
    };
    let token: CancelToken = rx.into();
    tokio::time::timeout(Duration::from_secs(5), run_session(&deps, cfg, token))
        .await
        .expect("run_session hung")
}

fn cfg_with_timing_log() -> SessionConfig {
    SessionConfig {
        post_speech_guard: Duration::from_millis(0),
        quiet_wait: Duration::from_millis(5),
        timing_log: true,
        ..SessionConfig::default()
    }
}

#[tokio::test]
async fn full_turn_text_never_reaches_the_payload_or_the_timing_lines() {
    let emitted = Emitted::default();
    let panel = Panel::new(true);
    let sink = make_latency_sink(emitted.emit_fn(), panel.fn_());
    let reporter = SinkReporter {
        sink,
        timing: Mutex::new(Vec::new()),
    };
    let ears = Ears1 {
        script: Mutex::new(
            vec![
                ListenOutcome::Utterance(format!("tell me about {SECRET}")),
                ListenOutcome::Aborted,
            ]
            .into(),
        ),
        pending: Mutex::new(None),
    };
    let brain = Brain1 {
        replies: Mutex::new(vec![answer_plan(&format!("You said {SECRET}, noted."))].into()),
    };
    let _ = run_loop(&ears, &brain, &reporter, &cfg_with_timing_log()).await;

    let events = emitted.all();
    assert_eq!(events.len(), 1, "one completed turn, one payload");
    let json = serde_json::to_string(&events[0]).expect("serializes");
    assert!(!json.contains(SECRET), "payload leaked the text: {json}");
    assert!(!json.contains("zebra"), "payload leaked the text: {json}");
    assert!(!json.contains("quartz"), "payload leaked the text: {json}");
    let lines = reporter.timing.lock().unwrap().clone();
    assert!(
        !lines.is_empty(),
        "timing_log is on, so timing lines were written"
    );
    for line in &lines {
        assert!(
            !line.contains("zebra") && !line.contains("quartz"),
            "timing line leaked: {line}"
        );
        assert!(!line.contains(SECRET), "timing line leaked: {line}");
    }
}

#[tokio::test]
async fn production_shaped_scenario_yields_two_correlated_reports() {
    // NoSpeech, Ignored, Utterance, TooLong, Utterance: two completed turns, numbered 1 and 2.
    let emitted = Emitted::default();
    let panel = Panel::new(true);
    let sink = make_latency_sink(emitted.emit_fn(), panel.fn_());
    let reporter = SinkReporter {
        sink,
        timing: Mutex::new(Vec::new()),
    };
    let ears = Ears1 {
        script: Mutex::new(
            vec![
                ListenOutcome::NoSpeech,
                ListenOutcome::Ignored,
                ListenOutcome::Utterance("first".into()),
                ListenOutcome::TooLong,
                ListenOutcome::Utterance("second".into()),
                ListenOutcome::Aborted,
            ]
            .into(),
        ),
        pending: Mutex::new(None),
    };
    let brain = Brain1 {
        replies: Mutex::new(vec![answer_plan("one"), answer_plan("two")].into()),
    };
    let _ = run_loop(&ears, &brain, &reporter, &SessionConfig::default()).await;

    let events = emitted.all();
    let turns: Vec<u32> = events.iter().map(|e| e.report.turn).collect();
    assert_eq!(turns, vec![1, 2]);
    let samples: Vec<u32> = events.iter().map(|e| e.stats.samples).collect();
    assert_eq!(samples, vec![1, 2]);
    assert_eq!(events[1].stats.samples, 2);
    assert_eq!(events[1].stats.endpoint_wait.median, Some(250));
    assert_eq!(events[1].stats.endpoint_wait.p90, Some(250));
}
