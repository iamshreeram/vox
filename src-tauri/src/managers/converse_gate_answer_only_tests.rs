//! RED tests for M3 S3c-1 (DESIGN Rev 5, D3): answer-only mode refuses a Job plan exactly
//! like a Paste plan (same message path, zero start_job calls, the model's own speech is
//! never spoken as the action). Phase A: fails at runtime until the behavior is implemented.
//! Included from converse.rs. Do not edit to make code pass.
#![allow(dead_code)]

use super::*;
use crate::managers::converse_confirm::{
    ActionPolicy, JobStartOutcome, LaunchConfig, PolicySource,
};
use crate::managers::converse_plan::{JobRequest, PlanError, PlanInput, PlanKind, TurnPlan};
use crate::managers::emotion::Emotion;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::watch;

fn job_plan(speech: &str) -> TurnPlan {
    TurnPlan {
        kind: PlanKind::Job,
        speech: speech.to_string(),
        emotion: Emotion::Neutral,
        paste_text: None,
        command: None,
        job: Some(JobRequest {
            project: "alpha".to_string(),
            task: "fix the tests".to_string(),
            agent: None,
            mode: crate::managers::converse_plan::JobMode::Task,
        }),
    }
}

fn paste_plan(speech: &str) -> TurnPlan {
    TurnPlan {
        kind: PlanKind::Paste,
        speech: speech.to_string(),
        emotion: Emotion::Neutral,
        paste_text: Some("model-written body".to_string()),
        command: None,
        job: None,
    }
}

/// Brain that returns one scripted plan and counts calls.
struct OnePlanBrain {
    plan: Mutex<Option<TurnPlan>>,
    calls: AtomicUsize,
}

impl Brain for OnePlanBrain {
    fn plan<'a>(&'a self, _input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let next = self.plan.lock().expect("lock").take();
        Box::pin(async move { next.ok_or(PlanError::Backend) })
    }
}

/// Hands that count start_job calls and fail every other effect loudly by counting.
#[derive(Default)]
struct CountingHands {
    starts: AtomicUsize,
    pastes: AtomicUsize,
}

impl Hands for CountingHands {
    fn paste_generated(&self, _text: &str) -> Result<(), String> {
        self.pastes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn run_user_phrase<'a>(&'a self, _phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        Box::pin(async { PhraseOutcome::NoMatch })
    }
    fn run_model_command<'a>(&'a self, _phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        Box::pin(async { PhraseOutcome::NoMatch })
    }
    fn start_job(&self, _launch: crate::managers::converse_confirm::JobLaunch) -> JobStartOutcome {
        self.starts.fetch_add(1, Ordering::SeqCst);
        JobStartOutcome::Started(1)
    }
}

/// Policy with jobs allowed and the gate on (so a Job is a gated plan).
struct JobsOnPolicy;

impl PolicySource for JobsOnPolicy {
    fn current(&self) -> ActionPolicy {
        ActionPolicy {
            gate_active: true,
            jobs_allowed: true,
        }
    }
    fn launch_config(&self) -> Option<LaunchConfig> {
        None
    }
}

/// Ears that give one utterance then abort.
struct OneUtterance(Mutex<VecDeque<ListenOutcome>>);

impl Ears for OneUtterance {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        let next = self
            .0
            .lock()
            .expect("lock")
            .pop_front()
            .unwrap_or(ListenOutcome::Aborted);
        Box::pin(async move { next })
    }
}

/// Mouth that records every spoken string.
#[derive(Default)]
struct RecMouth(Mutex<Vec<String>>);

impl Mouth for RecMouth {
    fn say<'a>(&'a self, text: &'a str, _e: Emotion) -> BoxFuture<'a, SayOutcome> {
        self.0.lock().expect("lock").push(text.to_string());
        Box::pin(async { SayOutcome::Done })
    }
    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async { QuietOutcome::Quiet })
    }
    fn stop(&self) {}
}

/// Reporter that discards events.
struct NoReport;

impl Reporter for NoReport {
    fn state(&self, _s: SessionState) {}
}

/// Runs one answer-only (or not) session over one utterance with the given plan.
async fn run_one(answer_only: bool, plan: TurnPlan) -> (CountingHands, Vec<String>) {
    let brain = OnePlanBrain {
        plan: Mutex::new(Some(plan)),
        calls: AtomicUsize::new(0),
    };
    let hands = CountingHands::default();
    let ears = OneUtterance(Mutex::new(VecDeque::from(vec![ListenOutcome::Utterance(
        "do it".to_string(),
    )])));
    let mouth = RecMouth::default();
    let policy = JobsOnPolicy;
    let announcer = crate::managers::job_announcer::Announcer::new(Arc::new(NoAnnounceMouth));
    let deps = Deps {
        ears: &ears,
        mouth: &mouth,
        hands: &hands,
        brain: &brain,
        reporter: &NoReport,
        policy: Some(&policy),
        announcer: Some(&announcer),
    };
    let cfg = SessionConfig {
        post_speech_guard: Duration::from_millis(0),
        quiet_wait: Duration::from_millis(5),
        answer_only,
        ..SessionConfig::default()
    };
    let (_tx, rx) = watch::channel(false);
    let _ = tokio::time::timeout(Duration::from_secs(10), run_session(&deps, &cfg, rx))
        .await
        .expect("run_session hung");
    let spoken = mouth.0.lock().expect("lock").clone();
    (hands, spoken)
}

struct NoAnnounceMouth;

impl crate::managers::job_announcer::LowPriorityMouth for NoAnnounceMouth {
    fn try_speak_if_idle(&self, _text: &str) -> bool {
        false
    }
}

#[tokio::test]
async fn answer_only_refuses_a_job_plan_with_zero_start_job_calls() {
    let (hands, spoken) = run_one(true, job_plan("Starting the job now.")).await;
    assert_eq!(
        hands.starts.load(Ordering::SeqCst),
        0,
        "answer-only mode must never start a coding job"
    );
    // The refusal is the answer-only (Paste) refusal, spoken exactly once.
    assert_eq!(
        spoken.len(),
        1,
        "exactly one fixed refusal must be spoken, got {spoken:?}"
    );
}

#[tokio::test]
async fn answer_only_does_not_speak_the_models_own_job_speech() {
    let (_hands, spoken) = run_one(true, job_plan("Starting the job now.")).await;
    assert!(
        !spoken.iter().any(|s| s.contains("Starting the job now.")),
        "the model's job speech must not be spoken in answer-only mode, got {spoken:?}"
    );
    assert!(
        !spoken
            .iter()
            .any(|s| s.contains("Coding jobs aren't set up")),
        "answer-only must not use the job-policy refusal, got {spoken:?}"
    );
}

#[tokio::test]
async fn answer_only_speaks_the_same_refusal_it_speaks_for_a_paste_plan() {
    let (_h1, paste_spoken) = run_one(true, paste_plan("Pasting it.")).await;
    let (_h2, job_spoken) = run_one(true, job_plan("Starting the job now.")).await;
    assert!(
        !paste_spoken.is_empty(),
        "the paste refusal must be spoken (baseline for this test)"
    );
    assert_eq!(
        job_spoken, paste_spoken,
        "a Job refusal must reuse the Paste refusal path and text"
    );
}

#[tokio::test]
async fn answer_only_paste_still_zero_pastes_so_the_fixture_is_meaningful() {
    let (hands, _spoken) = run_one(true, paste_plan("Pasting it.")).await;
    assert_eq!(hands.pastes.load(Ordering::SeqCst), 0);
}
