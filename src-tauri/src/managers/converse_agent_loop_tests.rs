//! Stage 2 agent hand-off, loop-level behaviour (DESIGN-PR2-STAGE2 Revision 3, sections 2.1, 2.3,
//! 2.4, 2.6 and tests A1-A7). Authoritative spec, written before the loop wiring (step C2).
//! Black box through `run_session`: fakes for ears, mouth, brain, hands and the agent run.
//! The fakes are copied from converse_role_loop_tests.rs (same barge-in watch, same queue and
//! mouth). Items are `pub(super)` because converse_agent_taint_tests.rs reuses the rig.
//! Included from converse.rs. Do not edit to make code pass.
#![allow(dead_code)]

#[allow(unused_imports)]
use super::*;
use crate::managers::converse_agent::{
    AgentEvent, AgentPoll, AgentRun, AgentStartError, PreparedAgentLaunch,
};
use crate::managers::converse_barge::{BargeCapture, BargeIn, BargeWatch, ReplyGate};
use crate::managers::converse_confirm::{ActionPolicy, FileId, LaunchConfig};
use crate::managers::converse_context::note_agent_result;
use crate::managers::converse_plan::{
    AgentRequest, JobMode, JobRequest, PlanError, PlanInput, PlanKind, RoleDirective, TurnPlan,
};
use crate::managers::converse_role::split_chunks;
use crate::managers::emotion::Emotion;
use std::collections::VecDeque;
use std::future::pending;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::{oneshot, watch};

// ---- fixed texts the loop must speak (pinned by the design, Revision 3) ----------------------

pub(super) const ACK_DEFAULT: &str = "On it.";
pub(super) const BUSY_RUNNING: &str = "Still working on the last request. Say cancel to stop it.";
pub(super) const BUSY_CANCELLING: &str = "The agent is still shutting down. Try again in a moment.";
pub(super) const CANCELLED: &str = "Cancelled.";
pub(super) const PROGRESS: &str = "Still working on it.";
pub(super) const LOST: &str = "The agent stopped unexpectedly.";
pub(super) const TOO_LONG_RUN: &str = "The agent took too long.";
pub(super) const CANT_REACH: &str = "I can't reach the agent right now.";
pub(super) const AGENT_BUSY: &str = "The agent is busy right now.";
pub(super) const DECLINED: &str = "Okay, I won't.";
pub(super) const SETTINGS_CHANGED: &str = "The agent settings changed. Please ask again.";
pub(super) const FULL_ON_SCREEN: &str = "The full answer is on screen.";
pub(super) const NOT_EFFECTIVE: &str = "I can't do that here.";
pub(super) const FAILED_LINE: &str = "The agent could not finish.";

pub(super) const REQ: &str = "find the release notes";
pub(super) const TASK: &str = "find the release notes";
pub(super) const ACK_TEXT: &str = "Let me look that up.";
pub(super) const ROLE_TEACHER: &str =
    "a patient teacher for six year olds and their parents who ask many questions";

/// A short result: one spoken chunk, well under the 600-char speech cap.
pub(super) const RESULT: &str =
    "The release notes list three changes. The first is faster startup. The second fixes the audio bug.";

/// A result well over the 600-char speech cap (about 1900 chars).
pub(super) fn long_result() -> String {
    (0..80)
        .map(|i| format!("Fact {i} is recorded here."))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A result of several chunks, for barge-in cuts.
pub(super) const CHUNKED: &str = "Alpha is one. Beta is two. Gamma is three. Delta is four. Epsilon is five. Zeta is six. Eta is seven.";

pub(super) type Shared = Arc<Mutex<Vec<String>>>;

pub(super) fn note(log: &Shared, entry: impl Into<String>) {
    log.lock().unwrap().push(entry.into());
}

// ---- barge-in fake (same mechanism as converse_role_loop_tests.rs) ---------------------------

pub(super) struct WatchCore {
    log: Shared,
    gate: Arc<ReplyGate>,
    reply: watch::Sender<bool>,
    latched: AtomicBool,
    active: AtomicBool,
    captures: Arc<Mutex<VecDeque<String>>>,
    capture_tx: Mutex<Option<oneshot::Sender<BargeCapture>>>,
    capture_rx: Mutex<Option<oneshot::Receiver<BargeCapture>>>,
}

impl WatchCore {
    /// The worker's sustained-speech sequence: close the gate, latch, deliver the capture the
    /// barge hears (`NoSpeech` when none is scripted), then set the reply token.
    pub(super) fn fire(&self) -> bool {
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

pub(super) struct FakeBargeIn {
    pub(super) log: Shared,
    pub(super) arms: Mutex<Vec<Arc<WatchCore>>>,
    pub(super) captures: Arc<Mutex<VecDeque<String>>>,
}

impl FakeBargeIn {
    pub(super) fn latest(&self) -> Arc<WatchCore> {
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

pub(super) struct FakeWatch {
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

// ---- ears: a script of steps; a step may run a hook when the listen starts ------------------

/// One scripted listen. `hook` runs synchronously when `listen` is called, which is the moment
/// the listen is "in flight": the loop cannot observe anything the hook changes until it returns.
pub(super) struct Step {
    pub(super) outcome: ListenOutcome,
    pub(super) hook: Option<Box<dyn FnMut() + Send>>,
}

pub(super) fn heard(text: &str) -> Step {
    Step {
        outcome: ListenOutcome::Utterance(text.to_string()),
        hook: None,
    }
}

pub(super) fn silent() -> Step {
    Step {
        outcome: ListenOutcome::NoSpeech,
        hook: None,
    }
}

pub(super) fn outcome(outcome: ListenOutcome) -> Step {
    Step {
        outcome,
        hook: None,
    }
}

pub(super) fn with_hook(mut step: Step, hook: impl FnMut() + Send + 'static) -> Step {
    step.hook = Some(Box::new(hook));
    step
}

pub(super) struct LEars {
    pub(super) script: Mutex<VecDeque<Step>>,
    pub(super) log: Shared,
    pub(super) barge: Arc<FakeBargeIn>,
    /// When true the loop arms a barge watch per reply.
    pub(super) barge_on: bool,
}

impl Ears for LEars {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        note(&self.log, "listen");
        let step = self.script.lock().unwrap().pop_front();
        let result = match step {
            Some(mut step) => {
                if let Some(hook) = step.hook.as_mut() {
                    hook();
                }
                step.outcome
            }
            None => ListenOutcome::Aborted,
        };
        Box::pin(async move { result })
    }

    fn barge_in(&self) -> Option<&dyn BargeIn> {
        let barge: &dyn BargeIn = self.barge.as_ref();
        self.barge_on.then_some(barge)
    }
}

// ---- mouth and speech queue ------------------------------------------------------------------

pub(super) struct LQueue {
    log: Shared,
    accept: Option<usize>,
    finish: SayOutcome,
    pushes: Mutex<usize>,
}

impl SpeechQueue for LQueue {
    fn push(&self, text: &str, _e: Emotion) -> bool {
        let mut n = self.pushes.lock().unwrap();
        if self.accept.is_some_and(|max| *n >= max) {
            return false;
        }
        *n += 1;
        note(&self.log, format!("push:{text}"));
        true
    }

    fn abort(&self) {
        note(&self.log, "abort");
    }

    fn finish(&self) -> BoxFuture<'_, SayOutcome> {
        note(&self.log, "finish");
        let o = self.finish;
        Box::pin(async move { o })
    }
}

pub(super) struct LMouth {
    pub(super) log: Shared,
    /// Scripted outcomes for `say`, one per call; empty => `Done`.
    pub(super) says: Mutex<VecDeque<SayOutcome>>,
    /// Whether `open_queue` offers a queue (false => legacy mouth).
    pub(super) offer: bool,
    pub(super) accept: Option<usize>,
    pub(super) finish: SayOutcome,
    /// The text of the most recent `say`.
    pub(super) last_said: Mutex<Option<String>>,
    /// When the last `say` was this text, the next `wait_quiet` fires the barge (once).
    pub(super) fire_after_said: Mutex<Option<String>>,
    pub(super) barge: Arc<FakeBargeIn>,
}

impl Mouth for LMouth {
    fn say<'a>(&'a self, text: &'a str, _e: Emotion) -> BoxFuture<'a, SayOutcome> {
        note(&self.log, format!("say:{text}"));
        *self.last_said.lock().unwrap() = Some(text.to_string());
        let o = self
            .says
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(SayOutcome::Done);
        Box::pin(async move { o })
    }

    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        let last = self.last_said.lock().unwrap().clone();
        let mut fire = self.fire_after_said.lock().unwrap();
        if fire.is_some() && *fire == last {
            *fire = None;
            drop(fire);
            self.barge.latest().fire();
        }
        Box::pin(async { QuietOutcome::Quiet })
    }

    fn stop(&self) {
        note(&self.log, "stop");
    }

    fn open_queue(&self) -> Option<Arc<dyn SpeechQueue>> {
        if !self.offer {
            return None;
        }
        note(&self.log, "open");
        Some(Arc::new(LQueue {
            log: self.log.clone(),
            accept: self.accept,
            finish: self.finish,
            pushes: Mutex::new(0),
        }))
    }
}

// ---- the agent run: a shared control handle the test holds ----------------------------------

/// Control handle of one fake agent run. The test keeps a clone to script polls, move the
/// clock (`elapsed`) and count `cancel` calls.
pub(super) struct RunCtl {
    polls: Mutex<VecDeque<AgentPoll>>,
    cancels: AtomicUsize,
    elapsed: Mutex<Duration>,
}

impl RunCtl {
    pub(super) fn new() -> Arc<RunCtl> {
        Arc::new(RunCtl {
            polls: Mutex::new(VecDeque::new()),
            cancels: AtomicUsize::new(0),
            elapsed: Mutex::new(Duration::from_secs(0)),
        })
    }

    pub(super) fn push(&self, poll: AgentPoll) {
        self.polls.lock().unwrap().push_back(poll);
    }

    pub(super) fn set_elapsed(&self, secs: u64) {
        *self.elapsed.lock().unwrap() = Duration::from_secs(secs);
    }

    pub(super) fn cancels(&self) -> usize {
        self.cancels.load(Ordering::SeqCst)
    }
}

struct FakeRun(Arc<RunCtl>);

impl AgentRun for FakeRun {
    fn poll(&mut self) -> AgentPoll {
        self.0
            .polls
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(AgentPoll::Pending)
    }

    fn cancel(&self) {
        self.0.cancels.fetch_add(1, Ordering::SeqCst);
    }

    fn elapsed(&self) -> Duration {
        *self.0.elapsed.lock().unwrap()
    }
}

// ---- hands: scripted agent hand-off plus the user/model/paste tiers --------------------------

pub(super) struct AHands {
    pub(super) log: Shared,
    pub(super) paste: Mutex<VecDeque<Result<(), String>>>,
    pub(super) user: Mutex<VecDeque<PhraseOutcome>>,
    pub(super) model: Mutex<VecDeque<PhraseOutcome>>,
    /// Scripted prepare errors, one per call; empty => a launch is built.
    pub(super) prepare_errs: Mutex<VecDeque<AgentStartError>>,
    /// Scripted start results, one per call; empty => a fresh run.
    pub(super) starts: Mutex<VecDeque<Result<Arc<RunCtl>, AgentStartError>>>,
    /// Every run that was started, in order.
    pub(super) runs: Mutex<Vec<Arc<RunCtl>>>,
    /// Every launch handed to `start_agent`, in order.
    pub(super) launches: Mutex<Vec<PreparedAgentLaunch>>,
    /// What `agent_launch_still_valid` answers (shared so a hook can change it).
    pub(super) valid: Arc<AtomicBool>,
    pub(super) published: Mutex<Vec<String>>,
    /// Whether `pin_project` succeeds (for the Job path).
    pub(super) pin_ok: bool,
    pub(super) active_job: AtomicBool,
}

impl AHands {
    pub(super) fn new(log: Shared) -> AHands {
        AHands {
            log,
            paste: Mutex::new(VecDeque::new()),
            user: Mutex::new(VecDeque::new()),
            model: Mutex::new(VecDeque::new()),
            prepare_errs: Mutex::new(VecDeque::new()),
            starts: Mutex::new(VecDeque::new()),
            runs: Mutex::new(Vec::new()),
            launches: Mutex::new(Vec::new()),
            valid: Arc::new(AtomicBool::new(true)),
            published: Mutex::new(Vec::new()),
            pin_ok: true,
            active_job: AtomicBool::new(false),
        }
    }
}

impl Hands for AHands {
    fn paste_generated(&self, text: &str) -> Result<(), String> {
        note(&self.log, format!("paste:{text}"));
        self.paste.lock().unwrap().pop_front().unwrap_or(Ok(()))
    }

    fn run_user_phrase<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        note(&self.log, format!("user:{phrase}"));
        let o = self
            .user
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(PhraseOutcome::NoMatch);
        Box::pin(async move { o })
    }

    fn run_model_command<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        note(&self.log, format!("model:{phrase}"));
        let o = self
            .model
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(PhraseOutcome::NoMatch);
        Box::pin(async move { o })
    }

    fn pin_project(&self, name: &str) -> Result<DirFd, PinError> {
        note(&self.log, format!("pin:{name}"));
        if !self.pin_ok {
            return Err(PinError::Unavailable);
        }
        let dir = std::fs::File::open(std::env::temp_dir()).map_err(|_| PinError::Unavailable)?;
        Ok(DirFd::from(dir))
    }

    fn start_job(&self, launch: JobLaunch) -> JobStartOutcome {
        note(
            &self.log,
            format!("start_job:{}:{}", launch.project, launch.task),
        );
        JobStartOutcome::Started(1)
    }

    fn cancel_job(&self) -> bool {
        note(&self.log, "cancel_job");
        true
    }

    fn has_active_job(&self) -> bool {
        self.active_job.load(Ordering::SeqCst)
    }

    fn prepare_agent_launch(
        &self,
        task: &str,
        role: Option<&str>,
    ) -> Result<PreparedAgentLaunch, AgentStartError> {
        note(&self.log, format!("prepare:{task}"));
        if let Some(err) = self.prepare_errs.lock().unwrap().pop_front() {
            return Err(err);
        }
        Ok(PreparedAgentLaunch::new(
            task.to_string(),
            role.map(str::to_string),
            "fake-agent".to_string(),
            vec!["--acp".to_string()],
            "/tmp/fake-work".to_string(),
            Duration::from_secs(10),
        ))
    }

    fn agent_launch_still_valid(&self, _launch: &PreparedAgentLaunch) -> bool {
        self.valid.load(Ordering::SeqCst)
    }

    fn start_agent(
        &self,
        launch: PreparedAgentLaunch,
    ) -> Result<Box<dyn AgentRun>, AgentStartError> {
        note(&self.log, format!("start:{}", launch.task));
        let next = self
            .starts
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(RunCtl::new()));
        self.launches.lock().unwrap().push(launch);
        match next {
            Ok(ctl) => {
                self.runs.lock().unwrap().push(ctl.clone());
                Ok(Box::new(FakeRun(ctl)))
            }
            Err(err) => Err(err),
        }
    }

    fn publish_agent_result(&self, text: &str) {
        note(&self.log, format!("publish:{text}"));
        self.published.lock().unwrap().push(text.to_string());
    }
}

/// Fixed policy: the gate and job switches, and a launch configuration for jobs.
pub(super) struct AgentPolicy {
    pub(super) gate_active: bool,
    pub(super) jobs_allowed: bool,
}

impl PolicySource for AgentPolicy {
    fn current(&self) -> ActionPolicy {
        ActionPolicy {
            gate_active: self.gate_active,
            jobs_allowed: self.jobs_allowed,
        }
    }

    fn launch_config(&self) -> Option<LaunchConfig> {
        Some(LaunchConfig {
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
        })
    }
}

// ---- brain -----------------------------------------------------------------------------------

/// One scripted brain call: streamed deltas (streaming path), a delay, then the plan.
pub(super) struct Turn {
    pub(super) deltas: Vec<String>,
    pub(super) delay: u64,
    pub(super) plan: Result<TurnPlan, PlanError>,
}

/// What the loop gave the brain on one call (B1 records `agent_enabled`).
#[derive(Debug, Clone)]
pub(super) struct Seen {
    pub(super) utterance: String,
    pub(super) role: Option<String>,
    pub(super) agent_enabled: bool,
    pub(super) history: Vec<(String, String)>,
}

pub(super) struct LBrain {
    pub(super) script: Mutex<VecDeque<Turn>>,
    pub(super) seen: Mutex<Vec<Seen>>,
    pub(super) log: Shared,
}

impl LBrain {
    fn record(&self, input: &PlanInput<'_>) {
        note(&self.log, format!("plan:{}", input.utterance));
        self.seen.lock().unwrap().push(Seen {
            utterance: input.utterance.to_string(),
            role: input.role.map(str::to_string),
            agent_enabled: input.agent_enabled,
            history: input.history.to_vec(),
        });
    }

    fn next(&self) -> Option<Turn> {
        self.script.lock().unwrap().pop_front()
    }
}

impl Brain for LBrain {
    fn plan<'a>(&'a self, input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        self.record(&input);
        let t = self.next();
        Box::pin(async move {
            match t {
                Some(t) => {
                    if t.delay > 0 {
                        tokio::time::sleep(Duration::from_millis(t.delay)).await;
                    }
                    t.plan
                }
                None => Err(PlanError::Backend),
            }
        })
    }

    fn plan_streaming<'a>(
        &'a self,
        input: PlanInput<'a>,
        on_text: &'a mut (dyn FnMut(&str) + Send),
    ) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        self.record(&input);
        let t = self.next();
        Box::pin(async move {
            let Some(t) = t else {
                return Err(PlanError::Backend);
            };
            for delta in &t.deltas {
                on_text(delta);
            }
            if t.delay > 0 {
                tokio::time::sleep(Duration::from_millis(t.delay)).await;
            }
            t.plan
        })
    }
}

pub(super) struct LReporter;

impl Reporter for LReporter {
    fn state(&self, _s: SessionState) {}
}

pub(super) struct Rig {
    pub(super) log: Shared,
    pub(super) cancel: Arc<watch::Sender<bool>>,
    pub(super) ears: LEars,
    pub(super) mouth: LMouth,
    pub(super) hands: AHands,
    pub(super) brain: LBrain,
    pub(super) reporter: LReporter,
    pub(super) barge: Arc<FakeBargeIn>,
}

/// A rig with scripted listens (`steps`) and brain calls (`turns`). The mouth offers a queue.
pub(super) fn rig_with(steps: Vec<Step>, turns: Vec<Turn>) -> Rig {
    let log: Shared = Arc::new(Mutex::new(vec![]));
    let captures: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
    let barge = Arc::new(FakeBargeIn {
        log: log.clone(),
        arms: Mutex::new(vec![]),
        captures: captures.clone(),
    });
    let (tx, _rx) = watch::channel(false);
    Rig {
        ears: LEars {
            script: Mutex::new(steps.into()),
            log: log.clone(),
            barge: barge.clone(),
            barge_on: false,
        },
        mouth: LMouth {
            log: log.clone(),
            says: Mutex::new(VecDeque::new()),
            offer: true,
            accept: None,
            finish: SayOutcome::Done,
            last_said: Mutex::new(None),
            fire_after_said: Mutex::new(None),
            barge: barge.clone(),
        },
        hands: AHands::new(log.clone()),
        brain: LBrain {
            script: Mutex::new(turns.into()),
            seen: Mutex::new(vec![]),
            log: log.clone(),
        },
        reporter: LReporter,
        cancel: Arc::new(tx),
        barge,
        log,
    }
}

/// The default session settings for these tests: the hand-off is on, no speech guard.
pub(super) fn acfg() -> SessionConfig {
    SessionConfig {
        post_speech_guard: Duration::from_millis(0),
        quiet_wait: Duration::from_millis(5),
        agent_handoff: true,
        ..SessionConfig::default()
    }
}

pub(super) async fn run_with(
    r: &Rig,
    cfg: &SessionConfig,
    policy: Option<&dyn PolicySource>,
) -> SessionEnd {
    let deps = Deps {
        policy,
        announcer: None,
        ears: &r.ears,
        mouth: &r.mouth,
        hands: &r.hands,
        brain: &r.brain,
        reporter: &r.reporter,
    };
    let token = r.cancel.subscribe();
    tokio::time::timeout(Duration::from_secs(10), run_session(&deps, cfg, token))
        .await
        .expect("run_session hung")
}

pub(super) async fn run(r: &Rig, cfg: &SessionConfig) -> SessionEnd {
    run_with(r, cfg, None).await
}

// ---- plan builders ----------------------------------------------------------------------------

pub(super) fn tp(kind: PlanKind, speech: &str) -> TurnPlan {
    TurnPlan {
        plan: None,
        job: None,
        role: None,
        kind,
        speech: speech.to_string(),
        emotion: Emotion::Neutral,
        paste_text: None,
        command: None,
        agent: None,
    }
}

pub(super) fn agent_plan(task: &str, speech: &str) -> TurnPlan {
    TurnPlan {
        agent: Some(AgentRequest {
            kind: crate::managers::converse_plan::StepKind::General,
            task: task.to_string(),
        }),
        ..tp(PlanKind::Agent, speech)
    }
}

pub(super) fn set_role(role: &str, speech: &str) -> TurnPlan {
    TurnPlan {
        role: Some(RoleDirective {
            role: role.to_string(),
            task: None,
            resume: false,
        }),
        ..tp(PlanKind::SetRole, speech)
    }
}

pub(super) fn paste_plan(text: &str, speech: &str) -> TurnPlan {
    TurnPlan {
        paste_text: Some(text.to_string()),
        ..tp(PlanKind::Paste, speech)
    }
}

pub(super) fn cmd_plan(command: &str, speech: &str) -> TurnPlan {
    TurnPlan {
        command: Some(command.to_string()),
        ..tp(PlanKind::Command, speech)
    }
}

pub(super) fn job_plan(project: &str, task: &str, speech: &str) -> TurnPlan {
    TurnPlan {
        job: Some(JobRequest {
            project: project.to_string(),
            task: task.to_string(),
            agent: None,
            mode: JobMode::Task,
        }),
        ..tp(PlanKind::Job, speech)
    }
}

/// A brain call with no streamed deltas resolving to `p`.
pub(super) fn ok(p: TurnPlan) -> Turn {
    Turn {
        deltas: vec![],
        delay: 0,
        plan: Ok(p),
    }
}

/// A plain Answer call.
pub(super) fn answer(speech: &str) -> Turn {
    ok(tp(PlanKind::Answer, speech))
}

/// An agent plan call.
pub(super) fn agent_turn(task: &str, speech: &str) -> Turn {
    ok(agent_plan(task, speech))
}

// ---- observation helpers ----------------------------------------------------------------------

pub(super) fn entries(r: &Rig) -> Vec<String> {
    r.log.lock().unwrap().clone()
}

/// Everything the mouth spoke, in order: `say` lines and queued sentences.
pub(super) fn spoken(r: &Rig) -> Vec<String> {
    entries(r)
        .into_iter()
        .filter_map(|e| {
            e.strip_prefix("say:")
                .or_else(|| e.strip_prefix("push:"))
                .map(str::to_string)
        })
        .collect()
}

pub(super) fn strs(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

pub(super) fn count_prefix(r: &Rig, prefix: &str) -> usize {
    entries(r).iter().filter(|e| e.starts_with(prefix)).count()
}

pub(super) fn count_entry(r: &Rig, entry: &str) -> usize {
    entries(r).iter().filter(|e| e.as_str() == entry).count()
}

pub(super) fn first_index(r: &Rig, pred: impl Fn(&str) -> bool) -> Option<usize> {
    entries(r).iter().position(|e| pred(e))
}

pub(super) fn listens(r: &Rig) -> usize {
    count_entry(r, "listen")
}

pub(super) fn prepares(r: &Rig) -> usize {
    count_prefix(r, "prepare:")
}

pub(super) fn starts(r: &Rig) -> usize {
    count_prefix(r, "start:")
}

pub(super) fn published(r: &Rig) -> Vec<String> {
    r.hands.published.lock().unwrap().clone()
}

pub(super) fn seen(r: &Rig) -> Vec<Seen> {
    r.brain.seen.lock().unwrap().clone()
}

/// The first brain call whose utterance is `utt`.
pub(super) fn seen_by_utt(r: &Rig, utt: &str) -> Seen {
    seen(r)
        .into_iter()
        .find(|s| s.utterance == utt)
        .unwrap_or_else(|| panic!("no brain call for {utt:?}; calls: {:?}", seen(r)))
}

/// Control handle of the `i`-th started run.
pub(super) fn ctl(r: &Rig, i: usize) -> Arc<RunCtl> {
    r.hands
        .runs
        .lock()
        .unwrap()
        .get(i)
        .unwrap_or_else(|| panic!("run {i} was never started"))
        .clone()
}

/// Pre-registers the next started run and returns its control handle.
pub(super) fn queue_run(r: &Rig) -> Arc<RunCtl> {
    let run = RunCtl::new();
    r.hands.starts.lock().unwrap().push_back(Ok(run.clone()));
    run
}

// ---- tests ------------------------------------------------------------------------------------

/// B1: the planner is told about the agent exactly when the hand-off is on and not answer-only.
#[tokio::test]
async fn b1_agent_enabled_is_handoff_and_not_answer_only() {
    for (handoff, answer_only) in [(true, false), (true, true), (false, false)] {
        let r = rig_with(vec![heard("hello there")], vec![answer("Hi.")]);
        let cfg = SessionConfig {
            agent_handoff: handoff,
            answer_only,
            ..acfg()
        };
        run(&r, &cfg).await;
        assert_eq!(
            seen(&r)[0].agent_enabled,
            handoff && !answer_only,
            "handoff={handoff} answer_only={answer_only}"
        );
    }
}

/// B2: with the hand-off off, an Agent plan is a plain Answer: no prepare, no start.
#[tokio::test]
async fn b2_agent_plan_without_handoff_is_a_plain_answer() {
    let r = rig_with(
        vec![heard(REQ)],
        vec![ok(agent_plan(TASK, "Looking that up."))],
    );
    let cfg = SessionConfig {
        agent_handoff: false,
        ..acfg()
    };
    run(&r, &cfg).await;
    assert_eq!(spoken(&r), strs(&["Looking that up."]));
    assert_eq!(prepares(&r), 0, "no prepare without the hand-off");
    assert_eq!(starts(&r), 0, "no start without the hand-off");
}

/// B2: an empty speech on a plain Answer says the fixed "can't do that here" line.
#[tokio::test]
async fn b2_empty_agent_plan_without_handoff_says_cannot_do_that_here() {
    let r = rig_with(vec![heard(REQ)], vec![ok(agent_plan(TASK, ""))]);
    let cfg = SessionConfig {
        agent_handoff: false,
        ..acfg()
    };
    run(&r, &cfg).await;
    assert_eq!(spoken(&r), strs(&[NOT_EFFECTIVE]));
    assert_eq!(starts(&r), 0);
}

/// B2: answer-only refuses a forged Agent plan with the existing fixed text, with zero prepare and
/// zero start calls, on the streaming and the non-streaming path.
#[tokio::test]
async fn b2_answer_only_refuses_a_forged_agent_plan_with_zero_agent_calls() {
    for (early, ack) in [(false, None), (true, Some(1200))] {
        let r = rig_with(vec![heard(REQ)], vec![agent_turn(TASK, ACK_TEXT)]);
        let cfg = SessionConfig {
            answer_only: true,
            early_speech: early,
            ack_after: ack.map(Duration::from_millis),
            ..acfg()
        };
        run(&r, &cfg).await;
        assert_eq!(
            spoken(&r),
            strs(&[MSG_ANSWER_ONLY]),
            "early={early} ack={ack:?}"
        );
        assert_eq!(prepares(&r), 0);
        assert_eq!(starts(&r), 0);
    }
}

/// B3: prepare gets the task and the current role; the ack is spoken only after start succeeds;
/// the result is published once and recorded as one untrusted note.
#[tokio::test]
async fn b3_request_is_prepared_started_and_acknowledged_after_start() {
    let r = rig_with(
        vec![heard(REQ), heard("thanks")],
        vec![agent_turn(TASK, ACK_TEXT), answer("You're welcome.")],
    );
    let run_ctl = queue_run(&r);
    run_ctl.push(AgentPoll::Event(AgentEvent::Done(RESULT.to_string())));
    run(&r, &acfg()).await;

    assert_eq!(prepares(&r), 1, "one prepare: {:?}", entries(&r));
    assert_eq!(starts(&r), 1, "one start: {:?}", entries(&r));
    let launch = r.hands.launches.lock().unwrap()[0].clone();
    assert_eq!(launch.task, TASK);
    assert_eq!(launch.role, None, "no role was set");

    let start_at = first_index(&r, |e| e.starts_with("start:")).expect("start recorded");
    let ack_at = first_index(&r, |e| {
        e == format!("say:{ACK_TEXT}") || e == format!("push:{ACK_TEXT}")
    })
    .expect("ack spoken");
    assert!(start_at < ack_at, "the ack is spoken after the start");

    let mut expected = vec![ACK_TEXT.to_string()];
    expected.extend(split_chunks(RESULT));
    expected.push("You're welcome.".to_string());
    assert_eq!(spoken(&r), expected);
    assert_eq!(
        published(&r),
        vec![RESULT.to_string()],
        "published once, in full"
    );

    let call = seen_by_utt(&r, "thanks");
    let note = note_agent_result(RESULT);
    let notes: Vec<_> = call.history.iter().filter(|(_, a)| *a == note).collect();
    assert_eq!(
        notes.len(),
        1,
        "history holds one agent note: {:?}",
        call.history
    );
    assert_eq!(
        call.history
            .iter()
            .filter(|(_, a)| a.contains("agent_result"))
            .count(),
        1,
        "only one agent note in history"
    );
}

/// B3: a failed or busy start replaces the ack with the matching fixed line.
#[tokio::test]
async fn b3_a_failed_or_busy_start_replaces_the_ack() {
    let cases = [
        (AgentStartError::Failed("boom".to_string()), CANT_REACH),
        (AgentStartError::Busy, AGENT_BUSY),
    ];
    for (err, line) in cases {
        let r = rig_with(vec![heard(REQ)], vec![agent_turn(TASK, ACK_TEXT)]);
        r.hands.starts.lock().unwrap().push_back(Err(err.clone()));
        run(&r, &acfg()).await;
        assert_eq!(spoken(&r), strs(&[line]), "start error {err:?}");
    }
}

/// B3: an unsupported, unconfigured or failing prepare says the reach line and starts nothing.
#[tokio::test]
async fn b3_a_failed_prepare_says_cannot_reach_and_starts_nothing() {
    let errors = [
        AgentStartError::Unsupported,
        AgentStartError::NotConfigured,
        AgentStartError::Failed("x".to_string()),
    ];
    for err in errors {
        let r = rig_with(vec![heard(REQ)], vec![agent_turn(TASK, ACK_TEXT)]);
        r.hands.prepare_errs.lock().unwrap().push_back(err.clone());
        run(&r, &acfg()).await;
        assert_eq!(spoken(&r), strs(&[CANT_REACH]), "prepare error {err:?}");
        assert_eq!(starts(&r), 0, "no start after a failed prepare");
    }
}

/// B3: a second Agent plan while the run is Running speaks the busy line and makes no
/// prepare or start call.
#[tokio::test]
async fn b3_second_agent_request_while_running_is_refused_without_prepare() {
    let r = rig_with(
        vec![heard(REQ), heard("check the disk too")],
        vec![
            agent_turn(TASK, ACK_TEXT),
            agent_turn("check the disk", "Checking the disk."),
        ],
    );
    let first = queue_run(&r);
    run(&r, &acfg()).await;
    assert_eq!(spoken(&r), strs(&[ACK_TEXT, BUSY_RUNNING]));
    assert_eq!(prepares(&r), 1, "no second prepare");
    assert_eq!(starts(&r), 1, "no second start");
    assert_eq!(
        first.cancels(),
        1,
        "the session end cancels the first run once"
    );
}

/// B3: a result over 600 chars is spoken as the first 600 chars in chunks, then the pointer line;
/// the full text is published once.
#[tokio::test]
async fn b3_long_result_is_spoken_in_chunks_then_points_to_the_screen() {
    let long = long_result();
    let spoken_part: String = long.chars().take(600).collect();
    let chunks = split_chunks(&spoken_part);
    assert!(chunks.len() >= 2, "precondition: several chunks");

    let r = rig_with(vec![heard(REQ)], vec![agent_turn(TASK, ACK_TEXT)]);
    let run_ctl = queue_run(&r);
    run_ctl.push(AgentPoll::Event(AgentEvent::Done(long.clone())));
    run(&r, &acfg()).await;

    let mut expected = vec![ACK_TEXT.to_string()];
    expected.extend(chunks);
    expected.push(FULL_ON_SCREEN.to_string());
    assert_eq!(spoken(&r), expected);
    assert_eq!(published(&r), vec![long]);
}

/// B3: an empty Agent speech defaults to "On it.".
#[tokio::test]
async fn b3_empty_agent_speech_defaults_to_on_it() {
    let r = rig_with(vec![heard(REQ)], vec![agent_turn(TASK, "")]);
    run(&r, &acfg()).await;
    assert_eq!(spoken(&r).first().map(String::as_str), Some(ACK_DEFAULT));
}

/// B3: progress appears at 20 s and only once in the following seconds.
#[tokio::test]
async fn b3_progress_appears_at_twenty_seconds_and_not_again_soon() {
    let run_ctl = RunCtl::new();
    let r = rig_with(
        vec![
            heard(REQ),
            with_hook(silent(), {
                let c = run_ctl.clone();
                move || c.set_elapsed(19)
            }),
            with_hook(silent(), {
                let c = run_ctl.clone();
                move || c.set_elapsed(25)
            }),
            with_hook(silent(), {
                let c = run_ctl.clone();
                move || c.set_elapsed(30)
            }),
        ],
        vec![agent_turn(TASK, ACK_TEXT)],
    );
    r.hands
        .starts
        .lock()
        .unwrap()
        .push_back(Ok(run_ctl.clone()));
    run(&r, &acfg()).await;
    assert_eq!(
        spoken(&r),
        strs(&[ACK_TEXT, PROGRESS]),
        "exactly one progress line"
    );
}

/// B3: a run that is 59 s old is not yet over the deadline (3 x 10 s + 30 s = 60 s): no
/// "took too long" line, and the run is still outstanding until the session ends.
#[tokio::test]
async fn b3_a_run_at_fifty_nine_seconds_is_not_timed_out() {
    let run_ctl = RunCtl::new();
    let c = run_ctl.clone();
    let r = rig_with(
        vec![heard(REQ), with_hook(silent(), move || c.set_elapsed(59))],
        vec![agent_turn(TASK, ACK_TEXT)],
    );
    r.hands
        .starts
        .lock()
        .unwrap()
        .push_back(Ok(run_ctl.clone()));
    run(&r, &acfg()).await;
    assert!(
        !spoken(&r).iter().any(|s| s == TOO_LONG_RUN),
        "no deadline line at 59 s: {:?}",
        spoken(&r)
    );
}

/// B3 / A3: the absolute deadline (60 s) cancels the run once, speaks the timeout line, frees the
/// slot, so a later Agent request starts again.
#[tokio::test]
async fn b3_deadline_cancels_once_speaks_and_frees_the_slot() {
    let run_ctl = RunCtl::new();
    let c = run_ctl.clone();
    let r = rig_with(
        vec![
            heard(REQ),
            with_hook(silent(), move || c.set_elapsed(61)),
            heard("try the request again"),
        ],
        vec![
            agent_turn(TASK, ACK_TEXT),
            agent_turn("try the request again", "Trying again."),
        ],
    );
    r.hands
        .starts
        .lock()
        .unwrap()
        .push_back(Ok(run_ctl.clone()));
    run(&r, &acfg()).await;
    assert_eq!(run_ctl.cancels(), 1, "the deadline cancels the run once");
    let s = spoken(&r);
    let timed_out = s.iter().filter(|x| *x == TOO_LONG_RUN).count();
    assert_eq!(timed_out, 1, "one timeout line: {s:?}");
    let line_at = s.iter().position(|x| x == TOO_LONG_RUN).unwrap();
    let again_at = s.iter().position(|x| x == "Trying again.").unwrap();
    assert!(
        line_at < again_at,
        "the timeout line comes before the new request"
    );
    assert_eq!(starts(&r), 2, "the slot was freed: a second start happened");
}

/// B4 / A3: with max_idle_turns = 1 and NoSpeech while the run is Pending, the session stays
/// alive; it ends only when the listen itself aborts, and the run is cancelled once.
#[tokio::test]
async fn b4_idle_one_with_a_pending_run_keeps_the_session_alive() {
    let r = rig_with(
        vec![heard(REQ), silent(), silent(), silent()],
        vec![agent_turn(TASK, ACK_TEXT)],
    );
    let run_ctl = queue_run(&r);
    let cfg = SessionConfig {
        max_idle_turns: 1,
        ..acfg()
    };
    let end = run(&r, &cfg).await;
    assert_eq!(end, SessionEnd::Cancelled, "ended by the abort, not idle");
    assert_eq!(listens(&r), 5, "req, three silences and the abort");
    assert_eq!(run_ctl.cancels(), 1, "cancelled once at the session end");
    assert!(!spoken(&r).iter().any(|s| s == MSG_STOP_LISTENING));
}

/// B4 / A3: each terminal event arriving together with NoSpeech is spoken before the idle end,
/// and the idle budget restarts afterwards (so the session ends on the next idle, not before).
#[tokio::test]
async fn b4_terminal_with_nospeech_is_spoken_before_idle_ends_the_session() {
    #[derive(Clone, Copy, Debug)]
    enum Terminal {
        Done,
        Failed,
        Lost,
        Deadline,
    }
    let variants = [
        Terminal::Done,
        Terminal::Failed,
        Terminal::Lost,
        Terminal::Deadline,
    ];
    for variant in variants {
        let run_ctl = RunCtl::new();
        let c = run_ctl.clone();
        let r = rig_with(
            vec![
                heard(REQ),
                with_hook(silent(), move || match variant {
                    Terminal::Done => {
                        c.push(AgentPoll::Event(AgentEvent::Done(RESULT.to_string())))
                    }
                    Terminal::Failed => c.push(AgentPoll::Event(AgentEvent::Failed(
                        FAILED_LINE.to_string(),
                    ))),
                    Terminal::Lost => c.push(AgentPoll::Lost),
                    Terminal::Deadline => c.set_elapsed(61),
                }),
                silent(),
            ],
            vec![agent_turn(TASK, ACK_TEXT)],
        );
        r.hands
            .starts
            .lock()
            .unwrap()
            .push_back(Ok(run_ctl.clone()));
        let cfg = SessionConfig {
            max_idle_turns: 1,
            ..acfg()
        };
        let end = run(&r, &cfg).await;
        assert_eq!(end, SessionEnd::Idle, "{variant:?}: ends on the next idle");
        assert_eq!(
            listens(&r),
            3,
            "{variant:?}: utterance, terminal silence, idle silence"
        );
        let s = spoken(&r);
        assert_eq!(
            s.last().map(String::as_str),
            Some(MSG_STOP_LISTENING),
            "{variant:?}: {s:?}"
        );
        let expected_line = match variant {
            Terminal::Done => RESULT.to_string(),
            Terminal::Failed => FAILED_LINE.to_string(),
            Terminal::Lost => LOST.to_string(),
            Terminal::Deadline => TOO_LONG_RUN.to_string(),
        };
        assert_eq!(
            s.get(s.len().wrapping_sub(2)),
            Some(&expected_line),
            "{variant:?}: the terminal line is the last line before the idle end: {s:?}"
        );
    }
}

/// B4: a microphone failure while the run is pending ends the session and cancels the run once.
#[tokio::test]
async fn b4_microphone_failure_while_pending_ends_and_cancels_once() {
    let r = rig_with(
        vec![
            heard(REQ),
            outcome(ListenOutcome::Failed(ListenFailure::Other)),
        ],
        vec![agent_turn(TASK, ACK_TEXT)],
    );
    let run_ctl = queue_run(&r);
    let end = run(&r, &acfg()).await;
    assert_eq!(end, SessionEnd::AudioUnavailable);
    assert_eq!(spoken(&r).last().map(String::as_str), Some(MSG_MIC));
    assert_eq!(run_ctl.cancels(), 1);
}

/// B4: the end phrase while the run is pending ends the session and cancels the run once.
#[tokio::test]
async fn b4_end_phrase_while_pending_ends_and_cancels_once() {
    let r = rig_with(
        vec![heard(REQ), heard("stop")],
        vec![agent_turn(TASK, ACK_TEXT)],
    );
    let run_ctl = queue_run(&r);
    let end = run(&r, &acfg()).await;
    assert_eq!(end, SessionEnd::UserEnded);
    assert_eq!(spoken(&r).last().map(String::as_str), Some(MSG_OKAY));
    assert_eq!(run_ctl.cancels(), 1);
}

/// B4: a session cancel while the run is pending ends the session and cancels the run once.
#[tokio::test]
async fn b4_session_cancel_while_pending_cancels_once() {
    let r = rig_with(vec![heard(REQ)], vec![agent_turn(TASK, ACK_TEXT)]);
    let run_ctl = queue_run(&r);
    let tx = r.cancel.clone();
    r.ears
        .script
        .lock()
        .unwrap()
        .push_back(with_hook(silent(), move || {
            tx.send_replace(true);
        }));
    let end = run(&r, &acfg()).await;
    assert_eq!(end, SessionEnd::Cancelled);
    assert_eq!(run_ctl.cancels(), 1);
}

/// B4: TooLong outcomes while the run is pending never trigger the two-in-a-row shutdown.
#[tokio::test]
async fn b4_too_long_while_pending_does_not_end_the_session() {
    let r = rig_with(
        vec![
            heard(REQ),
            outcome(ListenOutcome::TooLong),
            outcome(ListenOutcome::TooLong),
            outcome(ListenOutcome::TooLong),
        ],
        vec![agent_turn(TASK, ACK_TEXT)],
    );
    let run_ctl = queue_run(&r);
    let end = run(&r, &acfg()).await;
    assert_eq!(
        end,
        SessionEnd::Cancelled,
        "the abort ends it, not the too-long rule"
    );
    assert_eq!(run_ctl.cancels(), 1);
}

/// B4 / A3: an event that arrives while a listen is in flight waits for the listen to return; the
/// utterance heard in that listen is dispatched first, then the result is spoken.
#[tokio::test]
async fn b4_event_during_a_listen_is_delivered_after_the_utterance() {
    let run_ctl = RunCtl::new();
    let c = run_ctl.clone();
    let r = rig_with(
        vec![
            heard(REQ),
            with_hook(heard("what time is it"), move || {
                c.push(AgentPoll::Event(AgentEvent::Done(RESULT.to_string())));
            }),
        ],
        vec![agent_turn(TASK, ACK_TEXT), answer("It is noon.")],
    );
    r.hands
        .starts
        .lock()
        .unwrap()
        .push_back(Ok(run_ctl.clone()));
    run(&r, &acfg()).await;

    let utter_at = first_index(&r, |e| e == "plan:what time is it").expect("utterance dispatched");
    let noon_at = first_index(&r, |e| e == "say:It is noon.").expect("answer spoken");
    let result_at = first_index(&r, |e| e == format!("say:{RESULT}")).expect("result spoken");
    assert!(utter_at < noon_at, "the utterance is dispatched first");
    assert!(noon_at < result_at, "the result comes after the answer");
}

/// B5: a barge latched during an Answer that captured "stop", while the run completed meanwhile,
/// dispatches the captured End before any result speech.
#[tokio::test]
async fn b5_captured_end_is_dispatched_before_result_speech() {
    let run_ctl = RunCtl::new();
    let c = run_ctl.clone();
    let r = rig_with(
        vec![
            heard(REQ),
            with_hook(heard("tell me about birds"), move || {
                c.push(AgentPoll::Event(AgentEvent::Done(RESULT.to_string())));
            }),
        ],
        vec![agent_turn(TASK, ACK_TEXT), answer("Here is the answer.")],
    );
    r.hands
        .starts
        .lock()
        .unwrap()
        .push_back(Ok(run_ctl.clone()));
    let mut r = r;
    r.ears.barge_on = true;
    r.barge
        .captures
        .lock()
        .unwrap()
        .push_back("stop".to_string());
    *r.mouth.fire_after_said.lock().unwrap() = Some("Here is the answer.".to_string());

    let end = run(&r, &acfg()).await;
    assert_eq!(
        end,
        SessionEnd::UserEnded,
        "the captured stop ends the session"
    );
    assert_eq!(
        spoken(&r),
        strs(&[ACK_TEXT, "Here is the answer.", MSG_OKAY]),
        "no result or progress speech before the captured End"
    );
}

/// B5 / A4: the result speech is interruptible by a real barge: speech stops at once, the
/// remaining chunks are dropped, and the captured utterance becomes the next turn.
#[tokio::test]
async fn b5_result_speech_is_cut_by_a_real_barge() {
    let chunks = split_chunks(CHUNKED);
    assert!(chunks.len() >= 2, "precondition: several chunks");
    let r = rig_with(
        vec![heard(REQ)],
        vec![agent_turn(TASK, ACK_TEXT), answer("Four.")],
    );
    let run_ctl = queue_run(&r);
    run_ctl.push(AgentPoll::Event(AgentEvent::Done(CHUNKED.to_string())));
    let mut r = r;
    r.ears.barge_on = true;
    r.barge
        .captures
        .lock()
        .unwrap()
        .push_back("what is two plus two".to_string());
    *r.mouth.fire_after_said.lock().unwrap() = Some(chunks[0].clone());

    run(&r, &acfg()).await;

    assert_eq!(
        spoken(&r),
        vec![ACK_TEXT.to_string(), chunks[0].clone(), "Four.".to_string()],
        "chunk one, then the captured turn; the other chunks are dropped"
    );
    assert_eq!(published(&r), vec![CHUNKED.to_string()], "published once");
    assert!(seen(&r)
        .iter()
        .any(|s| s.utterance == "what is two plus two"));
}

/// B5 / A4: a barge latched during the ack dispatches the captured utterance first; the queued
/// result is spoken only after that turn.
#[tokio::test]
async fn b5_barge_before_a_queued_result_dispatches_the_captured_utterance_first() {
    let r = rig_with(
        vec![heard(REQ)],
        vec![agent_turn(TASK, ACK_TEXT), answer("Four.")],
    );
    let run_ctl = queue_run(&r);
    run_ctl.push(AgentPoll::Event(AgentEvent::Done(RESULT.to_string())));
    let mut r = r;
    r.ears.barge_on = true;
    r.barge
        .captures
        .lock()
        .unwrap()
        .push_back("what is two plus two".to_string());
    *r.mouth.fire_after_said.lock().unwrap() = Some(ACK_TEXT.to_string());

    run(&r, &acfg()).await;

    let captured_at = first_index(&r, |e| e == "plan:what is two plus two").expect("captured turn");
    let result_at = first_index(&r, |e| e == format!("say:{RESULT}")).expect("result spoken");
    assert!(
        captured_at < result_at,
        "the captured utterance runs before the result"
    );
    assert_eq!(spoken(&r).last().map(String::as_str), Some(RESULT));
}

/// B6: "cancel the request", "cancel that", "cancel the agent", "stop the agent" cancel the run
/// once, speak "Cancelled.", and never reach the planner.
#[tokio::test]
async fn b6_cancel_phrases_cancel_the_run_once_and_speak_cancelled() {
    for phrase in [
        "cancel the request",
        "cancel that",
        "cancel the agent",
        "stop the agent",
    ] {
        let r = rig_with(
            vec![heard(REQ), heard(phrase)],
            vec![agent_turn(TASK, ACK_TEXT)],
        );
        let run_ctl = queue_run(&r);
        run(&r, &acfg()).await;
        assert_eq!(run_ctl.cancels(), 1, "{phrase}: cancelled once");
        assert_eq!(spoken(&r), strs(&[ACK_TEXT, CANCELLED]), "{phrase}");
        assert_eq!(
            seen(&r).len(),
            1,
            "{phrase}: the phrase must not reach the planner"
        );
    }
}

/// B6 / A5: a late Done or Failed from a cancelled run is suppressed: not spoken, not published,
/// not recorded in history.
#[tokio::test]
async fn b6_a_late_result_after_cancel_is_suppressed() {
    let lates = [
        AgentEvent::Done(RESULT.to_string()),
        AgentEvent::Failed("late failure".to_string()),
    ];
    for late in lates {
        let run_ctl = RunCtl::new();
        let c = run_ctl.clone();
        let late_for_hook = late.clone();
        let r = rig_with(
            vec![
                heard(REQ),
                heard("cancel that"),
                with_hook(silent(), move || {
                    c.push(AgentPoll::Event(late_for_hook.clone()))
                }),
                heard("thanks"),
            ],
            vec![agent_turn(TASK, ACK_TEXT), answer("You're welcome.")],
        );
        r.hands
            .starts
            .lock()
            .unwrap()
            .push_back(Ok(run_ctl.clone()));
        run(&r, &acfg()).await;
        assert_eq!(
            spoken(&r),
            strs(&[ACK_TEXT, CANCELLED, "You're welcome."]),
            "{late:?}: the late event is not spoken"
        );
        assert_eq!(published(&r).len(), 0, "{late:?}: nothing published");
        let call = seen_by_utt(&r, "thanks");
        assert!(
            !call.history.iter().any(|(_, a)| a.contains("agent_result")),
            "{late:?}: nothing recorded in history"
        );
    }
}

/// B6 / A5: after cancelling, the slot is released once 10 s have passed: a new request starts.
#[tokio::test]
async fn b6_slot_is_released_ten_seconds_after_cancel() {
    let run_ctl = RunCtl::new();
    let c = run_ctl.clone();
    let r = rig_with(
        vec![
            heard(REQ),
            heard("cancel that"),
            with_hook(silent(), move || c.set_elapsed(40)),
            heard("try the request again"),
        ],
        vec![
            agent_turn(TASK, ACK_TEXT),
            agent_turn("try the request again", "Trying again."),
        ],
    );
    r.hands
        .starts
        .lock()
        .unwrap()
        .push_back(Ok(run_ctl.clone()));
    run(&r, &acfg()).await;
    let s: Vec<String> = spoken(&r).into_iter().filter(|x| x != PROGRESS).collect();
    assert_eq!(s, strs(&[ACK_TEXT, CANCELLED, "Trying again."]));
    assert_eq!(starts(&r), 2, "the second request started");
}

/// B6: an Agent plan while the cancelled run is still shutting down speaks the shutting-down line
/// and starts nothing.
#[tokio::test]
async fn b6_agent_plan_while_shutting_down_speaks_the_line() {
    let r = rig_with(
        vec![
            heard(REQ),
            heard("cancel that"),
            heard("try the request again"),
        ],
        vec![
            agent_turn(TASK, ACK_TEXT),
            agent_turn("try the request again", "Trying again."),
        ],
    );
    let run_ctl = queue_run(&r);
    run(&r, &acfg()).await;
    assert_eq!(spoken(&r), strs(&[ACK_TEXT, CANCELLED, BUSY_CANCELLING]));
    assert_eq!(starts(&r), 1, "no second start while shutting down");
    assert_eq!(run_ctl.cancels(), 1);
}

/// B6: with no run active the cancel phrases are not local: they reach the planner.
#[tokio::test]
async fn b6_cancel_phrases_without_a_run_reach_the_planner() {
    for phrase in ["cancel that", "cancel the request"] {
        let r = rig_with(vec![heard(phrase)], vec![answer("Which one?")]);
        run(&r, &acfg()).await;
        assert_eq!(seen(&r)[0].utterance, phrase);
        assert_eq!(spoken(&r), strs(&["Which one?"]));
    }
}

/// B7: local intents (exact user phrases, dictation, cancel job) win while a run is active and
/// never reach the agent; the run is not cancelled by them.
#[tokio::test]
async fn b7_local_intents_never_reach_the_agent_while_running() {
    let r = rig_with(
        vec![
            heard(REQ),
            heard("open Safari"),
            heard("dictate hello there"),
            heard("cancel the job"),
        ],
        vec![agent_turn(TASK, ACK_TEXT)],
    );
    r.hands.active_job.store(true, Ordering::SeqCst);
    r.hands
        .user
        .lock()
        .unwrap()
        .push_back(PhraseOutcome::Done("Opened Safari.".to_string()));
    let run_ctl = queue_run(&r);
    run(&r, &acfg()).await;

    assert_eq!(count_entry(&r, "user:open Safari"), 1, "exact tier ran");
    assert_eq!(count_entry(&r, "paste:hello there"), 1, "dictation pasted");
    assert_eq!(count_entry(&r, "cancel_job"), 1, "cancel job ran locally");
    assert_eq!(starts(&r), 1, "no local intent started another agent run");
    assert_eq!(run_ctl.cancels(), 1, "only the session end cancels the run");
    assert_eq!(seen(&r).len(), 1, "no local intent reached the planner");
}

/// B8: a session end of any kind cancels an outstanding run once and drops it.
#[tokio::test]
async fn b8_session_end_cancels_an_outstanding_run_once() {
    let r = rig_with(vec![heard(REQ)], vec![agent_turn(TASK, ACK_TEXT)]);
    let run_ctl = queue_run(&r);
    let end = run(&r, &acfg()).await;
    assert_eq!(end, SessionEnd::Cancelled);
    assert_eq!(run_ctl.cancels(), 1);
}

/// B8: a new conversation starts with no run slot and no role: the role of the earlier session is
/// not handed to the agent or the planner.
#[tokio::test]
async fn b8_a_new_conversation_starts_with_no_slot_and_no_role() {
    let r = rig_with(
        vec![heard("act like a teacher"), heard(REQ)],
        vec![
            ok(set_role(ROLE_TEACHER, "Okay.")),
            agent_turn(TASK, ACK_TEXT),
        ],
    );
    let first = queue_run(&r);
    run(&r, &acfg()).await;
    assert_eq!(
        first.cancels(),
        1,
        "session one ended with the run outstanding"
    );
    assert_eq!(
        r.hands.launches.lock().unwrap()[0].role.as_deref(),
        Some(ROLE_TEACHER),
        "precondition: the role was passed to the first request"
    );

    r.ears
        .script
        .lock()
        .unwrap()
        .push_back(heard("another request"));
    r.brain
        .script
        .lock()
        .unwrap()
        .push_back(agent_turn("another request", "Looking."));
    run(&r, &acfg()).await;

    assert_eq!(starts(&r), 2, "the slot was free in the new conversation");
    assert_eq!(
        r.hands.launches.lock().unwrap()[1].role,
        None,
        "no role carried into the new conversation"
    );
    assert_eq!(seen_by_utt(&r, "another request").role, None);
}
