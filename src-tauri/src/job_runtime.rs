//! App wiring for the background coding job (M3 S3d, DESIGN Rev 5 R2-8, R4-2, R5-1, D6, D7).
//!
//! Builds the one shared job manager, announcer and exit coordinator, and the status source
//! the `job_status` tool reads. Tests live in `job_runtime_tests.rs`.

use std::sync::Arc;

use tauri::{AppHandle, Emitter};

use crate::commands::agent_jobs::AgentJobStatusDto;
use crate::managers::agent_jobs::{ExitCoordinator, JobManager, JobSnapshot};
use crate::managers::agent_tools::job_status::JobStatusSource;
use crate::managers::converse_platform::{JobObserverFactory, JobRuntime};
use crate::managers::job_announcer::{Announcer, TtsLowPriorityMouth};
use crate::managers::job_speaker::{JobEventSink, JobSpeaker};
use crate::managers::tts::TtsManager;
use crate::settings::{clamp_job_timeout_minutes, get_settings};
use crate::tray;

/// The single shared job objects the app registers as Tauri state.
pub struct JobWiring {
    /// The one job manager for the app.
    pub manager: Arc<JobManager>,
    /// The one idempotent exit coordinator for both quit paths.
    pub exit: Arc<ExitCoordinator>,
}

/// Builds the shared wiring and the runtime handed to the conversation platform.
pub fn build(app: &AppHandle, tts: Arc<TtsManager>) -> (JobWiring, JobRuntime) {
    let manager = JobManager::new();
    let announcer = Arc::new(Announcer::new(TtsLowPriorityMouth::shared(tts)));
    let exit = ExitCoordinator::new();
    let observer_factory = observer_factory(app.clone(), announcer.clone());
    let runtime = JobRuntime {
        manager: manager.clone(),
        announcer,
        observer_factory,
    };
    let cancel_manager = manager.clone();
    tray::set_cancel_job_handler(Arc::new(move || {
        cancel_manager.cancel();
    }));
    let wiring = JobWiring { manager, exit };
    (wiring, runtime)
}

/// Emits job status to the window and refreshes the tray. Errors from emit are ignored:
/// a missing window must never stop a job from finishing.
struct AppEventSink {
    app: AppHandle,
}

impl JobEventSink for AppEventSink {
    fn emit(&self, dto: &AgentJobStatusDto) {
        // Runs before refresh_tray on every event, so the tray flag follows the job.
        tray::set_job_running(&self.app, job_ui_running(dto));
        let _ = self.app.emit("agent-job-update", dto.clone());
    }

    fn refresh_tray(&self) {
        tray::sync_tray(&self.app);
    }
}

/// Per-job observer factory: one `JobSpeaker` per job, reading the settings at each event.
fn observer_factory(app: AppHandle, announcer: Arc<Announcer>) -> JobObserverFactory {
    Arc::new(move |project: &str| {
        let settings_app = app.clone();
        let tts_enabled: Arc<dyn Fn() -> bool + Send + Sync> =
            Arc::new(move || get_settings(&settings_app).tts_enabled);
        let timeout = clamp_job_timeout_minutes(get_settings(&app).agent_jobs_timeout_minutes);
        let sink = Arc::new(AppEventSink { app: app.clone() });
        let speaker = JobSpeaker::new(project.to_string(), announcer.clone(), tts_enabled, sink)
            .with_timeout_minutes(timeout);
        Arc::new(speaker)
    })
}

/// Reports the shared manager's current or most recent snapshot to the `job_status` tool.
pub struct ManagerStatusSource(pub Arc<JobManager>);

impl JobStatusSource for ManagerStatusSource {
    fn status(&self) -> Option<JobSnapshot> {
        self.0.snapshot()
    }
}

/// True when the status DTO reports a running job (state is exactly `running`).
pub fn job_ui_running(dto: &AgentJobStatusDto) -> bool {
    dto.state == "running"
}

#[cfg(test)]
#[path = "job_runtime_tests.rs"]
mod job_runtime_tests;
