//! Speaks and emits background coding job progress (M3 S3c-2, DESIGN D5, D7, R2-6, R2-8).
//!
//! Speech and status events for background coding jobs (M3 S3c-2, DESIGN D5, R2-6, R2-8).
//!
//! Only fixed templates are ever spoken. The job output tail is never spoken, emitted or
//! logged. M4: an ask job that succeeded may speak a short summary of its result instead
//! of the fixed finish sentence (DESIGN D6).

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::commands::agent_jobs::AgentJobStatusDto;
use crate::managers::agent_jobs::{JobManager, JobObserver, JobResult, JobSnapshot, JobState};
use crate::managers::job_announcer::{AnnounceKind, Announcer};
use crate::managers::job_summary::{
    clean_summary_text, spoken_summary, ResultStore, ResultSummarizer, SpeechGuard, SummaryError,
    SummaryPolicy, SummaryTokens, Ticket, FALLBACK_SPEECH, SUMMARY_DEADLINE,
};
use crate::managers::tts::sanitize_speech_text;

/// Receives the job status events and tray refresh requests from the speaker.
pub trait JobEventSink: Send + Sync {
    /// Emits one status event (the `agent-job-update` payload).
    fn emit(&self, dto: &AgentJobStatusDto);
    /// Asks the tray to refresh its menu and icon.
    fn refresh_tray(&self);
}

/// Observer that speaks and emits the progress and final state of one coding job.
pub struct JobSpeaker {
    project: String,
    announcer: Arc<Announcer>,
    tts_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    sink: Arc<dyn JobEventSink>,
    /// Configured time limit in minutes, used only in the timed-out message.
    timeout_minutes: Option<u32>,
    /// M4: summary pieces, present only after `with_summary`.
    summary: Option<SummaryParts>,
    /// M4: how long a summary may take before the fixed fallback is spoken.
    summary_deadline: Duration,
    /// M4: outcome of the latest summary attempt, keyed by job id, consumed by `on_finish`.
    outcome: Mutex<Option<(u64, SummaryOutcome)>>,
}

/// M4: the summary dependencies of a speaker (see `JobSpeaker::with_summary`).
struct SummaryParts {
    summarizer: Arc<dyn ResultSummarizer>,
    results: Arc<ResultStore>,
    tokens: Arc<SummaryTokens>,
    policy: SummaryPolicy,
}

/// M4: what the summary attempt for one job produced.
enum SummaryOutcome {
    /// No summary was requested (policy off, no ticket, or no result).
    NotAttempted,
    /// The summary was superseded or the tokens changed; the fixed text is spoken.
    Invalidated,
    /// The summary failed (error, timeout, empty, NUL, panic); the fallback is spoken.
    Failed,
    /// A valid summary, guarded by the ticket it was issued under.
    Summary {
        tokens: Arc<SummaryTokens>,
        ticket: Ticket,
        text: String,
    },
}

impl JobSpeaker {
    /// Creates the speaker for a job in `project`. `tts_enabled` is read at each event.
    pub fn new(
        project: String,
        announcer: Arc<Announcer>,
        tts_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
        sink: Arc<dyn JobEventSink>,
    ) -> Self {
        Self {
            project,
            announcer,
            tts_enabled,
            sink,
            timeout_minutes: None,
            summary: None,
            summary_deadline: SUMMARY_DEADLINE,
            outcome: Mutex::new(None),
        }
    }

    /// Sets the configured time limit, so the timed-out message names the minutes.
    pub fn with_timeout_minutes(mut self, minutes: u32) -> Self {
        self.timeout_minutes = Some(minutes);
        self
    }

    /// Overrides how long a summary may take before the fixed fallback is spoken.
    /// Test-only: production uses the default deadline.
    #[cfg(test)]
    pub fn with_summary_deadline(mut self, deadline: Duration) -> Self {
        self.summary_deadline = deadline;
        self
    }

    /// M4: attaches the summary pieces. Without this call the speaker behaves as in M3.
    pub fn with_summary(
        mut self,
        summarizer: Arc<dyn ResultSummarizer>,
        results: Arc<ResultStore>,
        tokens: Arc<SummaryTokens>,
        policy: SummaryPolicy,
    ) -> Self {
        self.summary = Some(SummaryParts {
            summarizer,
            results,
            tokens,
            policy,
        });
        self
    }

    /// Whether speech is enabled right now.
    fn speech_enabled(&self) -> bool {
        (self.tts_enabled)()
    }

    /// "The coding job in <project>", or "The coding job" when the name sanitizes to nothing.
    fn subject(&self) -> String {
        match sanitize_speech_text(&self.project) {
            Some(name) => format!("The coding job in {name}"),
            None => "The coding job".to_string(),
        }
    }

    /// Fixed final-state sentence, or `None` for a state that is not final.
    fn finish_text(&self, state: JobState) -> Option<String> {
        let subject = self.subject();
        let text = match state {
            JobState::Succeeded => format!("{subject} finished successfully."),
            JobState::Failed { code: Some(code) } => {
                format!("{subject} failed with exit code {code}.")
            }
            JobState::Failed { code: None } => format!("{subject} failed."),
            JobState::Cancelled => format!("{subject} was cancelled."),
            JobState::TimedOut => match self.timeout_minutes {
                Some(minutes) => format!("{subject} timed out after {minutes} minutes."),
                None => format!("{subject} timed out."),
            },
            JobState::SpawnFailed => format!("{subject} couldn't start."),
            JobState::Running => return None,
        };
        Some(text)
    }

    /// Fixed progress sentence naming the project when it is speakable.
    fn progress_text(&self) -> String {
        match sanitize_speech_text(&self.project) {
            Some(name) => format!("Still working on the job in {name}."),
            None => "Still working on the coding job.".to_string(),
        }
    }

    /// Status DTO for `snap`, carrying the stored result of that job when one exists.
    fn dto(&self, snap: &JobSnapshot) -> AgentJobStatusDto {
        let mut dto = snapshot_to_dto(snap);
        dto.result = self
            .summary
            .as_ref()
            .and_then(|parts| parts.results.get_for(snap.id));
        dto
    }

    /// Runs the summary attempt for one successful ask result.
    fn attempt_summary(&self, parts: &SummaryParts, r: &JobResult) -> SummaryOutcome {
        let enabled = (parts.policy.ask_enabled)() && (parts.policy.tts_enabled)();
        if !enabled {
            return SummaryOutcome::NotAttempted;
        }
        let Some(ticket) = parts.tokens.begin(r.id) else {
            return SummaryOutcome::NotAttempted;
        };
        let submitted = parts.tokens.run_if_valid(&ticket, |valid| {
            valid.then(|| catch_unwind(AssertUnwindSafe(|| parts.summarizer.submit(&r.text))))
        });
        let handle = match submitted {
            None => return SummaryOutcome::Invalidated,
            Some(Ok(handle)) => handle,
            Some(Err(_)) => return SummaryOutcome::Failed,
        };
        let still_current = || parts.tokens.is_current(&ticket);
        let waited = catch_unwind(AssertUnwindSafe(|| {
            handle.wait(&still_current, self.summary_deadline)
        }));
        match waited {
            Ok(Ok(raw)) => validate_summary(parts, ticket, &raw),
            Ok(Err(SummaryError::Aborted)) => SummaryOutcome::Invalidated,
            Ok(Err(SummaryError::TimedOut | SummaryError::Failed)) | Err(_) => {
                SummaryOutcome::Failed
            }
        }
    }

    /// Takes the stored outcome when it belongs to `job_id`; otherwise leaves it in place.
    fn take_outcome(&self, job_id: u64) -> Option<SummaryOutcome> {
        let mut slot = lock_slot(&self.outcome);
        if slot.as_ref().is_some_and(|(id, _)| *id == job_id) {
            slot.take().map(|(_, outcome)| outcome)
        } else {
            None
        }
    }
}

/// Cleans the raw answer and re-checks validity under the gate before using it.
fn validate_summary(parts: &SummaryParts, ticket: Ticket, raw: &str) -> SummaryOutcome {
    let Some(clean) = clean_summary_text(raw) else {
        return SummaryOutcome::Failed;
    };
    parts.tokens.run_if_valid(&ticket, |valid| {
        if valid {
            SummaryOutcome::Summary {
                tokens: parts.tokens.clone(),
                ticket,
                text: spoken_summary(&clean),
            }
        } else {
            SummaryOutcome::Invalidated
        }
    })
}

/// Locks the outcome slot, recovering from a poisoned mutex.
fn lock_slot(
    slot: &Mutex<Option<(u64, SummaryOutcome)>>,
) -> MutexGuard<'_, Option<(u64, SummaryOutcome)>> {
    slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl JobObserver for JobSpeaker {
    fn on_progress(&self, snap: &JobSnapshot) {
        self.sink.emit(&self.dto(snap));
        if self.speech_enabled() {
            self.announcer
                .announce(AnnounceKind::Progress, self.progress_text());
        }
    }

    fn on_result(&self, r: &JobResult) {
        let Some(parts) = self.summary.as_ref() else {
            return;
        };
        parts.results.set(r.id, r.text.clone());
        let outcome = self.attempt_summary(parts, r);
        *lock_slot(&self.outcome) = Some((r.id, outcome));
    }

    fn on_finish(&self, snap: &JobSnapshot) {
        self.sink.emit(&self.dto(snap));
        self.sink.refresh_tray();
        let outcome = self.take_outcome(snap.id);
        if !self.speech_enabled() {
            return;
        }
        let Some(text) = self.finish_text(snap.state) else {
            return;
        };
        let outcome = if matches!(snap.state, JobState::Succeeded) {
            outcome
        } else {
            None
        };
        match outcome {
            Some(SummaryOutcome::Summary {
                tokens,
                ticket,
                text: spoken,
            }) => {
                let guard = SpeechGuard {
                    tokens,
                    ticket,
                    fallback: text,
                };
                self.announcer
                    .announce_guarded(AnnounceKind::Finish, spoken, guard);
            }
            Some(SummaryOutcome::Failed) => {
                self.announcer
                    .announce(AnnounceKind::Finish, FALLBACK_SPEECH.to_string());
            }
            Some(SummaryOutcome::NotAttempted | SummaryOutcome::Invalidated) | None => {
                self.announcer.announce(AnnounceKind::Finish, text);
            }
        }
    }
}

/// Maps a job snapshot to the status DTO (state name, project, elapsed seconds; no output).
pub fn snapshot_to_dto(snap: &JobSnapshot) -> AgentJobStatusDto {
    AgentJobStatusDto {
        state: state_name(snap.state).to_string(),
        project: Some(snap.project.clone()),
        elapsed_secs: snap.elapsed_secs,
        result: None,
    }
}

/// Stable wire name of a job state (used by the status DTO and the `job_status` tool).
pub fn state_name(state: JobState) -> &'static str {
    match state {
        JobState::Running => "running",
        JobState::Succeeded => "succeeded",
        JobState::Failed { .. } => "failed",
        JobState::Cancelled => "cancelled",
        JobState::TimedOut => "timed_out",
        JobState::SpawnFailed => "spawn_failed",
    }
}

/// The DTO reported when no job exists: state `idle`, no project, zero elapsed seconds.
pub fn idle_dto() -> AgentJobStatusDto {
    AgentJobStatusDto {
        state: "idle".to_string(),
        project: None,
        elapsed_secs: 0,
        result: None,
    }
}

/// Status of the current or most recent job, or `idle_dto()` when none exists.
/// Test-only: production uses `status_dto_with` (carries the stored result).
#[cfg(test)]
pub fn status_dto(manager: &JobManager) -> AgentJobStatusDto {
    manager
        .snapshot()
        .map(|snap| snapshot_to_dto(&snap))
        .unwrap_or_else(idle_dto)
}

/// Cancels the running job. Returns `true` when a running job was asked to stop.
pub fn cancel(manager: &JobManager) -> bool {
    manager.cancel()
}

#[cfg(test)]
#[path = "job_speaker_tests.rs"]
mod job_speaker_tests;

#[cfg(test)]
#[path = "job_speaker_timeout_tests.rs"]
mod job_speaker_timeout_tests;

#[cfg(test)]
#[path = "job_speaker_m4_tests.rs"]
mod job_speaker_m4_tests;
