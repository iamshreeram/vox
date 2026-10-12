//! The conversation session loop: listen -> route -> act/answer -> speak ->
//! listen. Pure and trait-generic (no tauri types); all hardware sits behind
//! the traits below. Contract: DESIGN.md section 3.3 (Revision 4).

use crate::managers::converse_local::{classify_local, LocalIntent};
use crate::managers::converse_plan::{
    PlanError, PlanInput, PlanKind, TurnPlan, MAX_UTTERANCE_CHARS,
};
use crate::managers::converse_stream::{reconcile, PlanStreamer, Reconcile};
use crate::managers::emotion::Emotion;
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

pub trait Reporter: Send + Sync {
    fn state(&self, state: SessionState);
}

pub struct Deps<'a> {
    pub ears: &'a dyn Ears,
    pub mouth: &'a dyn Mouth,
    pub hands: &'a dyn Hands,
    pub brain: &'a dyn Brain,
    pub reporter: &'a dyn Reporter,
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
    let mut cancel = cancel;
    let end = run_turns(deps, cfg, &mut cancel).await;
    if end == SessionEnd::Cancelled {
        deps.mouth.stop();
    }
    deps.reporter.state(SessionState::Idle);
    end
}

// Fixed spoken messages. The loop never speaks text taken from an error.
const MSG_TOO_LONG: &str = "That was too long for me. Please say it in parts.";
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
/// Queued once when the brain is slow (`SessionConfig::ack_after`).
const MSG_ACK: &str = "One moment.";

const HISTORY_SIDE_MAX_CHARS: usize = 600;
const STILL_SPEAKING_RETRY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpeakResult {
    /// Speech finished (Done, Interrupted or Failed) and the settle steps ran.
    Spoken,
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
        SpeakResult::Spoken => None,
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
        return SpeakResult::Spoken;
    }
    deps.reporter.state(SessionState::Speaking);
    if is_cancelled(cancel) {
        return SpeakResult::Cancelled;
    }
    if race(cancel, deps.mouth.say(text, emotion)).await.is_none() {
        return SpeakResult::Cancelled;
    }
    settle(deps, cfg, cancel).await
}

/// After any say: wait until quiet. If still speaking, stop once and wait
/// once more; still speaking => Stuck. On quiet, sleep the post-speech guard.
async fn settle(deps: &Deps<'_>, cfg: &SessionConfig, cancel: &mut CancelToken) -> SpeakResult {
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
    SpeakResult::Spoken
}

fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn push_history(history: &mut Vec<(String, String)>, utterance: &str, summary: &str, keep: usize) {
    history.push((
        truncate_chars(utterance, HISTORY_SIDE_MAX_CHARS),
        truncate_chars(summary, HISTORY_SIDE_MAX_CHARS),
    ));
    let excess = history.len().saturating_sub(keep);
    history.drain(..excess);
}

/// Converts an optional end into a `Result` so `?` can propagate it.
fn into_result(end: Option<SessionEnd>) -> Result<(), SessionEnd> {
    match end {
        None => Ok(()),
        Some(end) => Err(end),
    }
}

/// Restricted tier for a model-proposed command. `Ok(Some(summary))` when the
/// command executed (the summary goes into history); `Ok(None)` when a fixed
/// message was spoken instead; `Err(end)` when the session must end.
async fn run_command_plan(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    plan: &TurnPlan,
) -> Result<Option<String>, SessionEnd> {
    let Some(command) = plan.command.as_deref() else {
        into_result(speak_or_end(deps, cfg, cancel, MSG_NO_MATCH_COMMAND, Emotion::Neutral).await)?;
        return Ok(None);
    };
    if is_cancelled(cancel) {
        return Err(SessionEnd::Cancelled);
    }
    let fixed = match deps.hands.run_model_command(command).await {
        PhraseOutcome::Done(message) => {
            let spoken = if plan.speech.is_empty() {
                message
            } else {
                plan.speech.clone()
            };
            into_result(speak_or_end(deps, cfg, cancel, &spoken, plan.emotion).await)?;
            return Ok(Some(plan.speech.clone()));
        }
        PhraseOutcome::NoMatch => MSG_NO_MATCH_COMMAND,
        PhraseOutcome::Refused => MSG_REFUSED,
        PhraseOutcome::Failed => MSG_FAILED,
    };
    into_result(speak_or_end(deps, cfg, cancel, fixed, Emotion::Neutral).await)?;
    Ok(None)
}

/// Shared state of one streamed brain turn (see `stream_plan`).
#[derive(Default)]
struct Early {
    streamer: PlanStreamer,
    /// Sentences already queued: canonical, in order, never the acknowledgement.
    spoken: Vec<String>,
    /// The queue refused a push: the user or the system took the speaker. Stay silent.
    dead: bool,
    ack_pushed: bool,
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
    delta: &str,
) {
    let mut early = lock_early(early);
    early.first_text.get_or_insert_with(Instant::now);
    if !cfg.early_speech || early.dead {
        return;
    }
    for sentence in early.streamer.push(delta) {
        if queue.push(&sentence.text, sentence.emotion) {
            early.spoken.push(sentence.text);
            mark_speaking(&mut early, reporter);
        } else {
            early.dead = true;
            break;
        }
    }
}

/// The acknowledgement: queued once, only while nothing has been spoken yet.
fn push_ack(queue: &dyn SpeechQueue, early: &Mutex<Early>, reporter: &dyn Reporter) {
    let mut early = lock_early(early);
    if early.dead || !early.spoken.is_empty() || early.ack_pushed {
        return;
    }
    if queue.push(MSG_ACK, Emotion::Neutral) {
        early.ack_pushed = true;
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
    input: PlanInput<'_>,
) -> Result<TurnPlan, PlanError> {
    let mut on_text = |delta: &str| on_delta(early, cfg, queue, deps.reporter, delta);
    let mut plan = deps.brain.plan_streaming(input, &mut on_text);
    if let Some(after) = cfg.ack_after {
        let timer = std::pin::pin!(tokio::time::sleep(after));
        match select(&mut plan, timer).await {
            Either::Left((result, _timer)) => return result,
            Either::Right((_elapsed, _plan)) => push_ack(queue, early, deps.reporter),
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
    last_draft: &mut Option<String>,
) -> Result<Option<String>, SessionEnd> {
    if early.dead {
        queue.abort();
        return Ok(None);
    }
    let text = match reconcile(&plan, &early.spoken) {
        Reconcile::Abort => {
            queue.abort();
            into_result(speak_or_end(deps, cfg, cancel, MSG_PLAN_FAILED, Emotion::Neutral).await)?;
            return Ok(None);
        }
        Reconcile::Legacy => {
            // Only an already-pushed acknowledgement is replaced by the legacy say.
            if early.ack_pushed {
                queue.abort();
            }
            return execute_plan(deps, cfg, cancel, plan, last_draft).await;
        }
        Reconcile::Remainder { text } => text,
    };
    if !text.is_empty() {
        mark_speaking(early, deps.reporter);
        if !queue.push(&text, plan.emotion) {
            queue.abort();
            return Ok(None);
        }
    }
    if race(cancel, queue.finish()).await.is_none() {
        queue.abort();
        return Err(SessionEnd::Cancelled);
    }
    let result = settle(deps, cfg, cancel).await;
    if matches!(plan.kind, PlanKind::End) {
        return Err(after_speech(result, SessionEnd::UserEnded));
    }
    if let Some(end) = speech_to_end(result) {
        return Err(end);
    }
    Ok(Some(plan.speech.clone()))
}

/// A plan request that failed or was superseded. `Ok(None)` = continue the loop.
async fn plan_failed(
    deps: &Deps<'_>,
    cfg: &SessionConfig,
    cancel: &mut CancelToken,
    err: PlanError,
) -> Result<Option<String>, SessionEnd> {
    match err {
        PlanError::NotConfigured => Err(after_speech(
            speak(deps, cfg, cancel, MSG_NOT_CONFIGURED, Emotion::Neutral).await,
            SessionEnd::NotConfigured,
        )),
        PlanError::Superseded => Ok(None),
        _ => {
            into_result(speak_or_end(deps, cfg, cancel, MSG_PLAN_FAILED, Emotion::Neutral).await)?;
            Ok(None)
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
    last_draft: &mut Option<String>,
) -> Result<Option<String>, SessionEnd> {
    let tier_start = Instant::now();
    let start = history.len().saturating_sub(cfg.context_turns);
    let input = PlanInput {
        utterance,
        history: history.get(start..).unwrap_or_default(),
        last_draft: last_draft.as_deref(),
        style: &cfg.style,
    };
    let queue = if cfg.early_speech || cfg.ack_after.is_some() {
        deps.mouth.open_queue()
    } else {
        None
    };
    let Some(queue) = queue else {
        return match race(cancel, deps.brain.plan(input)).await {
            None => Err(SessionEnd::Cancelled),
            Some(Ok(plan)) => execute_plan(deps, cfg, cancel, plan, last_draft).await,
            Some(Err(err)) => plan_failed(deps, cfg, cancel, err).await,
        };
    };
    let early = Mutex::new(Early::default());
    let planned = race(
        cancel,
        stream_plan(deps, cfg, queue.as_ref(), &early, input),
    )
    .await;
    let plan_at = Instant::now();
    let mut early = take_early(early);
    let turn = match planned {
        None => {
            queue.abort();
            Err(SessionEnd::Cancelled)
        }
        Some(Ok(plan)) => {
            streamed_outcome(
                deps,
                cfg,
                cancel,
                queue.as_ref(),
                &mut early,
                plan,
                last_draft,
            )
            .await
        }
        Some(Err(err)) => {
            if !early.spoken.is_empty() || early.ack_pushed || early.dead {
                queue.abort();
            }
            plan_failed(deps, cfg, cancel, err).await
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
    last_draft: &mut Option<String>,
) -> Result<Option<String>, SessionEnd> {
    // Cancel is checked before each effect.
    let summary: String = match plan.kind {
        PlanKind::End => {
            return Err(after_speech(
                speak(deps, cfg, cancel, &plan.speech, plan.emotion).await,
                SessionEnd::UserEnded,
            ));
        }
        PlanKind::Answer | PlanKind::Clarify => {
            into_result(speak_or_end(deps, cfg, cancel, &plan.speech, plan.emotion).await)?;
            plan.speech.clone()
        }
        PlanKind::Paste => {
            let Some(pasted) = plan.paste_text.clone() else {
                into_result(
                    speak_or_end(deps, cfg, cancel, MSG_PASTE_FAILED, Emotion::Neutral).await,
                )?;
                return Ok(None);
            };
            if is_cancelled(cancel) {
                return Err(SessionEnd::Cancelled);
            }
            if deps.hands.paste_generated(&pasted).is_err() {
                into_result(
                    speak_or_end(deps, cfg, cancel, MSG_PASTE_FAILED, Emotion::Neutral).await,
                )?;
                return Ok(None);
            }
            *last_draft = Some(pasted);
            let spoken = if plan.speech.is_empty() {
                MSG_DONE.to_string()
            } else {
                plan.speech.clone()
            };
            into_result(speak_or_end(deps, cfg, cancel, &spoken, plan.emotion).await)?;
            format!("{spoken} (pasted text)")
        }
        PlanKind::Command => match run_command_plan(deps, cfg, cancel, &plan).await? {
            Some(summary) => summary,
            None => return Ok(None),
        },
    };
    Ok(Some(summary))
}

/// The turn loop. Returns the end; the wrapper reports Idle and stops.
async fn run_turns(deps: &Deps<'_>, cfg: &SessionConfig, cancel: &mut CancelToken) -> SessionEnd {
    let mut history: Vec<(String, String)> = Vec::new();
    let mut last_draft: Option<String> = None;
    let mut idle_turns: u32 = 0;
    let mut turns: usize = 0;

    loop {
        if is_cancelled(cancel) {
            return SessionEnd::Cancelled;
        }
        if turns >= cfg.max_turns {
            return SessionEnd::TurnLimit;
        }
        turns += 1;

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

        // Local tier: end phrases and explicit dictation.
        match classify_local(&utterance) {
            LocalIntent::End => {
                return after_speech(
                    speak(deps, cfg, cancel, MSG_OKAY, Emotion::Neutral).await,
                    SessionEnd::UserEnded,
                );
            }
            LocalIntent::Dictate(text) => {
                if is_cancelled(cancel) {
                    return SessionEnd::Cancelled;
                }
                if deps.hands.paste_generated(&text).is_err() {
                    if let Some(end) =
                        speak_or_end(deps, cfg, cancel, MSG_PASTE_FAILED, Emotion::Neutral).await
                    {
                        return end;
                    }
                }
                continue;
            }
            LocalIntent::None => {}
        }

        // Exact tier: what the user said (shortcuts, media, router).
        deps.reporter.state(SessionState::Acting);
        if is_cancelled(cancel) {
            return SessionEnd::Cancelled;
        }
        let fixed = match deps.hands.run_user_phrase(&utterance).await {
            PhraseOutcome::NoMatch => None,
            PhraseOutcome::Done(message) => Some(message),
            PhraseOutcome::Failed => Some(MSG_FAILED.to_string()),
            PhraseOutcome::Refused => Some(MSG_REFUSED.to_string()),
        };
        if let Some(message) = fixed {
            if let Some(end) = speak_or_end(deps, cfg, cancel, &message, Emotion::Neutral).await {
                return end;
            }
            continue;
        }

        // Brain tier. The plan is dropped if cancel wins the race.
        deps.reporter.state(SessionState::Thinking);
        let summary =
            match brain_turn(deps, cfg, cancel, &utterance, &history, &mut last_draft).await {
                Ok(Some(summary)) => summary,
                Ok(None) => continue,
                Err(end) => return end,
            };

        push_history(&mut history, &utterance, &summary, cfg.context_turns);
        if cfg.context_turns == 0 {
            last_draft = None;
        }
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
            paste_text: Some(text.to_string()),
            ..plan(PlanKind::Paste, speech)
        }
    }
    fn command_plan(cmd: &str, speech: &str) -> TurnPlan {
        TurnPlan {
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
        // other errors: apologise, keep going, nothing recorded
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
            assert!(
                h[1].is_empty(),
                "failed turn must not enter history ({e:?})"
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
        assert_eq!(h[1][0].1, "Pasted. (pasted text)");
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
