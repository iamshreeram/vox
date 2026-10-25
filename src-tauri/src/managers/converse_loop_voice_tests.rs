//! Authoritative T9b tests for the conversation loop (design D4, A7, A8, G8/G9 LOOP):
//! `ListenOutcome::Ignored` handling, `Ears::take_timing` calls and per-turn latency
//! reports. Included from converse.rs (child module). Do not edit to make code pass.
//!
//! Clock note: `tokio` has no `test-util` feature in this crate, so paused time is not
//! available. Brain latency is bounded with real sleeps; endpoint/transcribe/verify come
//! from the scripted `take_timing` and are asserted exactly.

use super::*;
use crate::managers::converse_latency::{EndpointMode, LatencyReport, LatencyStats, ListenTiming};
use crate::managers::converse_plan::{PlanKind, TurnPlan};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::watch;

type Shared = Arc<Mutex<Vec<String>>>;

fn push(log: &Shared, entry: impl Into<String>) {
    log.lock().unwrap().push(entry.into());
}

fn entries(log: &Shared) -> Vec<String> {
    log.lock().unwrap().clone()
}

/// The log without the `take` entries (for the flag-off identity check).
fn without_take(log: &Shared) -> Vec<String> {
    entries(log).into_iter().filter(|e| e != "take").collect()
}

fn says(log: &Shared) -> Vec<String> {
    entries(log)
        .into_iter()
        .filter_map(|e| e.strip_prefix("say:").map(str::to_string))
        .collect()
}

fn timing(
    endpoint_wait_ms: Option<u32>,
    transcribe_ms: Option<u32>,
    verify_ms: Option<u32>,
    mode: EndpointMode,
) -> ListenTiming {
    ListenTiming {
        endpoint_wait_ms,
        transcribe_ms,
        verify_ms,
        mode,
    }
}

// ---- fakes -------------------------------------------------------------------------------

/// One scripted listen: the outcome, the timing `take_timing` returns for it, and whether the
/// session is cancelled while `listen` is in flight.
#[derive(Clone)]
struct Step {
    outcome: ListenOutcome,
    timing: Option<ListenTiming>,
    cancel_during: bool,
}

fn step(outcome: ListenOutcome) -> Step {
    Step {
        outcome,
        timing: None,
        cancel_during: false,
    }
}

fn timed(outcome: ListenOutcome, timing: ListenTiming) -> Step {
    Step {
        outcome,
        timing: Some(timing),
        cancel_during: false,
    }
}

fn utt(text: &str) -> Step {
    step(ListenOutcome::Utterance(text.to_string()))
}

fn ignored() -> Step {
    step(ListenOutcome::Ignored)
}

fn no_speech() -> Step {
    step(ListenOutcome::NoSpeech)
}

fn too_long() -> Step {
    step(ListenOutcome::TooLong)
}

fn aborted() -> Step {
    step(ListenOutcome::Aborted)
}

struct FakeEars {
    script: Mutex<VecDeque<Step>>,
    pending: Mutex<Option<ListenTiming>>,
    log: Shared,
    cancel: Arc<watch::Sender<bool>>,
}

impl Ears for FakeEars {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        push(&self.log, "listen");
        let next = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(aborted);
        *self.pending.lock().unwrap() = next.timing;
        if next.cancel_during {
            self.cancel.send_replace(true);
        }
        let outcome = next.outcome;
        Box::pin(async move { outcome })
    }

    fn take_timing(&self) -> Option<ListenTiming> {
        push(&self.log, "take");
        self.pending.lock().unwrap().take()
    }
}

struct FakeMouth {
    log: Shared,
}

impl Mouth for FakeMouth {
    fn say<'a>(&'a self, text: &'a str, _emotion: Emotion) -> BoxFuture<'a, SayOutcome> {
        push(&self.log, format!("say:{text}"));
        Box::pin(async { SayOutcome::Done })
    }
    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async { QuietOutcome::Quiet })
    }
    fn stop(&self) {
        push(&self.log, "stop");
    }
}

struct FakeHands;

impl Hands for FakeHands {
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

/// Scripted replies; each `plan` sleeps `delay` (real time) before its reply is ready.
struct FakeBrain {
    replies: Mutex<VecDeque<Result<TurnPlan, PlanError>>>,
    delay: Duration,
    log: Shared,
    calls: AtomicUsize,
}

impl Brain for FakeBrain {
    fn plan<'a>(&'a self, input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        push(&self.log, format!("plan:{}", input.utterance));
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(PlanError::Backend));
        let delay = self.delay;
        Box::pin(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            reply
        })
    }
}

#[derive(Default)]
struct RecReporter {
    states: Mutex<Vec<SessionState>>,
    timing: Mutex<Vec<String>>,
    latency: Mutex<Vec<(LatencyReport, LatencyStats)>>,
}

impl Reporter for RecReporter {
    fn state(&self, state: SessionState) {
        self.states.lock().unwrap().push(state);
    }
    fn timing_line(&self, line: &str) {
        self.timing.lock().unwrap().push(line.to_string());
    }
    fn latency(&self, report: &LatencyReport, stats: &LatencyStats) {
        self.latency
            .lock()
            .unwrap()
            .push((report.clone(), stats.clone()));
    }
}

struct Rig {
    log: Shared,
    cancel: Arc<watch::Sender<bool>>,
    ears: FakeEars,
    mouth: FakeMouth,
    hands: FakeHands,
    brain: FakeBrain,
    reporter: RecReporter,
}

fn rig(steps: Vec<Step>, replies: Vec<Result<TurnPlan, PlanError>>, delay: Duration) -> Rig {
    let log: Shared = Arc::new(Mutex::new(Vec::new()));
    let (tx, _rx) = watch::channel(false);
    let cancel = Arc::new(tx);
    Rig {
        ears: FakeEars {
            script: Mutex::new(steps.into()),
            pending: Mutex::new(None),
            log: log.clone(),
            cancel: cancel.clone(),
        },
        mouth: FakeMouth { log: log.clone() },
        hands: FakeHands,
        brain: FakeBrain {
            replies: Mutex::new(replies.into()),
            delay,
            log: log.clone(),
            calls: AtomicUsize::new(0),
        },
        reporter: RecReporter::default(),
        log,
        cancel,
    }
}

fn cfg() -> SessionConfig {
    SessionConfig {
        post_speech_guard: Duration::from_millis(0),
        quiet_wait: Duration::from_millis(5),
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
    tokio::time::timeout(Duration::from_secs(5), run_session(&deps, cfg, token))
        .await
        .expect("run_session hung")
}

fn answer(speech: &str) -> Result<TurnPlan, PlanError> {
    Ok(TurnPlan {
        plan: None,
        job: None,
        role: None,
        kind: PlanKind::Answer,
        speech: speech.to_string(),
        emotion: Emotion::Neutral,
        paste_text: None,
        command: None,
        agent: None,
    })
}

fn brain_calls(r: &Rig) -> usize {
    r.brain.calls.load(Ordering::SeqCst)
}

// ---- Ignored: no idle count, no brain, no speech ------------------------------------------

#[tokio::test]
async fn ignored_listens_again_without_planning_or_speaking() {
    let r = rig(
        vec![ignored(), utt("a"), aborted()],
        vec![answer("ok")],
        Duration::ZERO,
    );
    let end = run(&r, &cfg()).await;
    assert_eq!(end, SessionEnd::Cancelled);
    assert_eq!(brain_calls(&r), 1, "only the real utterance is planned");
    assert_eq!(says(&r.log), vec!["ok".to_string()]);
}

#[tokio::test]
async fn ten_continuous_ignored_end_idle_with_no_brain_calls() {
    let steps = (0..10).map(|_| ignored()).collect();
    let r = rig(steps, vec![], Duration::ZERO);
    let end = run(&r, &cfg()).await;
    assert_eq!(end, SessionEnd::Idle);
    assert_eq!(brain_calls(&r), 0);
    assert_eq!(says(&r.log), vec![MSG_STOP_LISTENING.to_string()]);
    let listens = entries(&r.log).iter().filter(|e| *e == "listen").count();
    assert_eq!(
        listens, 10,
        "the session must stop at the 10th ignored listen"
    );
}

#[tokio::test]
async fn fifth_consecutive_ignored_counts_as_one_idle_turn() {
    let mut c = cfg();
    c.max_idle_turns = 1;
    let steps = (0..5).map(|_| ignored()).collect();
    let r = rig(steps, vec![], Duration::ZERO);
    let end = run(&r, &c).await;
    assert_eq!(end, SessionEnd::Idle);
    assert_eq!(brain_calls(&r), 0);
    assert_eq!(says(&r.log), vec![MSG_STOP_LISTENING.to_string()]);
}

#[tokio::test]
async fn four_ignored_do_not_count_as_idle_even_when_one_idle_ends_the_session() {
    let mut c = cfg();
    c.max_idle_turns = 1;
    let r = rig(
        vec![ignored(), ignored(), ignored(), ignored(), aborted()],
        vec![],
        Duration::ZERO,
    );
    let end = run(&r, &c).await;
    assert_eq!(end, SessionEnd::Cancelled);
    assert!(says(&r.log).is_empty(), "four ignored listens say nothing");
}

#[tokio::test]
async fn utterance_after_four_ignored_resets_the_ignored_counter() {
    let mut c = cfg();
    c.max_idle_turns = 1;
    let mut steps: Vec<Step> = (0..4).map(|_| ignored()).collect();
    steps.push(utt("hello"));
    steps.extend((0..4).map(|_| ignored()));
    steps.push(aborted());
    let r = rig(steps, vec![answer("hi")], Duration::ZERO);
    let end = run(&r, &c).await;
    assert_eq!(
        end,
        SessionEnd::Cancelled,
        "a fresh run of four ignored must not reach the fifth"
    );
    assert_eq!(brain_calls(&r), 1);
}

#[tokio::test]
async fn no_speech_resets_the_ignored_counter_and_counts_as_idle() {
    let mut c = cfg();
    c.max_idle_turns = 2;
    let mut steps: Vec<Step> = (0..4).map(|_| ignored()).collect();
    steps.push(no_speech());
    steps.extend((0..4).map(|_| ignored()));
    steps.push(aborted());
    let r = rig(steps, vec![], Duration::ZERO);
    let end = run(&r, &c).await;
    assert_eq!(
        end,
        SessionEnd::Cancelled,
        "one NoSpeech (idle 1) then four ignored must not end the session"
    );
    assert!(says(&r.log).is_empty());
}

#[tokio::test]
async fn two_no_speech_after_ignored_end_idle_normally() {
    let mut c = cfg();
    c.max_idle_turns = 2;
    let mut steps: Vec<Step> = (0..4).map(|_| ignored()).collect();
    steps.push(no_speech());
    steps.push(no_speech());
    let r = rig(steps, vec![], Duration::ZERO);
    let end = run(&r, &c).await;
    assert_eq!(end, SessionEnd::Idle);
    assert_eq!(brain_calls(&r), 0);
    assert_eq!(says(&r.log), vec![MSG_STOP_LISTENING.to_string()]);
}

#[tokio::test]
async fn ignored_with_cancel_set_ends_cancelled() {
    let mut cancelling = ignored();
    cancelling.cancel_during = true;
    let r = rig(
        vec![cancelling, utt("never")],
        vec![answer("no")],
        Duration::ZERO,
    );
    let end = run(&r, &cfg()).await;
    assert_eq!(end, SessionEnd::Cancelled);
    assert_eq!(brain_calls(&r), 0);
    assert!(says(&r.log).is_empty());
}

#[tokio::test]
async fn ignored_listens_do_not_consume_turn_budget() {
    // Reading of max_turns: it counts completed loop turns that reach a listen outcome
    // other than Ignored. Three utterances fit in max_turns = 3; the Ignored listen before
    // them is free, and the fourth utterance is never heard (TurnLimit before listening).
    let mut c = cfg();
    c.max_turns = 3;
    let r = rig(
        vec![ignored(), utt("one"), utt("two"), utt("three"), utt("four")],
        vec![answer("1"), answer("2"), answer("3")],
        Duration::ZERO,
    );
    let end = run(&r, &c).await;
    assert_eq!(end, SessionEnd::TurnLimit);
    assert_eq!(brain_calls(&r), 3);
    let listens = entries(&r.log).iter().filter(|e| *e == "listen").count();
    assert_eq!(listens, 4, "one ignored listen plus three utterances");
}

#[tokio::test]
async fn failed_listen_after_ignored_is_unchanged() {
    let r = rig(
        vec![ignored(), step(ListenOutcome::Failed(ListenFailure::Busy))],
        vec![],
        Duration::ZERO,
    );
    let end = run(&r, &cfg()).await;
    assert_eq!(end, SessionEnd::AudioUnavailable);
    assert_eq!(says(&r.log), vec![MSG_MIC.to_string()]);
    assert_eq!(brain_calls(&r), 0);
}

/// Flag-off identity: with ears that never return Ignored, the exact sequence of fake calls
/// is the existing one (this must already be green).
#[tokio::test]
async fn without_ignored_the_call_sequence_is_unchanged() {
    let r = rig(
        vec![no_speech(), utt("a"), aborted()],
        vec![answer("ok")],
        Duration::ZERO,
    );
    let end = run(&r, &cfg()).await;
    assert_eq!(end, SessionEnd::Cancelled);
    assert_eq!(
        without_take(&r.log),
        vec!["listen", "listen", "plan:a", "say:ok", "listen", "stop"]
    );
}

// ---- take_timing: exactly once after every listen ------------------------------------------

#[tokio::test]
async fn take_timing_follows_every_listen_exactly_once() {
    let r = rig(
        vec![no_speech(), ignored(), too_long(), utt("a"), aborted()],
        vec![answer("ok")],
        Duration::ZERO,
    );
    let _ = run(&r, &cfg()).await;
    let calls: Vec<String> = entries(&r.log)
        .into_iter()
        .filter(|e| e == "listen" || e == "take")
        .collect();
    let expected: Vec<String> = (0..5)
        .flat_map(|_| ["listen".to_string(), "take".to_string()])
        .collect();
    assert_eq!(calls, expected);
}

#[tokio::test]
async fn take_timing_runs_for_failed_and_aborted_listens() {
    let r = rig(
        vec![step(ListenOutcome::Failed(ListenFailure::Busy))],
        vec![],
        Duration::ZERO,
    );
    let _ = run(&r, &cfg()).await;
    let calls: Vec<String> = entries(&r.log)
        .into_iter()
        .filter(|e| e == "listen" || e == "take")
        .collect();
    assert_eq!(calls, vec!["listen", "take"]);
}

// ---- latency: one report per completed turn, per-session stats ---------------------------

#[tokio::test]
async fn completed_turn_builds_one_report_from_listen_and_plan_timing() {
    let r = rig(
        vec![
            timed(
                ListenOutcome::Utterance("a".into()),
                timing(Some(300), Some(120), Some(40), EndpointMode::SmartTurn),
            ),
            aborted(),
        ],
        vec![answer("reply")],
        Duration::from_millis(200),
    );
    let _ = run(&r, &cfg()).await;
    let reports = r.reporter.latency.lock().unwrap().clone();
    assert_eq!(
        reports.len(),
        1,
        "exactly one report for the one completed turn"
    );
    let (report, stats) = &reports[0];
    assert_eq!(report.turn, 1);
    assert_eq!(report.endpoint_wait_ms, Some(300));
    assert_eq!(report.transcribe_ms, Some(120));
    assert_eq!(report.verify_ms, Some(40));
    assert_eq!(report.mode, EndpointMode::SmartTurn);
    // brain_ms = plan request -> plan ready; the fake sleeps 200 ms inside plan().
    let brain = report.brain_ms.expect("brain_ms present");
    assert!((200..2000).contains(&brain), "brain_ms={brain}");
    // to_speech_ms = plan ready -> speech handed to the voice; the mouth is instant.
    let to_speech = report.to_speech_ms.expect("to_speech_ms present");
    assert!(
        to_speech < 100,
        "to_speech_ms={to_speech} must not include the brain delay"
    );
    assert_eq!(
        report.total_ms,
        Some(300 + 120 + 40 + brain + to_speech),
        "total = endpoint + transcribe + verify + brain + to_speech"
    );
    assert_eq!(stats.samples, 1);
    assert_eq!(stats.endpoint_wait.median, Some(300));
    assert_eq!(stats.brain.median, Some(brain));
}

#[tokio::test]
async fn missing_listen_timing_gives_none_fields_and_fixed_mode() {
    let r = rig(
        vec![utt("a"), aborted()],
        vec![answer("ok")],
        Duration::ZERO,
    );
    let _ = run(&r, &cfg()).await;
    let reports = r.reporter.latency.lock().unwrap().clone();
    assert_eq!(reports.len(), 1);
    let report = &reports[0].0;
    assert_eq!(report.endpoint_wait_ms, None);
    assert_eq!(report.transcribe_ms, None);
    assert_eq!(report.verify_ms, None);
    assert_eq!(report.mode, EndpointMode::Fixed);
    assert_eq!(report.total_ms, None);
    assert!(
        report.brain_ms.is_some(),
        "loop-measured brain_ms is still present"
    );
    assert!(
        report.to_speech_ms.is_some(),
        "loop-measured to_speech_ms is still present"
    );
}

#[tokio::test]
async fn no_report_for_no_speech_ignored_too_long_or_aborted() {
    let r = rig(
        vec![no_speech(), ignored(), too_long(), aborted()],
        vec![],
        Duration::ZERO,
    );
    let end = run(&r, &cfg()).await;
    assert_eq!(end, SessionEnd::Cancelled);
    assert!(r.reporter.latency.lock().unwrap().is_empty());
    assert_eq!(brain_calls(&r), 0);
}

#[tokio::test]
async fn no_report_for_a_listen_failure() {
    let r = rig(
        vec![
            no_speech(),
            ignored(),
            too_long(),
            step(ListenOutcome::Failed(ListenFailure::Busy)),
        ],
        vec![],
        Duration::ZERO,
    );
    let end = run(&r, &cfg()).await;
    assert_eq!(end, SessionEnd::AudioUnavailable);
    assert!(r.reporter.latency.lock().unwrap().is_empty());
}

#[tokio::test]
async fn no_report_when_the_plan_fails() {
    let r = rig(
        vec![utt("a"), aborted()],
        vec![Err(PlanError::Backend)],
        Duration::ZERO,
    );
    let _ = run(&r, &cfg()).await;
    assert!(
        r.reporter.latency.lock().unwrap().is_empty(),
        "a turn whose plan failed is not a completed turn"
    );
}

#[tokio::test]
async fn turn_numbers_count_completed_turns_only() {
    let r = rig(
        vec![
            no_speech(),
            utt("a"),
            ignored(),
            utt("b"),
            too_long(),
            utt("c"),
            aborted(),
        ],
        vec![answer("1"), answer("2"), answer("3")],
        Duration::ZERO,
    );
    let _ = run(&r, &cfg()).await;
    let reports = r.reporter.latency.lock().unwrap().clone();
    let turns: Vec<u32> = reports.iter().map(|(rep, _)| rep.turn).collect();
    let samples: Vec<u32> = reports.iter().map(|(_, s)| s.samples).collect();
    assert_eq!(turns, vec![1, 2, 3]);
    assert_eq!(
        samples,
        vec![1, 2, 3],
        "one call per completed turn, never twice"
    );
}

#[tokio::test]
async fn rolling_stats_start_empty_in_each_session() {
    let r = rig(vec![utt("a"), aborted()], vec![answer("1")], Duration::ZERO);
    let _ = run(&r, &cfg()).await;
    // Second session on the same reporter: the same ears/brain, new script.
    r.ears
        .script
        .lock()
        .unwrap()
        .extend(vec![utt("b"), aborted()]);
    r.brain.replies.lock().unwrap().push_back(answer("2"));
    let _ = run(&r, &cfg()).await;
    let reports = r.reporter.latency.lock().unwrap().clone();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[1].0.turn, 1, "turn numbering restarts per session");
    assert_eq!(reports[1].1.samples, 1, "stats restart per session");
}
