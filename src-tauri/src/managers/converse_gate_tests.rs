//! RED tests for the M3 S3b gate (DESIGN.md Revision 5: D1, D2, D5, D6, R2-1,
//! R2-2, R2-3, R2-6, R2-10, R2-12, R3-1, R3-2, R4-1). Written before the
//! implementation: they compile against the stubs and fail at runtime because
//! `gate_and_run` is `todo!()` or because the asserted behavior is missing.

use super::*;
use crate::managers::command_router::CommandAction;
use crate::managers::converse_confirm::{
    ActionPolicy, FileId, JobStartOutcome, LaunchConfig, PolicySource, ValidatedCommand,
};
use crate::managers::converse_plan::{JobRequest, PlanKind};
use crate::managers::emotion::Emotion;
use crate::managers::job_announcer::{AnnounceKind, Announcer, LowPriorityMouth};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

// ------------------------------------------------------------------ shared log

/// Ordered record of every effect the gate can have.
#[derive(Default)]
struct Events(Mutex<Vec<String>>);

impl Events {
    fn push(&self, s: impl Into<String>) {
        self.0.lock().unwrap().push(s.into());
    }
    fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
    fn count(&self, prefix: &str) -> usize {
        self.all().iter().filter(|e| e.starts_with(prefix)).count()
    }
    fn says(&self) -> Vec<String> {
        self.all()
            .iter()
            .filter_map(|e| e.strip_prefix("say:").map(str::to_string))
            .collect()
    }
}

// ------------------------------------------------------------------ fakes

enum Heard {
    Out(ListenOutcome),
    Hang,
}

/// A settings change that happens while the question is being answered.
enum Flip {
    Policy(ActionPolicy),
    Launch(Option<LaunchConfig>),
}

struct GateEars {
    script: Mutex<VecDeque<Heard>>,
    ev: Arc<Events>,
    /// Shared policy the flip mutates (the user changes settings mid-question).
    policy: Option<Arc<GatePolicy>>,
    flip: Option<Flip>,
}

impl Ears for GateEars {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        self.ev.push("listen");
        if let (Some(pol), Some(flip)) = (self.policy.as_ref(), self.flip.as_ref()) {
            match flip {
                Flip::Policy(p) => *pol.policy.lock().unwrap() = p.clone(),
                Flip::Launch(l) => *pol.launch.lock().unwrap() = l.clone(),
            }
        }
        let item = self.script.lock().unwrap().pop_front();
        Box::pin(async move {
            match item {
                Some(Heard::Out(o)) => o,
                Some(Heard::Hang) => std::future::pending().await,
                None => ListenOutcome::Aborted,
            }
        })
    }
}

struct GateMouth {
    ev: Arc<Events>,
    say_result: SayOutcome,
}

impl Mouth for GateMouth {
    fn say<'a>(&'a self, text: &'a str, _e: Emotion) -> BoxFuture<'a, SayOutcome> {
        self.ev.push(format!("say:{text}"));
        let result = self.say_result;
        Box::pin(async move { result })
    }
    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async { QuietOutcome::Quiet })
    }
    fn stop(&self) {
        self.ev.push("stop");
    }
}

/// Records every Hands effect, including the payload it received.
struct GateHands {
    ev: Arc<Events>,
    /// What `validate_command` returns for any text (None = refused).
    validated: Option<ValidatedCommand>,
    /// What `pin_project` returns (Err = unavailable).
    pin: Result<(), PinError>,
    /// What `start_job` returns.
    start: Mutex<Option<JobStartOutcome>>,
    run_validated_result: bool,
    cancel_result: bool,
    active_job: bool,
    start_calls: AtomicUsize,
    cancel_calls: AtomicUsize,
}

impl GateHands {
    fn new(ev: Arc<Events>) -> Self {
        GateHands {
            ev,
            validated: None,
            pin: Ok(()),
            start: Mutex::new(None),
            run_validated_result: true,
            cancel_result: true,
            active_job: false,
            start_calls: AtomicUsize::new(0),
            cancel_calls: AtomicUsize::new(0),
        }
    }
}

impl Hands for GateHands {
    fn paste_generated(&self, text: &str) -> Result<(), String> {
        self.ev.push(format!("paste:{text}"));
        Ok(())
    }
    fn run_user_phrase<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        self.ev.push(format!("user:{phrase}"));
        Box::pin(async { PhraseOutcome::NoMatch })
    }
    fn run_model_command<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        self.ev.push(format!("model:{phrase}"));
        Box::pin(async { PhraseOutcome::NoMatch })
    }
    fn validate_command(&self, text: &str) -> Option<ValidatedCommand> {
        self.ev.push(format!("validate:{text}"));
        self.validated.as_ref().map(|v| ValidatedCommand {
            label: v.label.clone(),
            action: v.action.clone(),
        })
    }
    fn run_validated_command(&self, cmd: ValidatedCommand) -> bool {
        self.ev.push(format!("run_validated:{}", cmd.label));
        self.run_validated_result
    }
    fn pin_project(&self, name: &str) -> Result<DirFd, PinError> {
        self.ev.push(format!("pin:{name}"));
        match &self.pin {
            Ok(()) => {
                let dir =
                    std::fs::File::open(std::env::temp_dir()).map_err(|_| PinError::Unavailable)?;
                Ok(DirFd::from(dir))
            }
            Err(PinError::Unknown) => Err(PinError::Unknown),
            Err(PinError::Ambiguous) => Err(PinError::Ambiguous),
            Err(PinError::Unavailable) => Err(PinError::Unavailable),
        }
    }
    fn start_job(&self, launch: JobLaunch) -> JobStartOutcome {
        self.start_calls.fetch_add(1, Ordering::SeqCst);
        self.ev
            .push(format!("start_job:{}:{}", launch.project, launch.task));
        self.start
            .lock()
            .unwrap()
            .take()
            .unwrap_or(JobStartOutcome::Started(1))
    }
    fn cancel_job(&self) -> bool {
        self.cancel_calls.fetch_add(1, Ordering::SeqCst);
        self.ev.push("cancel_job");
        self.cancel_result
    }
    fn has_active_job(&self) -> bool {
        self.active_job
    }
}

/// Fixed policy. `launch` = None means no usable launch configuration.
struct GatePolicy {
    policy: Mutex<ActionPolicy>,
    launch: Mutex<Option<LaunchConfig>>,
}

impl PolicySource for GatePolicy {
    fn current(&self) -> ActionPolicy {
        self.policy.lock().unwrap().clone()
    }
    fn launch_config(&self) -> Option<LaunchConfig> {
        self.launch.lock().unwrap().clone()
    }
}

fn policy(gate_active: bool, jobs_allowed: bool) -> Arc<GatePolicy> {
    Arc::new(GatePolicy {
        policy: Mutex::new(ActionPolicy {
            gate_active,
            jobs_allowed,
        }),
        launch: Mutex::new(Some(launch_config())),
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
        timeout: Duration::from_secs(60),
    }
}

/// Fixed brain: returns the scripted plan for the single utterance.
struct GateBrain {
    plan: TurnPlan,
}

impl Brain for GateBrain {
    fn plan<'a>(&'a self, _input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        let plan = self.plan.clone();
        Box::pin(async move { Ok(plan) })
    }
}

struct NoReporter;
impl Reporter for NoReporter {
    fn state(&self, _s: SessionState) {}
}

/// Mouth for the announcer: records direct speech.
struct RecordingLowMouth {
    ev: Arc<Events>,
}

impl LowPriorityMouth for RecordingLowMouth {
    fn try_speak_if_idle(&self, text: &str) -> bool {
        self.ev.push(format!("announce:{text}"));
        true
    }
}

// ------------------------------------------------------------------ plan helpers

fn job_plan(project: &str, task: &str) -> TurnPlan {
    TurnPlan {
        kind: PlanKind::Job,
        speech: "On it.".to_string(),
        emotion: Emotion::Neutral,
        paste_text: None,
        command: None,
        job: Some(JobRequest {
            project: project.to_string(),
            task: task.to_string(),
        }),
    }
}

fn paste_plan(text: &str) -> TurnPlan {
    TurnPlan {
        kind: PlanKind::Paste,
        speech: "Typing.".to_string(),
        emotion: Emotion::Neutral,
        paste_text: Some(text.to_string()),
        command: None,
        job: None,
    }
}

fn command_plan(cmd: &str) -> TurnPlan {
    TurnPlan {
        kind: PlanKind::Command,
        speech: "Opening.".to_string(),
        emotion: Emotion::Neutral,
        paste_text: None,
        command: Some(cmd.to_string()),
        job: None,
    }
}

fn answer_plan(speech: &str) -> TurnPlan {
    TurnPlan {
        kind: PlanKind::Answer,
        speech: speech.to_string(),
        emotion: Emotion::Neutral,
        paste_text: None,
        command: None,
        job: None,
    }
}

fn utt(s: &str) -> Heard {
    Heard::Out(ListenOutcome::Utterance(s.to_string()))
}

fn gate_cfg() -> SessionConfig {
    SessionConfig {
        post_speech_guard: Duration::from_millis(0),
        quiet_wait: Duration::from_millis(5),
        ..SessionConfig::default()
    }
}

/// Runs one gated plan through `gate_and_run` with the given policy and answer.
///
/// `answers` is the script of listen outcomes after the question is spoken.
/// Returns the events in order.
async fn run_gate(
    ev: &Arc<Events>,
    plan: TurnPlan,
    pol: Arc<GatePolicy>,
    hands: &GateHands,
    answers: Vec<Heard>,
    flip: Option<Flip>,
) -> (Arc<Events>, Result<Option<String>, SessionEnd>) {
    let ears = GateEars {
        script: Mutex::new(answers.into()),
        ev: ev.clone(),
        policy: Some(pol.clone()),
        flip,
    };
    let mouth = GateMouth {
        ev: ev.clone(),
        say_result: SayOutcome::Done,
    };
    let brain = GateBrain { plan: plan.clone() };
    let announcer = Announcer::new(Arc::new(RecordingLowMouth { ev: ev.clone() }));
    let (_tx, rx) = watch::channel(false);
    let mut cancel: CancelToken = rx;
    let deps = Deps {
        ears: &ears,
        mouth: &mouth,
        hands,
        brain: &brain,
        reporter: &NoReporter,
        policy: Some(&*pol),
        announcer: Some(&announcer),
    };
    let cfg = gate_cfg();
    let mut carry = Carry::default();
    let outcome = gate_and_run(&deps, &cfg, &mut cancel, plan, &mut carry).await;
    (ev.clone(), outcome)
}

// ------------------------------------------------------------------ tests

/// Sanity: the test module compiles and the harness runs; the gate itself is still
/// a stub, so this must fail at runtime with the `todo!()` panic.
#[tokio::test]
async fn gate_stub_is_reached_for_paste_with_gate_active() {
    let ev = Arc::new(Events::default());
    let hands = GateHands::new(ev.clone());
    let pol = policy(true, false);
    let (ev, _outcome) = run_gate(
        &ev,
        paste_plan("hello"),
        pol,
        &hands,
        vec![utt("yes")],
        None,
    )
    .await;
    assert!(ev.says().iter().any(|s| s.contains("type that")));
}

// ---------------------------------------------------------- gate table (D2, R2-12)

/// Gate active + brain-planned Paste: the question is asked and the SAME text is pasted on yes.
#[tokio::test]
async fn paste_with_gate_active_asks_then_pastes_the_same_text_on_yes() {
    let ev = Arc::new(Events::default());
    let hands = GateHands::new(ev.clone());
    let (ev, outcome) = run_gate(
        &ev,
        paste_plan("hello world"),
        policy(true, false),
        &hands,
        vec![utt("yes")],
        None,
    )
    .await;
    assert!(
        outcome.is_ok(),
        "approved paste must not end the session: {outcome:?}"
    );
    assert_eq!(
        ev.says().first().map(String::as_str),
        Some("I'll type that into the active app. Say yes or no.")
    );
    assert!(ev.all().iter().any(|e| e == "paste:hello world"));
}

/// Gate inactive: no question is spoken and the paste runs directly (legacy behavior).
#[tokio::test]
async fn paste_with_gate_inactive_runs_without_asking() {
    let ev = Arc::new(Events::default());
    let hands = GateHands::new(ev.clone());
    let (ev, _outcome) = run_gate(
        &ev,
        paste_plan("hello world"),
        policy(false, false),
        &hands,
        vec![],
        None,
    )
    .await;
    assert_eq!(ev.count("listen"), 0, "inactive gate must not listen");
    assert!(ev.all().iter().any(|e| e == "paste:hello world"));
}

/// Gate active + "no": nothing is pasted and the decline line is spoken.
#[tokio::test]
async fn paste_declined_with_no_pastes_nothing() {
    let ev = Arc::new(Events::default());
    let hands = GateHands::new(ev.clone());
    let (ev, _outcome) = run_gate(
        &ev,
        paste_plan("secret draft"),
        policy(true, false),
        &hands,
        vec![utt("no")],
        None,
    )
    .await;
    assert_eq!(ev.count("paste:"), 0);
    assert!(ev.says().iter().any(|s| s == "Okay, I won't do that."));
}

/// Gate active + "maybe": Unclear, nothing pasted, the unclear line is spoken.
#[tokio::test]
async fn paste_declined_with_unclear_answer_pastes_nothing() {
    let ev = Arc::new(Events::default());
    let hands = GateHands::new(ev.clone());
    let (ev, _outcome) = run_gate(
        &ev,
        paste_plan("x"),
        policy(true, false),
        &hands,
        vec![utt("maybe later")],
        None,
    )
    .await;
    assert_eq!(ev.count("paste:"), 0);
    assert!(ev
        .says()
        .iter()
        .any(|s| s == "I didn't catch a clear yes, so I didn't do it."));
}

/// Gate active + silence: nothing executed, the silence decline is spoken.
#[tokio::test]
async fn paste_declined_on_silence_pastes_nothing() {
    let ev = Arc::new(Events::default());
    let hands = GateHands::new(ev.clone());
    let (ev, _outcome) = run_gate(
        &ev,
        paste_plan("x"),
        policy(true, false),
        &hands,
        vec![Heard::Out(ListenOutcome::NoSpeech)],
        None,
    )
    .await;
    assert_eq!(ev.count("paste:"), 0);
    assert!(ev.says().iter().any(|s| s == "Okay, I won't do that."));
}

/// Gate active + listen error: nothing executed, the error decline is spoken.
#[tokio::test]
async fn paste_declined_on_listen_error_pastes_nothing() {
    let ev = Arc::new(Events::default());
    let hands = GateHands::new(ev.clone());
    let (ev, _outcome) = run_gate(
        &ev,
        paste_plan("x"),
        policy(true, false),
        &hands,
        vec![Heard::Out(ListenOutcome::Failed(ListenFailure::Other))],
        None,
    )
    .await;
    assert_eq!(ev.count("paste:"), 0);
    assert!(ev
        .says()
        .iter()
        .any(|s| s == "Something went wrong, so I didn't do it."));
}

/// Gate active + brain-planned Command: the validated command runs and the question names the label.
#[tokio::test]
async fn command_with_gate_active_asks_then_runs_the_validated_command_on_yes() {
    let ev = Arc::new(Events::default());
    let mut hands = GateHands::new(ev.clone());
    hands.validated = Some(ValidatedCommand {
        label: "Notes".to_string(),
        action: CommandAction::OpenUrl {
            url: "https://example.invalid/".to_string(),
        },
    });
    let (ev, outcome) = run_gate(
        &ev,
        command_plan("open notes"),
        policy(true, false),
        &hands,
        vec![utt("yes")],
        None,
    )
    .await;
    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(
        ev.says().first().map(String::as_str),
        Some("I'll open Notes. Say yes or no.")
    );
    assert!(ev.all().iter().any(|e| e == "run_validated:Notes"));
    assert_eq!(
        ev.count("model:"),
        0,
        "the unvalidated legacy path must not run"
    );
}

/// Payload equality for a Command: the label spoken equals the label that ran.
#[tokio::test]
async fn command_payload_asked_equals_payload_run() {
    let ev = Arc::new(Events::default());
    let mut hands = GateHands::new(ev.clone());
    hands.validated = Some(ValidatedCommand {
        label: "Mail App".to_string(),
        action: CommandAction::OpenUrl {
            url: "https://example.invalid/".to_string(),
        },
    });
    let (ev, _o) = run_gate(
        &ev,
        command_plan("open mail"),
        policy(true, false),
        &hands,
        vec![utt("yes")],
        None,
    )
    .await;
    let asked = ev
        .says()
        .into_iter()
        .find(|s| s.starts_with("I'll open "))
        .unwrap_or_default();
    assert!(asked.contains("Mail App"), "asked: {asked}");
    assert!(ev.all().iter().any(|e| e == "run_validated:Mail App"));
}

/// Gate inactive + Command: the validated command runs without a question.
#[tokio::test]
async fn command_with_gate_inactive_runs_without_asking() {
    let ev = Arc::new(Events::default());
    let mut hands = GateHands::new(ev.clone());
    hands.validated = Some(ValidatedCommand {
        label: "Notes".to_string(),
        action: CommandAction::OpenUrl {
            url: "https://example.invalid/".to_string(),
        },
    });
    let (ev, _o) = run_gate(
        &ev,
        command_plan("open notes"),
        policy(false, false),
        &hands,
        vec![],
        None,
    )
    .await;
    // Gate inactive keeps the legacy route (R2-12): no question, and the model command is
    // handed to run_model_command exactly as before, not to the validated path.
    assert_eq!(ev.count("listen"), 0);
    assert!(ev.all().iter().any(|e| e == "model:open notes"));
    assert_eq!(ev.count("run_validated:"), 0);
}

/// A command the router refuses (validate returns None) is never confirmed or run.
#[tokio::test]
async fn command_refused_by_validation_is_never_asked_or_run() {
    let ev = Arc::new(Events::default());
    let hands = GateHands::new(ev.clone());
    let (ev, _o) = run_gate(
        &ev,
        command_plan("format the disk"),
        policy(true, false),
        &hands,
        vec![utt("yes")],
        None,
    )
    .await;
    assert_eq!(
        ev.count("listen"),
        0,
        "a refused command must not be confirmed"
    );
    assert_eq!(ev.count("run_validated:"), 0);
    assert_eq!(ev.count("model:"), 0);
}

/// Answer and Clarify plans never enter the gate: no question, no action.
#[tokio::test]
async fn answer_and_clarify_are_not_gated() {
    for kind in [PlanKind::Answer, PlanKind::Clarify] {
        let ev = Arc::new(Events::default());
        let hands = GateHands::new(ev.clone());
        let plan = TurnPlan {
            kind,
            ..answer_plan("Here you go.")
        };
        let (ev, _o) = run_gate(&ev, plan, policy(true, true), &hands, vec![], None).await;
        assert_eq!(ev.count("listen"), 0, "{kind:?} must not ask");
        assert_eq!(ev.count("start_job:"), 0);
        assert_eq!(ev.count("paste:"), 0);
    }
}

#[cfg(test)]
#[path = "converse_gate_dispatch_tests.rs"]
mod converse_gate_dispatch_tests;

// ---------------------------------------------------------- default invariance (M3-12)

/// Policy None (defaults): the gate is inactive and `gate_and_run` must match the legacy
/// Paste path exactly (paste, then the paste confirmation text is spoken).
#[tokio::test]
async fn default_invariance_paste_without_policy_runs_legacy_path() {
    let ev = Arc::new(Events::default());
    let hands = GateHands::new(ev.clone());
    let ears = GateEars {
        script: Mutex::new(VecDeque::new()),
        ev: ev.clone(),
        policy: None,
        flip: None,
    };
    let mouth = GateMouth {
        ev: ev.clone(),
        say_result: SayOutcome::Done,
    };
    let brain = GateBrain {
        plan: paste_plan("legacy text"),
    };
    let (_tx, rx) = watch::channel(false);
    let mut cancel: CancelToken = rx;
    let deps = Deps {
        ears: &ears,
        mouth: &mouth,
        hands: &hands,
        brain: &brain,
        reporter: &NoReporter,
        policy: None,
        announcer: None,
    };
    let mut carry = Carry::default();
    let _ = gate_and_run(
        &deps,
        &gate_cfg(),
        &mut cancel,
        paste_plan("legacy text"),
        &mut carry,
    )
    .await;
    assert_eq!(ev.count("listen"), 0, "no policy must never ask");
    assert!(ev.all().iter().any(|e| e == "paste:legacy text"));
}

// ---------------------------------------------------------- CancelJob (R2-10, D6)

/// A "stop job" utterance is CancelJob only when cancel is enabled; the cancel reply is spoken.
#[tokio::test]
async fn cancel_job_utterance_cancels_when_a_job_is_active() {
    let ev = Arc::new(Events::default());
    let mut hands = GateHands::new(ev.clone());
    hands.active_job = true;
    let (ev, _o) = run_dispatch(
        &ev,
        "stop job",
        policy(true, true),
        &hands,
        Arc::new(GatePolicy {
            policy: Mutex::new(ActionPolicy {
                gate_active: true,
                jobs_allowed: true,
            }),
            launch: Mutex::new(Some(launch_config())),
        }),
    )
    .await;
    assert_eq!(
        hands.cancel_calls.load(Ordering::SeqCst),
        1,
        "{:?}",
        ev.all()
    );
    assert!(ev.says().iter().any(|s| s == "Cancelled the coding job."));
}

/// With no active job and jobs enabled, the cancel intent says nothing is running.
#[tokio::test]
async fn cancel_job_utterance_reports_no_job_running() {
    let ev = Arc::new(Events::default());
    let mut hands = GateHands::new(ev.clone());
    hands.cancel_result = false;
    let (ev, _o) = run_dispatch(
        &ev,
        "cancel the job",
        policy(true, true),
        &hands,
        Arc::new(GatePolicy {
            policy: Mutex::new(ActionPolicy {
                gate_active: true,
                jobs_allowed: true,
            }),
            launch: Mutex::new(Some(launch_config())),
        }),
    )
    .await;
    assert_eq!(
        hands.cancel_calls.load(Ordering::SeqCst),
        1,
        "{:?}",
        ev.all()
    );
    assert!(ev.says().iter().any(|s| s == "No coding job is running."));
}

/// Defaults (jobs off, no active job): "stop job" keeps its legacy meaning and never cancels.
#[tokio::test]
async fn stop_job_with_defaults_keeps_legacy_meaning_and_does_not_cancel() {
    let ev = Arc::new(Events::default());
    let hands = GateHands::new(ev.clone());
    let (_ev, _o) = run_dispatch(
        &ev,
        "stop job",
        policy(false, false),
        &hands,
        Arc::new(GatePolicy {
            policy: Mutex::new(ActionPolicy {
                gate_active: false,
                jobs_allowed: false,
            }),
            launch: Mutex::new(None),
        }),
    )
    .await;
    assert_eq!(hands.cancel_calls.load(Ordering::SeqCst), 0);
}

/// Jobs disabled but a job is active: "cancel job" still cancels it.
#[tokio::test]
async fn cancel_job_with_active_job_cancels_even_if_jobs_disabled() {
    let ev = Arc::new(Events::default());
    let mut hands = GateHands::new(ev.clone());
    hands.active_job = true;
    let (_ev, _o) = run_dispatch(
        &ev,
        "cancel job",
        policy(false, false),
        &hands,
        Arc::new(GatePolicy {
            policy: Mutex::new(ActionPolicy {
                gate_active: false,
                jobs_allowed: false,
            }),
            launch: Mutex::new(None),
        }),
    )
    .await;
    assert_eq!(hands.cancel_calls.load(Ordering::SeqCst), 1);
}

/// Runs one utterance through `run_turns` (the real dispatch path) with a single listen.
async fn run_dispatch(
    ev: &Arc<Events>,
    utterance: &str,
    _unused: Arc<GatePolicy>,
    hands: &GateHands,
    pol: Arc<GatePolicy>,
) -> (Arc<Events>, Result<(), SessionEnd>) {
    let ears = GateEars {
        script: Mutex::new(vec![utt(utterance)].into()),
        ev: ev.clone(),
        policy: Some(pol.clone()),
        flip: None,
    };
    let mouth = GateMouth {
        ev: ev.clone(),
        say_result: SayOutcome::Done,
    };
    let brain = GateBrain {
        plan: answer_plan("unused"),
    };
    let (_tx, _rx) = watch::channel(false);
    let deps = Deps {
        ears: &ears,
        mouth: &mouth,
        hands,
        brain: &brain,
        reporter: &NoReporter,
        policy: Some(&*pol),
        announcer: None,
    };
    let outcome = run_session(&deps, &gate_cfg(), watch::channel(false).1).await;
    (
        ev.clone(),
        if outcome == SessionEnd::Idle {
            Ok(())
        } else {
            Err(outcome)
        },
    )
}

#[cfg(test)]
#[path = "converse_gate_announce_tests.rs"]
mod converse_gate_announce_tests;
