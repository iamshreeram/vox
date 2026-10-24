//! Stage 2 agent hand-off inside the conversation loop (DESIGN-PR2-STAGE2 Revision 3, sections
//! 2.1, 2.3, 2.4 and 2.6).
//!
//! A child module of `converse`, so it reads the loop's private items. The loop calls:
//! - `collect_agent_events` at every collection point (never speaks);
//! - `speak_unsolicited` to deliver one queued narration in its own reply scope;
//! - `agent_turn` for an Agent plan, `cancel_agent_run` for the voice cancel;
//! - `end_agent_session` on every session end.

use super::*;
use crate::managers::converse_agent::{
    AgentEvent, AgentPoll, AgentRun, AgentStartError, PreparedAgentLaunch, AGENT_FAILED_FALLBACK,
};
use crate::managers::converse_context::note_agent_result;

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

/// Whether the agent hand-off can run in this session (DESIGN 2.1): enabled and not answer-only.
pub(super) fn handoff_effective(cfg: &SessionConfig) -> bool {
    cfg.agent_handoff && !cfg.answer_only
}

/// Lifecycle of the owned run. `Cancelling` keeps the slot until the worker reports, so a second
/// request cannot start while the first is still shutting down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Running,
    Cancelling { since: Duration },
}

/// The one agent run of a conversation (DESIGN 2.3).
pub(super) struct AgentSlot {
    /// Behind a mutex so the loop future stays `Sync` (the run trait is only `Send`).
    run: Mutex<Box<dyn AgentRun>>,
    turn_timeout: Duration,
    phase: Phase,
    /// Run time of the last progress line, if one was spoken.
    last_progress: Option<Duration>,
}

impl AgentSlot {
    fn new(run: Box<dyn AgentRun>, turn_timeout: Duration) -> AgentSlot {
        AgentSlot {
            run: Mutex::new(run),
            turn_timeout,
            phase: Phase::Running,
            last_progress: None,
        }
    }

    fn poll(&self) -> AgentPoll {
        lock_or_recover(&self.run).poll()
    }

    fn elapsed(&self) -> Duration {
        lock_or_recover(&self.run).elapsed()
    }

    fn cancel(&self) {
        lock_or_recover(&self.run).cancel();
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
/// too-long streak read this.
pub(super) fn agent_occupied(carry: &Carry) -> bool {
    carry.agent.is_some() || !carry.undelivered.is_empty()
}

/// True when a run is in the `Running` phase (the voice cancel phrases are local only then).
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

/// Polls the owned run and turns what it reports into queued speech. Never speaks. Terminal
/// events of a cancelled run are suppressed (2.3, 2.6).
pub(super) fn collect_agent_events(
    carry: &mut Carry,
    history: &mut Vec<(String, String)>,
    cfg: &SessionConfig,
    hands: &dyn Hands,
) {
    let Some(mut slot) = carry.agent.take() else {
        return;
    };
    let poll = slot.poll();
    let elapsed = slot.elapsed();
    let keep = match slot.phase {
        Phase::Cancelling { since } => {
            // A late terminal event is dropped silently; so is the release after 10 s.
            poll == AgentPoll::Pending && elapsed < since + CANCEL_RELEASE
        }
        Phase::Running => match poll {
            AgentPoll::Event(AgentEvent::Done(text)) => {
                finish_done(carry, history, cfg, hands, &text);
                false
            }
            AgentPoll::Event(AgentEvent::Failed(message)) => {
                push_line(carry, &failure_line(&message));
                false
            }
            // The Cancelled line was spoken by the voice cancel; nothing more to say.
            AgentPoll::Event(AgentEvent::Cancelled) => false,
            AgentPoll::Lost => {
                slot.cancel();
                push_line(carry, LINE_LOST);
                false
            }
            AgentPoll::Pending => {
                if elapsed >= slot.deadline() {
                    slot.cancel();
                    push_line(carry, LINE_TIMEOUT);
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

/// Spoken when the launch settings changed between the question and the yes (DESIGN 2.5, C6).
pub(super) const LINE_SETTINGS_CHANGED: &str = "The agent settings changed. Please ask again.";

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
    match deps
        .hands
        .prepare_agent_launch(&request.task, carry.role.as_deref())
    {
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

/// Voice cancel (2.6): the run is asked to stop once, without waiting; the slot moves to
/// `Cancelling` until the worker reports. Nothing happens unless a run is `Running`.
pub(super) fn cancel_agent_run(carry: &mut Carry) {
    if let Some(slot) = carry.agent.as_mut() {
        if slot.phase == Phase::Running {
            slot.cancel();
            slot.phase = Phase::Cancelling {
                since: slot.elapsed(),
            };
        }
    }
}

/// Session end (2.3): a run still `Running` is cancelled once and dropped. Nothing is spoken and
/// nothing waits. A `Cancelling` run was already cancelled.
pub(super) fn end_agent_session(carry: &mut Carry) {
    if let Some(slot) = carry.agent.take() {
        if slot.phase == Phase::Running {
            slot.cancel();
        }
    }
    carry.undelivered.clear();
}
