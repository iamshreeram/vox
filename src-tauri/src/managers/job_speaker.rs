//! Speaks and emits background coding job progress (M3 S3c-2, DESIGN D5, D7, R2-6, R2-8).
//!
//! Phase A stub: every body is `todo!()` so the tests in `job_speaker_tests.rs` compile and
//! fail at runtime until the implementation lands.

#![allow(dead_code, unused_variables)]

use std::sync::Arc;
use std::time::Instant;

use crate::commands::agent_jobs::AgentJobStatusDto;
use crate::managers::agent_jobs::{JobManager, JobObserver, JobSnapshot};
use crate::managers::job_announcer::Announcer;

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
    started: Instant,
}

impl JobSpeaker {
    /// Creates the speaker for a job in `project`. `tts_enabled` is read at each event.
    pub fn new(
        project: String,
        announcer: Arc<Announcer>,
        tts_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
        sink: Arc<dyn JobEventSink>,
    ) -> Self {
        todo!("S3c-2 Phase B: JobSpeaker::new")
    }
}

impl JobObserver for JobSpeaker {
    fn on_progress(&self, snap: &JobSnapshot) {
        todo!("S3c-2 Phase B: JobSpeaker::on_progress")
    }

    fn on_finish(&self, snap: &JobSnapshot) {
        todo!("S3c-2 Phase B: JobSpeaker::on_finish")
    }
}

/// Maps a job snapshot to the status DTO (state name, project, elapsed seconds; no output).
pub fn snapshot_to_dto(snap: &JobSnapshot) -> AgentJobStatusDto {
    todo!("S3c-2 Phase B: snapshot_to_dto")
}

/// The DTO reported when no job exists: state `idle`, no project, zero elapsed seconds.
pub fn idle_dto() -> AgentJobStatusDto {
    todo!("S3c-2 Phase B: idle_dto")
}

/// Status of the current or most recent job, or `idle_dto()` when none exists.
pub fn status_dto(manager: &JobManager) -> AgentJobStatusDto {
    todo!("S3c-2 Phase B: status_dto")
}

/// Cancels the running job. Returns `true` when a running job was asked to stop.
pub fn cancel(manager: &JobManager) -> bool {
    todo!("S3c-2 Phase B: cancel")
}

#[cfg(test)]
#[path = "job_speaker_tests.rs"]
mod job_speaker_tests;
