//! The conversation session loop: listen -> route -> act/answer -> speak ->
//! listen. Pure and trait-generic (no tauri types); all hardware sits behind
//! the traits below. Contract: DESIGN.md section 3.3 (Revision 4).

use crate::managers::chat_log::ChatRole;
use crate::managers::converse_ack::choose;
use crate::managers::converse_agent::{
    AgentRun, AgentSource, AgentStartError, PreparedAgentLaunch,
};
use crate::managers::converse_barge::{BargeCapture, BargeIn, BargeWatch, ReplyGate};
use crate::managers::converse_confirm::{
    confirm_action_recorded, confirm_config_for, confirm_forced, confirmation_question,
    decline_message, fingerprint_for, forced_prompt, prepare_action, Decision, DeclineReason,
    DirFd, JobLaunch, JobStartOutcome, PinError, PolicySource, PrepareError, PreparedAction,
    ValidatedCommand,
};
use crate::managers::converse_context::{
    compose, neutralize, note_command, note_delivery_failed, note_failed, note_incomplete,
    note_interrupted, note_paste, note_paste_failed, CommandNote,
};
use crate::managers::converse_latency::{LatencyReport, ListenTiming, RollingStats};
use crate::managers::converse_local::{classify_local_in_session, LocalIntent};
use crate::managers::converse_plan::{
    PlanError, PlanInput, PlanKind, StepKind, TurnPlan, MAX_UTTERANCE_CHARS,
};
use crate::managers::converse_role::{
    now as lesson_now, split_chunks, Lesson, CLEARED_SPOKEN, CONTINUE_SPOKEN,
    LESSON_START_FAILED_SPOKEN,
};
use crate::managers::converse_stream::{reconcile, PlanStreamer, Reconcile};
use crate::managers::emotion::Emotion;
use crate::managers::job_announcer::{Announcer, LowPriorityMouth};
// Re-exported so the planner modules and the tests reach the plan clock through `converse::`.
// `advance_plan_clock` is only called from cfg(test) code, so a non-test build can leave the
// re-export without a user; the allow keeps that build free of unused-import warnings.
#[allow(unused_imports)]
pub(crate) use crate::managers::plan_clock::{advance_plan_clock, agent_now};
use futures_util::future::{select, Either};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
/// Cancellation seen by one step of a session: the session stop and, during a reply, the
/// barge-in reply token (DESIGN 12.2.1). `true` on either side means cancelled. Clones share
/// both channels.
#[derive(Clone)]
pub struct CancelToken {
    session: tokio::sync::watch::Receiver<bool>,
    reply: Option<tokio::sync::watch::Receiver<bool>>,
}

/// Both sides of a `CancelToken` are closed, so nothing can cancel it any more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CancelClosed;

impl From<tokio::sync::watch::Receiver<bool>> for CancelToken {
    fn from(session: tokio::sync::watch::Receiver<bool>) -> Self {
        CancelToken {
            session,
            reply: None,
        }
    }
}

impl CancelToken {
    /// True when the session stop or the reply token is set. Both sides are read synchronously.
    pub fn is_cancelled(&self) -> bool {
        self.session_cancelled() || self.reply.as_ref().is_some_and(|reply| *reply.borrow())
    }

    /// True only when the session stop is set.
    pub fn session_cancelled(&self) -> bool {
        *self.session.borrow()
    }

    /// The same session with a reply token added. Any earlier reply token is replaced.
    pub fn with_reply(&self, reply: tokio::sync::watch::Receiver<bool>) -> CancelToken {
        CancelToken {
            session: self.session.clone(),
            reply: Some(reply),
        }
    }

    /// The session side only. Kept for the pre-listen `prepare` step, which runs before any
    /// reply token exists. Use `is_cancelled` for the full state.
    pub fn session_ref(&self) -> tokio::sync::watch::Ref<'_, bool> {
        self.session.borrow()
    }

    /// Resolves `Ok` once `pred` holds for the full state (`is_cancelled`). A closed side never
    /// fires: the wait goes on with the other live side. `Err` when both sides are closed and
    /// the predicate still does not hold.
    pub async fn wait_for(&mut self, pred: impl Fn(&bool) -> bool) -> Result<(), CancelClosed> {
        loop {
            if pred(&self.is_cancelled()) {
                return Ok(());
            }
            let CancelToken { session, reply } = self;
            let session_live = session.has_changed().is_ok();
            let reply_live = reply.as_mut().is_some_and(|rx| rx.has_changed().is_ok());
            if !session_live && !reply_live {
                return Err(CancelClosed);
            }
            tokio::select! {
                _ = session.changed(), if session_live => {}
                _ = changed_or_pending(reply), if reply_live => {}
            }
        }
    }
}

/// `changed()` on the reply side, or pending when there is no reply side.
async fn changed_or_pending(reply: &mut Option<tokio::sync::watch::Receiver<bool>>) {
    match reply {
        Some(rx) => {
            let _ = rx.changed().await;
        }
        None => std::future::pending::<()>().await,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenFailure {
    Busy,
    NoMicrophone,
    Permission,
    ModelUnavailable,
    Other,
    /// The microphone stream could not be opened or started (a raw device/OS error, not a
    /// permission or missing-device case). Reported to the user exactly like `Other`; it is the
    /// one start failure the barge-in recovery retries (DESIGN 12.4).
    StreamOpen,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenOutcome {
    Utterance(String),
    NoSpeech,
    TooLong,
    Aborted,
    Failed(ListenFailure),
    /// The optional speaker filter heard a voice that is not the enrolled one: the audio was
    /// NOT transcribed and nothing about it is kept. Only produced when the filter is enabled.
    Ignored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SayOutcome {
    Done,
    Interrupted,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuietOutcome {
    Quiet,
    StillSpeaking,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhraseOutcome {
    /// Executed; the string is a short spoken confirmation.
    Done(String),
    NoMatch,
    Refused,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Listening,
    Thinking,
    Speaking,
    Acting,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
    UserEnded,
    Idle,
    Cancelled,
    NotConfigured,
    AudioUnavailable,
    SpeechStuck,
    TurnLimit,
}

pub trait Ears: Send + Sync {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome>;

    /// Timing numbers of the listen that just returned (numbers/enums only), or `None` when
    /// the ears do not measure it. The loop calls this exactly once right after `listen`.
    /// Default: `None`, so existing ears and fakes are unchanged.
    fn take_timing(&self) -> Option<crate::managers::converse_latency::ListenTiming> {
        None
    }

    /// Barge-in monitor for this session (DESIGN 12.2.2). `None` (default): the loop behaves
    /// exactly as before and no watch is armed.
    fn barge_in(&self) -> Option<&dyn BargeIn> {
        None
    }

    /// Stops this session's barge-in monitor, if any. The session finalizer calls it before the
    /// capture worker is shut down. Default: nothing to stop.
    fn barge_shutdown(&self) {}
}

pub trait Mouth: Send + Sync {
    fn say<'a>(&'a self, text: &'a str, emotion: Emotion) -> BoxFuture<'a, SayOutcome>;
    fn wait_quiet(&self, max: Duration) -> BoxFuture<'_, QuietOutcome>;
    fn stop(&self);
    /// Opens a speech queue for ONE turn (early speech / acknowledgement), or `None` when the
    /// mouth cannot queue (legacy `say` mouths) or speech is unavailable right now.
    /// Default: `None`.
    fn open_queue(&self) -> Option<Arc<dyn SpeechQueue>> {
        None
    }
}

/// One turn's queued speech (see `converse_queue::TtsQueueSession`).
pub trait SpeechQueue: Send + Sync {
    /// Appends a sentence without waiting. `false` = refused (the session is dead: it was
    /// superseded, a recording started, speech was stopped, or the speaker failed). Empty text
    /// is a harmless `true`.
    fn push(&self, text: &str, emotion: Emotion) -> bool;
    /// Stops this session's speech (never a newer owner's) and kills the session.
    fn abort(&self);
    /// Resolves when everything queued has been spoken (`Done`), the session was superseded
    /// (`Interrupted`) or the speaker did not finish in time (`Failed`).
    fn finish(&self) -> BoxFuture<'_, SayOutcome>;
}

pub trait Hands: Send + Sync {
    /// Paste generated/dictated text into the active window (never submits).
    fn paste_generated(&self, text: &str) -> Result<(), String>;
    /// Exact tier: what the USER said (shortcuts, media, router).
    fn run_user_phrase<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome>;
    /// Restricted tier: a command PROPOSED BY THE MODEL.
    fn run_model_command<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome>;
    /// M3: validates a model-proposed command against the allowlist. Default: refused.
    #[allow(dead_code)]
    fn validate_command(&self, _text: &str) -> Option<ValidatedCommand> {
        None
    }
    /// M3: runs a command that was already validated. Default: not run.
    #[allow(dead_code)]
    fn run_validated_command(&self, _cmd: ValidatedCommand) -> bool {
        false
    }
    /// M3: pins a configured project to a directory handle. Default: unavailable.
    #[allow(dead_code)]
    fn pin_project(&self, _name: &str) -> Result<DirFd, PinError> {
        Err(PinError::Unavailable)
    }
    /// M3: starts a background coding job. Default: unsupported.
    #[allow(dead_code)]
    fn start_job(&self, _launch: JobLaunch) -> JobStartOutcome {
        JobStartOutcome::Unsupported
    }
    /// M3: cancels the active background job. Default: nothing to cancel.
    #[allow(dead_code)]
    fn cancel_job(&self) -> bool {
        false
    }
    /// M3: whether a background job is active. Default: none.
    #[allow(dead_code)]
    fn has_active_job(&self) -> bool {
        false
    }
    /// Stage 2 agent hand-off: prepares an agent launch from the live settings, with the task
    /// and role fixed. Default: unsupported.
    #[allow(dead_code)]
    fn prepare_agent_launch(
        &self,
        _task: &str,
        _role: Option<&str>,
    ) -> Result<PreparedAgentLaunch, AgentStartError> {
        Err(AgentStartError::Unsupported)
    }
    /// Stage 2 agent hand-off: whether a prepared launch still matches the live settings (the
    /// fingerprint is recomputed). Default: false.
    #[allow(dead_code)]
    fn agent_launch_still_valid(&self, _launch: &PreparedAgentLaunch) -> bool {
        false
    }
    /// Stage 2 agent hand-off: starts a prepared agent run. Default: unsupported.
    #[allow(dead_code)]
    fn start_agent(
        &self,
        _launch: PreparedAgentLaunch,
    ) -> Result<Box<dyn AgentRun>, AgentStartError> {
        Err(AgentStartError::Unsupported)
    }
    /// Stage 2 agent hand-off: publishes the full agent reply to the UI. Default: nothing.
    #[allow(dead_code)]
    fn publish_agent_result(&self, _text: &str) {}
    /// Stage 4: prepares the launch of one plan step, with its step kind (its agent profile).
    /// Default: the single-agent preparation, recorded as a main-agent step of a plan.
    fn prepare_agent_launch_for(
        &self,
        kind: StepKind,
        task: &str,
        role: Option<&str>,
    ) -> Result<PreparedAgentLaunch, AgentStartError> {
        self.prepare_agent_launch(task, role)
            .map(|launch| launch.with_step(kind, AgentSource::Main, true))
    }
    /// Stage 4 (validator round 1, F2): prepares a typed single agent request (`type` given, not
    /// `general`). It uses the specialist profile of that type and the step role, and is not a
    /// plan step. Default: the single-agent preparation recorded as a main-agent single request.
    fn prepare_agent_launch_typed(
        &self,
        kind: StepKind,
        task: &str,
        role: Option<&str>,
    ) -> Result<PreparedAgentLaunch, AgentStartError> {
        self.prepare_agent_launch(task, role)
            .map(|launch| launch.with_step(kind, AgentSource::Main, false))
    }
}

pub trait Brain: Send + Sync {
    fn plan<'a>(&'a self, input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>>;

    /// Like `plan`, additionally reporting every raw text delta of the model reply to
    /// `on_text` as it arrives (the callback must be quick and non-blocking). The default
    /// ignores `on_text` and calls `plan`, so non-streaming brains keep working.
    fn plan_streaming<'a>(
        &'a self,
        input: PlanInput<'a>,
        on_text: &'a mut (dyn FnMut(&str) + Send),
    ) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        let _ = on_text;
        self.plan(input)
    }
}

/// How one accepted user utterance ended (DESIGN Rev 3, 8.2). Compile-only stub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcome {
    Answered,
    Clarified,
    Ended,
    Dictated,
    Acted,
    Failed,
    Interrupted,
    Blocked,
    NoReply,
}

/// Whether the reply text was submitted for speech (DESIGN Rev 3, 8.2). Compile-only stub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Submitted,
    Interrupted,
    NotSpoken,
}

/// One transcript record per accepted user utterance (DESIGN Rev 3, 8.2). Compile-only stub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptEvent {
    pub user: String,
    pub assistant: Option<String>,
    pub outcome: TurnOutcome,
    pub delivery: Delivery,
    pub detail: Option<String>,
}

/// Receives transcript events for one session. Compile-only stub.
pub trait TranscriptSink: Send + Sync {
    fn record(&self, ev: TranscriptEvent);
}

/// Session-scoped journal of the utterance in progress: the speech handed to the mouth and the
/// outcome the turn reached. Shared by the speech helpers and the transcript accumulator.
#[derive(Clone, Default)]
struct Journal(Arc<JournalState>);

#[derive(Default)]
struct JournalState {
    submitted: Mutex<Vec<String>>,
    outcome: Mutex<Option<(TurnOutcome, Option<String>)>>,
}

impl Journal {
    /// Resets the journal for a newly accepted utterance.
    fn begin(&self) {
        lock_or_recover(&self.0.submitted).clear();
        *lock_or_recover(&self.0.outcome) = None;
    }

    /// Records text submitted for speech (blank text is ignored).
    fn submit(&self, text: &str) {
        let text = text.trim();
        if !text.is_empty() {
            lock_or_recover(&self.0.submitted).push(text.to_string());
        }
    }

    /// Number of speech entries recorded so far.
    fn submitted_len(&self) -> usize {
        lock_or_recover(&self.0.submitted).len()
    }

    /// The submitted speech so far, joined, without taking it (`None` if nothing).
    fn peek_submitted(&self) -> Option<String> {
        let submitted = lock_or_recover(&self.0.submitted);
        (!submitted.is_empty()).then(|| submitted.join(" "))
    }

    /// Drops speech entries recorded after `len` (keeps confirmation speech out of the transcript).
    fn truncate_submitted(&self, len: usize) {
        lock_or_recover(&self.0.submitted).truncate(len);
    }

    /// Records the outcome the turn reached, with an optional machine-generated detail.
    fn set(&self, outcome: TurnOutcome, detail: Option<String>) {
        *lock_or_recover(&self.0.outcome) = Some((outcome, detail));
    }

    /// Takes the submitted speech (joined, `None` if nothing) and the recorded outcome.
    fn take(&self) -> (Option<String>, Option<(TurnOutcome, Option<String>)>) {
        let submitted = std::mem::take(&mut *lock_or_recover(&self.0.submitted));
        let assistant = (!submitted.is_empty()).then(|| submitted.join(" "));
        let outcome = lock_or_recover(&self.0.outcome).take();
        (assistant, outcome)
    }
}

fn lock_or_recover<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The history-independent label of a speech plan kind.
fn speech_outcome(kind: &PlanKind) -> TurnOutcome {
    match kind {
        PlanKind::Clarify => TurnOutcome::Clarified,
        PlanKind::End => TurnOutcome::Ended,
        // Job plans are parsed only when jobs are allowed and are routed through gate_and_run;
        // without a policy they are never produced, so this branch only sees answer-like speech.
        // role kinds are answer-like for the history label (their own handling is in execute_plan).
        PlanKind::Answer
        | PlanKind::Paste
        | PlanKind::Command
        | PlanKind::Job
        | PlanKind::SetRole
        | PlanKind::ClearRole
        // Stage 2 (C3 wires the real behaviour): an agent plan is answer-like until then.
        | PlanKind::Agent
        // Stage 4 (S4 wires the executor): a plan is answer-like for the history label.
        | PlanKind::Plan
        | PlanKind::Resume => TurnOutcome::Answered,
    }
}

/// Utterance-scoped transcript accumulator (DESIGN Rev 3, 8.2). Created when an utterance is
/// accepted for dispatch and finalized exactly once: `finish` on every normal exit, and a `Drop`
/// fallback that emits an Interrupted event if the turn unwinds before `finish`. Independent of
/// the model history.
struct UtteranceLog<'a> {
    reporter: &'a dyn Reporter,
    user: String,
    journal: Journal,
    done: bool,
}

impl<'a> UtteranceLog<'a> {
    fn new(reporter: &'a dyn Reporter, user: &str, journal: &Journal) -> Self {
        journal.begin();
        UtteranceLog {
            reporter,
            user: user.to_string(),
            journal: journal.clone(),
            done: false,
        }
    }

    /// Emits the event for this utterance. `cancelled` marks a turn cut short by cancel or
    /// barge-in: it is reported as Interrupted, never as the outcome it had reached.
    fn finish(mut self, cancelled: bool) {
        self.emit(cancelled);
    }

    fn emit(&mut self, cancelled: bool) {
        if self.done {
            return;
        }
        self.done = true;
        let (assistant, recorded) = self.journal.take();
        let delivery = match (assistant.is_some(), cancelled) {
            (true, true) => Delivery::Interrupted,
            (true, false) => Delivery::Submitted,
            (false, _) => Delivery::NotSpoken,
        };
        let (outcome, detail) = if cancelled {
            let detail = recorded.and_then(|(_, detail)| detail);
            (TurnOutcome::Interrupted, detail)
        } else {
            match recorded {
                Some(pair) => pair,
                None => (TurnOutcome::NoReply, None),
            }
        };
        self.reporter.transcript(TranscriptEvent {
            user: self.user.clone(),
            assistant,
            outcome,
            delivery,
            detail,
        });
    }
}

impl Drop for UtteranceLog<'_> {
    fn drop(&mut self) {
        self.emit(true);
    }
}

/// Mouth wrapper that journals every line handed to `say`: the session's submitted speech.
struct JournalMouth<'m> {
    inner: &'m dyn Mouth,
    journal: Journal,
    /// Chat window recording of every line handed to `say` (DESIGN-CHAT 2.2, A10).
    reporter: &'m dyn Reporter,
}

/// Records one assistant line for the chat window: the speech-normalised form, and only when
/// something speakable remains (DESIGN-CHAT A10). Speech-path safe: one call, no waiting.
fn record_speech(reporter: &dyn Reporter, text: &str) {
    if let Some(spoken) = crate::managers::tts::sanitize_speech_text(text) {
        if !spoken.is_empty() {
            reporter.chat(ChatRole::Assistant, &spoken);
        }
    }
}

impl Mouth for JournalMouth<'_> {
    fn say<'a>(&'a self, text: &'a str, emotion: Emotion) -> BoxFuture<'a, SayOutcome> {
        self.journal.submit(text);
        record_speech(self.reporter, text);
        self.inner.say(text, emotion)
    }
    fn wait_quiet(&self, max: Duration) -> BoxFuture<'_, QuietOutcome> {
        self.inner.wait_quiet(max)
    }
    fn stop(&self) {
        self.inner.stop();
    }
    fn open_queue(&self) -> Option<Arc<dyn SpeechQueue>> {
        self.inner.open_queue()
    }
}

/// Turn-scoped mouth for a reply that barge-in can cancel (DESIGN 12.7.C). Every admission
/// (say, open_queue, queue push) takes the reply gate and runs the inner synchronous admission
/// under it, so once the gate is closed the inner backend is never invoked again.
pub(crate) struct GatedMouth<'m> {
    pub(crate) inner: &'m dyn Mouth,
    pub(crate) gate: Arc<ReplyGate>,
}

/// What a say admission produced: finished on its first poll, or still running.
enum Admitted<'a> {
    Done(SayOutcome),
    Running(BoxFuture<'a, SayOutcome>),
}

impl Mouth for GatedMouth<'_> {
    fn say<'a>(&'a self, text: &'a str, emotion: Emotion) -> BoxFuture<'a, SayOutcome> {
        let admitted = self.gate.with_admission(|| {
            let mut say = self.inner.say(text, emotion);
            // The first poll is the admission: the inner say does its synchronous work (epoch
            // reservation, speech submission) before its first await. The waker is not needed
            // here; the caller awaits the future afterwards with its own waker.
            let mut cx = std::task::Context::from_waker(futures_util::task::noop_waker_ref());
            match say.as_mut().poll(&mut cx) {
                std::task::Poll::Ready(outcome) => Admitted::Done(outcome),
                std::task::Poll::Pending => Admitted::Running(say),
            }
        });
        match admitted {
            None => Box::pin(async { SayOutcome::Interrupted }),
            Some(Admitted::Done(outcome)) => Box::pin(async move { outcome }),
            Some(Admitted::Running(say)) => say,
        }
    }

    fn wait_quiet(&self, max: Duration) -> BoxFuture<'_, QuietOutcome> {
        self.inner.wait_quiet(max)
    }

    fn stop(&self) {
        self.inner.stop();
    }

    fn open_queue(&self) -> Option<Arc<dyn SpeechQueue>> {
        let queue = self
            .gate
            .with_admission(|| self.inner.open_queue())
            .flatten()?;
        Some(Arc::new(GatedQueue {
            inner: queue,
            gate: self.gate.clone(),
        }))
    }
}

/// The speech queue of a `GatedMouth`: every push is an admission under the reply gate.
struct GatedQueue {
    inner: Arc<dyn SpeechQueue>,
    gate: Arc<ReplyGate>,
}

impl SpeechQueue for GatedQueue {
    fn push(&self, text: &str, emotion: Emotion) -> bool {
        self.gate
            .with_admission(|| self.inner.push(text, emotion))
            .unwrap_or(false)
    }

    fn abort(&self) {
        self.inner.abort();
    }

    fn finish(&self) -> BoxFuture<'_, SayOutcome> {
        self.inner.finish()
    }
}

impl From<TurnOutcome> for crate::managers::conversation_record::TurnOutcomeLabel {
    fn from(outcome: TurnOutcome) -> Self {
        use crate::managers::conversation_record::TurnOutcomeLabel as L;
        match outcome {
            TurnOutcome::Answered => L::Answered,
            TurnOutcome::Clarified => L::Clarified,
            TurnOutcome::Ended => L::Ended,
            TurnOutcome::Dictated => L::Dictated,
            TurnOutcome::Acted => L::Acted,
            TurnOutcome::Failed => L::Failed,
            TurnOutcome::Interrupted => L::Interrupted,
            TurnOutcome::Blocked => L::Blocked,
            TurnOutcome::NoReply => L::NoReply,
        }
    }
}

impl From<Delivery> for crate::managers::conversation_record::DeliveryLabel {
    fn from(delivery: Delivery) -> Self {
        use crate::managers::conversation_record::DeliveryLabel as L;
        match delivery {
            Delivery::Submitted => L::Submitted,
            Delivery::Interrupted => L::Interrupted,
            Delivery::NotSpoken => L::NotSpoken,
        }
    }
}

impl From<SessionEnd> for crate::managers::conversation_record::EndLabel {
    fn from(end: SessionEnd) -> Self {
        use crate::managers::conversation_record::EndLabel as L;
        match end {
            SessionEnd::UserEnded => L::UserEnded,
            SessionEnd::Idle => L::Idle,
            SessionEnd::Cancelled => L::Cancelled,
            SessionEnd::NotConfigured => L::NotConfigured,
            SessionEnd::AudioUnavailable => L::AudioUnavailable,
            SessionEnd::SpeechStuck => L::SpeechStuck,
            SessionEnd::TurnLimit => L::TurnLimit,
        }
    }
}

pub trait Reporter: Send + Sync {
    fn state(&self, state: SessionState);

    /// Receives one transcript event per accepted utterance. Default: ignored.
    fn transcript(&self, _ev: TranscriptEvent) {}

    /// Chat window: receives one recorded line (user text or speech handed to the voice).
    /// Default: ignored (DESIGN-CHAT 2.2). Stub: the spec tests are red until the hooks land.
    fn chat(&self, _role: crate::managers::chat_log::ChatRole, _text: &str) {}

    /// M4: receives one timing line (observability only). Default: written to the log at info.
    fn timing_line(&self, line: &str) {
        log::info!("{line}");
    }

    /// Receives one latency report (numbers/enums only) per completed turn, together with the
    /// rolling statistics of the session. Default: ignored.
    fn latency(
        &self,
        _report: &crate::managers::converse_latency::LatencyReport,
        _stats: &crate::managers::converse_latency::LatencyStats,
    ) {
    }
}

pub struct Deps<'a> {
    pub ears: &'a dyn Ears,
    pub mouth: &'a dyn Mouth,
    pub hands: &'a dyn Hands,
    pub brain: &'a dyn Brain,
    pub reporter: &'a dyn Reporter,
    /// M3: action policy source. `None` keeps the gate inactive (legacy behavior).
    #[allow(dead_code)]
    pub policy: Option<&'a dyn PolicySource>,
    /// M3: announcer for background job speech. `None` means no announcements.
    #[allow(dead_code)]
    pub announcer: Option<&'a Announcer>,
}

impl<'a> Deps<'a> {
    /// The same dependencies with another mouth (the turn-scoped barge-in gate).
    fn with_mouth<'m>(&self, mouth: &'m dyn Mouth) -> Deps<'m>
    where
        'a: 'm,
    {
        Deps {
            ears: self.ears,
            mouth,
            hands: self.hands,
            brain: self.brain,
            reporter: self.reporter,
            policy: self.policy,
            announcer: self.announcer,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub max_idle_turns: u8,
    pub context_turns: usize,
    pub max_turns: usize,
    pub post_speech_guard: Duration,
    pub quiet_wait: Duration,
    pub style: String,
    /// Realtime: speak completed sentences while the reply is still streaming (needs a mouth
    /// whose `open_queue` returns a queue). Default false.
    pub early_speech: bool,
    /// Realtime: if no speech was queued this long after the brain request started, queue
    /// `MSG_ACK` once. `None` disables. Default None.
    pub ack_after: Option<Duration>,
    /// Realtime: log per-turn latency numbers (never any text). Default false.
    pub timing_log: bool,
    /// Choose the acknowledgement from the utterance (`converse_ack`) instead of the static
    /// `MSG_ACK` / `ack_after` pair. Only meaningful when `ack_after` is `Some`. Default false.
    pub smart_ack: bool,
    /// Vault-context boundary (DESIGN Rev 3, 3.6): no model-derived paste or command. Default false.
    pub answer_only: bool,
    /// Stage 2 agent hand-off: the ACP agent is enabled with a command and the conversation
    /// hand-off setting is on (`from_settings`). Default false.
    pub agent_handoff: bool,
    /// Stage 4 planner: a plan may run, that is the hand-off and the planner setting are both on
    /// (`from_settings`). Default false.
    pub agent_planner: bool,
    /// How long a confirmation waits for the answer. 8 s for voice (the existing window); a text
    /// session waits longer for a typed answer. Default 8 s.
    pub confirm_listen_timeout: Duration,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            max_idle_turns: 2,
            context_turns: 6,
            max_turns: 50,
            post_speech_guard: Duration::from_millis(350),
            quiet_wait: Duration::from_secs(2),
            style: String::new(),
            early_speech: false,
            ack_after: None,
            timing_log: false,
            smart_ack: false,
            answer_only: false,
            agent_handoff: false,
            agent_planner: false,
            confirm_listen_timeout: Duration::from_secs(8),
        }
    }
}

impl SessionConfig {
    /// Session settings from the user's AppSettings (clamped; DESIGN 3.3).
    pub fn from_settings(s: &crate::settings::AppSettings) -> SessionConfig {
        let rt = crate::managers::realtime_config::RealtimeConfig::from_settings(s);
        let agent_handoff = crate::managers::converse_agent::agent_launch_allowed(
            s.acp_agent_enabled,
            s.acp_agent_command.as_deref(),
            s.conversation_agent_handoff,
        );
        SessionConfig {
            max_idle_turns: s.converse_max_idle_turns.clamp(1, 5),
            context_turns: usize::from(s.converse_context_turns.min(12)),
            max_turns: 50,
            post_speech_guard: Duration::from_millis(350),
            quiet_wait: Duration::from_secs(2),
            style: s.tts_composer_style.clone(),
            early_speech: rt.early_speech,
            ack_after: if rt.ack {
                Some(Duration::from_millis(1200))
            } else {
                None
            },
            timing_log: rt.timing_log,
            smart_ack: rt.ack,
            answer_only: crate::managers::conversation_save::answer_only_for(s),
            agent_handoff,
            agent_planner: agent_handoff && s.conversation_planner,
            confirm_listen_timeout: Duration::from_secs(8),
        }
    }
}

/// Endpointing config from the user's silence setting, clamped 600..=4000 ms.
pub fn endpoint_config(
    s: &crate::settings::AppSettings,
) -> crate::managers::converse_audio::EndpointConfig {
    crate::managers::converse_audio::EndpointConfig::with_silence(u64::from(
        s.converse_silence_ms.clamp(600, 4000),
    ))
}

/// Run one conversation session to its end. Reports `Idle` exactly once on
/// every exit path and calls `mouth.stop()` when the end is `Cancelled`. The cancel token may
/// be a plain session receiver (converted here) or a composite `CancelToken`.
pub async fn run_session(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: impl Into<CancelToken>,
) -> SessionEnd {
    let _conversation = ConversationGuard::enter(deps.announcer);
    let mut cancel: CancelToken = cancel.into();
    let end = run_turns(deps, cfg, &mut cancel).await;
    if end == SessionEnd::Cancelled {
        deps.mouth.stop();
    }
    deps.reporter.state(SessionState::Idle);
    end
}

/// Keeps the announcer's conversation flag set for one session. Cleared on every exit path,
/// including unwinding (DESIGN R2-6).
struct ConversationGuard<'a> {
    announcer: Option<&'a Announcer>,
}

impl<'a> ConversationGuard<'a> {
    fn enter(announcer: Option<&'a Announcer>) -> Self {
        if let Some(announcer) = announcer {
            announcer.set_conversation_active(true);
        }
        ConversationGuard { announcer }
    }
}

impl Drop for ConversationGuard<'_> {
    fn drop(&mut self) {
        if let Some(announcer) = self.announcer {
            announcer.set_conversation_active(false);
        }
    }
}

// Fixed spoken messages. The loop never speaks text taken from an error.
const MSG_TOO_LONG: &str = "That was too long for me. Please say it in parts.";
/// Spoken once when the capture reports `TooLong` twice in a row (the
/// endpointer cannot find the end of speech).
const MSG_CANT_HEAR: &str =
    "I can't tell when you stop talking, so I'm ending this. Please try again somewhere quieter.";
const MSG_MIC: &str = "I can't use the microphone right now.";
/// Spoken when the speech-to-text model is missing or unloaded (#119). Not a microphone problem.
const MSG_STT_MODEL: &str = "I can't use the speech recognition model right now.";
const MSG_STOP_LISTENING: &str = "I'll stop listening now.";
const MSG_OKAY: &str = "Okay.";
const MSG_PASTE_FAILED: &str = "I couldn't paste that.";
const MSG_FAILED: &str = "That didn't work.";
const MSG_REFUSED: &str = "I can't do that.";
const MSG_NO_MATCH_COMMAND: &str = "I can't do that yet.";
const MSG_NOT_CONFIGURED: &str = "Set up a reply backend in the speech settings first.";
const MSG_PLAN_FAILED: &str = "Sorry, I couldn't work that out.";
const MSG_DONE: &str = "Done.";
/// Spoken instead of any model-proposed paste or command while vault context is on (DESIGN 3.6).
/// Spoken when a gated job's settings changed between approval and dispatch (DESIGN R3-2).
const MSG_SETTINGS_CHANGED: &str = "Settings changed, so I didn't start it.";
/// Spoken when a forced confirmation is not an exact yes (DESIGN 2.5, C6).
const MSG_FORCED_DECLINED: &str = "Okay, I won't.";
const MSG_ANSWER_ONLY: &str =
    "Vault context is on, so I can only answer questions. Turn it off to dictate text or open things.";
/// Queued once when the brain is slow (`SessionConfig::ack_after`).
const MSG_ACK: &str = "One moment.";

/// The fixed sentence spoken for a failed listen. Only a model problem gets its own sentence;
/// every other kind is reported as a microphone problem.
fn listen_failure_message(kind: &ListenFailure) -> &'static str {
    match kind {
        ListenFailure::ModelUnavailable => MSG_STT_MODEL,
        ListenFailure::Busy
        | ListenFailure::NoMicrophone
        | ListenFailure::Permission
        | ListenFailure::Other
        | ListenFailure::StreamOpen => MSG_MIC,
    }
}

const HISTORY_SIDE_MAX_CHARS: usize = 600;
const STILL_SPEAKING_RETRY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpeakResult {
    /// Speech finished (Done, Interrupted or Failed) and the settle steps ran.
    Spoken(SayOutcome),
    Cancelled,
    /// Speech still running after the retry; the mic must not reopen.
    Stuck,
}

fn is_cancelled(cancel: &CancelToken) -> bool {
    cancel.is_cancelled()
}

/// Completes when the token reads `true`. A closed channel never cancels.
async fn wait_cancelled(cancel: &mut CancelToken) {
    let closed = cancel.wait_for(|cancelled| *cancelled).await.is_err();
    if closed {
        std::future::pending::<()>().await;
    }
}

/// Races `fut` against cancellation. Cancellation is checked first and again
/// after the future completes, so a cancel that is ready at the same instant
/// wins: `None` means cancelled and the result must not be used.
async fn race<F: Future>(cancel: &mut CancelToken, fut: F) -> Option<F::Output> {
    if is_cancelled(cancel) {
        return None;
    }
    let finished = {
        let fut = std::pin::pin!(fut);
        let cancelled = std::pin::pin!(wait_cancelled(cancel));
        match futures_util::future::select(fut, cancelled).await {
            Either::Left((output, _rest)) => Some(output),
            Either::Right((_, _rest)) => None,
        }
    };
    if is_cancelled(cancel) {
        None
    } else {
        finished
    }
}

fn speech_to_end(result: SpeakResult) -> Option<SessionEnd> {
    match result {
        SpeakResult::Spoken(_) => None,
        SpeakResult::Cancelled => Some(SessionEnd::Cancelled),
        SpeakResult::Stuck => Some(SessionEnd::SpeechStuck),
    }
}

/// The end for a say that should end the session with `then` when it worked.
fn after_speech(result: SpeakResult, then: SessionEnd) -> SessionEnd {
    speech_to_end(result).unwrap_or(then)
}

async fn speak_or_end(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    text: &str,
    emotion: Emotion,
) -> Option<SessionEnd> {
    speech_to_end(speak(deps, cfg, cancel, text, emotion).await)
}

/// Speaks one line and reports how the delivery went (`Done` / `Interrupted` / `Failed`) in
/// addition to the settle result. Only Cancelled and Stuck end the session.
async fn speak_outcome(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    text: &str,
    emotion: Emotion,
) -> Result<SayOutcome, SessionEnd> {
    match speak(deps, cfg, cancel, text, emotion).await {
        SpeakResult::Spoken(outcome) => Ok(outcome),
        SpeakResult::Cancelled => Err(SessionEnd::Cancelled),
        SpeakResult::Stuck => Err(SessionEnd::SpeechStuck),
    }
}

/// Speaks one line. Reports Speaking first, races the say against cancel and
/// then settles (wait for quiet, optional guard).
async fn speak(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    text: &str,
    emotion: Emotion,
) -> SpeakResult {
    if text.trim().is_empty() {
        return SpeakResult::Spoken(SayOutcome::Done);
    }
    deps.reporter.state(SessionState::Speaking);
    if is_cancelled(cancel) {
        return SpeakResult::Cancelled;
    }
    let Some(outcome) = race(cancel, deps.mouth.say(text, emotion)).await else {
        return SpeakResult::Cancelled;
    };
    settle(deps, cfg, cancel, outcome).await
}

/// After any say: wait until quiet. If still speaking, stop once and wait
/// once more; still speaking => Stuck. On quiet, sleep the post-speech guard.
async fn settle(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    outcome: SayOutcome,
) -> SpeakResult {
    let quiet = match race(cancel, deps.mouth.wait_quiet(cfg.quiet_wait)).await {
        Some(outcome) => outcome,
        None => return SpeakResult::Cancelled,
    };
    if quiet == QuietOutcome::StillSpeaking {
        deps.mouth.stop();
        match race(cancel, deps.mouth.wait_quiet(STILL_SPEAKING_RETRY)).await {
            None => return SpeakResult::Cancelled,
            Some(QuietOutcome::StillSpeaking) => return SpeakResult::Stuck,
            Some(QuietOutcome::Quiet) => {}
        }
    }
    if !cfg.post_speech_guard.is_zero()
        && race(cancel, tokio::time::sleep(cfg.post_speech_guard))
            .await
            .is_none()
    {
        return SpeakResult::Cancelled;
    }
    SpeakResult::Spoken(outcome)
}

fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Appends one turn. The user side is neutralized; the summary is already composed (a note is
/// kept whole by `compose`), so the cap below only trims plain text.
fn push_history(history: &mut Vec<(String, String)>, utterance: &str, summary: &str, keep: usize) {
    history.push((
        truncate_chars(&neutralize(utterance), HISTORY_SIDE_MAX_CHARS),
        truncate_chars(summary, HISTORY_SIDE_MAX_CHARS),
    ));
    let excess = history.len().saturating_sub(keep);
    history.drain(..excess);
}

/// History side of a recorded failure (nothing was delivered this turn).
fn failed_summary() -> String {
    compose("", &note_failed())
}

/// History side of a paste that did not reach the active window.
fn paste_failed_summary() -> String {
    compose("", &note_paste_failed())
}

/// History side of a plan failure: nothing was queued, so the turn failed outright; otherwise the
/// queued prefix is kept and marked incomplete.
fn failure_summary(spoken: &[String]) -> String {
    if spoken.is_empty() {
        failed_summary()
    } else {
        compose(&spoken.join(" "), &note_incomplete())
    }
}

/// History side of an answer or clarify: the speech as delivered, with a note when delivery was
/// interrupted or failed.
fn answer_summary(speech: &str, outcome: SayOutcome) -> String {
    match outcome {
        SayOutcome::Done => neutralize(speech),
        SayOutcome::Interrupted => compose(speech, &note_interrupted()),
        SayOutcome::Failed => compose(speech, &note_delivery_failed()),
    }
}

/// A failed turn is recorded once: skip it when the newest entry is already a failure for the
/// same full utterance.
fn is_repeated_failure(
    history: &[(String, String)],
    last_failed: Option<&str>,
    utterance: &str,
    summary: &str,
) -> bool {
    let failed = failed_summary();
    summary == failed
        && last_failed == Some(utterance)
        && history.last().is_some_and(|(_, side)| *side == failed)
}

/// Converts an optional end into a `Result` so `?` can propagate it.
fn into_result(end: Option<SessionEnd>) -> Result<(), SessionEnd> {
    match end {
        None => Ok(()),
        Some(end) => Err(end),
    }
}

/// Restricted tier for a model-proposed command. `Ok(Some(summary))` is the history side of a
/// command that was answered (executed or refused or failed, with a fixed message spoken);
/// `Ok(None)` when no command was given (fixed message, nothing recorded); `Err(end)` when the
/// session must end.
async fn run_command_plan(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    plan: &TurnPlan,
    journal: &Journal,
) -> Result<Option<String>, SessionEnd> {
    let Some(command) = plan.command.as_deref() else {
        into_result(speak_or_end(deps, cfg, cancel, MSG_NO_MATCH_COMMAND, Emotion::Neutral).await)?;
        journal.set(TurnOutcome::Failed, None);
        return Ok(None);
    };
    if is_cancelled(cancel) {
        return Err(SessionEnd::Cancelled);
    }
    let (fixed, outcome) = match deps.hands.run_model_command(command).await {
        PhraseOutcome::Done(message) => {
            // The effect is recorded even if the confirmation speech fails.
            let note = note_command(command, &CommandNote::Done(message.clone()));
            journal.set(TurnOutcome::Acted, Some("action: command".to_string()));
            let spoken = if plan.speech.is_empty() {
                message
            } else {
                plan.speech.clone()
            };
            speak_outcome(deps, cfg, cancel, &spoken, plan.emotion).await?;
            return Ok(Some(compose(&plan.speech, &note)));
        }
        PhraseOutcome::NoMatch => (MSG_NO_MATCH_COMMAND, CommandNote::NoMatch),
        PhraseOutcome::Refused => (MSG_REFUSED, CommandNote::Refused),
        PhraseOutcome::Failed => (MSG_FAILED, CommandNote::Failed),
    };
    journal.set(TurnOutcome::Failed, None);
    into_result(speak_or_end(deps, cfg, cancel, fixed, Emotion::Neutral).await)?;
    Ok(Some(compose("", &note_command(command, &outcome))))
}

/// Shared state of one streamed brain turn (see `stream_plan`).
#[derive(Default)]
struct Early {
    /// Transcript journal of this turn (submitted sentences are recorded here).
    journal: Journal,
    streamer: PlanStreamer,
    /// Sentences already queued: canonical, in order, never the acknowledgement.
    spoken: Vec<String>,
    /// The queue refused a push: the user or the system took the speaker. Stay silent.
    dead: bool,
    /// The acknowledgement queued this turn, if any (never part of `spoken`).
    ack: Option<&'static str>,
    speaking_reported: bool,
    first_text: Option<Instant>,
    first_speech: Option<Instant>,
}

fn lock_early(early: &Mutex<Early>) -> std::sync::MutexGuard<'_, Early> {
    early
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn take_early(early: Mutex<Early>) -> Early {
    early
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Reports Speaking once per turn and records the first speech time.
fn mark_speaking(early: &mut Early, reporter: &dyn Reporter) {
    if !early.speaking_reported {
        early.speaking_reported = true;
        reporter.state(SessionState::Speaking);
    }
    early.first_speech.get_or_insert_with(Instant::now);
}

/// One streamed text delta from the brain: release completed sentences to the queue.
/// Runs inside the brain callback, so it only takes the lock and pushes.
fn on_delta(
    early: &Mutex<Early>,
    cfg: &SessionConfig,
    queue: &dyn SpeechQueue,
    reporter: &dyn Reporter,
    cancel: &CancelToken,
    delta: &str,
) {
    let mut early = lock_early(early);
    early.first_text.get_or_insert_with(Instant::now);
    // After a cancel (barge-in) no further sentence may be submitted (DESIGN 8.2).
    if !cfg.early_speech || early.dead || is_cancelled(cancel) {
        return;
    }
    for sentence in early.streamer.push(delta) {
        if queue.push(&sentence.text, sentence.emotion) {
            record_speech(reporter, &sentence.text);
            early.journal.submit(&sentence.text);
            early.spoken.push(sentence.text);
            mark_speaking(&mut early, reporter);
        } else {
            early.dead = true;
            break;
        }
    }
}

/// Session state carried from turn to turn.
#[derive(Default)]
struct Carry {
    last_draft: Option<String>,
    /// Transcript journal of the utterance in progress (DESIGN 8.2).
    journal: Journal,
    /// The acknowledgement last queued (smart acknowledgement avoids repeating it).
    last_ack: Option<&'static str>,
    /// Full utterance of the last recorded `failed` entry (see `record_turn`).
    last_failed: Option<String>,
    /// Latency instants of the utterance in progress (reset before each dispatch).
    clock: TurnClock,
    /// Stage 1 roles: the active role text. Never persisted and never in history; it ends with
    /// the session (a new session starts with a fresh `Carry`).
    role: Option<String>,
    /// Stage 1 lessons: the paused lesson, if any (see `converse_role::Lesson`).
    lesson: Option<Lesson>,
    /// True when the turn's plan was an Answer delivered in full (see `continue_lesson`).
    answer_delivered: bool,
    /// Stage 2: the agent run owned by the loop (`converse_agent_loop`). Ends with the session.
    agent: Option<converse_agent_loop::AgentSlot>,
    /// Stage 2: speech owed to the user from the agent run, delivered at loop boundaries.
    undelivered: VecDeque<converse_agent_loop::Narration>,
    /// Stage 2: set when an agent result was recorded. Lasts the rest of the session and makes
    /// `run_final_plan` ask a forced yes/no question before each planner-derived effect.
    tainted: bool,
}

/// Consecutive `Ignored` listens that count as one idle turn (T9b).
const IGNORED_STREAK_LIMIT: u8 = 5;

/// Loop-measured instants of one brain turn (T9b, D4). Numbers only.
#[derive(Default)]
struct TurnClock {
    /// Just before the plan request is issued.
    plan_requested: Option<Instant>,
    /// When the plan request returned (ok or failed).
    plan_ready: Option<Instant>,
    /// The plan request succeeded.
    plan_ok: bool,
    /// When speech was first handed to the voice (queued, or the first `say` was issued).
    speech_at: Option<Instant>,
}

impl TurnClock {
    /// `(brain_ms, to_speech_ms)` when the turn is complete: the plan succeeded and speech was
    /// handed to the voice. `to_speech_ms` saturates to 0 when speech started during streaming.
    fn completed(&self) -> Option<(u32, u32)> {
        if !self.plan_ok {
            return None;
        }
        let requested = self.plan_requested?;
        let ready = self.plan_ready?;
        let speech = self.speech_at?;
        Some((
            millis_u32(ready.saturating_duration_since(requested)),
            millis_u32(speech.saturating_duration_since(ready)),
        ))
    }
}

fn millis_u32(elapsed: Duration) -> u32 {
    u32::try_from(elapsed.as_millis()).unwrap_or(u32::MAX)
}

/// The acknowledgement for a turn: its delay and text. `None` when acknowledgements are off.
/// `ack_after` is the enable flag; with `smart_ack` the text and delay come from `converse_ack`.
fn ack_for(
    cfg: &SessionConfig,
    utterance: &str,
    has_draft: bool,
    last_ack: Option<&'static str>,
) -> Option<(Duration, &'static str)> {
    let after = cfg.ack_after?;
    if cfg.smart_ack {
        let ack = choose(utterance, has_draft, last_ack);
        Some((ack.after, ack.text))
    } else {
        Some((after, MSG_ACK))
    }
}

/// The acknowledgement: queued once, only while nothing has been spoken yet.
fn push_ack(
    queue: &dyn SpeechQueue,
    early: &Mutex<Early>,
    reporter: &dyn Reporter,
    text: &'static str,
) {
    let mut early = lock_early(early);
    if early.dead || !early.spoken.is_empty() || early.ack.is_some() {
        return;
    }
    if queue.push(text, Emotion::Neutral) {
        record_speech(reporter, text);
        early.ack = Some(text);
        mark_speaking(&mut early, reporter);
    } else {
        early.dead = true;
    }
}

/// The streamed brain request. Text deltas feed `early`; if the brain is still silent
/// `ack_after` into the request, the acknowledgement is queued once and the same request
/// keeps running.
async fn stream_plan(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    queue: &dyn SpeechQueue,
    early: &Mutex<Early>,
    ack: Option<(Duration, &'static str)>,
    cancel: &CancelToken,
    input: PlanInput<'_>,
) -> Result<TurnPlan, PlanError> {
    let mut on_text = |delta: &str| on_delta(early, cfg, queue, deps.reporter, cancel, delta);
    let mut plan = deps.brain.plan_streaming(input, &mut on_text);
    if let Some((after, text)) = ack {
        let timer = std::pin::pin!(tokio::time::sleep(after));
        match select(&mut plan, timer).await {
            Either::Left((result, _timer)) => return result,
            Either::Right((_elapsed, _plan)) => push_ack(queue, early, deps.reporter, text),
        }
    }
    plan.await
}

/// Turns a streamed plan into the turn's outcome: `Ok(Some(summary))` to record history,
/// `Ok(None)` to continue silently, `Err(end)` to end the session.
async fn streamed_outcome(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    queue: &dyn SpeechQueue,
    early: &mut Early,
    plan: TurnPlan,
    carry: &mut Carry,
) -> Result<Option<String>, SessionEnd> {
    if early.dead {
        queue.abort();
        return Ok(None);
    }
    if matches!(
        plan.kind,
        PlanKind::SetRole
            | PlanKind::ClearRole
            | PlanKind::Resume
            | PlanKind::Agent
            | PlanKind::Plan
            | PlanKind::Job
    ) {
        // Agent and Job plans too: the streamer never releases them as early speech (2.1). A Job
        // plan is executed through `run_final_plan` like the other effect kinds (DESIGN 2.5).
        // Role plans are never released as early speech (the streamer accepts only answer and
        // clarify), so the final plan runs in full through the one-shot path, as `Legacy` does.
        if early.ack.is_some() {
            queue.abort();
        }
        return run_final_plan(deps, cfg, cancel, plan, carry).await;
    }
    let text = match reconcile(&plan, &early.spoken) {
        Reconcile::Abort => {
            queue.abort();
            into_result(speak_or_end(deps, cfg, cancel, MSG_PLAN_FAILED, Emotion::Neutral).await)?;
            return Ok(Some(failure_summary(&early.spoken)));
        }
        Reconcile::Legacy => {
            // Only an already-pushed acknowledgement is replaced by the legacy say.
            if early.ack.is_some() {
                queue.abort();
            }
            return run_final_plan(deps, cfg, cancel, plan, carry).await;
        }
        Reconcile::Remainder { text } => text,
    };
    if !text.is_empty() {
        mark_speaking(early, deps.reporter);
        if !queue.push(&text, plan.emotion) {
            queue.abort();
            return Ok(None);
        }
        record_speech(deps.reporter, &text);
        carry.journal.submit(&text);
    }
    let Some(outcome) = race(cancel, queue.finish()).await else {
        queue.abort();
        return Err(SessionEnd::Cancelled);
    };
    let result = settle(deps, cfg, cancel, outcome).await;
    if matches!(plan.kind, PlanKind::End) {
        return Err(after_speech(result, SessionEnd::UserEnded));
    }
    if let Some(end) = speech_to_end(result) {
        return Err(end);
    }
    Ok(Some(answer_summary(&plan.speech, outcome)))
}

/// A plan request that failed or was superseded. `Ok(Some(summary))` records the failure in
/// history (`spoken` is the queued prefix of this turn); `Ok(None)` = continue without recording.
async fn plan_failed(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    err: PlanError,
    spoken: &[String],
    journal: &Journal,
) -> Result<Option<String>, SessionEnd> {
    match err {
        PlanError::NotConfigured => Err(after_speech(
            speak(deps, cfg, cancel, MSG_NOT_CONFIGURED, Emotion::Neutral).await,
            SessionEnd::NotConfigured,
        )),
        PlanError::Superseded => Ok(None),
        _ => {
            journal.set(TurnOutcome::Failed, None);
            into_result(speak_or_end(deps, cfg, cancel, MSG_PLAN_FAILED, Emotion::Neutral).await)?;
            Ok(Some(failure_summary(spoken)))
        }
    }
}

fn ms_since(base: Instant, at: Option<Instant>) -> String {
    match at {
        Some(at) => at.saturating_duration_since(base).as_millis().to_string(),
        None => "-".to_string(),
    }
}

/// Timing-only pass-through around the turn's mouth. Every call is forwarded unchanged, in the
/// same order; the only addition is recording when the mouth accepted a say. A say counts as
/// accepted when it did not fail (`Done` or `Interrupted`).
struct TimedMouth<'a> {
    inner: &'a dyn Mouth,
    first_speech: Mutex<Option<Instant>>,
    first_call: Mutex<Option<Instant>>,
}

impl<'a> TimedMouth<'a> {
    fn new(inner: &'a dyn Mouth) -> Self {
        Self {
            inner,
            first_speech: Mutex::new(None),
            first_call: Mutex::new(None),
        }
    }

    /// The instant the first accepted say returned, if any.
    fn first_speech(&self) -> Option<Instant> {
        *self
            .first_speech
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The instant the first say was issued, if any (T9b: speech handed to the voice).
    fn first_call(&self) -> Option<Instant> {
        *self
            .first_call
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Mouth for TimedMouth<'_> {
    fn say<'b>(&'b self, text: &'b str, emotion: Emotion) -> BoxFuture<'b, SayOutcome> {
        let inner = self.inner;
        Box::pin(async move {
            self.first_call
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get_or_insert_with(Instant::now);
            let outcome = inner.say(text, emotion).await;
            if outcome != SayOutcome::Failed {
                let mut first = self
                    .first_speech
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                first.get_or_insert_with(Instant::now);
            }
            outcome
        })
    }

    fn wait_quiet(&self, max: Duration) -> BoxFuture<'_, QuietOutcome> {
        self.inner.wait_quiet(max)
    }

    fn stop(&self) {
        self.inner.stop();
    }

    fn open_queue(&self) -> Option<Arc<dyn SpeechQueue>> {
        self.inner.open_queue()
    }
}

/// One latency line per turn, emitted through the reporter. Only when `timing_log` is on.
/// Never contains any text.
fn log_timing(
    reporter: &dyn Reporter,
    cfg: &SessionConfig,
    tier_start: Instant,
    first_text: Option<Instant>,
    first_speech: Option<Instant>,
    plan_at: Instant,
) {
    if !cfg.timing_log {
        return;
    }
    let line = format_timing(
        tier_start,
        first_text,
        first_speech,
        plan_at,
        Instant::now(),
    );
    reporter.timing_line(&line);
}

/// The streaming turn latency line (`converse timing: ...`). Missing values print as `-`.
/// Never contains any text.
pub(crate) fn format_timing(
    tier_start: Instant,
    first_text: Option<Instant>,
    first_speech: Option<Instant>,
    plan_at: Instant,
    now: Instant,
) -> String {
    format!(
        "converse timing: first_text_ms={} first_speech_ms={} plan_ms={} total_ms={}",
        ms_since(tier_start, first_text),
        ms_since(tier_start, first_speech),
        ms_since(tier_start, Some(plan_at)),
        ms_since(tier_start, Some(now)),
    )
}

/// The transcription latency line (`converse transcribe: ms=.. audio_ms=..`). Never contains
/// any text.
pub(crate) fn format_transcribe(ms: u64, audio_ms: u64) -> String {
    format!("converse transcribe: ms={ms} audio_ms={audio_ms}")
}

/// Brain tier: plan the turn and carry it out. Streams (early speech / acknowledgement)
/// when the mouth opens a queue; otherwise the legacy one-shot path.
async fn brain_turn(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    utterance: &str,
    history: &[(String, String)],
    carry: &mut Carry,
) -> Result<Option<String>, SessionEnd> {
    let tier_start = Instant::now();
    // Timing only: the mouth is wrapped so the turn's speech is timed. Every call still
    // reaches the real mouth in the same order.
    let timed = TimedMouth::new(deps.mouth);
    let timed_deps = Deps {
        mouth: &timed,
        ears: deps.ears,
        hands: deps.hands,
        brain: deps.brain,
        reporter: deps.reporter,
        policy: deps.policy,
        announcer: deps.announcer,
    };
    let deps = &timed_deps;
    let start = history.len().saturating_sub(cfg.context_turns);
    expire_lesson(carry);
    let input = PlanInput {
        utterance,
        history: history.get(start..).unwrap_or_default(),
        last_draft: carry.last_draft.as_deref(),
        style: &cfg.style,
        vault_context: None,
        memory_context: None,
        role: carry.role.as_deref(),
        lesson_paused: carry.lesson.is_some(),
        roles_enabled: true,
        agent_enabled: converse_agent_loop::handoff_effective(cfg),
        planner_enabled: cfg.agent_planner,
    };
    // Answer-only (DESIGN 8.1): no streamed model text may reach the mouth before a complete
    // plan is parsed, so the streaming path is never used. Role-active turns (Stage 1 roles,
    // DESIGN 2.5) use the final plan only: no early speech and no acknowledgement.
    let queue = if cfg.answer_only || carry.role.is_some() {
        None
    } else if cfg.early_speech || cfg.ack_after.is_some() {
        deps.mouth.open_queue()
    } else {
        None
    };
    let Some(queue) = queue else {
        carry.clock.plan_requested = Some(Instant::now());
        let Some(planned) = race(cancel, deps.brain.plan(input)).await else {
            return Err(SessionEnd::Cancelled);
        };
        let plan_at = Instant::now();
        carry.clock.plan_ready = Some(plan_at);
        carry.clock.plan_ok = planned.is_ok();
        let turn = match planned {
            Ok(plan) => run_final_plan(deps, cfg, cancel, plan, carry).await,
            Err(err) => plan_failed(deps, cfg, cancel, err, &[], &carry.journal).await,
        };
        carry.clock.speech_at = timed.first_call();
        log_timing(
            deps.reporter,
            cfg,
            tier_start,
            None,
            timed.first_speech(),
            plan_at,
        );
        return turn;
    };
    let ack = ack_for(cfg, utterance, carry.last_draft.is_some(), carry.last_ack);
    let early = Mutex::new(Early {
        journal: carry.journal.clone(),
        ..Early::default()
    });
    let stream_cancel = cancel.clone();
    carry.clock.plan_requested = Some(Instant::now());
    let planned = race(
        cancel,
        stream_plan(
            deps,
            cfg,
            queue.as_ref(),
            &early,
            ack,
            &stream_cancel,
            input,
        ),
    )
    .await;
    let plan_at = Instant::now();
    carry.clock.plan_ready = Some(plan_at);
    carry.clock.plan_ok = matches!(planned, Some(Ok(_)));
    let mut early = take_early(early);
    carry.last_ack = early.ack.or(carry.last_ack);
    let turn = match planned {
        None => {
            queue.abort();
            Err(SessionEnd::Cancelled)
        }
        Some(Ok(plan)) => {
            streamed_outcome(deps, cfg, cancel, queue.as_ref(), &mut early, plan, carry).await
        }
        Some(Err(err)) => {
            if !early.spoken.is_empty() || early.ack.is_some() || early.dead {
                queue.abort();
            }
            // A dead queue means the user or system took the speaker: nothing is recorded.
            let turn = plan_failed(deps, cfg, cancel, err, &early.spoken, &carry.journal).await;
            if early.dead {
                turn.map(|_| None)
            } else {
                turn
            }
        }
    };
    carry.clock.speech_at = early.first_speech.or_else(|| timed.first_call());
    log_timing(
        deps.reporter,
        cfg,
        tier_start,
        early.first_text,
        early.first_speech,
        plan_at,
    );
    turn
}

/// Drops the paused lesson when it is older than its lifetime. Checked whenever it is read.
fn expire_lesson(carry: &mut Carry) {
    if carry
        .lesson
        .as_ref()
        .is_some_and(|lesson| lesson.expired(lesson_now()))
    {
        carry.lesson = None;
    }
}

/// Says the front chunk of a speech queue. The chunk leaves the queue only when its say returned
/// `Done`, before the settle steps. `Interrupted` and `Failed` keep it at the front. A session stop
/// or a reply cancel drops the say and leaves the queue as it was (the queue is owned by the caller,
/// not by this future).
async fn say_front_chunk(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    queue: &mut VecDeque<String>,
    emotion: Emotion,
) -> Result<SayOutcome, SessionEnd> {
    let Some(text) = queue.front().cloned() else {
        return Ok(SayOutcome::Done);
    };
    deps.reporter.state(SessionState::Speaking);
    if is_cancelled(cancel) {
        return Err(SessionEnd::Cancelled);
    }
    let Some(outcome) = race(cancel, deps.mouth.say(&text, emotion)).await else {
        return Err(SessionEnd::Cancelled);
    };
    if outcome == SayOutcome::Done {
        queue.pop_front();
    }
    match settle(deps, cfg, cancel, outcome).await {
        SpeakResult::Spoken(_) => Ok(outcome),
        SpeakResult::Cancelled => Err(SessionEnd::Cancelled),
        SpeakResult::Stuck => Err(SessionEnd::SpeechStuck),
    }
}

/// Speaks a queue in order until it is empty or a say does not finish as `Done`. Returns the
/// outcome of the last say (`Done` for an empty queue).
async fn speak_queue(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    queue: &mut VecDeque<String>,
    emotion: Emotion,
) -> Result<SayOutcome, SessionEnd> {
    loop {
        if queue.is_empty() {
            return Ok(SayOutcome::Done);
        }
        match say_front_chunk(deps, cfg, cancel, queue, emotion).await? {
            SayOutcome::Done => {}
            other => return Ok(other),
        }
    }
}

/// Speaks the paused lesson from its front. The lesson is dropped once it is fully spoken. Each
/// chunk that is committed (said `Done`) touches the lesson, so its ten minutes run from the last
/// chunk rather than from creation.
async fn speak_lesson(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    carry: &mut Carry,
) -> Result<SayOutcome, SessionEnd> {
    loop {
        let Some(lesson) = carry.lesson.as_mut() else {
            return Ok(SayOutcome::Done);
        };
        if lesson.chunks.is_empty() {
            carry.lesson = None;
            return Ok(SayOutcome::Done);
        }
        let outcome =
            say_front_chunk(deps, cfg, cancel, &mut lesson.chunks, Emotion::Neutral).await?;
        if outcome != SayOutcome::Done {
            return Ok(outcome);
        }
        touch_lesson(carry);
    }
}

/// Marks the paused lesson, if any, as used now.
fn touch_lesson(carry: &mut Carry) {
    if let Some(lesson) = carry.lesson.as_mut() {
        lesson.touch(lesson_now());
    }
}

/// The lesson chunks still unspoken, in speaking order (empty without a lesson).
fn lesson_chunks_left(carry: &Carry) -> Vec<String> {
    carry
        .lesson
        .as_ref()
        .map(|lesson| lesson.chunks.iter().cloned().collect())
        .unwrap_or_default()
}

/// An Answer plan. With a role active the speech is spoken as chunks from a turn-local queue: an
/// interrupted answer drops its remainder and never touches the paused lesson. Without a role the
/// speech is one `say`, as before.
async fn answer_turn(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    plan: &TurnPlan,
    carry: &mut Carry,
) -> Result<String, SessionEnd> {
    let outcome = if carry.role.is_some() {
        let mut queue: VecDeque<String> = split_chunks(&plan.speech).into();
        speak_queue(deps, cfg, cancel, &mut queue, plan.emotion).await?
    } else {
        speak_outcome(deps, cfg, cancel, &plan.speech, plan.emotion).await?
    };
    carry.answer_delivered = outcome == SayOutcome::Done;
    Ok(answer_summary(&plan.speech, outcome))
}

/// A SetRole plan. The role is committed before anything is spoken. The acknowledgement is spoken
/// next; only when it finished (`Done`) and the plan carries a first request is the one extra
/// planner call made (`lesson_start_turn`). An interrupted acknowledgement keeps the role and makes
/// no call.
async fn set_role_turn(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    plan: &TurnPlan,
    carry: &mut Carry,
) -> Result<String, SessionEnd> {
    let Some(directive) = plan.role.as_ref() else {
        // A role plan without its directive is not validated: nothing is committed.
        carry.journal.set(TurnOutcome::Failed, None);
        into_result(speak_or_end(deps, cfg, cancel, MSG_PLAN_FAILED, Emotion::Neutral).await)?;
        return Ok(failed_summary());
    };
    carry.role = Some(directive.role.clone());
    carry.lesson = None;
    let outcome = speak_outcome(deps, cfg, cancel, &plan.speech, plan.emotion).await?;
    let summary = answer_summary(&plan.speech, outcome);
    let task = directive
        .task
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    if let (SayOutcome::Done, Some(task)) = (outcome, task) {
        lesson_start_turn(deps, cfg, cancel, task, directive.resume, carry).await?;
    }
    Ok(summary)
}

/// The one extra planner call of a compound start: the task, answered in role. Only an Answer is
/// accepted. Any other kind is ignored with no effect. A failed call speaks one fixed line. The
/// accepted lesson is spoken from `carry.lesson`, so an interruption leaves its remainder paused.
async fn lesson_start_turn(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    task: &str,
    resume_auto: bool,
    carry: &mut Carry,
) -> Result<(), SessionEnd> {
    let input = PlanInput {
        utterance: task,
        history: &[],
        last_draft: None,
        style: &cfg.style,
        vault_context: None,
        memory_context: None,
        role: carry.role.as_deref(),
        lesson_paused: false,
        roles_enabled: true,
        agent_enabled: converse_agent_loop::handoff_effective(cfg),
        planner_enabled: cfg.agent_planner,
    };
    deps.reporter.state(SessionState::Thinking);
    let Some(planned) = race(cancel, deps.brain.plan(input)).await else {
        return Err(SessionEnd::Cancelled);
    };
    match planned {
        Ok(plan) if plan.kind == PlanKind::Answer => {
            let chunks = split_chunks(&plan.speech);
            carry.lesson =
                (!chunks.is_empty()).then(|| Lesson::new(chunks, resume_auto, lesson_now()));
            speak_lesson(deps, cfg, cancel, carry).await?;
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(_) => into_result(
            speak_or_end(
                deps,
                cfg,
                cancel,
                LESSON_START_FAILED_SPOKEN,
                Emotion::Neutral,
            )
            .await,
        ),
    }
}

/// A Resume plan speaks the paused lesson from its front; its own speech is ignored. With no lesson
/// it behaves like an Answer.
async fn resume_turn(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    plan: &TurnPlan,
    carry: &mut Carry,
) -> Result<String, SessionEnd> {
    expire_lesson(carry);
    if carry.lesson.is_none() {
        return answer_turn(deps, cfg, cancel, plan, carry).await;
    }
    touch_lesson(carry);
    let before = lesson_chunks_left(carry);
    let outcome = speak_lesson(deps, cfg, cancel, carry).await?;
    let left = lesson_chunks_left(carry).len();
    let spoken = before
        .iter()
        .take(before.len().saturating_sub(left))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    Ok(answer_summary(&spoken, outcome))
}

/// After an Answer that was delivered in full, a lesson with `resume_auto` continues: the fixed
/// continuation line, then the remainder from its front. Runs inside the reply scope of the turn.
/// `Ok(None)` when nothing continues; otherwise the history side of what was spoken (the line and
/// the chunks actually committed), with the usual note when it was not delivered in full.
async fn continue_lesson(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    carry: &mut Carry,
) -> Result<Option<String>, SessionEnd> {
    expire_lesson(carry);
    if !carry
        .lesson
        .as_ref()
        .is_some_and(|lesson| lesson.resume_auto)
    {
        return Ok(None);
    }
    touch_lesson(carry);
    let outcome = speak_outcome(deps, cfg, cancel, CONTINUE_SPOKEN, Emotion::Neutral).await?;
    if outcome != SayOutcome::Done {
        return Ok(Some(answer_summary(CONTINUE_SPOKEN, outcome)));
    }
    let before = lesson_chunks_left(carry);
    let lesson_outcome = speak_lesson(deps, cfg, cancel, carry).await?;
    let spoken_count = before.len().saturating_sub(lesson_chunks_left(carry).len());
    let spoken: Vec<String> = std::iter::once(CONTINUE_SPOKEN.to_string())
        .chain(before.into_iter().take(spoken_count))
        .collect();
    Ok(Some(answer_summary(&spoken.join(" "), lesson_outcome)))
}

/// Executes a final plan with the one-shot `Mouth::say` speech. `Ok(None)` = continue the
/// loop without recording history.
async fn execute_plan(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    plan: TurnPlan,
    carry: &mut Carry,
) -> Result<Option<String>, SessionEnd> {
    // Cancel is checked before each effect.
    if cfg.answer_only
        && matches!(
            plan.kind,
            PlanKind::Paste | PlanKind::Command | PlanKind::Job | PlanKind::Agent | PlanKind::Plan
        )
    {
        // Answer-only boundary (DESIGN 3.6): zero paste/command/job calls; only the fixed refusal.
        carry.journal.set(TurnOutcome::Blocked, None);
        into_result(speak_or_end(deps, cfg, cancel, MSG_ANSWER_ONLY, Emotion::Neutral).await)?;
        return Ok(Some(compose(MSG_ANSWER_ONLY, "")));
    }
    let summary: String = match plan.kind {
        PlanKind::End => {
            return Err(after_speech(
                speak(deps, cfg, cancel, &plan.speech, plan.emotion).await,
                SessionEnd::UserEnded,
            ));
        }
        // Job plans are parsed only when jobs are allowed and are routed through gate_and_run.
        // With no policy a Job plan never reaches this answer path (parsing refuses it), so it
        // is treated as answer text here.
        PlanKind::Answer => {
            carry.journal.set(speech_outcome(&plan.kind), None);
            answer_turn(deps, cfg, cancel, &plan, carry).await?
        }
        // Stage 2: the hand-off is effective only when enabled and not answer-only. Otherwise the
        // plan is a plain Answer (2.1).
        PlanKind::Agent => {
            carry.journal.set(speech_outcome(&plan.kind), None);
            if converse_agent_loop::handoff_effective(cfg) {
                converse_agent_loop::agent_turn(deps, cfg, cancel, &plan, carry).await?
            } else {
                let mut as_answer = plan.clone();
                if as_answer.speech.trim().is_empty() {
                    as_answer.speech = converse_agent_loop::LINE_NOT_EFFECTIVE.to_string();
                }
                answer_turn(deps, cfg, cancel, &as_answer, carry).await?
            }
        }
        // Stage 4 (DESIGN 2.1, 2.5): the planner runs only when effective; otherwise the plan is
        // answered as plain speech.
        PlanKind::Plan => {
            carry.journal.set(speech_outcome(&plan.kind), None);
            if converse_agent_loop::plan_effective(cfg) {
                converse_agent_loop::plan_turn(deps, cfg, cancel, &plan, carry, false).await?
            } else {
                let mut as_answer = plan.clone();
                if as_answer.speech.trim().is_empty() {
                    as_answer.speech = converse_agent_loop::LINE_NOT_EFFECTIVE.to_string();
                }
                answer_turn(deps, cfg, cancel, &as_answer, carry).await?
            }
        }
        PlanKind::Clarify | PlanKind::Job => {
            carry.journal.set(speech_outcome(&plan.kind), None);
            let outcome = speak_outcome(deps, cfg, cancel, &plan.speech, plan.emotion).await?;
            answer_summary(&plan.speech, outcome)
        }
        PlanKind::SetRole => {
            carry.journal.set(speech_outcome(&plan.kind), None);
            set_role_turn(deps, cfg, cancel, &plan, carry).await?
        }
        PlanKind::ClearRole => {
            carry.journal.set(speech_outcome(&plan.kind), None);
            carry.role = None;
            carry.lesson = None;
            let outcome = speak_outcome(deps, cfg, cancel, &plan.speech, plan.emotion).await?;
            answer_summary(&plan.speech, outcome)
        }
        PlanKind::Resume => {
            carry.journal.set(speech_outcome(&plan.kind), None);
            resume_turn(deps, cfg, cancel, &plan, carry).await?
        }
        PlanKind::Paste => {
            let Some(pasted) = plan.paste_text.clone() else {
                into_result(
                    speak_or_end(deps, cfg, cancel, MSG_PASTE_FAILED, Emotion::Neutral).await,
                )?;
                return Ok(Some(paste_failed_summary()));
            };
            if is_cancelled(cancel) {
                return Err(SessionEnd::Cancelled);
            }
            if deps.hands.paste_generated(&pasted).is_err() {
                into_result(
                    speak_or_end(deps, cfg, cancel, MSG_PASTE_FAILED, Emotion::Neutral).await,
                )?;
                return Ok(Some(paste_failed_summary()));
            }
            // The paste reached the window: the effect is recorded even if the confirmation fails.
            let note = note_paste(&pasted);
            carry.journal.set(
                TurnOutcome::Dictated,
                Some(format!("dictated {} chars", pasted.chars().count())),
            );
            carry.last_draft = Some(pasted);
            let spoken = if plan.speech.is_empty() {
                MSG_DONE.to_string()
            } else {
                plan.speech.clone()
            };
            speak_outcome(deps, cfg, cancel, &spoken, plan.emotion).await?;
            compose(&spoken, &note)
        }
        PlanKind::Command => {
            match run_command_plan(deps, cfg, cancel, &plan, &carry.journal).await? {
                Some(summary) => summary,
                None => return Ok(None),
            }
        }
    };
    Ok(Some(summary))
}

/// The single gated dispatch point (DESIGN R2-12). Without a policy, and for every plan the
/// gate does not cover, this is the legacy `execute_plan`. Gated and Job plans are prepared,
/// confirmed (when the gate is active), re-checked against the approval fingerprint, and then
/// executed from the same prepared payload.
async fn gate_and_run(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    plan: TurnPlan,
    carry: &mut Carry,
) -> Result<Option<String>, SessionEnd> {
    let Some(policy) = deps.policy else {
        return execute_plan(deps, cfg, cancel, plan, carry).await;
    };
    let gated = matches!(
        plan.kind,
        PlanKind::Paste | PlanKind::Command | PlanKind::Job
    );
    if !gated {
        return execute_plan(deps, cfg, cancel, plan, carry).await;
    }
    let state = policy.current();
    if !state.gate_active && !matches!(plan.kind, PlanKind::Job) {
        return execute_plan(deps, cfg, cancel, plan, carry).await;
    }
    if cfg.answer_only
        && matches!(
            plan.kind,
            PlanKind::Paste | PlanKind::Command | PlanKind::Job | PlanKind::Agent | PlanKind::Plan
        )
    {
        return execute_plan(deps, cfg, cancel, plan, carry).await;
    }
    let prepared = match prepare_action(&plan, policy, deps.hands).await {
        Ok(prepared) => prepared,
        Err(PrepareError::Refused(text)) => {
            carry.journal.set(TurnOutcome::Blocked, None);
            into_result(speak_or_end(deps, cfg, cancel, &text, Emotion::Neutral).await)?;
            return Ok(Some(compose(&text, "")));
        }
        Err(PrepareError::Unsupported) => {
            return execute_plan(deps, cfg, cancel, plan, carry).await;
        }
    };
    let fallback = Announcer::new(Arc::new(NoAnnounce));
    let announcer = deps.announcer.unwrap_or(&fallback);
    // R3-2: only a job is bound to the approval fingerprint. Paste and Command proceed on
    // the user's yes even if the gate is switched off meanwhile.
    let selection = prepared.selection().cloned();
    let approved_at = selection
        .as_ref()
        .map(|sel| fingerprint_for(policy, Some(sel)));
    if state.gate_active {
        let question = confirmation_question(&prepared);
        let decision = ask_confirmation(deps, cfg, announcer, cancel, carry, &question).await;
        if let Decision::Declined(reason) = &decision {
            carry
                .journal
                .set(TurnOutcome::Blocked, Some("declined".to_string()));
            let message = decline_message(reason.clone());
            into_result(speak_or_end(deps, cfg, cancel, message, Emotion::Neutral).await)?;
            return Ok(Some(compose(message, "")));
        }
        carry
            .journal
            .set(TurnOutcome::Acted, Some("confirmed".to_string()));
    }
    if approved_at
        .as_ref()
        .is_some_and(|approved| fingerprint_for(policy, selection.as_ref()) != *approved)
    {
        carry.journal.set(TurnOutcome::Blocked, None);
        into_result(speak_or_end(deps, cfg, cancel, MSG_SETTINGS_CHANGED, Emotion::Neutral).await)?;
        return Ok(Some(compose(MSG_SETTINGS_CHANGED, "")));
    }
    match prepared {
        PreparedAction::Job { launch } => {
            if is_cancelled(cancel) {
                return Err(SessionEnd::Cancelled);
            }
            let message = job_outcome_message(deps.hands.start_job(launch));
            carry
                .journal
                .set(TurnOutcome::Acted, Some("job".to_string()));
            speak_outcome(deps, cfg, cancel, &message, Emotion::Neutral).await?;
            Ok(Some(compose(&message, "")))
        }
        other => dispatch_prepared(deps, cfg, cancel, other, carry).await,
    }
}

/// The one entry for planner-derived plans (DESIGN 2.5, C6): every final plan from the brain
/// (streamed or not) passes here and nothing else sends it to `gate_and_run` or `execute_plan`.
/// Order: (1) the answer-only refusal (`execute_plan` speaks it for Paste, Command, Job, Agent);
/// (2) while the session is tainted by an agent result, a forced yes/no question for Paste,
/// Command, Job, Agent and SetRole, which runs an immutable clone of the plan only on an exact
/// yes; (3) the gate and execute path. Local user-intent paths never reach this function.
async fn run_final_plan(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    plan: TurnPlan,
    carry: &mut Carry,
) -> Result<Option<String>, SessionEnd> {
    if cfg.answer_only
        && matches!(
            plan.kind,
            PlanKind::Paste | PlanKind::Command | PlanKind::Job | PlanKind::Agent | PlanKind::Plan
        )
    {
        return execute_plan(deps, cfg, cancel, plan, carry).await;
    }
    if !(carry.tainted && needs_forced_confirmation(cfg, &plan)) {
        return gate_and_run(deps, cfg, cancel, plan, carry).await;
    }
    if plan.kind == PlanKind::Agent {
        return forced_agent(deps, cfg, cancel, plan, carry).await;
    }
    if plan.kind == PlanKind::Plan {
        return converse_agent_loop::plan_turn(deps, cfg, cancel, &plan, carry, true)
            .await
            .map(Some);
    }
    let confirmed = plan.clone();
    let Some(question) = forced_prompt(&confirmed) else {
        return gate_and_run(deps, cfg, cancel, confirmed, carry).await;
    };
    match ask_forced(deps, cfg, cancel, carry, &question).await {
        Decision::Approved => gate_and_run(deps, cfg, cancel, confirmed, carry).await,
        Decision::Declined(_) => decline_forced(deps, cfg, cancel, carry).await,
    }
}

/// True for the planner-derived kinds that need the forced question while tainted. An Agent plan
/// needs it only when the hand-off is effective; otherwise it is answered as plain speech.
fn needs_forced_confirmation(cfg: &SessionConfig, plan: &TurnPlan) -> bool {
    match plan.kind {
        PlanKind::Paste | PlanKind::Command | PlanKind::Job | PlanKind::SetRole => true,
        PlanKind::Agent => converse_agent_loop::handoff_effective(cfg),
        PlanKind::Plan => converse_agent_loop::plan_effective(cfg),
        _ => false,
    }
}

/// A tainted Agent plan (DESIGN 2.5, C6): the launch is prepared before the question, and after
/// an exact yes it is started only if the settings still produce the same launch. The same
/// prepared launch is the one started.
async fn forced_agent(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    plan: TurnPlan,
    carry: &mut Carry,
) -> Result<Option<String>, SessionEnd> {
    carry.journal.set(speech_outcome(&plan.kind), None);
    let launch =
        match converse_agent_loop::prepare_agent_plan(deps, cfg, cancel, &plan, carry).await? {
            Ok(launch) => launch,
            Err(summary) => return Ok(Some(summary)),
        };
    let Some(question) = forced_prompt(&plan) else {
        return Ok(Some(failed_summary()));
    };
    if let Decision::Declined(_) = ask_forced(deps, cfg, cancel, carry, &question).await {
        return decline_forced(deps, cfg, cancel, carry).await;
    }
    if !deps.hands.agent_launch_still_valid(&launch) {
        carry.journal.set(TurnOutcome::Blocked, None);
        let line = converse_agent_loop::LINE_SETTINGS_CHANGED;
        into_result(speak_or_end(deps, cfg, cancel, line, Emotion::Neutral).await)?;
        return Ok(Some(compose(line, "")));
    }
    let summary = converse_agent_loop::run_prepared_agent(
        deps,
        cfg,
        cancel,
        launch,
        &plan.speech,
        plan.emotion,
        carry,
    )
    .await?;
    Ok(Some(summary))
}

/// Speaks one forced question and waits for the answer (DESIGN 2.5). The question's own speech
/// is rolled back out of the transcript journal, as for `ask_confirmation`.
async fn ask_forced(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    carry: &Carry,
    question: &str,
) -> Decision {
    let before = carry.journal.submitted_len();
    let fallback = Announcer::new(Arc::new(NoAnnounce));
    let announcer = deps.announcer.unwrap_or(&fallback);
    let config = confirm_config_for(cfg);
    let asked = confirm_forced(
        deps.ears,
        deps.mouth,
        announcer,
        question,
        &config,
        Some(deps.reporter),
    );
    let decision = match race(cancel, asked).await {
        Some(decision) => decision,
        None => Decision::Declined(DeclineReason::Cancelled),
    };
    carry.journal.truncate_submitted(before);
    decision
}

/// Declines a forced question: nothing runs and the decline line is spoken. A cancelled
/// session ends without speaking.
async fn decline_forced(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    carry: &mut Carry,
) -> Result<Option<String>, SessionEnd> {
    if is_cancelled(cancel) {
        return Err(SessionEnd::Cancelled);
    }
    carry
        .journal
        .set(TurnOutcome::Blocked, Some("declined".to_string()));
    into_result(speak_or_end(deps, cfg, cancel, MSG_FORCED_DECLINED, Emotion::Neutral).await)?;
    Ok(Some(compose(MSG_FORCED_DECLINED, "")))
}

/// Announcer stand-in used when the session has none: it never speaks queued text, so a
/// confirmation still runs without holding anything back.
struct NoAnnounce;

impl LowPriorityMouth for NoAnnounce {
    fn try_speak_if_idle(&self, _text: &str) -> bool {
        false
    }
}

/// Asks one confirmation question. Fails closed: no announcer, cancellation or a missing answer
/// declines. The question's own speech is rolled back out of the transcript journal, so the
/// question text and the answer are never stored (DESIGN D2).
async fn ask_confirmation(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    announcer: &Announcer,
    cancel: &mut CancelToken,
    carry: &Carry,
    question: &str,
) -> Decision {
    let before = carry.journal.submitted_len();
    let config = confirm_config_for(cfg);
    let asked = confirm_action_recorded(
        deps.ears,
        deps.mouth,
        announcer,
        question,
        &config,
        Some(deps.reporter),
    );
    let decision = match race(cancel, asked).await {
        Some(decision) => decision,
        None => Decision::Declined(DeclineReason::Cancelled),
    };
    carry.journal.truncate_submitted(before);
    decision
}

/// Executes a prepared, approved payload. Only the payload that was presented is used.
async fn dispatch_prepared(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    prepared: PreparedAction,
    carry: &mut Carry,
) -> Result<Option<String>, SessionEnd> {
    if is_cancelled(cancel) {
        return Err(SessionEnd::Cancelled);
    }
    match prepared {
        PreparedAction::Paste { text } => {
            if deps.hands.paste_generated(&text).is_err() {
                carry.journal.set(TurnOutcome::Failed, None);
                into_result(
                    speak_or_end(deps, cfg, cancel, MSG_PASTE_FAILED, Emotion::Neutral).await,
                )?;
                return Ok(Some(paste_failed_summary()));
            }
            let note = note_paste(&text);
            carry.journal.set(
                TurnOutcome::Dictated,
                Some(format!("dictated {} chars", text.chars().count())),
            );
            carry.last_draft = Some(text);
            speak_outcome(deps, cfg, cancel, MSG_DONE, Emotion::Neutral).await?;
            Ok(Some(compose(MSG_DONE, &note)))
        }
        PreparedAction::Command { cmd } => {
            let label = cmd.label.clone();
            if !deps.hands.run_validated_command(cmd) {
                carry.journal.set(TurnOutcome::Failed, None);
                into_result(speak_or_end(deps, cfg, cancel, MSG_FAILED, Emotion::Neutral).await)?;
                return Ok(Some(compose(
                    "",
                    &note_command(&label, &CommandNote::Failed),
                )));
            }
            carry
                .journal
                .set(TurnOutcome::Acted, Some("action: command".to_string()));
            speak_outcome(deps, cfg, cancel, MSG_DONE, Emotion::Neutral).await?;
            Ok(Some(compose(
                MSG_DONE,
                &note_command(&label, &CommandNote::Done(String::new())),
            )))
        }
        PreparedAction::Job { launch } => {
            // Unreachable: `gate_and_run` handles Job before dispatch_prepared.
            let _ = launch;
            Err(SessionEnd::Cancelled)
        }
    }
}

/// Fixed spoken text for each job start outcome (DESIGN D5, D8).
fn job_outcome_message(outcome: JobStartOutcome) -> String {
    match outcome {
        JobStartOutcome::Started(id) => {
            log::info!("coding job {id} started");
            "Started. I'll tell you when it's done.".to_string()
        }
        JobStartOutcome::Busy => "A coding job is already running.".to_string(),
        JobStartOutcome::NotConfigured => "Coding jobs aren't set up yet.".to_string(),
        JobStartOutcome::UnknownProject => "I couldn't find that project.".to_string(),
        JobStartOutcome::Unsupported => "Coding jobs aren't supported on this system.".to_string(),
        JobStartOutcome::Refused(text) => text,
    }
}

/// Speaks the background announcements queued during the last turn, in order, through the
/// session mouth (DESIGN R2-6). Called at each turn boundary before the next listen.
async fn speak_pending(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    announcer: Option<&Announcer>,
) -> Option<SessionEnd> {
    let announcer = announcer?;
    for item in announcer.take_pending_items() {
        // Resolve each item only now, just before it is spoken, so a change made during an
        // earlier item still degrades a later guarded summary (DESIGN D6, Revision 5).
        let text = item.resolve();
        if let Some(end) = speak_or_end(deps, cfg, cancel, &text, Emotion::Neutral).await {
            return Some(end);
        }
    }
    None
}

/// The turn loop. Returns the end; the wrapper reports Idle and stops.
/// Ends the session's agent run (cancel once, nothing spoken) on every return path (2.3).
async fn run_turns(outer: &Deps<'_>, cfg: &SessionConfig, cancel: &mut CancelToken) -> SessionEnd {
    let mut carry = Carry::default();
    let end = run_turns_loop(outer, cfg, cancel, &mut carry).await;
    converse_agent_loop::end_agent_session(&mut carry);
    end
}

async fn run_turns_loop(
    outer: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    carry: &mut Carry,
) -> SessionEnd {
    let mut history: Vec<(String, String)> = Vec::new();
    let mut idle_turns: u32 = 0;
    let mut consecutive_too_long: u32 = 0;
    let mut consecutive_ignored: u8 = 0;
    let mut turns: usize = 0;
    // T9b: completed turns (report numbering) and the session's rolling latency window.
    let mut completed_turns: u32 = 0;
    let mut rolling = RollingStats::new();
    // Barge-in: the utterance a barge-in capture heard, which replaces the next listen (12.2.3).
    let mut barge_next: Option<String> = None;
    // Every speech line handed to the mouth is journaled for the transcript (DESIGN 8.2).
    let tap = JournalMouth {
        inner: outer.mouth,
        journal: carry.journal.clone(),
        reporter: outer.reporter,
    };
    let tapped = Deps {
        policy: outer.policy,
        announcer: outer.announcer,
        ears: outer.ears,
        mouth: &tap,
        hands: outer.hands,
        brain: outer.brain,
        reporter: outer.reporter,
    };
    let deps = &tapped;

    loop {
        if is_cancelled(cancel) {
            return SessionEnd::Cancelled;
        }
        // Agent events are collected first; speech is delivered only here, outside any listen
        // and outside any reply scope. A captured barge-in is dispatched before anything else:
        // while one is pending, nothing is collected or advanced (2.4, A1).
        if barge_next.is_none() {
            converse_agent_loop::collect_agent_events(carry, &mut history, cfg, deps.hands);
            if let Some(mut narration) = carry.undelivered.pop_front() {
                match converse_agent_loop::speak_unsolicited(deps, cfg, cancel, &mut narration)
                    .await
                {
                    Ok(captured) => barge_next = captured,
                    Err(end) => return end,
                }
                if carry.undelivered.is_empty() {
                    idle_turns = 0;
                }
                continue;
            }
            // Safe boundary (A1): the successor of a finished plan step starts only here.
            if converse_agent_loop::advance_plan(carry, deps.hands) {
                continue;
            }
        }
        if turns >= cfg.max_turns {
            return SessionEnd::TurnLimit;
        }
        turns += 1;
        if let Some(end) = speak_pending(deps, cfg, cancel, outer.announcer).await {
            return end;
        }

        deps.reporter.state(SessionState::Listening);
        let (heard, listen_timing): (Option<ListenOutcome>, Option<ListenTiming>) =
            match barge_next.take() {
                // The barge-in capture already heard this turn: no listen (DESIGN 12.2.3).
                Some(text) => (Some(ListenOutcome::Utterance(text)), None),
                None => {
                    let heard = race(cancel, deps.ears.listen()).await;
                    // Timing of this listen, taken exactly once right after it returns (every outcome).
                    (heard, deps.ears.take_timing())
                }
            };
        let heard = match heard {
            None => return SessionEnd::Cancelled,
            Some(outcome) => outcome,
        };
        if !matches!(heard, ListenOutcome::Ignored) {
            consecutive_ignored = 0;
        }
        if !matches!(heard, ListenOutcome::Utterance(_)) {
            converse_agent_loop::collect_agent_events(carry, &mut history, cfg, deps.hands);
        }
        let utterance = match heard {
            ListenOutcome::Aborted => return SessionEnd::Cancelled,
            ListenOutcome::Failed(kind) => {
                return after_speech(
                    speak(
                        deps,
                        cfg,
                        cancel,
                        listen_failure_message(&kind),
                        Emotion::Neutral,
                    )
                    .await,
                    SessionEnd::AudioUnavailable,
                );
            }
            ListenOutcome::TooLong => {
                idle_turns = 0;
                // While the agent is occupied, too-long captures do not count toward the streak
                // and do not use the turn budget (max_turns counts user turns only, 2.3).
                if converse_agent_loop::agent_occupied(carry) {
                    turns = turns.saturating_sub(1);
                } else {
                    consecutive_too_long += 1;
                    if consecutive_too_long >= 2 {
                        return after_speech(
                            speak(deps, cfg, cancel, MSG_CANT_HEAR, Emotion::Neutral).await,
                            SessionEnd::AudioUnavailable,
                        );
                    }
                }
                if let Some(end) =
                    speak_or_end(deps, cfg, cancel, MSG_TOO_LONG, Emotion::Neutral).await
                {
                    return end;
                }
                continue;
            }
            ListenOutcome::NoSpeech => String::new(),
            ListenOutcome::Ignored => {
                // A foreign voice: no turn budget, no brain, no speech, not a too-long streak.
                turns = turns.saturating_sub(1);
                consecutive_too_long = 0;
                consecutive_ignored += 1;
                if consecutive_ignored < IGNORED_STREAK_LIMIT {
                    continue;
                }
                // The fifth in a row counts exactly like NoSpeech.
                consecutive_ignored = 0;
                // Restore the turn taken above: this capture counts exactly like NoSpeech.
                turns += 1;
                String::new()
            }
            ListenOutcome::Utterance(text) => text,
        };
        // Any capture that did not report TooLong breaks the streak.
        consecutive_too_long = 0;

        if utterance.trim().is_empty() {
            // While the agent is occupied (run or owed speech), silence does not count and does
            // not use the turn budget (2.3).
            if converse_agent_loop::agent_occupied(carry) {
                turns = turns.saturating_sub(1);
                continue;
            }
            idle_turns += 1;
            if idle_turns >= u32::from(cfg.max_idle_turns) {
                return after_speech(
                    speak(deps, cfg, cancel, MSG_STOP_LISTENING, Emotion::Neutral).await,
                    SessionEnd::Idle,
                );
            }
            continue;
        }
        idle_turns = 0;

        if utterance.chars().count() > MAX_UTTERANCE_CHARS {
            if let Some(end) = speak_or_end(deps, cfg, cancel, MSG_TOO_LONG, Emotion::Neutral).await
            {
                return end;
            }
            continue;
        }

        // Accepted for dispatch: the chat window records the user line once (DESIGN-CHAT 2.2).
        deps.reporter.chat(ChatRole::User, &utterance);
        // Accepted for dispatch: exactly one transcript event follows (DESIGN 8.2).
        let log = UtteranceLog::new(deps.reporter, &utterance, &carry.journal);
        carry.clock = TurnClock::default();
        // Barge-in (DESIGN 12.2.3, 12.7.C): one watch per reply. Without a monitor, nothing here
        // changes: the turn runs with the plain cancel token and the plain mouth.
        let barge_watch = deps.ears.barge_in().and_then(|barge| barge.arm());
        let reply_scope = barge_watch
            .as_deref()
            .map(|watch| (watch.reply_token(), watch.gate()));
        let dispatched = match reply_scope {
            None => {
                let turn = dispatch_utterance(deps, cfg, cancel, &utterance, &history, carry).await;
                finish_answer_with_lesson(deps, cfg, cancel, carry, turn, false).await
            }
            Some((reply, gate)) => {
                let mut turn_cancel = cancel.with_reply(reply);
                let gated = GatedMouth {
                    inner: deps.mouth,
                    gate,
                };
                let turn_deps = deps.with_mouth(&gated);
                let turn = dispatch_utterance(
                    &turn_deps,
                    cfg,
                    &mut turn_cancel,
                    &utterance,
                    &history,
                    carry,
                )
                .await;
                let latched = barge_watch
                    .as_deref()
                    .is_some_and(|watch| watch.cancel_latched());
                finish_answer_with_lesson(&turn_deps, cfg, &mut turn_cancel, carry, turn, latched)
                    .await
            }
        };
        // T9b: one latency report per completed turn (plan succeeded, speech handed to the voice).
        if let Some((brain_ms, to_speech_ms)) = carry.clock.completed() {
            completed_turns += 1;
            let report = LatencyReport::build(
                completed_turns,
                listen_timing.as_ref(),
                Some(brain_ms),
                Some(to_speech_ms),
            );
            rolling.push(&report);
            deps.reporter.latency(&report, &rolling.stats());
        }
        // A latched barge-in wins over a normal finish, but never over the session stop (12.2.3).
        let barged =
            barge_watch.filter(|watch| !cancel.session_cancelled() && watch.cancel_latched());
        if let Some(barge) = barged {
            deps.mouth.stop();
            // Built before `finish` consumes the journal: only the speech actually submitted.
            let spoken = carry.journal.peek_submitted().unwrap_or_default();
            log.finish(true);
            record_turn(
                &mut history,
                carry,
                cfg,
                &utterance,
                &interrupted_summary(&spoken),
            );
            barge_next = match capture_barge(cancel, barge).await {
                Ok(next) => next,
                Err(end) => return end,
            };
            continue;
        }
        match dispatched {
            Err(end) => {
                log.finish(end == SessionEnd::Cancelled);
                return end;
            }
            Ok(None) => log.finish(false),
            Ok(Some(summary)) => {
                log.finish(false);
                record_turn(&mut history, carry, cfg, &utterance, &summary);
            }
        }
        // Agent events that arrived during this turn are queued now; they are spoken at the top
        // of the next iteration (2.3, collection point after dispatch).
        converse_agent_loop::collect_agent_events(carry, &mut history, cfg, deps.hands);
    }
}

/// Handles one accepted utterance: local tier, exact tier, then the brain. `Ok(Some(summary))`
/// records history, `Ok(None)` continues silently, `Err(end)` ends the session.
async fn dispatch_utterance(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    utterance: &str,
    history: &[(String, String)],
    carry: &mut Carry,
) -> Result<Option<String>, SessionEnd> {
    carry.answer_delivered = false;
    // Local tier: end phrases, explicit dictation and role clear phrases.
    let cancel_enabled =
        deps.policy.is_some_and(|p| p.current().jobs_allowed) || deps.hands.has_active_job();
    let agent_active = converse_agent_loop::agent_running(carry);
    match classify_local_in_session(
        utterance,
        cancel_enabled,
        carry.role.is_some(),
        agent_active,
    ) {
        LocalIntent::ClearRole => {
            carry.role = None;
            carry.lesson = None;
            carry.journal.set(TurnOutcome::Acted, None);
            return match speak_or_end(deps, cfg, cancel, CLEARED_SPOKEN, Emotion::Neutral).await {
                Some(end) => Err(end),
                None => Ok(None),
            };
        }
        LocalIntent::End => {
            carry.journal.set(TurnOutcome::Ended, None);
            return Err(after_speech(
                speak(deps, cfg, cancel, MSG_OKAY, Emotion::Neutral).await,
                SessionEnd::UserEnded,
            ));
        }
        LocalIntent::Dictate(text) => {
            if is_cancelled(cancel) {
                return Err(SessionEnd::Cancelled);
            }
            if deps.hands.paste_generated(&text).is_err() {
                carry.journal.set(TurnOutcome::Failed, None);
                return match speak_or_end(deps, cfg, cancel, MSG_PASTE_FAILED, Emotion::Neutral)
                    .await
                {
                    Some(end) => Err(end),
                    None => Ok(None),
                };
            }
            let chars = text.chars().count();
            carry.journal.set(
                TurnOutcome::Dictated,
                Some(format!("dictated {chars} chars")),
            );
            return Ok(None);
        }
        LocalIntent::CancelJob => {
            if is_cancelled(cancel) {
                return Err(SessionEnd::Cancelled);
            }
            let message = if deps.hands.cancel_job() {
                "Cancelled the coding job."
            } else {
                "No coding job is running."
            };
            carry.journal.set(TurnOutcome::Acted, None);
            return match speak_or_end(deps, cfg, cancel, message, Emotion::Neutral).await {
                Some(end) => Err(end),
                None => Ok(None),
            };
        }
        // Stage 2 (2.6): voice cancel of a running agent request. Non-blocking; the slot waits in
        // `Cancelling` for the worker's report.
        LocalIntent::CancelAgent => {
            if is_cancelled(cancel) {
                return Err(SessionEnd::Cancelled);
            }
            converse_agent_loop::cancel_agent_run(carry);
            carry.journal.set(TurnOutcome::Acted, None);
            return match speak_or_end(
                deps,
                cfg,
                cancel,
                converse_agent_loop::LINE_CANCELLED,
                Emotion::Neutral,
            )
            .await
            {
                Some(end) => Err(end),
                None => Ok(Some(compose(converse_agent_loop::LINE_CANCELLED, ""))),
            };
        }
        LocalIntent::None => {}
    }

    // Exact tier: what the user said (shortcuts, media, router).
    deps.reporter.state(SessionState::Acting);
    if is_cancelled(cancel) {
        return Err(SessionEnd::Cancelled);
    }
    let fixed = match deps.hands.run_user_phrase(utterance).await {
        PhraseOutcome::NoMatch => None,
        PhraseOutcome::Done(message) => {
            carry.journal.set(TurnOutcome::Acted, None);
            Some(message)
        }
        PhraseOutcome::Failed => {
            carry.journal.set(TurnOutcome::Failed, None);
            Some(MSG_FAILED.to_string())
        }
        PhraseOutcome::Refused => {
            carry.journal.set(TurnOutcome::Failed, None);
            Some(MSG_REFUSED.to_string())
        }
    };
    if let Some(message) = fixed {
        return match speak_or_end(deps, cfg, cancel, &message, Emotion::Neutral).await {
            Some(end) => Err(end),
            None => Ok(None),
        };
    }

    // Brain tier. The plan is dropped if cancel wins the race.
    deps.reporter.state(SessionState::Thinking);
    brain_turn(deps, cfg, cancel, utterance, history, carry).await
}

/// Stage 1 lessons (DESIGN 2.6): an Answer delivered in full continues a lesson that asked for it.
/// Called inside the turn's reply scope with that scope's mouth and cancel token. It never runs
/// after a cancel or a latched barge-in: then the barge utterance becomes the next turn and the
/// lesson stays paused. `latched` is read synchronously by the caller, so no watch reference is
/// held across an await.
async fn finish_answer_with_lesson(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    carry: &mut Carry,
    turn: Result<Option<String>, SessionEnd>,
    latched: bool,
) -> Result<Option<String>, SessionEnd> {
    if turn.is_err() || !carry.answer_delivered || latched || cancel.is_cancelled() {
        return turn;
    }
    let continuation = continue_lesson(deps, cfg, cancel, carry).await?;
    // The continued speech joins the turn's history side, so the next plan sees what was spoken.
    match (turn, continuation) {
        (Ok(Some(answer)), Some(spoken)) => Ok(Some(format!("{answer} {spoken}"))),
        (turn, _) => turn,
    }
}

/// Barge-in capture of the interrupted reply's speech (DESIGN 12.2.3): the endpointer's maximum
/// utterance (30 s) plus a 10 s speech-to-text allowance. Past it the loop listens normally.
const BARGE_CAPTURE_BOUND: Duration = Duration::from_secs(40);

/// History side of a reply cut short by a barge-in: the speech actually submitted plus the
/// existing structured `interrupted` note (the same event the plain interrupted path uses), through
/// the same neutralize and length cap as every other history entry.
fn interrupted_summary(submitted: &str) -> String {
    compose(submitted, &note_interrupted())
}

/// Waits for the barge-in capture of the interrupted reply (DESIGN 12.2.3). `Ok(Some(text))`
/// is the utterance heard (no listen needed); `Ok(None)` means listen normally (no speech, or
/// the bound passed). The session stop always wins. The watch is dropped on every path.
async fn capture_barge(
    cancel: &mut CancelToken,
    mut barge: Box<dyn BargeWatch>,
) -> Result<Option<String>, SessionEnd> {
    let bounded = tokio::time::timeout(BARGE_CAPTURE_BOUND, barge.capture());
    let captured = race(cancel, bounded).await;
    drop(barge);
    match captured {
        None => Err(SessionEnd::Cancelled),
        Some(Ok(BargeCapture::Utterance(text))) => Ok(Some(text)),
        Some(_) => Ok(None),
    }
}

/// Appends a turn to history (failure records are deduplicated on the full utterance) and keeps
/// `last_draft` in step with `context_turns == 0`.
fn record_turn(
    history: &mut Vec<(String, String)>,
    carry: &mut Carry,
    cfg: &SessionConfig,
    utterance: &str,
    summary: &str,
) {
    let repeated = is_repeated_failure(history, carry.last_failed.as_deref(), utterance, summary);
    if !repeated {
        push_history(history, utterance, summary, cfg.context_turns);
    }
    carry.last_failed = if summary == failed_summary() {
        Some(utterance.to_string())
    } else {
        None
    };
    if cfg.context_turns == 0 {
        carry.last_draft = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryState {
    Idle,
    Starting(SessionId),
    Running(SessionId),
    Draining(SessionId),
}

/// Identity of one session, assigned at admission and strictly increasing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub u64);

/// Single-active-session lifecycle: Idle -> Starting -> Running -> Draining -> Idle.
/// Starting lasts from admission until the session reports it is listening; a failed start
/// returns to Idle.
pub struct SessionRegistry {
    #[allow(dead_code)]
    inner: Mutex<RegistryInner>,
}

#[allow(dead_code)]
struct RegistryInner {
    state: RegistryState,
    /// The last id issued by `try_begin` (0 before the first session).
    last_id: SessionId,
    cancel_tx: Option<tokio::sync::watch::Sender<bool>>,
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionRegistry {
    pub fn new() -> Self {
        SessionRegistry {
            inner: Mutex::new(RegistryInner {
                state: RegistryState::Idle,
                last_id: SessionId(0),
                cancel_tx: None,
            }),
        }
    }
    /// Only from Idle: becomes Starting(id) and returns the cancel receiver.
    pub fn try_begin(&self) -> Option<(SessionId, CancelToken)> {
        let mut inner = self.lock();
        if inner.state != RegistryState::Idle {
            return None;
        }
        let id = SessionId(inner.last_id.0.saturating_add(1));
        inner.last_id = id;
        let (tx, rx) = tokio::sync::watch::channel(false);
        inner.cancel_tx = Some(tx);
        inner.state = RegistryState::Starting(id);
        Some((id, rx.into()))
    }
    /// Starting(id) -> Running(id): the session reports it is listening. False otherwise.
    pub fn mark_running(&self, id: SessionId) -> bool {
        let mut inner = self.lock();
        if inner.state == RegistryState::Starting(id) {
            inner.state = RegistryState::Running(id);
            return true;
        }
        false
    }
    /// Legacy stop (programmatic, shutdown, `converse_stop`): resolves the CURRENT id under the
    /// same lock and stops it when Starting or Running. False when Idle or already Draining.
    /// Activation sources do not use this: they go through `orb_activation_decision`, which
    /// ignores a Starting session.
    pub fn request_stop(&self) -> bool {
        let mut inner = self.lock();
        match inner.state {
            RegistryState::Starting(id) | RegistryState::Running(id) => {
                Self::stop_running(&mut inner, id)
            }
            RegistryState::Idle | RegistryState::Draining(_) => false,
        }
    }
    /// Compare-and-stop: Running(id) -> Draining(id) and cancel. False when the current
    /// session is not `id` (a replaced session is never stopped), or it is not Running.
    pub fn request_stop_if(&self, id: SessionId) -> bool {
        let mut inner = self.lock();
        match inner.state {
            RegistryState::Running(current) if current == id => Self::stop_running(&mut inner, id),
            _ => false,
        }
    }
    /// Running(id) -> Draining(id) and send cancel (`send_replace`: `send` drops the value when
    /// no receiver is alive).
    fn stop_running(inner: &mut RegistryInner, id: SessionId) -> bool {
        inner.state = RegistryState::Draining(id);
        if let Some(tx) = inner.cancel_tx.as_ref() {
            tx.send_replace(true);
        }
        true
    }
    /// Frees the registry only when `id` matches the current session (any phase).
    pub fn finish(&self, id: SessionId) -> bool {
        let mut inner = self.lock();
        let matches = matches!(
            inner.state,
            RegistryState::Starting(current)
                | RegistryState::Running(current)
                | RegistryState::Draining(current) if current == id
        );
        if matches {
            inner.state = RegistryState::Idle;
            inner.cancel_tx = None;
        }
        matches
    }
    /// True for Starting, Running and Draining.
    pub fn is_active(&self) -> bool {
        self.state() != RegistryState::Idle
    }
    pub fn state(&self) -> RegistryState {
        self.lock().state
    }
    /// The phase and the session id, read under one lock. When Idle the id is the last issued.
    pub fn snapshot(&self) -> (RegistryState, SessionId) {
        let inner = self.lock();
        (inner.state, inner.last_id)
    }

    /// Never panics on a poisoned mutex: the inner state is still valid.
    fn lock(&self) -> std::sync::MutexGuard<'_, RegistryInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managers::converse_plan::PlanKind;
    use std::collections::VecDeque;
    use std::sync::Arc;
    use tokio::sync::watch;

    // ------------------------------------------------------------------
    // fakes
    // ------------------------------------------------------------------

    #[derive(Default)]
    struct Log(Mutex<Vec<String>>);
    impl Log {
        fn push(&self, s: impl Into<String>) {
            self.0.lock().unwrap().push(s.into());
        }
        fn all(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
        fn count(&self, prefix: &str) -> usize {
            self.all().iter().filter(|e| e.starts_with(prefix)).count()
        }
        fn has(&self, exact: &str) -> bool {
            self.all().iter().any(|e| e == exact)
        }
        fn says(&self) -> Vec<String> {
            self.all()
                .iter()
                .filter_map(|e| e.strip_prefix("say:").map(|s| s.to_string()))
                .collect()
        }
        #[allow(dead_code)] // test helper kept for ordering assertions
        fn index_of(&self, exact: &str) -> Option<usize> {
            self.all().iter().position(|e| e == exact)
        }
    }

    enum L {
        Out(ListenOutcome),
        Hang,
        /// Cancel the session, then return the outcome (both "ready at once").
        CancelThen(ListenOutcome),
    }
    struct FakeEars {
        script: Mutex<VecDeque<L>>,
        log: Arc<Log>,
        cancel: Arc<watch::Sender<bool>>,
    }
    impl Ears for FakeEars {
        fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
            self.log.push("listen");
            let item = self.script.lock().unwrap().pop_front();
            let cancel = self.cancel.clone();
            Box::pin(async move {
                match item {
                    Some(L::Out(o)) => o,
                    Some(L::CancelThen(o)) => {
                        cancel.send_replace(true);
                        o
                    }
                    Some(L::Hang) => std::future::pending().await,
                    None => ListenOutcome::Aborted,
                }
            })
        }
    }

    enum S {
        Out(SayOutcome),
        Hang,
    }
    struct FakeMouth {
        say_script: Mutex<VecDeque<S>>,
        quiet_script: Mutex<VecDeque<QuietOutcome>>,
        log: Arc<Log>,
    }
    impl Mouth for FakeMouth {
        fn say<'a>(&'a self, text: &'a str, _e: Emotion) -> BoxFuture<'a, SayOutcome> {
            self.log.push(format!("say:{text}"));
            let item = self.say_script.lock().unwrap().pop_front();
            Box::pin(async move {
                match item {
                    Some(S::Out(o)) => o,
                    Some(S::Hang) => std::future::pending().await,
                    None => SayOutcome::Done,
                }
            })
        }
        fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
            self.log.push("quiet");
            let o = self
                .quiet_script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(QuietOutcome::Quiet);
            Box::pin(async move { o })
        }
        fn stop(&self) {
            self.log.push("stop");
        }
    }

    struct FakeHands {
        paste_script: Mutex<VecDeque<Result<(), String>>>,
        user_script: Mutex<VecDeque<PhraseOutcome>>,
        model_script: Mutex<VecDeque<PhraseOutcome>>,
        log: Arc<Log>,
    }
    impl Hands for FakeHands {
        fn paste_generated(&self, text: &str) -> Result<(), String> {
            self.log.push(format!("paste:{text}"));
            self.paste_script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(()))
        }
        fn run_user_phrase<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
            self.log.push(format!("user:{phrase}"));
            let o = self
                .user_script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(PhraseOutcome::NoMatch);
            Box::pin(async move { o })
        }
        fn run_model_command<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
            self.log.push(format!("model:{phrase}"));
            let o = self
                .model_script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(PhraseOutcome::NoMatch);
            Box::pin(async move { o })
        }
    }

    enum B {
        Plan(Result<TurnPlan, PlanError>),
        /// Cancel the session, then return the plan (ready at the same instant).
        CancelThen(Result<TurnPlan, PlanError>),
    }
    struct FakeBrain {
        script: Mutex<VecDeque<B>>,
        log: Arc<Log>,
        cancel: Arc<watch::Sender<bool>>,
        histories: Mutex<Vec<Vec<(String, String)>>>,
        drafts: Mutex<Vec<Option<String>>>,
    }
    impl Brain for FakeBrain {
        fn plan<'a>(&'a self, input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
            self.log.push(format!("plan:{}", input.utterance));
            self.histories.lock().unwrap().push(input.history.to_vec());
            self.drafts
                .lock()
                .unwrap()
                .push(input.last_draft.map(|s| s.to_string()));
            let item = self.script.lock().unwrap().pop_front();
            let cancel = self.cancel.clone();
            Box::pin(async move {
                match item {
                    Some(B::Plan(r)) => r,
                    Some(B::CancelThen(r)) => {
                        cancel.send_replace(true);
                        r
                    }
                    None => Err(PlanError::Backend),
                }
            })
        }
    }

    struct FakeReporter(Mutex<Vec<SessionState>>);
    impl Reporter for FakeReporter {
        fn state(&self, s: SessionState) {
            self.0.lock().unwrap().push(s);
        }
    }

    struct Rig {
        log: Arc<Log>,
        cancel: Arc<watch::Sender<bool>>,
        ears: FakeEars,
        mouth: FakeMouth,
        hands: FakeHands,
        brain: FakeBrain,
        reporter: FakeReporter,
    }

    fn rig(listens: Vec<L>, plans: Vec<B>) -> Rig {
        let log = Arc::new(Log::default());
        let (tx, _rx) = watch::channel(false);
        let cancel = Arc::new(tx);
        Rig {
            ears: FakeEars {
                script: Mutex::new(listens.into()),
                log: log.clone(),
                cancel: cancel.clone(),
            },
            mouth: FakeMouth {
                say_script: Mutex::new(VecDeque::new()),
                quiet_script: Mutex::new(VecDeque::new()),
                log: log.clone(),
            },
            hands: FakeHands {
                paste_script: Mutex::new(VecDeque::new()),
                user_script: Mutex::new(VecDeque::new()),
                model_script: Mutex::new(VecDeque::new()),
                log: log.clone(),
            },
            brain: FakeBrain {
                script: Mutex::new(plans.into()),
                log: log.clone(),
                cancel: cancel.clone(),
                histories: Mutex::new(vec![]),
                drafts: Mutex::new(vec![]),
            },
            reporter: FakeReporter(Mutex::new(vec![])),
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
        // Hard test timeout so a regression hangs loudly, not forever.
        tokio::time::timeout(Duration::from_secs(5), run_session(&deps, cfg, token))
            .await
            .expect("run_session hung")
    }

    fn utt(s: &str) -> L {
        L::Out(ListenOutcome::Utterance(s.to_string()))
    }
    fn plan(kind: PlanKind, speech: &str) -> TurnPlan {
        TurnPlan {
            plan: None,
            job: None,
            role: None,
            agent: None,
            kind,
            speech: speech.to_string(),
            emotion: Emotion::Neutral,
            paste_text: None,
            command: None,
        }
    }
    fn answer(speech: &str) -> B {
        B::Plan(Ok(plan(PlanKind::Answer, speech)))
    }
    fn paste_plan(text: &str, speech: &str) -> TurnPlan {
        TurnPlan {
            job: None,
            role: None,
            paste_text: Some(text.to_string()),
            ..plan(PlanKind::Paste, speech)
        }
    }
    fn command_plan(cmd: &str, speech: &str) -> TurnPlan {
        TurnPlan {
            job: None,
            role: None,
            command: Some(cmd.to_string()),
            ..plan(PlanKind::Command, speech)
        }
    }
    fn idle_count(r: &Rig) -> usize {
        r.reporter
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|s| **s == SessionState::Idle)
            .count()
    }

    // ------------------------------------------------------------------
    // listening outcomes
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn no_speech_streak_ends_the_session_with_a_spoken_notice() {
        let r = rig(
            vec![
                L::Out(ListenOutcome::NoSpeech),
                L::Out(ListenOutcome::NoSpeech),
            ],
            vec![],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Idle);
        assert_eq!(r.log.count("listen"), 2);
        assert_eq!(r.log.says(), vec!["I'll stop listening now."]);
        assert_eq!(r.log.count("plan:"), 0);
        assert_eq!(idle_count(&r), 1);
    }

    #[tokio::test]
    async fn speech_resets_the_idle_streak() {
        let r = rig(
            vec![
                L::Out(ListenOutcome::NoSpeech),
                utt("hello"),
                L::Out(ListenOutcome::NoSpeech),
                L::Out(ListenOutcome::NoSpeech),
            ],
            vec![answer("Hi!")],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Idle);
        assert_eq!(r.log.count("listen"), 4);
        assert_eq!(r.log.count("plan:"), 1);
    }

    #[tokio::test]
    async fn whitespace_only_utterances_count_as_no_speech() {
        let r = rig(vec![utt("   "), utt("\n\t")], vec![]);
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Idle);
        assert_eq!(r.log.count("plan:"), 0);
        assert_eq!(r.log.count("user:"), 0);
    }

    #[tokio::test]
    async fn max_idle_turns_is_honoured() {
        let mut c = cfg();
        c.max_idle_turns = 3;
        let r = rig(
            (0..3).map(|_| L::Out(ListenOutcome::NoSpeech)).collect(),
            vec![],
        );
        assert_eq!(run(&r, &c).await, SessionEnd::Idle);
        assert_eq!(r.log.count("listen"), 3);
    }

    #[tokio::test]
    async fn aborted_listen_cancels_the_session() {
        let r = rig(vec![L::Out(ListenOutcome::Aborted)], vec![]);
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
        assert!(r.log.has("stop"));
        assert_eq!(idle_count(&r), 1);
    }

    // ---- loop guard (design rev 2, F3): a capture that cannot find the end of
    // speech must not loop for 25 minutes saying "too long".

    #[tokio::test]
    async fn two_consecutive_too_long_listens_end_the_session_with_cant_hear() {
        let r = rig(
            vec![
                L::Out(ListenOutcome::TooLong),
                L::Out(ListenOutcome::TooLong),
                L::Out(ListenOutcome::Aborted),
            ],
            vec![],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::AudioUnavailable);
        assert_eq!(r.log.says(), vec![MSG_TOO_LONG, MSG_CANT_HEAR]);
        assert_eq!(
            r.log.count("plan:") + r.log.count("user:") + r.log.count("paste:"),
            0
        );
    }

    #[tokio::test]
    async fn a_heard_utterance_between_too_longs_resets_the_counter() {
        let r = rig(
            vec![
                L::Out(ListenOutcome::TooLong),
                utt("hello"),
                L::Out(ListenOutcome::TooLong),
                L::Out(ListenOutcome::Aborted),
            ],
            vec![answer("Hi there.")],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
        let says = r.log.says();
        assert_eq!(says.iter().filter(|s| *s == MSG_TOO_LONG).count(), 2);
        assert!(!says.iter().any(|s| s == MSG_CANT_HEAR));
    }

    #[tokio::test]
    async fn the_over_long_text_path_is_not_counted_as_a_capture_too_long() {
        let long = "a".repeat(1001);
        let r = rig(
            vec![utt(&long), utt(&long), L::Out(ListenOutcome::Aborted)],
            vec![],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
        assert_eq!(r.log.says(), vec![MSG_TOO_LONG, MSG_TOO_LONG]);
    }

    #[tokio::test]
    async fn no_speech_between_too_longs_resets_the_counter() {
        let mut c = cfg();
        c.max_idle_turns = 3;
        let r = rig(
            vec![
                L::Out(ListenOutcome::TooLong),
                L::Out(ListenOutcome::NoSpeech),
                L::Out(ListenOutcome::TooLong),
                L::Out(ListenOutcome::Aborted),
            ],
            vec![],
        );
        assert_eq!(run(&r, &c).await, SessionEnd::Cancelled);
        assert_eq!(r.log.says(), vec![MSG_TOO_LONG, MSG_TOO_LONG]);
    }

    #[tokio::test]
    async fn too_long_then_two_no_speech_listens_ends_idle_normally() {
        let mut c = cfg();
        c.max_idle_turns = 2;
        let r = rig(
            vec![
                L::Out(ListenOutcome::TooLong),
                L::Out(ListenOutcome::NoSpeech),
                L::Out(ListenOutcome::NoSpeech),
            ],
            vec![],
        );
        assert_eq!(run(&r, &c).await, SessionEnd::Idle);
        assert_eq!(r.log.says(), vec![MSG_TOO_LONG, MSG_STOP_LISTENING]);
    }

    #[tokio::test]
    async fn too_long_listen_is_rejected_without_routing() {
        let r = rig(
            vec![
                L::Out(ListenOutcome::TooLong),
                L::Out(ListenOutcome::Aborted),
            ],
            vec![],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
        assert_eq!(
            r.log.says(),
            vec!["That was too long for me. Please say it in parts."]
        );
        assert_eq!(
            r.log.count("plan:") + r.log.count("user:") + r.log.count("paste:"),
            0
        );
    }

    #[tokio::test]
    async fn over_long_utterance_is_rejected_not_truncated() {
        let long = "a".repeat(1001);
        let r = rig(vec![utt(&long), L::Out(ListenOutcome::Aborted)], vec![]);
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
        assert_eq!(
            r.log.says(),
            vec!["That was too long for me. Please say it in parts."]
        );
        assert_eq!(
            r.log.count("plan:") + r.log.count("user:") + r.log.count("paste:"),
            0
        );
        // exactly 1000 chars is accepted
        let ok = "a".repeat(1000);
        let r2 = rig(
            vec![utt(&ok), L::Out(ListenOutcome::Aborted)],
            vec![answer("ok")],
        );
        run(&r2, &cfg()).await;
        assert_eq!(r2.log.count("plan:"), 1);
    }

    #[tokio::test]
    async fn failed_listen_says_a_hint_and_ends_with_audio_unavailable() {
        for f in [
            ListenFailure::Busy,
            ListenFailure::NoMicrophone,
            ListenFailure::Permission,
            ListenFailure::ModelUnavailable,
            ListenFailure::Other,
            ListenFailure::StreamOpen,
        ] {
            // #119: a missing/unloaded speech-recognition model is NOT a microphone problem.
            let expected = if matches!(f, ListenFailure::ModelUnavailable) {
                "I can't use the speech recognition model right now."
            } else {
                "I can't use the microphone right now."
            };
            let r = rig(vec![L::Out(ListenOutcome::Failed(f))], vec![]);
            assert_eq!(run(&r, &cfg()).await, SessionEnd::AudioUnavailable);
            assert_eq!(r.log.says(), vec![expected]);
            assert_eq!(idle_count(&r), 1);
        }
    }

    #[test]
    fn listen_failure_message_names_the_model_only_for_model_unavailable() {
        assert_eq!(
            listen_failure_message(&ListenFailure::ModelUnavailable),
            MSG_STT_MODEL
        );
        for f in [
            ListenFailure::Busy,
            ListenFailure::NoMicrophone,
            ListenFailure::Permission,
            ListenFailure::Other,
            ListenFailure::StreamOpen,
        ] {
            assert_eq!(listen_failure_message(&f), MSG_MIC, "{f:?}");
        }
        assert_ne!(MSG_STT_MODEL, MSG_MIC);
        assert!(!MSG_STT_MODEL.to_lowercase().contains("microphone"));
        assert!(MSG_STT_MODEL.to_lowercase().contains("speech recognition"));
    }

    // ------------------------------------------------------------------
    // local tier
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn end_phrase_says_okay_and_ends() {
        let r = rig(vec![utt("goodbye")], vec![]);
        assert_eq!(run(&r, &cfg()).await, SessionEnd::UserEnded);
        assert_eq!(r.log.says(), vec!["Okay."]);
        assert_eq!(r.log.count("user:") + r.log.count("plan:"), 0);
        assert!(!r.log.has("stop"));
    }

    #[tokio::test]
    async fn dictation_pastes_silently_and_keeps_listening() {
        let r = rig(
            vec![utt("dictate Hello World"), L::Out(ListenOutcome::Aborted)],
            vec![],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
        assert!(r.log.has("paste:Hello World"));
        assert!(r.log.says().is_empty());
        assert_eq!(r.log.count("plan:") + r.log.count("user:"), 0);
        assert_eq!(r.log.count("listen"), 2);
    }

    #[tokio::test]
    async fn dictation_paste_failure_is_reported_without_the_error_text() {
        let r = rig(
            vec![utt("dictate secret thing"), L::Out(ListenOutcome::Aborted)],
            vec![],
        );
        r.hands
            .paste_script
            .lock()
            .unwrap()
            .push_back(Err("terminal refused: secret thing".into()));
        run(&r, &cfg()).await;
        assert_eq!(r.log.says(), vec!["I couldn't paste that."]);
    }

    // ------------------------------------------------------------------
    // exact tier
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn exact_tier_done_speaks_the_confirmation_and_skips_the_brain() {
        let r = rig(
            vec![utt("open Safari"), L::Out(ListenOutcome::Aborted)],
            vec![],
        );
        r.hands
            .user_script
            .lock()
            .unwrap()
            .push_back(PhraseOutcome::Done("Opening Safari.".into()));
        run(&r, &cfg()).await;
        assert_eq!(r.log.says(), vec!["Opening Safari."]);
        assert_eq!(r.log.count("plan:"), 0);
        assert!(r.reporter.0.lock().unwrap().contains(&SessionState::Acting));
    }

    #[tokio::test]
    async fn exact_tier_failure_and_refusal_have_fixed_messages() {
        let r = rig(
            vec![utt("a"), utt("b"), L::Out(ListenOutcome::Aborted)],
            vec![],
        );
        r.hands
            .user_script
            .lock()
            .unwrap()
            .extend([PhraseOutcome::Failed, PhraseOutcome::Refused]);
        run(&r, &cfg()).await;
        assert_eq!(r.log.says(), vec!["That didn't work.", "I can't do that."]);
    }

    #[tokio::test]
    async fn exact_tier_actions_do_not_enter_the_history() {
        let r = rig(
            vec![
                utt("open Safari"),
                utt("tell me a joke"),
                L::Out(ListenOutcome::Aborted),
            ],
            vec![answer("Knock knock.")],
        );
        r.hands
            .user_script
            .lock()
            .unwrap()
            .push_back(PhraseOutcome::Done("Opening Safari.".into()));
        run(&r, &cfg()).await;
        let h = r.brain.histories.lock().unwrap().clone();
        assert_eq!(h.len(), 1);
        assert!(h[0].is_empty());
    }

    // ------------------------------------------------------------------
    // brain tier
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn answer_is_spoken_recorded_and_visible_to_the_next_plan() {
        let r = rig(
            vec![
                utt("what is rust"),
                utt("and go?"),
                L::Out(ListenOutcome::Aborted),
            ],
            vec![answer("A language."), answer("Another language.")],
        );
        run(&r, &cfg()).await;
        assert_eq!(r.log.says(), vec!["A language.", "Another language."]);
        let h = r.brain.histories.lock().unwrap().clone();
        assert!(h[0].is_empty());
        assert_eq!(
            h[1],
            vec![("what is rust".to_string(), "A language.".to_string())]
        );
    }

    #[tokio::test]
    async fn state_order_for_an_answered_turn() {
        let r = rig(
            vec![utt("hi"), L::Out(ListenOutcome::Aborted)],
            vec![answer("Hello.")],
        );
        run(&r, &cfg()).await;
        let states = r.reporter.0.lock().unwrap().clone();
        let pos = |s: SessionState| states.iter().position(|x| *x == s).unwrap();
        assert!(pos(SessionState::Listening) < pos(SessionState::Thinking));
        assert!(pos(SessionState::Thinking) < pos(SessionState::Speaking));
        assert_eq!(*states.last().unwrap(), SessionState::Idle);
    }

    #[tokio::test]
    async fn plan_errors_have_fixed_behaviour() {
        // NotConfigured ends the session with a hint
        let r = rig(
            vec![utt("hi")],
            vec![B::Plan(Err(PlanError::NotConfigured))],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::NotConfigured);
        assert_eq!(
            r.log.says(),
            vec!["Set up a reply backend in the speech settings first."]
        );
        // other errors: apologise, keep going; deliberate spec change (DESIGN Revision 3, critic
        // C2): the failed turn is recorded as a structured `failed` note so a retry such as
        // "no, the other one" still sees the failed request.
        for e in [
            PlanError::Timeout,
            PlanError::Backend,
            PlanError::Unparseable,
            PlanError::Empty,
            PlanError::TooLong,
        ] {
            let r = rig(
                vec![utt("hi"), utt("again"), L::Out(ListenOutcome::Aborted)],
                vec![B::Plan(Err(e)), answer("ok")],
            );
            assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
            assert_eq!(r.log.says()[0], "Sorry, I couldn't work that out.");
            let h = r.brain.histories.lock().unwrap().clone();
            assert_eq!(
                h[1],
                vec![(
                    "hi".to_string(),
                    crate::managers::converse_context::compose(
                        "",
                        &crate::managers::converse_context::note_failed()
                    )
                )],
                "failed turn is recorded as a failure note ({e:?})"
            );
        }
        // Superseded: silent continue
        let r = rig(
            vec![utt("hi"), L::Out(ListenOutcome::Aborted)],
            vec![B::Plan(Err(PlanError::Superseded))],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
        assert!(r.log.says().is_empty());
    }

    #[tokio::test]
    async fn paste_plan_pastes_speaks_and_remembers_the_draft() {
        let r = rig(
            vec![
                utt("write a thank you note"),
                utt("make it shorter"),
                L::Out(ListenOutcome::Aborted),
            ],
            vec![
                B::Plan(Ok(paste_plan("Dear Sam, thank you so much.", "Pasted."))),
                B::Plan(Ok(paste_plan("Thanks Sam.", ""))),
            ],
        );
        run(&r, &cfg()).await;
        assert!(r.log.has("paste:Dear Sam, thank you so much."));
        assert!(r.log.has("paste:Thanks Sam."));
        assert_eq!(r.log.says(), vec!["Pasted.", "Done."]);
        let d = r.brain.drafts.lock().unwrap().clone();
        assert_eq!(d[0], None);
        assert_eq!(d[1].as_deref(), Some("Dear Sam, thank you so much."));
        let h = r.brain.histories.lock().unwrap().clone();
        // deliberate spec change (DESIGN Revision 3, Design A): the history now carries a
        // structured runtime note instead of the vague "(pasted text)" marker.
        assert_eq!(
            h[1][0].1,
            crate::managers::converse_context::compose(
                "Pasted.",
                &crate::managers::converse_context::note_paste("Dear Sam, thank you so much.")
            )
        );
    }

    #[tokio::test]
    async fn paste_plan_failure_speaks_fixed_text_and_keeps_no_draft() {
        let r = rig(
            vec![
                utt("write something"),
                utt("next"),
                L::Out(ListenOutcome::Aborted),
            ],
            vec![
                B::Plan(Ok(paste_plan("secret body", "Done."))),
                answer("ok"),
            ],
        );
        r.hands
            .paste_script
            .lock()
            .unwrap()
            .push_back(Err("refused: secret body".into()));
        run(&r, &cfg()).await;
        assert_eq!(r.log.says()[0], "I couldn't paste that.");
        assert!(!r.log.says().iter().any(|s| s.contains("secret")));
        let d = r.brain.drafts.lock().unwrap().clone();
        assert_eq!(d[1], None);
    }

    #[tokio::test]
    async fn command_plan_uses_the_restricted_tier_only() {
        let r = rig(
            vec![utt("get me safari"), L::Out(ListenOutcome::Aborted)],
            vec![B::Plan(Ok(command_plan("open Safari", "")))],
        );
        r.hands
            .model_script
            .lock()
            .unwrap()
            .push_back(PhraseOutcome::Done("Opening Safari.".into()));
        run(&r, &cfg()).await;
        assert!(r.log.has("model:open Safari"));
        // the exact tier ran once for the raw utterance (NoMatch), never for the model's phrase
        assert!(!r.log.has("user:open Safari"));
        assert_eq!(r.log.says(), vec!["Opening Safari."]);
    }

    #[tokio::test]
    async fn command_plan_speech_wins_over_the_executor_message() {
        let r = rig(
            vec![utt("x"), L::Out(ListenOutcome::Aborted)],
            vec![B::Plan(Ok(command_plan(
                "pause the music",
                "Pausing for you.",
            )))],
        );
        r.hands
            .model_script
            .lock()
            .unwrap()
            .push_back(PhraseOutcome::Done("Paused.".into()));
        run(&r, &cfg()).await;
        assert_eq!(r.log.says(), vec!["Pausing for you."]);
    }

    #[tokio::test]
    async fn command_plan_failure_messages() {
        let r = rig(
            vec![utt("a"), utt("b"), utt("c"), L::Out(ListenOutcome::Aborted)],
            vec![
                B::Plan(Ok(command_plan("open nothing", ""))),
                B::Plan(Ok(command_plan("open /etc/hosts", ""))),
                B::Plan(Ok(command_plan("open broken", ""))),
            ],
        );
        r.hands.model_script.lock().unwrap().extend([
            PhraseOutcome::NoMatch,
            PhraseOutcome::Refused,
            PhraseOutcome::Failed,
        ]);
        run(&r, &cfg()).await;
        assert_eq!(
            r.log.says(),
            vec![
                "I can't do that yet.",
                "I can't do that.",
                "That didn't work."
            ]
        );
    }

    #[tokio::test]
    async fn end_plan_speaks_then_ends() {
        let r = rig(
            vec![utt("we are finished here, thanks")],
            vec![B::Plan(Ok(plan(PlanKind::End, "Talk soon.")))],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::UserEnded);
        assert_eq!(r.log.says(), vec!["Talk soon."]);
    }

    #[tokio::test]
    async fn clarify_is_spoken_and_recorded() {
        let r = rig(
            vec![utt("email him"), utt("Sam"), L::Out(ListenOutcome::Aborted)],
            vec![
                B::Plan(Ok(plan(PlanKind::Clarify, "Which Sam?"))),
                answer("Got it."),
            ],
        );
        run(&r, &cfg()).await;
        let h = r.brain.histories.lock().unwrap().clone();
        assert_eq!(
            h[1],
            vec![("email him".to_string(), "Which Sam?".to_string())]
        );
    }

    // ------------------------------------------------------------------
    // history
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn history_is_capped_to_context_turns_newest_kept() {
        let mut c = cfg();
        c.context_turns = 2;
        let r = rig(
            vec![
                utt("q1"),
                utt("q2"),
                utt("q3"),
                utt("q4"),
                L::Out(ListenOutcome::Aborted),
            ],
            vec![answer("a1"), answer("a2"), answer("a3"), answer("a4")],
        );
        run(&r, &c).await;
        let h = r.brain.histories.lock().unwrap().clone();
        assert_eq!(
            h[3],
            vec![
                ("q2".to_string(), "a2".to_string()),
                ("q3".to_string(), "a3".to_string())
            ]
        );
    }

    #[tokio::test]
    async fn zero_context_turns_keeps_nothing_including_the_draft() {
        let mut c = cfg();
        c.context_turns = 0;
        let r = rig(
            vec![
                utt("write x"),
                utt("shorter"),
                L::Out(ListenOutcome::Aborted),
            ],
            vec![B::Plan(Ok(paste_plan("draft body", "Done."))), answer("ok")],
        );
        run(&r, &c).await;
        assert!(r.brain.histories.lock().unwrap()[1].is_empty());
        assert_eq!(r.brain.drafts.lock().unwrap()[1], None);
    }

    #[tokio::test]
    async fn history_sides_are_truncated_to_600_chars() {
        let q = "q".repeat(600);
        let a = "a".repeat(399);
        let r = rig(
            vec![utt(&q), utt("next"), L::Out(ListenOutcome::Aborted)],
            vec![answer(&a), answer("ok")],
        );
        run(&r, &cfg()).await;
        let h = r.brain.histories.lock().unwrap().clone();
        assert!(h[1][0].0.chars().count() <= 600);
        assert!(h[1][0].1.chars().count() <= 600);
    }

    // ------------------------------------------------------------------
    // speaking
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn every_say_is_followed_by_wait_quiet_before_the_next_listen() {
        let r = rig(
            vec![utt("hi"), L::Out(ListenOutcome::Aborted)],
            vec![answer("Hello.")],
        );
        run(&r, &cfg()).await;
        let log = r.log.all();
        let say = log.iter().position(|e| e == "say:Hello.").unwrap();
        let quiet = log.iter().position(|e| e == "quiet").unwrap();
        let second_listen = log.iter().rposition(|e| e == "listen").unwrap();
        assert!(say < quiet && quiet < second_listen);
    }

    #[tokio::test]
    async fn interrupted_and_failed_say_do_not_end_the_session() {
        for o in [SayOutcome::Interrupted, SayOutcome::Failed] {
            let r = rig(
                vec![utt("hi"), L::Out(ListenOutcome::Aborted)],
                vec![answer("Hello.")],
            );
            r.mouth.say_script.lock().unwrap().push_back(S::Out(o));
            assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
            assert_eq!(r.log.count("listen"), 2, "must keep listening after {o:?}");
            assert!(r.log.has("quiet"));
        }
    }

    #[tokio::test]
    async fn still_speaking_triggers_stop_then_one_more_wait() {
        let r = rig(
            vec![utt("hi"), L::Out(ListenOutcome::Aborted)],
            vec![answer("Hello.")],
        );
        r.mouth
            .quiet_script
            .lock()
            .unwrap()
            .extend([QuietOutcome::StillSpeaking, QuietOutcome::Quiet]);
        run(&r, &cfg()).await;
        let log = r.log.all();
        let say = log.iter().position(|e| e == "say:Hello.").unwrap();
        let stop = log.iter().position(|e| e == "stop").unwrap();
        assert!(say < stop);
        assert_eq!(
            r.log.count("quiet"),
            2 + 0,
            "one wait before stop, one after"
        );
        assert_eq!(r.log.count("listen"), 2);
    }

    #[tokio::test]
    async fn persistent_speech_ends_with_speech_stuck_and_never_reopens_the_mic() {
        let r = rig(
            vec![utt("hi"), utt("never reached")],
            vec![answer("Hello.")],
        );
        r.mouth
            .quiet_script
            .lock()
            .unwrap()
            .extend([QuietOutcome::StillSpeaking, QuietOutcome::StillSpeaking]);
        assert_eq!(run(&r, &cfg()).await, SessionEnd::SpeechStuck);
        assert_eq!(r.log.count("listen"), 1);
        assert_eq!(idle_count(&r), 1);
    }

    // ------------------------------------------------------------------
    // bounds, exactly-once Idle
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn max_turns_ends_with_turn_limit() {
        let mut c = cfg();
        c.max_turns = 3;
        let r = rig(
            vec![utt("a"), utt("b"), utt("c"), utt("d"), utt("e")],
            vec![
                answer("1"),
                answer("2"),
                answer("3"),
                answer("4"),
                answer("5"),
            ],
        );
        assert_eq!(run(&r, &c).await, SessionEnd::TurnLimit);
        assert_eq!(r.log.count("listen"), 3);
        assert_eq!(idle_count(&r), 1);
    }

    #[tokio::test]
    async fn idle_is_reported_exactly_once_for_every_end() {
        // UserEnded
        let r = rig(vec![utt("stop")], vec![]);
        run(&r, &cfg()).await;
        assert_eq!(idle_count(&r), 1);
        assert_eq!(
            *r.reporter.0.lock().unwrap().last().unwrap(),
            SessionState::Idle
        );
        // NotConfigured
        let r = rig(vec![utt("x")], vec![B::Plan(Err(PlanError::NotConfigured))]);
        run(&r, &cfg()).await;
        assert_eq!(idle_count(&r), 1);
        // Cancelled
        let r = rig(vec![L::Out(ListenOutcome::Aborted)], vec![]);
        run(&r, &cfg()).await;
        assert_eq!(idle_count(&r), 1);
    }

    #[tokio::test]
    async fn mouth_stop_is_only_called_on_cancel_in_simple_ends() {
        let r = rig(vec![utt("goodbye")], vec![]);
        run(&r, &cfg()).await;
        assert!(!r.log.has("stop"));
    }

    // ------------------------------------------------------------------
    // cancellation
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn cancel_while_listening_ends_cancelled() {
        let r = rig(vec![L::Hang], vec![]);
        let cancel = r.cancel.clone();
        let canceller = async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            cancel.send_replace(true);
        };
        let c = cfg();
        let (end, _) = tokio::join!(run(&r, &c), canceller);
        assert_eq!(end, SessionEnd::Cancelled);
        assert!(r.log.has("stop"));
        assert_eq!(idle_count(&r), 1);
    }

    #[tokio::test]
    async fn cancel_ready_at_the_same_instant_as_an_utterance_wins() {
        let r = rig(
            vec![L::CancelThen(ListenOutcome::Utterance("hello".into()))],
            vec![answer("never")],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
        assert_eq!(
            r.log.count("plan:") + r.log.count("user:") + r.log.count("paste:"),
            0
        );
        assert!(r.log.says().is_empty());
    }

    #[tokio::test]
    async fn cancel_ready_with_a_dictation_never_pastes() {
        let r = rig(
            vec![L::CancelThen(ListenOutcome::Utterance(
                "dictate hello there".into(),
            ))],
            vec![],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
        assert_eq!(r.log.count("paste:"), 0);
    }

    #[tokio::test]
    async fn cancel_ready_with_the_plan_drops_the_plan() {
        for plan_b in [
            B::CancelThen(Ok(paste_plan("body", "Done."))),
            B::CancelThen(Ok(command_plan("open Safari", ""))),
            B::CancelThen(Ok(plan(PlanKind::Answer, "never spoken"))),
        ] {
            let r = rig(vec![utt("do something")], vec![plan_b]);
            assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
            assert_eq!(r.log.count("paste:") + r.log.count("model:"), 0);
            assert!(r.log.says().is_empty());
            assert_eq!(idle_count(&r), 1);
        }
    }

    #[tokio::test]
    async fn cancel_during_say_stops_the_mouth_and_ends() {
        let r = rig(vec![utt("hi")], vec![answer("A very long answer.")]);
        r.mouth.say_script.lock().unwrap().push_back(S::Hang);
        let cancel = r.cancel.clone();
        let canceller = async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            cancel.send_replace(true);
        };
        let c = cfg();
        let (end, _) = tokio::join!(run(&r, &c), canceller);
        assert_eq!(end, SessionEnd::Cancelled);
        assert!(r.log.has("stop"));
        assert_eq!(r.log.count("listen"), 1);
    }

    #[tokio::test]
    async fn cancel_before_start_never_listens_or_acts() {
        let r = rig(vec![utt("hi")], vec![answer("x")]);
        r.cancel.send_replace(true);
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
        assert_eq!(r.log.count("listen"), 0);
        assert_eq!(idle_count(&r), 1);
    }

    #[tokio::test]
    async fn cancel_during_terminal_farewell_speech_is_prompt() {
        let r = rig(vec![utt("goodbye")], vec![]);
        r.mouth.say_script.lock().unwrap().push_back(S::Hang);
        let cancel = r.cancel.clone();
        let canceller = async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            cancel.send_replace(true);
        };
        let c = cfg();
        let (end, _) = tokio::join!(run(&r, &c), canceller);
        assert_eq!(end, SessionEnd::Cancelled);
    }

    // ------------------------------------------------------------------
    // registry
    // ------------------------------------------------------------------

    #[test]
    fn registry_begin_stop_finish_lifecycle() {
        let reg = SessionRegistry::new();
        assert!(!reg.is_active());
        assert!(!reg.request_stop(), "stop on Idle is a no-op");
        let (id, token) = reg.try_begin().expect("begin from idle");
        // A-C1b: admission is Starting; capture-ready (mark_running) makes it Running.
        assert_eq!(reg.state(), RegistryState::Starting(id));
        assert!(reg.mark_running(id));
        assert_eq!(reg.state(), RegistryState::Running(id));
        assert!(reg.is_active());
        assert!(reg.try_begin().is_none(), "double begin refused");
        assert!(!*token.session_ref());
        assert!(reg.request_stop());
        assert!(*token.session_ref(), "stop must cancel the token");
        assert_eq!(reg.state(), RegistryState::Draining(id));
        assert!(!reg.request_stop(), "second stop is idempotent no-op");
        assert!(reg.is_active(), "draining still counts as active");
        assert!(reg.try_begin().is_none(), "draining blocks a new session");
        assert!(reg.finish(id));
        assert_eq!(reg.state(), RegistryState::Idle);
        let (id2, _t) = reg.try_begin().expect("begin after finish");
        assert!(id2 > id);
    }

    #[test]
    fn registry_stale_finish_cannot_free_a_newer_session() {
        let reg = SessionRegistry::new();
        let (old, _t) = reg.try_begin().unwrap();
        assert!(reg.mark_running(old));
        reg.request_stop();
        assert!(reg.finish(old));
        let (new, _t2) = reg.try_begin().unwrap();
        assert!(!reg.finish(old), "stale id must not free the new session");
        assert_eq!(reg.state(), RegistryState::Starting(new));
        assert!(!reg.finish(SessionId(new.0 + 100)));
        assert!(reg.finish(new), "finish of a Starting session frees it too");
    }

    #[test]
    fn registry_concurrent_begin_yields_exactly_one_winner() {
        let reg = Arc::new(SessionRegistry::new());
        let wins = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let reg = reg.clone();
                let wins = wins.clone();
                std::thread::spawn(move || {
                    if reg.try_begin().is_some() {
                        wins.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(wins.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn registry_concurrent_stop_and_finish_never_wedge() {
        for _ in 0..50 {
            let reg = Arc::new(SessionRegistry::new());
            let (id, _t) = reg.try_begin().unwrap();
            let a = {
                let reg = reg.clone();
                std::thread::spawn(move || reg.request_stop())
            };
            let b = {
                let reg = reg.clone();
                std::thread::spawn(move || reg.finish(id))
            };
            a.join().unwrap();
            b.join().unwrap();
            // Whatever the interleaving, finishing once more must leave Idle.
            reg.finish(id);
            assert_eq!(reg.state(), RegistryState::Idle);
            assert!(reg.try_begin().is_some());
        }
    }

    // ------------------------------------------------------------------
    // config from settings (S4)
    // ------------------------------------------------------------------

    #[test]
    fn session_config_answer_only_follows_vault_context_and_path() {
        // RED (B3a, DESIGN Rev 3, 3.6): from_settings must set answer_only = answer_only_for(s).
        // Production still hard-codes false until B3b wires it.
        let mut s = crate::settings::get_default_settings();
        s.vault_context_enabled = true;
        s.vault_path = "/tmp/vox-synthetic-vault".to_string();
        assert!(SessionConfig::from_settings(&s).answer_only);

        s.vault_path = "   ".to_string();
        assert!(!SessionConfig::from_settings(&s).answer_only);

        s.vault_path = "/tmp/vox-synthetic-vault".to_string();
        s.vault_context_enabled = false;
        assert!(!SessionConfig::from_settings(&s).answer_only);
    }

    #[test]
    fn session_config_reads_and_clamps_the_settings() {
        let mut s = crate::settings::get_default_settings();
        let c = SessionConfig::from_settings(&s);
        assert_eq!((c.max_idle_turns, c.context_turns, c.max_turns), (2, 6, 50));
        assert_eq!(c.post_speech_guard, Duration::from_millis(350));
        assert_eq!(c.quiet_wait, Duration::from_secs(2));
        assert_eq!(c.style, s.tts_composer_style);
        s.converse_max_idle_turns = 0;
        s.converse_context_turns = 200;
        let c = SessionConfig::from_settings(&s);
        assert_eq!((c.max_idle_turns, c.context_turns), (1, 12));
        s.converse_max_idle_turns = 99;
        s.converse_context_turns = 0;
        let c = SessionConfig::from_settings(&s);
        assert_eq!((c.max_idle_turns, c.context_turns), (5, 0));
    }

    #[test]
    fn endpoint_config_clamps_silence_to_600_4000_ms() {
        let mut s = crate::settings::get_default_settings();
        assert_eq!(endpoint_config(&s).silence_ms, 1200);
        s.converse_silence_ms = 0;
        assert_eq!(endpoint_config(&s).silence_ms, 600);
        s.converse_silence_ms = 100_000;
        assert_eq!(endpoint_config(&s).silence_ms, 4000);
        s.converse_silence_ms = 2500;
        let e = endpoint_config(&s);
        assert_eq!(e.silence_ms, 2500);
        assert_eq!(
            (e.initial_grace_ms, e.max_utterance_ms, e.min_speech_ms),
            (4000, 30_000, 150)
        );
    }
}

#[cfg(test)]
#[path = "converse_stream_loop_tests.rs"]
mod stream_loop_tests;

#[cfg(test)]
#[path = "converse_context_loop_tests.rs"]
mod context_loop_tests;

#[cfg(test)]
#[path = "converse_ack_loop_tests.rs"]
mod ack_loop_tests;

#[cfg(test)]
#[path = "converse_transcript_tests.rs"]
mod transcript_tests;

#[cfg(test)]
#[path = "converse_chat_tests.rs"]
mod chat_tests;

#[cfg(test)]
#[path = "converse_answer_only_tests.rs"]
mod answer_only_tests;

#[cfg(test)]
#[path = "converse_gate_tests.rs"]
mod converse_gate_tests;

#[cfg(test)]
#[path = "converse_gate_answer_only_tests.rs"]
mod converse_gate_answer_only_tests;

#[cfg(test)]
#[path = "converse_gate_m4_tests.rs"]
mod converse_gate_m4_tests;

#[cfg(test)]
#[path = "converse_timing_m4_tests.rs"]
mod converse_timing_m4_tests;

#[cfg(test)]
#[path = "converse_stt_failure_tests.rs"]
mod converse_stt_failure_tests;

#[cfg(test)]
#[path = "converse_loop_voice_tests.rs"]
mod loop_voice_tests;

#[cfg(test)]
#[path = "converse_role_loop_tests.rs"]
mod role_loop_tests;

#[cfg(test)]
#[path = "converse_role_fix_tests.rs"]
mod role_fix_tests;

#[cfg(test)]
#[path = "converse_role_barge_tests.rs"]
mod role_barge_tests;

#[cfg(test)]
#[path = "converse_agent_loop_tests.rs"]
mod converse_agent_loop_tests;

#[path = "converse_agent_loop.rs"]
mod converse_agent_loop;

#[cfg(test)]
#[path = "converse_agent_taint_tests.rs"]
mod converse_agent_taint_tests;

#[cfg(test)]
#[path = "converse_agent_fix_tests.rs"]
mod converse_agent_fix_tests;
