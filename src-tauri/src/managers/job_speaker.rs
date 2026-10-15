//! Speaks and emits background coding job progress (M3 S3c-2, DESIGN D5, D7, R2-6, R2-8).
//!
//! Speech and status events for background coding jobs (M3 S3c-2, DESIGN D5, R2-6, R2-8).
//!
//! Only fixed templates are ever spoken. The job output tail is never spoken, emitted or
//! logged.

use std::sync::Arc;

use crate::commands::agent_jobs::AgentJobStatusDto;
use crate::managers::agent_jobs::{JobManager, JobObserver, JobSnapshot, JobState};
use crate::managers::job_announcer::{AnnounceKind, Announcer};
use crate::managers::job_summary::{ResultStore, ResultSummarizer, SummaryPolicy, SummaryTokens};
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
    /// M4: summary pieces. Stored by `with_summary`; not read until a later step.
    #[allow(dead_code)] // wired in a later M4 step
    summary: Option<SummaryParts>,
}

/// M4: the summary dependencies of a speaker (see `JobSpeaker::with_summary`).
#[allow(dead_code)] // wired in a later M4 step
struct SummaryParts {
    summarizer: Arc<dyn ResultSummarizer>,
    results: Arc<ResultStore>,
    tokens: Arc<SummaryTokens>,
    policy: SummaryPolicy,
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
        }
    }

    /// Sets the configured time limit, so the timed-out message names the minutes.
    pub fn with_timeout_minutes(mut self, minutes: u32) -> Self {
        self.timeout_minutes = Some(minutes);
        self
    }

    /// M4 S3 stub: overrides the summary deadline (todo until the S4 step).
    pub fn with_summary_deadline(self, _d: std::time::Duration) -> Self {
        todo!("M4 S3")
    }

    /// M4: attaches the summary pieces. Scaffolding: stored only, so behavior is unchanged.
    #[allow(dead_code)] // wired in a later M4 step
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
}

impl JobObserver for JobSpeaker {
    fn on_progress(&self, snap: &JobSnapshot) {
        self.sink.emit(&snapshot_to_dto(snap));
        if self.speech_enabled() {
            self.announcer
                .announce(AnnounceKind::Progress, self.progress_text());
        }
    }

    fn on_finish(&self, snap: &JobSnapshot) {
        self.sink.emit(&snapshot_to_dto(snap));
        self.sink.refresh_tray();
        if !self.speech_enabled() {
            return;
        }
        if let Some(text) = self.finish_text(snap.state) {
            self.announcer.announce(AnnounceKind::Finish, text);
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
