//! The conversation session loop: listen -> route -> act/answer -> speak ->
//! listen. Pure and trait-generic (no tauri types); all hardware sits behind
//! the traits below. Contract: DESIGN.md section 3.3 (Revision 4).

use crate::managers::converse_ack::choose;
use crate::managers::converse_confirm::{
    confirm_action, confirmation_question, decline_message, fingerprint_of, prepare_action,
    ConfirmConfig, Decision, DeclineReason, DirFd, JobLaunch, JobStartOutcome, PinError,
    PolicySource, PrepareError, PreparedAction, ValidatedCommand,
};
use crate::managers::converse_context::{
    compose, neutralize, note_command, note_delivery_failed, note_failed, note_incomplete,
    note_interrupted, note_paste, note_paste_failed, CommandNote,
};
use crate::managers::converse_local::{classify_local_with, LocalIntent};
use crate::managers::converse_plan::{
    PlanError, PlanInput, PlanKind, TurnPlan, MAX_UTTERANCE_CHARS,
};
use crate::managers::converse_stream::{reconcile, PlanStreamer, Reconcile};
use crate::managers::emotion::Emotion;
use crate::managers::job_announcer::{Announcer, LowPriorityMouth};
use futures_util::future::{select, Either};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
/// `true` means the session was cancelled.
pub type CancelToken = tokio::sync::watch::Receiver<bool>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenFailure {
    Busy,
    NoMicrophone,
    Permission,
    ModelUnavailable,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenOutcome {
    Utterance(String),
    NoSpeech,
    TooLong,
    Aborted,
    Failed(ListenFailure),
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
        PlanKind::Answer | PlanKind::Paste | PlanKind::Command | PlanKind::Job => {
            TurnOutcome::Answered
        }
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
}

impl Mouth for JournalMouth<'_> {
    fn say<'a>(&'a self, text: &'a str, emotion: Emotion) -> BoxFuture<'a, SayOutcome> {
        self.journal.submit(text);
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

    /// M4: receives one timing line (observability only). Default: written to the log at info.
    #[allow(dead_code)] // wired in a later M4 step
    fn timing_line(&self, line: &str) {
        log::info!("{line}");
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
        }
    }
}

impl SessionConfig {
    /// Session settings from the user's AppSettings (clamped; DESIGN 3.3).
    pub fn from_settings(s: &crate::settings::AppSettings) -> SessionConfig {
        let rt = crate::managers::realtime_config::RealtimeConfig::from_settings(s);
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
/// every exit path and calls `mouth.stop()` when the end is `Cancelled`.
pub async fn run_session(deps: &Deps<'_>, cfg: &SessionConfig, cancel: CancelToken) -> SessionEnd {
    let _conversation = ConversationGuard::enter(deps.announcer);
    let mut cancel = cancel;
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
const MSG_ANSWER_ONLY: &str =
    "Vault context is on, so I can only answer questions. Turn it off to dictate text or open things.";
/// Queued once when the brain is slow (`SessionConfig::ack_after`).
const MSG_ACK: &str = "One moment.";

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
    *cancel.borrow()
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
            return execute_plan(deps, cfg, cancel, plan, carry).await;
        }
        Reconcile::Remainder { text } => text,
    };
    if !text.is_empty() {
        mark_speaking(early, deps.reporter);
        if !queue.push(&text, plan.emotion) {
            queue.abort();
            return Ok(None);
        }
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

/// One latency line per streamed turn. Never logs any text.
fn log_timing(cfg: &SessionConfig, tier_start: Instant, early: &Early, plan_at: Instant) {
    if !cfg.timing_log {
        return;
    }
    log::info!(
        "converse timing: first_text_ms={} first_speech_ms={} plan_ms={} total_ms={}",
        ms_since(tier_start, early.first_text),
        ms_since(tier_start, early.first_speech),
        ms_since(tier_start, Some(plan_at)),
        ms_since(tier_start, Some(Instant::now())),
    );
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
    let start = history.len().saturating_sub(cfg.context_turns);
    let input = PlanInput {
        utterance,
        history: history.get(start..).unwrap_or_default(),
        last_draft: carry.last_draft.as_deref(),
        style: &cfg.style,
        vault_context: None,
    };
    let queue = if cfg.answer_only {
        // Answer-only (DESIGN 8.1): no streamed model text may reach the mouth before a complete
        // plan is parsed, so the streaming path is never used.
        None
    } else if cfg.early_speech || cfg.ack_after.is_some() {
        deps.mouth.open_queue()
    } else {
        None
    };
    let Some(queue) = queue else {
        return match race(cancel, deps.brain.plan(input)).await {
            None => Err(SessionEnd::Cancelled),
            Some(Ok(plan)) => gate_and_run(deps, cfg, cancel, plan, carry).await,
            Some(Err(err)) => plan_failed(deps, cfg, cancel, err, &[], &carry.journal).await,
        };
    };
    let ack = ack_for(cfg, utterance, carry.last_draft.is_some(), carry.last_ack);
    let early = Mutex::new(Early {
        journal: carry.journal.clone(),
        ..Early::default()
    });
    let stream_cancel = cancel.clone();
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
    log_timing(cfg, tier_start, &early, plan_at);
    turn
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
            PlanKind::Paste | PlanKind::Command | PlanKind::Job
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
        PlanKind::Answer | PlanKind::Clarify | PlanKind::Job => {
            carry.journal.set(speech_outcome(&plan.kind), None);
            let outcome = speak_outcome(deps, cfg, cancel, &plan.speech, plan.emotion).await?;
            answer_summary(&plan.speech, outcome)
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
            PlanKind::Paste | PlanKind::Command | PlanKind::Job
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
    let approved_at =
        matches!(prepared, PreparedAction::Job { .. }).then(|| fingerprint_of(policy));
    if state.gate_active {
        let question = confirmation_question(&prepared);
        let decision = ask_confirmation(deps, announcer, cancel, carry, &question).await;
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
    if approved_at.is_some_and(|approved| fingerprint_of(policy) != approved) {
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
    announcer: &Announcer,
    cancel: &mut CancelToken,
    carry: &Carry,
    question: &str,
) -> Decision {
    let before = carry.journal.submitted_len();
    let cfg = ConfirmConfig::default();
    let asked = confirm_action(deps.ears, deps.mouth, announcer, question, &cfg);
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
    for text in announcer.take_pending() {
        if let Some(end) = speak_or_end(deps, cfg, cancel, &text, Emotion::Neutral).await {
            return Some(end);
        }
    }
    None
}

/// The turn loop. Returns the end; the wrapper reports Idle and stops.
async fn run_turns(outer: &Deps<'_>, cfg: &SessionConfig, cancel: &mut CancelToken) -> SessionEnd {
    let mut history: Vec<(String, String)> = Vec::new();
    let mut carry = Carry::default();
    let mut idle_turns: u32 = 0;
    let mut consecutive_too_long: u32 = 0;
    let mut turns: usize = 0;
    // Every speech line handed to the mouth is journaled for the transcript (DESIGN 8.2).
    let tap = JournalMouth {
        inner: outer.mouth,
        journal: carry.journal.clone(),
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
        if turns >= cfg.max_turns {
            return SessionEnd::TurnLimit;
        }
        turns += 1;
        if let Some(end) = speak_pending(deps, cfg, cancel, outer.announcer).await {
            return end;
        }

        deps.reporter.state(SessionState::Listening);
        let heard = match race(cancel, deps.ears.listen()).await {
            None => return SessionEnd::Cancelled,
            Some(outcome) => outcome,
        };
        let utterance = match heard {
            ListenOutcome::Aborted => return SessionEnd::Cancelled,
            ListenOutcome::Failed(_) => {
                return after_speech(
                    speak(deps, cfg, cancel, MSG_MIC, Emotion::Neutral).await,
                    SessionEnd::AudioUnavailable,
                );
            }
            ListenOutcome::TooLong => {
                idle_turns = 0;
                consecutive_too_long += 1;
                if consecutive_too_long >= 2 {
                    return after_speech(
                        speak(deps, cfg, cancel, MSG_CANT_HEAR, Emotion::Neutral).await,
                        SessionEnd::AudioUnavailable,
                    );
                }
                if let Some(end) =
                    speak_or_end(deps, cfg, cancel, MSG_TOO_LONG, Emotion::Neutral).await
                {
                    return end;
                }
                continue;
            }
            ListenOutcome::NoSpeech => String::new(),
            ListenOutcome::Utterance(text) => text,
        };
        // Any capture that did not report TooLong breaks the streak.
        consecutive_too_long = 0;

        if utterance.trim().is_empty() {
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

        // Accepted for dispatch: exactly one transcript event follows (DESIGN 8.2).
        let log = UtteranceLog::new(deps.reporter, &utterance, &carry.journal);
        match dispatch_utterance(deps, cfg, cancel, &utterance, &history, &mut carry).await {
            Err(end) => {
                log.finish(end == SessionEnd::Cancelled);
                return end;
            }
            Ok(None) => log.finish(false),
            Ok(Some(summary)) => {
                log.finish(false);
                record_turn(&mut history, &mut carry, cfg, &utterance, &summary);
            }
        }
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
    // Local tier: end phrases and explicit dictation.
    let cancel_enabled =
        deps.policy.is_some_and(|p| p.current().jobs_allowed) || deps.hands.has_active_job();
    match classify_local_with(utterance, cancel_enabled) {
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
    Running(u64),
    Draining(u64),
}

/// Single-active-session lifecycle: Idle -> Running -> Draining -> Idle.
pub struct SessionRegistry {
    #[allow(dead_code)]
    inner: Mutex<RegistryInner>,
}

#[allow(dead_code)]
struct RegistryInner {
    state: RegistryState,
    next_id: u64,
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
                next_id: 1,
                cancel_tx: None,
            }),
        }
    }
    /// Only from Idle: becomes Running(id) and returns the cancel receiver.
    pub fn try_begin(&self) -> Option<(u64, CancelToken)> {
        let mut inner = self.lock();
        if inner.state != RegistryState::Idle {
            return None;
        }
        let id = inner.next_id;
        inner.next_id = inner.next_id.saturating_add(1);
        let (tx, rx) = tokio::sync::watch::channel(false);
        inner.cancel_tx = Some(tx);
        inner.state = RegistryState::Running(id);
        Some((id, rx))
    }
    /// Running -> Draining and send cancel (use `send_replace`: `send` drops the
    /// value when no receiver is alive). False when Idle or already Draining.
    pub fn request_stop(&self) -> bool {
        let mut inner = self.lock();
        match inner.state {
            RegistryState::Running(id) => {
                inner.state = RegistryState::Draining(id);
                if let Some(tx) = inner.cancel_tx.as_ref() {
                    tx.send_replace(true);
                }
                true
            }
            RegistryState::Idle | RegistryState::Draining(_) => false,
        }
    }
    /// Frees the registry only when `id` matches the current session.
    pub fn finish(&self, id: u64) -> bool {
        let mut inner = self.lock();
        let matches = matches!(
            inner.state,
            RegistryState::Running(current) | RegistryState::Draining(current) if current == id
        );
        if matches {
            inner.state = RegistryState::Idle;
            inner.cancel_tx = None;
        }
        matches
    }
    /// True for Running and Draining.
    pub fn is_active(&self) -> bool {
        self.state() != RegistryState::Idle
    }
    pub fn state(&self) -> RegistryState {
        self.lock().state
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
            job: None,
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
            paste_text: Some(text.to_string()),
            ..plan(PlanKind::Paste, speech)
        }
    }
    fn command_plan(cmd: &str, speech: &str) -> TurnPlan {
        TurnPlan {
            job: None,
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
        ] {
            let r = rig(vec![L::Out(ListenOutcome::Failed(f))], vec![]);
            assert_eq!(run(&r, &cfg()).await, SessionEnd::AudioUnavailable);
            assert_eq!(r.log.says(), vec!["I can't use the microphone right now."]);
            assert_eq!(idle_count(&r), 1);
        }
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
        assert_eq!(reg.state(), RegistryState::Running(id));
        assert!(reg.is_active());
        assert!(reg.try_begin().is_none(), "double begin refused");
        assert!(!*token.borrow());
        assert!(reg.request_stop());
        assert!(*token.borrow(), "stop must cancel the token");
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
        reg.request_stop();
        assert!(reg.finish(old));
        let (new, _t2) = reg.try_begin().unwrap();
        assert!(!reg.finish(old), "stale id must not free the new session");
        assert_eq!(reg.state(), RegistryState::Running(new));
        assert!(!reg.finish(new + 100));
        assert!(reg.finish(new), "finish of a Running session frees it too");
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
#[path = "converse_answer_only_tests.rs"]
mod answer_only_tests;

#[cfg(test)]
#[path = "converse_gate_tests.rs"]
mod converse_gate_tests;

#[cfg(test)]
#[path = "converse_gate_answer_only_tests.rs"]
mod converse_gate_answer_only_tests;
