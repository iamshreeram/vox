//! App wiring for the background coding job (M3 S3d, DESIGN Rev 5 R2-8, R4-2, R5-1, D6, D7).
//!
//! Phase A stub: every signature is final, every body is `todo!()`. The implementer replaces
//! the bodies in Phase B. Tests live in `job_runtime_tests.rs`.

#![allow(dead_code, unused_variables)]

use std::sync::Arc;

use tauri::AppHandle;

use crate::commands::agent_jobs::AgentJobStatusDto;
use crate::managers::agent_jobs::{ExitCoordinator, JobManager, JobSnapshot};
use crate::managers::agent_tools::job_status::JobStatusSource;
use crate::managers::converse_platform::JobRuntime;
use crate::managers::job_announcer::Announcer;
use crate::managers::tts::TtsManager;

/// The single shared job objects the app registers as Tauri state.
pub struct JobWiring {
    /// The one job manager for the app.
    pub manager: Arc<JobManager>,
    /// The one low-priority announcer.
    pub announcer: Arc<Announcer>,
    /// The one idempotent exit coordinator for both quit paths.
    pub exit: Arc<ExitCoordinator>,
}

/// Builds the shared wiring and the runtime handed to the conversation platform.
pub fn build(app: &AppHandle, tts: Arc<TtsManager>) -> (JobWiring, JobRuntime) {
    todo!("S3d Phase B: build the one manager, announcer and exit coordinator")
}

/// Reports the shared manager's current or most recent snapshot to the `job_status` tool.
pub struct ManagerStatusSource(pub Arc<JobManager>);

impl JobStatusSource for ManagerStatusSource {
    fn status(&self) -> Option<JobSnapshot> {
        todo!("S3d Phase B: return the manager snapshot")
    }
}

/// True when the status DTO reports a running job (state is exactly `running`).
pub fn job_ui_running(dto: &AgentJobStatusDto) -> bool {
    todo!("S3d Phase B: state == \"running\"")
}

#[cfg(test)]
#[path = "job_runtime_tests.rs"]
mod job_runtime_tests;
