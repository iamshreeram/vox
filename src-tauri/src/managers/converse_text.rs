//! Typed text in the conversation window (DESIGN-TYPE Rev 4): sanitising, the bounded inbox that
//! carries typed turns into a running session, the routing rule, and the collaborators a text
//! session uses instead of the microphone, the speaker and paste.
//!
//! Nothing in this module logs, and typed text is never written to disk. Typed text is handled
//! exactly like a transcribed utterance by the session loop.

use crate::managers::converse::{
    BoxFuture, Ears, Hands, ListenOutcome, Mouth, PhraseOutcome, QuietOutcome, SayOutcome,
};
use crate::managers::converse_agent::{AgentRun, AgentStartError, PreparedAgentLaunch};
use crate::managers::converse_barge::BargeIn;
use crate::managers::converse_confirm::{
    DirFd, JobLaunch, JobStartOutcome, PinError, ValidatedCommand,
};
use crate::managers::converse_latency::ListenTiming;
use crate::managers::converse_plan::StepKind;
use crate::managers::converse_runtime::ConverseStatus;
use crate::managers::emotion::Emotion;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::Notify;

/// Longest typed message, in characters after cleaning.
pub const MAX_TYPED_CHARS: usize = 2000;

/// Most typed messages that may wait for a session loop at once.
pub const INBOX_CAPACITY: usize = 16;

/// Why a typed message was not accepted. The codes are the wire strings the page maps to text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedError {
    /// Nothing left after cleaning.
    Empty,
    /// More than `MAX_TYPED_CHARS` characters after cleaning.
    TooLong,
    /// A session is stopping, the inbox is full, or the message belongs to another session.
    Busy,
    /// No reply backend is configured, so no text session can start.
    NotConfigured,
    /// The text session could not be started for another reason.
    Failed,
}

impl TypedError {
    /// The stable wire code of this error (never includes any text).
    pub fn code(self) -> &'static str {
        match self {
            TypedError::Empty => "empty",
            TypedError::TooLong => "too_long",
            TypedError::Busy => "busy",
            TypedError::NotConfigured => "not_configured",
            TypedError::Failed => "failed",
        }
    }
}

/// Cleans one typed message: line breaks and tabs become spaces, control characters are
/// removed, whitespace runs collapse to one space and the ends are trimmed. Markup stays plain
/// text. The length limit counts characters after cleaning.
pub fn sanitize_typed(raw: &str) -> Result<String, TypedError> {
    let mut cleaned = String::with_capacity(raw.len());
    let mut pending_space = false;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            pending_space = true;
            continue;
        }
        if ch.is_control() {
            continue;
        }
        if pending_space && !cleaned.is_empty() {
            cleaned.push(' ');
        }
        pending_space = false;
        cleaned.push(ch);
    }
    if cleaned.is_empty() {
        return Err(TypedError::Empty);
    }
    if cleaned.chars().count() > MAX_TYPED_CHARS {
        return Err(TypedError::TooLong);
    }
    Ok(cleaned)
}

/// Where a typed message goes, given the controller's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedRoute {
    /// A session is running: queue the message for its loop.
    Queue,
    /// Nothing is running: start a text session, then queue the message.
    StartText,
    /// A session is stopping: refuse.
    Busy,
}

/// The routing rule for a typed message.
pub fn route_typed(status: ConverseStatus) -> TypedRoute {
    match status {
        ConverseStatus::Idle => TypedRoute::StartText,
        ConverseStatus::Running => TypedRoute::Queue,
        ConverseStatus::Stopping => TypedRoute::Busy,
    }
}

/// Mutable inbox state, always behind one lock.
struct InboxState {
    /// True between `open` and `close`: only then is anything accepted.
    open: bool,
    /// Increases at every `open`, so a sender can bind a message to one session.
    generation: u64,
    queue: VecDeque<String>,
    /// The typed-seen flag of the open session, set when a message is delivered.
    seen: Option<Arc<AtomicBool>>,
}

/// The bounded FIFO that carries typed messages into one session's loop. Closed by default;
/// `open` starts a session and `close` ends it, and both clear what was queued.
pub struct TypedInbox {
    state: Mutex<InboxState>,
    wake: Notify,
}

impl TypedInbox {
    /// A new, closed inbox.
    pub fn new() -> Arc<Self> {
        Arc::new(TypedInbox {
            state: Mutex::new(InboxState {
                open: false,
                generation: 0,
                queue: VecDeque::new(),
                seen: None,
            }),
            wake: Notify::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, InboxState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Starts accepting for a new session: clears leftovers, bumps the generation, and returns
    /// the session's typed-seen flag (set once a typed message is delivered to its loop).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn open(&self) -> Arc<AtomicBool> {
        self.open_session().1
    }

    /// Like `open`, also returning the generation the new session is bound to.
    pub fn open_session(&self) -> (u64, Arc<AtomicBool>) {
        let mut state = self.lock();
        state.open = true;
        state.generation = state.generation.wrapping_add(1);
        state.queue.clear();
        let flag = Arc::new(AtomicBool::new(false));
        state.seen = Some(flag.clone());
        (state.generation, flag)
    }

    /// Stops accepting and drops whatever was queued.
    pub fn close(&self) {
        let mut state = self.lock();
        state.open = false;
        state.queue.clear();
        state.seen = None;
    }

    /// Drops whatever is queued, keeping the inbox open.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn clear(&self) {
        self.lock().queue.clear();
    }

    /// The generation of the open session, or `None` while the inbox is closed.
    pub fn generation(&self) -> Option<u64> {
        let state = self.lock();
        if state.open {
            Some(state.generation)
        } else {
            None
        }
    }

    /// How many messages wait for the loop.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn len(&self) -> usize {
        self.lock().queue.len()
    }

    /// Queues a message for the open session. `Busy` when closed or full.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn push(&self, text: String) -> Result<(), TypedError> {
        self.accept(text, None)
    }

    /// Queues a message only if the open session is `generation`. `Busy` when closed, full, or
    /// when the session has changed since the sender read the generation.
    pub fn push_for(&self, generation: u64, text: String) -> Result<(), TypedError> {
        self.accept(text, Some(generation))
    }

    /// The check and the enqueue happen under one lock, so a message is either queued for the
    /// session it was bound to or refused.
    fn accept(&self, text: String, generation: Option<u64>) -> Result<(), TypedError> {
        let mut state = self.lock();
        if !state.open {
            return Err(TypedError::Busy);
        }
        if generation.is_some_and(|expected| expected != state.generation) {
            return Err(TypedError::Busy);
        }
        if state.queue.len() >= INBOX_CAPACITY {
            return Err(TypedError::Busy);
        }
        state.queue.push_back(text);
        drop(state);
        self.wake.notify_one();
        Ok(())
    }

    /// Takes the oldest message, waiting until one arrives. Cancel-safe: the queue is the only
    /// source of truth, so a dropped waiter never loses a message.
    pub async fn next(&self) -> String {
        loop {
            let wake = self.wake.notified();
            if let Some(text) = self.take_front() {
                return text;
            }
            wake.await;
        }
    }

    fn take_front(&self) -> Option<String> {
        let mut state = self.lock();
        let text = state.queue.pop_front()?;
        if let Some(seen) = state.seen.as_ref() {
            seen.store(true, Ordering::SeqCst);
        }
        Some(text)
    }
}

/// Ears that never hear anything: a text session's listen never completes.
pub struct PendingEars;

impl Ears for PendingEars {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        Box::pin(std::future::pending::<ListenOutcome>())
    }
}

/// Wraps the session's ears so a typed message is heard as an utterance. Typed text wins over
/// the inner ears when both are ready; the inner listen is dropped, which cancels it the same
/// way the session's own race does.
pub struct WithTyped {
    inner: Box<dyn Ears>,
    inbox: Arc<TypedInbox>,
    /// True when the last listen returned typed text.
    typed_won: AtomicBool,
}

impl WithTyped {
    /// Wraps `inner` so that typed messages from `inbox` are heard too.
    pub fn new(inner: Box<dyn Ears>, inbox: Arc<TypedInbox>) -> Self {
        WithTyped {
            inner,
            inbox,
            typed_won: AtomicBool::new(false),
        }
    }
}

impl Ears for WithTyped {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        Box::pin(async move {
            self.typed_won.store(false, Ordering::SeqCst);
            tokio::select! {
                biased;
                text = self.inbox.next() => {
                    self.typed_won.store(true, Ordering::SeqCst);
                    ListenOutcome::Utterance(text)
                }
                outcome = self.inner.listen() => outcome,
            }
        })
    }

    fn take_timing(&self) -> Option<ListenTiming> {
        if self.typed_won.load(Ordering::SeqCst) {
            // The inner listen was cancelled: its timing belongs to nothing and is dropped.
            let _discarded = self.inner.take_timing();
            None
        } else {
            self.inner.take_timing()
        }
    }

    fn barge_in(&self) -> Option<&dyn BargeIn> {
        self.inner.barge_in()
    }

    fn barge_shutdown(&self) {
        self.inner.barge_shutdown();
    }
}

/// The mouth of a text session: nothing is spoken, every turn finishes at once.
pub struct SilentMouth;

impl Mouth for SilentMouth {
    fn say<'a>(&'a self, _text: &'a str, _emotion: Emotion) -> BoxFuture<'a, SayOutcome> {
        Box::pin(async { SayOutcome::Done })
    }

    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async { QuietOutcome::Quiet })
    }

    fn stop(&self) {}
}

/// Refusal text for a paste requested during a text session.
const PASTE_REFUSED: &str = "Text chat does not paste into other apps.";

/// The hands of a text session. Paste into the active window and background coding jobs are
/// refused (a job would announce its progress by voice). Everything else delegates, so local
/// commands, phrases and the agent hand-off work.
pub struct TextHands {
    inner: Box<dyn Hands>,
}

impl TextHands {
    /// Wraps `inner` with the text-session rules.
    pub fn new(inner: Box<dyn Hands>) -> Self {
        TextHands { inner }
    }
}

impl Hands for TextHands {
    fn paste_generated(&self, _text: &str) -> Result<(), String> {
        Err(PASTE_REFUSED.to_string())
    }

    fn run_user_phrase<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        self.inner.run_user_phrase(phrase)
    }

    fn run_model_command<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        self.inner.run_model_command(phrase)
    }

    fn validate_command(&self, text: &str) -> Option<ValidatedCommand> {
        self.inner.validate_command(text)
    }

    fn run_validated_command(&self, cmd: ValidatedCommand) -> bool {
        self.inner.run_validated_command(cmd)
    }

    fn pin_project(&self, name: &str) -> Result<DirFd, PinError> {
        self.inner.pin_project(name)
    }

    fn start_job(&self, _launch: JobLaunch) -> JobStartOutcome {
        JobStartOutcome::Unsupported
    }

    // A text session starts no background jobs, so it has none to cancel; the trait default
    // (false) applies and the single production `cancel_job` call site stays in the handler.

    fn has_active_job(&self) -> bool {
        false
    }

    fn prepare_agent_launch(
        &self,
        task: &str,
        role: Option<&str>,
    ) -> Result<PreparedAgentLaunch, AgentStartError> {
        self.inner.prepare_agent_launch(task, role)
    }

    fn agent_launch_still_valid(&self, launch: &PreparedAgentLaunch) -> bool {
        self.inner.agent_launch_still_valid(launch)
    }

    fn start_agent(
        &self,
        launch: PreparedAgentLaunch,
    ) -> Result<Box<dyn AgentRun>, AgentStartError> {
        self.inner.start_agent(launch)
    }

    fn publish_agent_result(&self, text: &str) {
        self.inner.publish_agent_result(text);
    }

    fn prepare_agent_launch_for(
        &self,
        kind: StepKind,
        task: &str,
        role: Option<&str>,
    ) -> Result<PreparedAgentLaunch, AgentStartError> {
        self.inner.prepare_agent_launch_for(kind, task, role)
    }

    fn prepare_agent_launch_typed(
        &self,
        kind: StepKind,
        task: &str,
        role: Option<&str>,
    ) -> Result<PreparedAgentLaunch, AgentStartError> {
        self.inner.prepare_agent_launch_typed(kind, task, role)
    }
}

#[cfg(test)]
#[path = "converse_text_tests.rs"]
mod converse_text_tests;
