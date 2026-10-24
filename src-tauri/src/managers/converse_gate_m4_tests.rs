//! RED tests for M4 S1 D3 at the gate (gate_and_run end to end with fakes): contextual
//! approval through a settings change during the question, default-job invariance,
//! question-time fingerprint, and start_job reachable only through the yes answer.
//! Written before the implementation; they fail at runtime because the gate still
//! compares only the M3 fingerprint. Included from converse.rs as a child module.
//! Do not edit these tests to make code pass.

use super::*;
use crate::managers::converse_confirm::{
    ActionPolicy, FileId, JobLaunch, JobStartOutcome, LaunchConfig, PolicySource,
};
use crate::managers::converse_plan::{JobMode, JobRequest, PlanKind, TurnPlan};
use crate::managers::emotion::Emotion;
use crate::managers::job_announcer::{Announcer, LowPriorityMouth};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

// ------------------------------------------------------------------ shared log

#[derive(Default)]
struct Log(Mutex<Vec<String>>);

impl Log {
    fn push(&self, s: impl Into<String>) {
        self.0.lock().unwrap().push(s.into());
    }
    fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
    fn says(&self) -> Vec<String> {
        self.all()
            .iter()
            .filter_map(|e| e.strip_prefix("say:").map(str::to_string))
            .collect()
    }
    fn count_exact(&self, event: &str) -> usize {
        self.all().iter().filter(|e| e.as_str() == event).count()
    }
}

// ------------------------------------------------------------------ fakes

/// A settings change applied during the question (the user edits settings mid-question).
type Flip = fn(&M4GatePolicy);

/// Settings the gate reads. Shared so a flip can change them while the question is open.
struct M4GatePolicy {
    log: Arc<Log>,
    policy: Mutex<ActionPolicy>,
    launch: Mutex<Option<LaunchConfig>>,
    ask: Mutex<bool>,
    agents: Mutex<Vec<String>>,
    flag: Mutex<String>,
}

impl PolicySource for M4GatePolicy {
    fn current(&self) -> ActionPolicy {
        self.policy.lock().unwrap().clone()
    }
    fn launch_config(&self) -> Option<LaunchConfig> {
        self.launch.lock().unwrap().clone()
    }
    fn ask_allowed(&self) -> bool {
        self.log.push("policy:ask_allowed");
        *self.ask.lock().unwrap()
    }
    fn allowed_agents(&self) -> Vec<String> {
        self.log.push("policy:allowed_agents");
        self.agents.lock().unwrap().clone()
    }
    fn agent_flag(&self) -> String {
        self.log.push("policy:agent_flag");
        self.flag.lock().unwrap().clone()
    }
}

fn gate_policy(log: &Arc<Log>, agents: &[&str], ask: bool) -> Arc<M4GatePolicy> {
    Arc::new(M4GatePolicy {
        log: log.clone(),
        policy: Mutex::new(ActionPolicy {
            gate_active: true,
            jobs_allowed: true,
        }),
        launch: Mutex::new(Some(launch_config())),
        ask: Mutex::new(ask),
        agents: Mutex::new(agents.iter().map(|a| a.to_string()).collect()),
        flag: Mutex::new("--agent".to_string()),
    })
}

fn launch_config() -> LaunchConfig {
    LaunchConfig {
        binary: PathBuf::from("/opt/fake-cli/bin/fake-cli"),
        binary_id: FileId {
            dev: 1,
            ino: 2,
            len: 3,
            mtime_secs: 4,
        },
        prompt_flag: "--prompt".to_string(),
        agent_flag: None,
        timeout: Duration::from_secs(60),
    }
}

struct M4Hands {
    log: Arc<Log>,
    start_calls: AtomicUsize,
}

impl Hands for M4Hands {
    fn paste_generated(&self, _text: &str) -> Result<(), String> {
        Ok(())
    }
    fn run_user_phrase<'a>(&'a self, _phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        Box::pin(async { PhraseOutcome::NoMatch })
    }
    fn run_model_command<'a>(&'a self, _phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        Box::pin(async { PhraseOutcome::NoMatch })
    }
    fn pin_project(&self, _name: &str) -> Result<DirFd, PinError> {
        std::fs::File::open(std::env::temp_dir())
            .map(DirFd::from)
            .map_err(|_| PinError::Unavailable)
    }
    fn start_job(&self, _launch: JobLaunch) -> JobStartOutcome {
        self.start_calls.fetch_add(1, Ordering::SeqCst);
        self.log.push("start_job");
        JobStartOutcome::Started(1)
    }
}

struct M4Ears {
    log: Arc<Log>,
    script: Mutex<VecDeque<ListenOutcome>>,
    policy: Arc<M4GatePolicy>,
    flip: Mutex<Option<Flip>>,
}

impl Ears for M4Ears {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        self.log.push("listen");
        if let Some(flip) = self.flip.lock().unwrap().take() {
            flip(&self.policy);
        }
        let item = self.script.lock().unwrap().pop_front();
        Box::pin(async move { item.unwrap_or(ListenOutcome::Aborted) })
    }
}

struct M4Mouth {
    log: Arc<Log>,
}

impl Mouth for M4Mouth {
    fn say<'a>(&'a self, text: &'a str, _e: Emotion) -> BoxFuture<'a, SayOutcome> {
        self.log.push(format!("say:{text}"));
        Box::pin(async { SayOutcome::Done })
    }
    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async { QuietOutcome::Quiet })
    }
    fn stop(&self) {}
}

struct M4Brain {
    plan: TurnPlan,
}

impl Brain for M4Brain {
    fn plan<'a>(&'a self, _input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        let plan = self.plan.clone();
        Box::pin(async move { Ok(plan) })
    }
}

struct M4Reporter;

impl Reporter for M4Reporter {
    fn state(&self, _s: SessionState) {}
}

struct M4LowMouth {
    log: Arc<Log>,
}

impl LowPriorityMouth for M4LowMouth {
    fn try_speak_if_idle(&self, text: &str) -> bool {
        self.log.push(format!("announce:{text}"));
        true
    }
}

// ------------------------------------------------------------------ fns that flip settings

fn drop_all_agents(p: &M4GatePolicy) {
    p.agents.lock().unwrap().clear();
}

fn change_flag(p: &M4GatePolicy) {
    *p.flag.lock().unwrap() = "--who".to_string();
}

fn turn_ask_off(p: &M4GatePolicy) {
    *p.ask.lock().unwrap() = false;
}

fn add_unrelated_agent(p: &M4GatePolicy) {
    p.agents.lock().unwrap().push("other-bot".to_string());
}

// ------------------------------------------------------------------ plans and runner

fn job_plan(agent: Option<&str>, mode: JobMode) -> TurnPlan {
    TurnPlan {
        kind: PlanKind::Job,
        speech: "On it.".to_string(),
        emotion: Emotion::Neutral,
        paste_text: None,
        command: None,
        role: None,
        job: Some(JobRequest {
            project: "vox".to_string(),
            task: "fix the build".to_string(),
            agent: agent.map(str::to_string),
            mode,
        }),
    }
}

fn gate_cfg() -> SessionConfig {
    SessionConfig {
        post_speech_guard: Duration::from_millis(0),
        quiet_wait: Duration::from_millis(5),
        ..SessionConfig::default()
    }
}

const MSG_CHANGED: &str = "Settings changed, so I didn't start it.";

/// Runs one gated job plan through `gate_and_run`. `answers` are the listen outcomes.
async fn run_m4_gate(
    log: &Arc<Log>,
    plan: TurnPlan,
    pol: Arc<M4GatePolicy>,
    answers: Vec<ListenOutcome>,
    flip: Option<Flip>,
) -> Result<Option<String>, SessionEnd> {
    let ears = M4Ears {
        log: log.clone(),
        script: Mutex::new(answers.into()),
        policy: pol.clone(),
        flip: Mutex::new(flip),
    };
    let mouth = M4Mouth { log: log.clone() };
    let hands = M4Hands {
        log: log.clone(),
        start_calls: AtomicUsize::new(0),
    };
    let brain = M4Brain { plan: plan.clone() };
    let announcer = Announcer::new(Arc::new(M4LowMouth { log: log.clone() }));
    let (_tx, rx) = watch::channel(false);
    let mut cancel: CancelToken = rx.into();
    let deps = Deps {
        ears: &ears,
        mouth: &mouth,
        hands: &hands,
        brain: &brain,
        reporter: &M4Reporter,
        policy: Some(&*pol),
        announcer: Some(&announcer),
    };
    let mut carry = Carry::default();
    gate_and_run(&deps, &gate_cfg(), &mut cancel, plan, &mut carry).await
}

const QUESTION_DEFAULT: &str =
    "I'll start a coding job in vox. The task is: fix the build. Say yes or no.";
const QUESTION_WITH_AGENT: &str =
    "I'll start a coding job in vox with the coder-one agent. The task is: fix the build. Say yes or no.";
const QUESTION_ASK_WITH_AGENT: &str =
    "I'll ask the coder-one agent about vox: fix the build. It can change files. Say yes or no.";

// ------------------------------------------------------------------ D3 test 10

#[tokio::test]
async fn m4_gate_default_job_yes_starts_once_with_no_new_refusal() {
    let log = Arc::new(Log::default());
    let pol = gate_policy(&log, &["coder-one"], false);
    let outcome = run_m4_gate(
        &log,
        job_plan(None, JobMode::Task),
        pol,
        vec![ListenOutcome::Utterance("yes".to_string())],
        None,
    )
    .await;
    assert!(
        outcome.is_ok(),
        "approved default job must not end the session"
    );
    assert_eq!(
        log.says().first().map(String::as_str),
        Some(QUESTION_DEFAULT)
    );
    assert_eq!(log.count_exact("start_job"), 1, "start_job called once");
    assert!(
        !log.says().iter().any(|s| s == MSG_CHANGED),
        "no new refusal"
    );
}

#[tokio::test]
async fn m4_gate_selected_agent_removed_during_question_refuses_and_never_starts() {
    let log = Arc::new(Log::default());
    let pol = gate_policy(&log, &["coder-one"], false);
    let outcome = run_m4_gate(
        &log,
        job_plan(Some("coder-one"), JobMode::Task),
        pol,
        vec![ListenOutcome::Utterance("yes".to_string())],
        Some(drop_all_agents),
    )
    .await;
    assert!(outcome.is_ok());
    assert_eq!(log.count_exact("start_job"), 0, "start_job must not run");
    assert!(
        log.says().iter().any(|s| s == MSG_CHANGED),
        "refusal spoken: {:?}",
        log.says()
    );
}

#[tokio::test]
async fn m4_gate_selected_agent_flag_changed_during_question_refuses_and_never_starts() {
    let log = Arc::new(Log::default());
    let pol = gate_policy(&log, &["coder-one"], false);
    let _ = run_m4_gate(
        &log,
        job_plan(Some("coder-one"), JobMode::Task),
        pol,
        vec![ListenOutcome::Utterance("yes".to_string())],
        Some(change_flag),
    )
    .await;
    assert_eq!(log.count_exact("start_job"), 0, "flag change must refuse");
    assert!(log.says().iter().any(|s| s == MSG_CHANGED));
}

#[tokio::test]
async fn m4_gate_ask_turned_off_during_question_refuses_and_never_starts() {
    let log = Arc::new(Log::default());
    let pol = gate_policy(&log, &["coder-one"], true);
    let _ = run_m4_gate(
        &log,
        job_plan(Some("coder-one"), JobMode::Ask),
        pol,
        vec![ListenOutcome::Utterance("yes".to_string())],
        Some(turn_ask_off),
    )
    .await;
    assert_eq!(
        log.count_exact("start_job"),
        0,
        "ask turned off must refuse"
    );
    assert!(log.says().iter().any(|s| s == MSG_CHANGED));
}

#[tokio::test]
async fn m4_gate_unrelated_agent_added_during_question_default_job_still_starts() {
    let log = Arc::new(Log::default());
    let pol = gate_policy(&log, &["coder-one"], false);
    let _ = run_m4_gate(
        &log,
        job_plan(None, JobMode::Task),
        pol,
        vec![ListenOutcome::Utterance("yes".to_string())],
        Some(add_unrelated_agent),
    )
    .await;
    assert_eq!(
        log.count_exact("start_job"),
        1,
        "unrelated allowlist edit must not refuse"
    );
    assert!(!log.says().iter().any(|s| s == MSG_CHANGED));
}

#[tokio::test]
async fn m4_gate_unrelated_agent_added_during_question_selected_job_still_starts() {
    let log = Arc::new(Log::default());
    let pol = gate_policy(&log, &["coder-one"], false);
    let _ = run_m4_gate(
        &log,
        job_plan(Some("coder-one"), JobMode::Task),
        pol,
        vec![ListenOutcome::Utterance("yes".to_string())],
        Some(add_unrelated_agent),
    )
    .await;
    assert_eq!(
        log.count_exact("start_job"),
        1,
        "the selected agent is still allowed"
    );
}

#[tokio::test]
async fn m4_gate_fingerprint_is_taken_at_question_time_and_again_after_yes() {
    let log = Arc::new(Log::default());
    let pol = gate_policy(&log, &["coder-one"], false);
    let _ = run_m4_gate(
        &log,
        job_plan(Some("coder-one"), JobMode::Task),
        pol,
        vec![ListenOutcome::Utterance("yes".to_string())],
        None,
    )
    .await;
    let all = log.all();
    let question_at = all
        .iter()
        .position(|e| e == &format!("say:{QUESTION_WITH_AGENT}"))
        .expect("the question with the agent is spoken");
    let first_compare = all
        .iter()
        .position(|e| e == "policy:allowed_agents")
        .expect("the selected-agent fingerprint reads the allowlist");
    assert!(
        first_compare < question_at,
        "the first fingerprint is taken before the question is spoken: {all:?}"
    );
    let compares = all.iter().filter(|e| *e == "policy:allowed_agents").count();
    assert!(compares >= 2, "a second fingerprint after yes: {all:?}");
    assert_eq!(log.count_exact("start_job"), 1);
}

#[tokio::test]
async fn m4_gate_ask_question_is_spoken_for_a_selected_ask_job() {
    let log = Arc::new(Log::default());
    let pol = gate_policy(&log, &["coder-one"], true);
    let _ = run_m4_gate(
        &log,
        job_plan(Some("coder-one"), JobMode::Ask),
        pol,
        vec![ListenOutcome::Utterance("yes".to_string())],
        None,
    )
    .await;
    assert!(
        log.says().iter().any(|s| s == QUESTION_ASK_WITH_AGENT),
        "ask question spoken exactly: {:?}",
        log.says()
    );
    assert_eq!(log.count_exact("start_job"), 1);
}

#[tokio::test]
async fn m4_gate_no_answer_never_starts_the_job() {
    let log = Arc::new(Log::default());
    let pol = gate_policy(&log, &["coder-one"], false);
    let _ = run_m4_gate(
        &log,
        job_plan(Some("coder-one"), JobMode::Task),
        pol,
        vec![ListenOutcome::Utterance("no".to_string())],
        None,
    )
    .await;
    assert_eq!(
        log.count_exact("start_job"),
        0,
        "start_job reachable only through yes"
    );
    assert!(log.says().iter().any(|s| s == "Okay, I won't do that."));
}
