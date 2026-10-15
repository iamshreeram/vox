//! Summary support for ask-mode jobs (M4): admission tokens, the summary handle, the
//! result store and the summarizer trait. Scaffolding in S0: every item is present
//! with safe disabled behavior (no summary is ever requested or spoken).
#![allow(dead_code)] // wired in a later M4 step: every item here is S0 scaffolding

use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

/// Identity of one summary attempt: the job it belongs to and the epoch it started in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket {
    /// Job the summary belongs to.
    pub job_id: u64,
    /// Token epoch when the ticket was issued.
    pub epoch: u64,
}

/// Admission and invalidation tokens shared by the job manager, speakers and announcer.
///
/// Scaffolding (S0): nothing is ever issued, so `begin` returns `None` and every
/// validity check reports invalid.
#[derive(Debug, Default)]
pub struct SummaryTokens;

impl SummaryTokens {
    /// Creates the shared token state.
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }

    /// Records that `job_id` was admitted (scaffolding: no-op).
    #[allow(dead_code)] // wired in a later M4 step
    pub fn admit(&self, _job_id: u64) {}

    /// Invalidates every outstanding ticket (scaffolding: no-op).
    #[allow(dead_code)] // wired in a later M4 step
    pub fn invalidate(&self) {}

    /// Closes the tokens for good (scaffolding: no-op).
    #[allow(dead_code)] // wired in a later M4 step
    pub fn close(&self) {}

    /// Issues a ticket for `job_id` if it is the latest admitted job (scaffolding: never).
    #[allow(dead_code)] // wired in a later M4 step
    pub fn begin(&self, _job_id: u64) -> Option<Ticket> {
        None
    }

    /// True while the ticket is still valid (scaffolding: never).
    #[allow(dead_code)] // wired in a later M4 step
    pub fn is_current(&self, _ticket: &Ticket) -> bool {
        false
    }

    /// Runs `f` with the validity of `ticket` (scaffolding: always `false`).
    #[allow(dead_code)] // wired in a later M4 step
    pub fn run_if_valid<R>(&self, _ticket: &Ticket, f: impl FnOnce(bool) -> R) -> R {
        f(false)
    }
}

/// Why a summary did not produce text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryError {
    /// The summary was abandoned because the job was superseded or the app is exiting.
    Aborted,
    /// The gateway did not answer in time.
    TimedOut,
    /// Any other failure (empty, invalid or failed answer).
    Failed,
}

/// Receives the answer of one summary request.
pub struct SummaryHandle {
    #[allow(dead_code)] // wired in a later M4 step
    rx: Receiver<Result<String, SummaryError>>,
}

impl SummaryHandle {
    /// Wraps the receiver of a summary worker.
    pub fn new(rx: Receiver<Result<String, SummaryError>>) -> Self {
        Self { rx }
    }

    /// Waits for the summary (scaffolding: reports `Failed` at once).
    #[allow(dead_code)] // wired in a later M4 step
    pub fn wait(
        self,
        _still_current: &dyn Fn() -> bool,
        _deadline: Duration,
    ) -> Result<String, SummaryError> {
        Err(SummaryError::Failed)
    }
}

/// Produces a spoken summary of a job result.
pub trait ResultSummarizer: Send + Sync {
    /// Starts a summary of `text` without blocking and returns its handle.
    fn submit(&self, text: &str) -> SummaryHandle;
}

/// Live switches that decide whether a summary may be requested and spoken.
pub struct SummaryPolicy {
    /// Whether ask mode is on.
    pub ask_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Whether text-to-speech is on.
    pub tts_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
}

/// Guards one finish announcement so stale summary text is never spoken.
#[derive(Clone)]
pub struct SpeechGuard {
    /// Shared token state checked at mouth acceptance.
    pub tokens: Arc<SummaryTokens>,
    /// Ticket the summary was issued under.
    pub ticket: Ticket,
    /// Fixed text spoken instead when the summary is no longer valid.
    pub fallback: String,
}

/// Holds the latest job result text, keyed by job id.
///
/// Scaffolding (S0): nothing is stored and `get_for` always returns `None`.
#[derive(Debug, Default)]
pub struct ResultStore;

impl ResultStore {
    /// Creates an empty store.
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }

    /// Stores the result of `job_id` (scaffolding: no-op).
    #[allow(dead_code)] // wired in a later M4 step
    pub fn set(&self, _job_id: u64, _text: String) {}

    /// Returns the stored result of `job_id` (scaffolding: always `None`).
    #[allow(dead_code)] // wired in a later M4 step
    pub fn get_for(&self, _job_id: u64) -> Option<String> {
        None
    }
}

/// Spoken when a summary is unavailable (error, timeout, empty, invalid).
pub const FALLBACK_SPEECH: &str = "The agent finished. The result is in Vox settings.";

/// Prefix of every spoken summary.
pub const SPEECH_PREFIX: &str = "The agent says: ";

/// How long a summary may take before the fixed fallback is spoken.
pub const SUMMARY_DEADLINE: Duration = Duration::from_secs(15);

/// System prompt for the summarizer (placeholder until the S4 step).
pub const ASK_SUMMARY_SYSTEM: &str = "todo";

/// M4 S3 stub: cleans a raw summary answer for speech (todo until the S4 step).
pub fn clean_summary_text(_raw: &str) -> Option<String> {
    todo!("M4 S3")
}

/// M4 S3 stub: wraps cleaned text as spoken summary (todo until the S4 step).
pub fn spoken_summary(_clean: &str) -> String {
    todo!("M4 S3")
}

/// M4 S3 stub: builds the fenced user prompt (todo until the S4 step).
pub fn summary_user_prompt(_text: &str) -> String {
    todo!("M4 S3")
}

#[cfg(test)]
#[path = "job_summary_m4_tests.rs"]
mod job_summary_m4_tests;
