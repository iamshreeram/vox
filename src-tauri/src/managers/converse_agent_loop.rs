//! Stage 2 agent hand-off inside the conversation loop (DESIGN-PR2-STAGE2 Revision 3, sections
//! 2.1, 2.3, 2.4 and 2.6) and the Stage 4 planner executor (DESIGN-PR4-PLANNER, sections 2.3 to
//! 2.5, A1, A4).
//!
//! A child module of `converse`, so it reads the loop's private items. The loop calls:
//! - `collect_agent_events` at every collection point (never speaks);
//! - `advance_plan` at the safe boundary, after the queued speech and before any listen;
//! - `speak_unsolicited` to deliver one queued narration in its own reply scope;
//! - `agent_turn` for an Agent request, `plan_turn` for a Plan, `cancel_agent_run` for the voice
//!   cancel;
//! - `end_agent_session` on every session end.

use super::*;
use crate::managers::converse_agent::{
    cap_result, plan_confirmation_prompt, AgentEvent, AgentPoll, AgentRun, AgentStartError,
    PreparedAgentLaunch, StepInput, AGENT_FAILED_FALLBACK,
};
use crate::managers::converse_context::note_agent_result;
use crate::managers::converse_plan::{
    default_plan_speech, PlanStep, StepKind, TurnPlan, MAX_PLAN_STEPS, MAX_STEP_NEEDS,
    MIN_PLAN_STEPS,
};

/// Spoken for an Agent plan while the previous run still runs (2.3).
const LINE_RUNNING: &str = "Still working on the last request. Say cancel to stop it.";
/// Spoken for an Agent plan while a cancelled run is still shutting down (2.3).
const LINE_SHUTTING_DOWN: &str = "The agent is still shutting down. Try again in a moment.";
/// Spoken when the hand-off could not be prepared or the agent could not be reached.
const LINE_CANT_REACH: &str = "I can't reach the agent right now.";
/// Spoken when another caller holds the agent.
const LINE_BUSY: &str = "The agent is busy right now.";
/// Spoken for an Agent plan while the hand-off is not effective (no plan speech).
pub(super) const LINE_NOT_EFFECTIVE: &str = "I can't do that here.";
/// Spoken once the agent is cancelled by voice.
pub(super) const LINE_CANCELLED: &str = "Cancelled.";
/// Spoken when the run is still going after 20 s, and at most once per further 60 s.
const LINE_PROGRESS: &str = "Still working on it.";
/// Spoken when the worker vanished without a terminal event.
const LINE_LOST: &str = "The agent stopped unexpectedly.";
/// Spoken when the absolute deadline passed.
const LINE_TIMEOUT: &str = "The agent took too long.";
/// Spoken after a result longer than the speech cap.
const LINE_ON_SCREEN: &str = "The full answer is on screen.";
/// The acknowledgement when the plan carries no speech.
const ACK_DEFAULT: &str = "On it.";
/// User side of the history pair that records an agent result.
const HISTORY_USER_AGENT: &str = "(agent request)";
/// The failure text the agent worker reports when the settings changed before admission.
const AGENT_SETTINGS_CHANGED: &str = "The agent settings changed.";
/// Spoken when a plan step's launch settings changed before the step could start (A2, A3).
const LINE_PLAN_SETTINGS_CHANGED: &str = "The agent settings changed. Stopping the plan.";
/// Spoken when the plan clock passed its cap (A4).
const LINE_PLAN_TOO_LONG: &str = "The plan took too long. Stopping.";
/// Spoken when a confirmed plan's settings changed between the yes and the first start.
pub(super) const LINE_SETTINGS_CHANGED: &str = "The agent settings changed. Please ask again.";

/// Longest result text spoken aloud (the rest is on screen).
const SPOKEN_RESULT_CHARS: usize = 600;
/// Longest failure line spoken aloud.
const FAILURE_LINE_CHARS: usize = 200;
const PROGRESS_FIRST: Duration = Duration::from_secs(20);
const PROGRESS_EVERY: Duration = Duration::from_secs(60);
/// A cancelled run is released this long after the cancel, even without a terminal event.
const CANCEL_RELEASE: Duration = Duration::from_secs(10);
/// Added to three launch timeouts for the absolute deadline (ACP's own deadline is three times).
const DEADLINE_SLACK: Duration = Duration::from_secs(30);
/// Stage 4 (A4): the plan clock cap, from the first step's start. Gaps between runs count.
pub(super) const PLAN_MAX_TOTAL: Duration = Duration::from_secs(15 * 60);

/// Whether the agent hand-off can run in this session (DESIGN 2.1): enabled and not answer-only.
pub(super) fn handoff_effective(cfg: &SessionConfig) -> bool {
    cfg.agent_handoff && !cfg.answer_only
}

/// Whether a planner-derived plan can run (DESIGN 2.1, 2.5): the hand-off and the planner are on,
/// and the session is not answer-only.
pub(super) fn plan_effective(cfg: &SessionConfig) -> bool {
    cfg.agent_handoff && cfg.agent_planner && !cfg.answer_only
}

/// Lifecycle of the owned run. `Cancelling` keeps the slot until the worker reports, so a second
/// request cannot start while the first is still shutting down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Running,
    Cancelling { since: Duration },
}

/// Stage 4 (DESIGN 2.3): one prepared plan step. Its launch is built before the first step starts.
pub(super) struct PreparedStep {
    kind: StepKind,
    task: String,
    needs: Vec<u8>,
    launch: PreparedAgentLaunch,
}

/// Stage 4 (DESIGN 2.4): the prepared steps of a confirmed plan and the results so far. Only the
/// slot's one live run belongs to it; the steps are immutable once planned.
pub(super) struct PlanRun {
    steps: Vec<PreparedStep>,
    /// Index of the step whose run is live, or was last live.
    current: usize,
    /// The capped result text of each step, once that step finished.
    results: Vec<Option<String>>,
    /// Plan clock start: the moment the first step started (`agent_now`).
    started_at: Instant,
}

/// The one agent run of a conversation (DESIGN 2.3).
pub(super) struct AgentSlot {
    /// Behind a mutex so the loop future stays `Sync` (the run trait is only `Send`).
    run: Mutex<Box<dyn AgentRun>>,
    turn_timeout: Duration,
    phase: Phase,
    /// Run time of the last progress line, if one was spoken.
    last_progress: Option<Duration>,
    /// Set once the run was cancelled or reported a terminal event. Nothing is cancelled again.
    settled: bool,
    /// The plan this run belongs to, if any (Stage 4).
    plan: Option<PlanRun>,
    /// A plan step finished and its successor has not started: no run is live (Stage 4, A1).
    between: bool,
}

/// A slot dropped without a terminal event (for example by a panic that unwinds through the loop)
/// cancels its run once, so no agent keeps running after its owner is gone.
impl Drop for AgentSlot {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl AgentSlot {
    fn new(run: Box<dyn AgentRun>, turn_timeout: Duration) -> AgentSlot {
        AgentSlot {
            run: Mutex::new(run),
            turn_timeout,
            phase: Phase::Running,
            last_progress: None,
            settled: false,
            plan: None,
            between: false,
        }
    }

    fn poll(&self) -> AgentPoll {
        lock_or_recover(&self.run).poll()
    }

    fn elapsed(&self) -> Duration {
        lock_or_recover(&self.run).elapsed()
    }

    /// Asks the run to stop, at most once. Never waits.
    fn cancel(&mut self) {
        if !self.settled {
            self.settled = true;
            lock_or_recover(&self.run).cancel();
        }
    }
    /// Absolute deadline from the start of the run: three launch timeouts plus the slack.
    fn deadline(&self) -> Duration {
        self.turn_timeout.saturating_mul(3) + DEADLINE_SLACK
    }
}

/// Speech owed to the user, delivered as one unit (a result, a line). A barge-in drops the rest
/// of the unit.
#[derive(Default)]
pub(super) struct Narration {
    chunks: VecDeque<String>,
}

impl Narration {
    fn line(text: &str) -> Narration {
        Narration {
            chunks: VecDeque::from([text.to_string()]),
        }
    }
}

/// True while a run occupies the agent slot or speech is still owed. The idle rules (2.3) and the
/// too-long streak read this. A plan between two steps occupies the slot too (A1).
pub(super) fn agent_occupied(carry: &Carry) -> bool {
    carry.agent.is_some() || !carry.undelivered.is_empty()
}

/// True when a run is in the `Running` phase (the voice cancel phrases are local only then). A
/// plan between two steps is `Running` as well.
pub(super) fn agent_running(carry: &Carry) -> bool {
    matches!(
        carry.agent.as_ref().map(|slot| slot.phase),
        Some(Phase::Running)
    )
}

fn push_line(carry: &mut Carry, text: &str) {
    carry.undelivered.push_back(Narration::line(text));
}

fn failure_line(message: &str) -> String {
    let line = truncate_chars(message.trim(), FAILURE_LINE_CHARS);
    if line.is_empty() {
        AGENT_FAILED_FALLBACK.to_string()
    } else {
        line
    }
}

/// The failure line of a plan step (DESIGN 2.4). A settings change stops the plan with its own
/// fixed line.
fn plan_failure_line(position: (usize, usize), message: &str) -> String {
    if message.trim() == AGENT_SETTINGS_CHANGED {
        return LINE_PLAN_SETTINGS_CHANGED.to_string();
    }
    let (step, of) = position;
    format!("Step {step} of {of} failed. {}", failure_line(message))
}

fn progress_due(slot: &AgentSlot, elapsed: Duration) -> bool {
    match slot.last_progress {
        None => elapsed >= PROGRESS_FIRST,
        Some(last) => elapsed >= last + PROGRESS_EVERY,
    }
}

/// A finished result: published once, recorded once as an untrusted note, and queued for speech.
fn finish_done(
    carry: &mut Carry,
    history: &mut Vec<(String, String)>,
    cfg: &SessionConfig,
    hands: &dyn Hands,
    text: &str,
) {
    hands.publish_agent_result(text);
    push_history(
        history,
        HISTORY_USER_AGENT,
        &note_agent_result(text),
        cfg.context_turns,
    );
    carry.tainted = true;
    let spoken: String = text.chars().take(SPOKEN_RESULT_CHARS).collect();
    let mut chunks: VecDeque<String> = split_chunks(&spoken).into();
    if text.chars().count() > SPOKEN_RESULT_CHARS {
        chunks.push_back(LINE_ON_SCREEN.to_string());
    }
    if !chunks.is_empty() {
        carry.undelivered.push_back(Narration { chunks });
    }
}

/// (step number, step count) of the live plan step, one-based.
fn plan_position(plan: &PlanRun) -> (usize, usize) {
    (plan.current + 1, plan.steps.len())
}

/// True once the plan clock reached its cap (A4).
fn plan_over_cap(plan: &PlanRun) -> bool {
    agent_now().saturating_duration_since(plan.started_at) >= PLAN_MAX_TOTAL
}

/// Polls the owned run and turns what it reports into queued speech. Never speaks. Terminal
/// events of a cancelled run are suppressed (2.3, 2.6). A plan step is handled by the plan rules
/// (2.4, A1, A4): its result is recorded and the slot moves to `between`; nothing is started here.
pub(super) fn collect_agent_events(
    carry: &mut Carry,
    history: &mut Vec<(String, String)>,
    cfg: &SessionConfig,
    hands: &dyn Hands,
) {
    let Some(mut slot) = carry.agent.take() else {
        return;
    };
    if slot.between {
        // Nothing is live between two steps: only the plan clock is checked (A4).
        if slot.plan.as_ref().is_some_and(plan_over_cap) {
            push_line(carry, LINE_PLAN_TOO_LONG);
        } else {
            carry.agent = Some(slot);
        }
        return;
    }
    let poll = slot.poll();
    let elapsed = slot.elapsed();
    let keep = match slot.phase {
        Phase::Cancelling { since } => {
            // A late terminal event is dropped silently; so is the release after 10 s.
            poll == AgentPoll::Pending && elapsed < since + CANCEL_RELEASE
        }
        Phase::Running => match poll {
            AgentPoll::Event(AgentEvent::Done(text)) => {
                slot.settled = true;
                if slot.plan.is_some() {
                    plan_step_done(carry, history, cfg, hands, &mut slot, &text)
                } else {
                    finish_done(carry, history, cfg, hands, &text);
                    false
                }
            }
            AgentPoll::Event(AgentEvent::Failed(message)) => {
                slot.settled = true;
                let line = match slot.plan.as_ref() {
                    Some(plan) => plan_failure_line(plan_position(plan), &message),
                    None => failure_line(&message),
                };
                push_line(carry, &line);
                false
            }
            // The Cancelled line was spoken by the voice cancel; nothing more to say.
            AgentPoll::Event(AgentEvent::Cancelled) => {
                slot.settled = true;
                false
            }
            AgentPoll::Lost => {
                slot.cancel();
                let line = match slot.plan.as_ref() {
                    Some(plan) => {
                        let (step, of) = plan_position(plan);
                        format!("Step {step} of {of} stopped unexpectedly. Stopping the plan.")
                    }
                    None => LINE_LOST.to_string(),
                };
                push_line(carry, &line);
                false
            }
            AgentPoll::Pending => {
                if slot.plan.as_ref().is_some_and(plan_over_cap) {
                    slot.cancel();
                    push_line(carry, LINE_PLAN_TOO_LONG);
                    false
                } else if elapsed >= slot.deadline() {
                    slot.cancel();
                    let line = match slot.plan.as_ref() {
                        Some(plan) => {
                            let (step, of) = plan_position(plan);
                            format!("Step {step} of {of} took too long. Stopping the plan.")
                        }
                        None => LINE_TIMEOUT.to_string(),
                    };
                    push_line(carry, &line);
                    false
                } else {
                    if progress_due(&slot, elapsed) {
                        push_line(carry, LINE_PROGRESS);
                        slot.last_progress = Some(elapsed);
                    }
                    true
                }
            }
        },
    };
    if keep {
        carry.agent = Some(slot);
    }
}

/// A plan step finished with a result (DESIGN 2.4, A1). The final step's result is delivered
/// like any result and the plan ends. A non-final result is only recorded: the slot moves to
/// `between` and the successor starts later, at the safe boundary. Returns whether the slot
/// stays. The plan clock check comes first (A4).
fn plan_step_done(
    carry: &mut Carry,
    history: &mut Vec<(String, String)>,
    cfg: &SessionConfig,
    hands: &dyn Hands,
    slot: &mut AgentSlot,
    text: &str,
) -> bool {
    let Some(plan) = slot.plan.as_mut() else {
        return false;
    };
    if plan.current + 1 >= plan.steps.len() {
        finish_done(carry, history, cfg, hands, text);
        return false;
    }
    if plan_over_cap(plan) {
        push_line(carry, LINE_PLAN_TOO_LONG);
        return false;
    }
    if let Some(result) = plan.results.get_mut(plan.current) {
        *result = Some(cap_result(text));
    }
    carry.tainted = true;
    slot.between = true;
    true
}

/// The earlier results a step receives: the results of its `needs`, in the order of `needs`
/// (DESIGN 2.3). A need without a result yields an empty text, which the prompt omits.
fn step_inputs(plan: &PlanRun, next: usize) -> Vec<StepInput> {
    let Some(step) = plan.steps.get(next) else {
        return Vec::new();
    };
    step.needs
        .iter()
        .filter_map(|id| {
            let index = usize::from(*id).checked_sub(1)?;
            let earlier = plan.steps.get(index)?;
            let text = plan.results.get(index)?.clone().unwrap_or_default();
            Some(StepInput {
                id: *id,
                task: earlier.task.clone(),
                text,
            })
        })
        .collect()
}

/// Starts the successor of a finished plan step (DESIGN 2.4, A1, A4). Runs only at the safe
/// boundary: the loop calls it after the queued speech and before any listen, and never while a
/// captured barge-in is pending. Returns true when it changed the state (a line was queued or a
/// step started), so the loop goes round again and speaks the line first. Starting is
/// non-blocking; the launch is re-checked before the start.
pub(super) fn advance_plan(carry: &mut Carry, hands: &dyn Hands) -> bool {
    if !carry.agent.as_ref().is_some_and(|slot| slot.between) {
        return false;
    }
    let Some(mut slot) = carry.agent.take() else {
        return false;
    };
    let Some(plan) = slot.plan.as_mut() else {
        return false;
    };
    if plan_over_cap(plan) {
        push_line(carry, LINE_PLAN_TOO_LONG);
        return true;
    }
    let next = plan.current + 1;
    let Some(step) = plan.steps.get(next) else {
        push_line(
            carry,
            &format!("I couldn't start step {}. Stopping the plan.", next + 1),
        );
        return true;
    };
    let launch = step.launch.clone().with_inputs(step_inputs(plan, next));
    if !hands.agent_launch_still_valid(&launch) {
        push_line(carry, LINE_PLAN_SETTINGS_CHANGED);
        return true;
    }
    let (step_number, of) = (next + 1, plan.steps.len());
    let word = step.kind.spoken_word();
    let turn_timeout = launch.turn_timeout;
    match hands.start_agent(launch) {
        Ok(run) => {
            plan.current = next;
            *lock_or_recover(&slot.run) = run;
            slot.turn_timeout = turn_timeout;
            slot.settled = false;
            slot.between = false;
            slot.last_progress = None;
            carry.agent = Some(slot);
            push_line(carry, &format!("Step {step_number} of {of}: {word}."));
        }
        Err(_) => {
            push_line(
                carry,
                &format!("I couldn't start step {step_number}. Stopping the plan."),
            );
        }
    }
    true
}

/// Delivers one queued narration in its own reply scope (2.4). The scope is opened only here,
/// after the previous one is closed. A barge-in captured while it speaks is returned, so the
/// loop dispatches it next; the rest of the narration is dropped.
pub(super) async fn speak_unsolicited(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    narration: &mut Narration,
) -> Result<Option<String>, SessionEnd> {
    let Some(barge) = deps.ears.barge_in().and_then(|monitor| monitor.arm()) else {
        speak_queue(deps, cfg, cancel, &mut narration.chunks, Emotion::Neutral).await?;
        return Ok(None);
    };
    let reply = barge.reply_token();
    let gate = barge.gate();
    let mut turn_cancel = cancel.with_reply(reply);
    let gated = GatedMouth {
        inner: deps.mouth,
        gate,
    };
    let turn_deps = deps.with_mouth(&gated);
    let spoken = speak_queue(
        &turn_deps,
        cfg,
        &mut turn_cancel,
        &mut narration.chunks,
        Emotion::Neutral,
    )
    .await;
    if cancel.session_cancelled() || !barge.cancel_latched() {
        drop(barge);
        return spoken.map(|_| None);
    }
    deps.mouth.stop();
    narration.chunks.clear();
    capture_barge(cancel, barge).await
}

/// Refuses an Agent plan with one fixed line: the turn is recorded, nothing else happens.
async fn refuse(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    carry: &mut Carry,
    line: &str,
    outcome: TurnOutcome,
) -> Result<String, SessionEnd> {
    carry.journal.set(outcome, None);
    into_result(speak_or_end(deps, cfg, cancel, line, Emotion::Neutral).await)?;
    Ok(compose(line, ""))
}

/// Prepares the launch of an Agent plan without starting it (DESIGN 2.5): the busy lines and the
/// prepare failure are spoken here. `Err` carries the turn summary of a refusal already spoken.
pub(super) async fn prepare_agent_plan(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    plan: &TurnPlan,
    carry: &mut Carry,
) -> Result<Result<PreparedAgentLaunch, String>, SessionEnd> {
    let Some(request) = plan.agent.as_ref() else {
        carry.journal.set(TurnOutcome::Failed, None);
        into_result(speak_or_end(deps, cfg, cancel, MSG_PLAN_FAILED, Emotion::Neutral).await)?;
        return Ok(Err(failed_summary()));
    };
    if let Some(slot) = carry.agent.as_ref() {
        let line = match slot.phase {
            Phase::Running => LINE_RUNNING,
            Phase::Cancelling { .. } => LINE_SHUTTING_DOWN,
        };
        return refuse(deps, cfg, cancel, carry, line, TurnOutcome::Blocked)
            .await
            .map(Err);
    }
    // Stage 4 (F2): a typed single request uses its type's specialist profile and step role;
    // the general (or absent) type keeps the main agent with no step role.
    let prepared = match request.kind {
        StepKind::General => deps
            .hands
            .prepare_agent_launch(&request.task, carry.role.as_deref()),
        kind => deps
            .hands
            .prepare_agent_launch_typed(kind, &request.task, carry.role.as_deref()),
    };
    match prepared {
        Ok(launch) => Ok(Ok(launch)),
        Err(_) => refuse(
            deps,
            cfg,
            cancel,
            carry,
            LINE_CANT_REACH,
            TurnOutcome::Failed,
        )
        .await
        .map(Err),
    }
}

/// An Agent plan under an effective hand-off (2.1, 2.3): busy lines, then prepare, then start.
pub(super) async fn agent_turn(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    plan: &TurnPlan,
    carry: &mut Carry,
) -> Result<String, SessionEnd> {
    match prepare_agent_plan(deps, cfg, cancel, plan, carry).await? {
        Ok(launch) => {
            run_prepared_agent(deps, cfg, cancel, launch, &plan.speech, plan.emotion, carry).await
        }
        Err(summary) => Ok(summary),
    }
}

/// Starts a prepared launch. On success the slot exists before the acknowledgement is spoken; a
/// failed start speaks its line instead of the acknowledgement.
pub(super) async fn run_prepared_agent(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    launch: PreparedAgentLaunch,
    speech: &str,
    emotion: Emotion,
    carry: &mut Carry,
) -> Result<String, SessionEnd> {
    let turn_timeout = launch.turn_timeout;
    match deps.hands.start_agent(launch) {
        Ok(run) => {
            carry.agent = Some(AgentSlot::new(run, turn_timeout));
            let ack = if speech.trim().is_empty() {
                ACK_DEFAULT
            } else {
                speech
            };
            let outcome = speak_outcome(deps, cfg, cancel, ack, emotion).await?;
            Ok(answer_summary(ack, outcome))
        }
        Err(AgentStartError::Busy) => {
            refuse(deps, cfg, cancel, carry, LINE_BUSY, TurnOutcome::Failed).await
        }
        Err(_) => {
            refuse(
                deps,
                cfg,
                cancel,
                carry,
                LINE_CANT_REACH,
                TurnOutcome::Failed,
            )
            .await
        }
    }
}

/// The spoken line of a plan step that could not be reached before it started.
fn cant_reach_line(kind: StepKind) -> String {
    match kind {
        StepKind::General => LINE_CANT_REACH.to_string(),
        other => format!("I can't reach the {} agent right now.", other.spoken_word()),
    }
}

/// Plan shape (DESIGN 2.1): 2 to 5 steps with ids exactly 1..n in order, non-empty tasks, and
/// needs that name only smaller ids, at most three, without duplicates. Checked again here so
/// the loop never trusts a plan it did not validate itself.
fn plan_shape_ok(steps: &[PlanStep]) -> bool {
    (MIN_PLAN_STEPS..=MAX_PLAN_STEPS).contains(&steps.len())
        && steps.iter().enumerate().all(|(index, step)| {
            usize::from(step.id) == index + 1
                && !step.task.trim().is_empty()
                && step.needs.len() <= MAX_STEP_NEEDS
                && step.needs.iter().enumerate().all(|(at, need)| {
                    *need >= 1 && *need < step.id && !step.needs[..at].contains(need)
                })
        })
}

/// Builds every step's launch before anything starts (DESIGN 2.3). The conversation role goes to
/// the final step only. `Err` carries the kind of the first step that could not be prepared.
fn prepare_plan_steps(
    hands: &dyn Hands,
    steps: &[PlanStep],
    role: Option<&str>,
) -> Result<Vec<PreparedStep>, StepKind> {
    let last = steps.len().saturating_sub(1);
    let mut prepared = Vec::with_capacity(steps.len());
    let mut failed: Option<StepKind> = None;
    for (index, step) in steps.iter().enumerate() {
        let step_role = if index == last { role } else { None };
        match hands.prepare_agent_launch_for(step.kind, &step.task, step_role) {
            Ok(launch) => prepared.push(PreparedStep {
                kind: step.kind,
                task: step.task.clone(),
                needs: step.needs.clone(),
                launch,
            }),
            Err(_) => {
                failed.get_or_insert(step.kind);
            }
        }
    }
    match failed {
        Some(kind) => Err(kind),
        None => Ok(prepared),
    }
}

/// A planner plan (DESIGN 2.1 to 2.5, A1 to A5). The busy check, the shape check and the
/// preparation of every step come first; nothing starts before all steps are prepared. With
/// `forced` (a tainted session) the exact yes is asked first. Then every launch is re-checked,
/// step 1 starts, and the announcement is spoken once step 1 runs. Returns the turn summary.
pub(super) async fn plan_turn(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    plan: &TurnPlan,
    carry: &mut Carry,
    forced: bool,
) -> Result<String, SessionEnd> {
    carry.journal.set(speech_outcome(&plan.kind), None);
    if let Some(slot) = carry.agent.as_ref() {
        let line = match slot.phase {
            Phase::Running => LINE_RUNNING,
            Phase::Cancelling { .. } => LINE_SHUTTING_DOWN,
        };
        return refuse(deps, cfg, cancel, carry, line, TurnOutcome::Blocked).await;
    }
    let Some(agent_plan) = plan.plan.as_ref().filter(|p| plan_shape_ok(&p.steps)) else {
        carry.journal.set(TurnOutcome::Failed, None);
        into_result(speak_or_end(deps, cfg, cancel, MSG_PLAN_FAILED, Emotion::Neutral).await)?;
        return Ok(failed_summary());
    };
    let prepared = match prepare_plan_steps(deps.hands, &agent_plan.steps, carry.role.as_deref()) {
        Ok(prepared) => prepared,
        Err(kind) => {
            let line = cant_reach_line(kind);
            return refuse(deps, cfg, cancel, carry, &line, TurnOutcome::Failed).await;
        }
    };
    if forced {
        let question = plan_confirmation_prompt(&agent_plan.steps);
        if let Decision::Declined(reason) = ask_forced(deps, cfg, cancel, carry, &question).await {
            return Ok(decline_forced(deps, cfg, cancel, carry, &reason)
                .await?
                .unwrap_or_default());
        }
    }
    if !prepared
        .iter()
        .all(|step| deps.hands.agent_launch_still_valid(&step.launch))
    {
        let line = if forced {
            LINE_SETTINGS_CHANGED
        } else {
            LINE_PLAN_SETTINGS_CHANGED
        };
        carry.journal.set(TurnOutcome::Blocked, None);
        into_result(speak_or_end(deps, cfg, cancel, line, Emotion::Neutral).await)?;
        return Ok(compose(line, ""));
    }
    let Some(first) = prepared.first().map(|step| step.launch.clone()) else {
        return refuse(
            deps,
            cfg,
            cancel,
            carry,
            LINE_CANT_REACH,
            TurnOutcome::Failed,
        )
        .await;
    };
    let turn_timeout = first.turn_timeout;
    match deps.hands.start_agent(first) {
        Ok(run) => {
            let mut slot = AgentSlot::new(run, turn_timeout);
            slot.plan = Some(PlanRun {
                results: vec![None; prepared.len()],
                steps: prepared,
                current: 0,
                started_at: agent_now(),
            });
            carry.agent = Some(slot);
        }
        Err(AgentStartError::Busy) => {
            return refuse(deps, cfg, cancel, carry, LINE_BUSY, TurnOutcome::Failed).await;
        }
        Err(_) => {
            return refuse(
                deps,
                cfg,
                cancel,
                carry,
                LINE_CANT_REACH,
                TurnOutcome::Failed,
            )
            .await;
        }
    }
    let speech = if plan.speech.trim().is_empty() {
        default_plan_speech(agent_plan)
    } else {
        plan.speech.clone()
    };
    let outcome = speak_outcome(deps, cfg, cancel, &speech, plan.emotion).await?;
    Ok(answer_summary(&speech, outcome))
}

/// Voice cancel (2.6): the run is asked to stop once, without waiting; the slot moves to
/// `Cancelling` until the worker reports. Nothing happens unless a run is `Running`. A plan is
/// dropped at once, so no later step starts (A1). A plan between two steps has no live run to
/// stop: its slot is freed here.
pub(super) fn cancel_agent_run(carry: &mut Carry) {
    let Some(mut slot) = carry.agent.take() else {
        return;
    };
    if slot.phase != Phase::Running {
        carry.agent = Some(slot);
        return;
    }
    if slot.plan.is_some() {
        carry.undelivered.clear();
        slot.plan = None;
        if slot.between {
            return;
        }
    }
    slot.cancel();
    slot.phase = Phase::Cancelling {
        since: slot.elapsed(),
    };
    carry.agent = Some(slot);
}

/// Session end (2.3): a run still `Running` is cancelled once and dropped. Nothing is spoken and
/// nothing waits. A `Cancelling` run was already cancelled.
pub(super) fn end_agent_session(carry: &mut Carry) {
    if let Some(mut slot) = carry.agent.take() {
        if slot.phase == Phase::Running {
            slot.cancel();
        }
    }
    carry.undelivered.clear();
}
