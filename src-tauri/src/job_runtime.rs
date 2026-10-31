//! App wiring for the background coding job (M3 S3d, DESIGN Rev 5 R2-8, R4-2, R5-1, D6, D7).
//!
//! Builds the one shared job manager, announcer and exit coordinator, and the status source
//! the `job_status` tool reads. Tests live in `job_runtime_tests.rs`.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use tauri::{AppHandle, Emitter};

use crate::commands::agent_jobs::AgentJobStatusDto;
use crate::managers::agent_jobs::{ExitCoordinator, JobHooks, JobManager, JobSnapshot};
use crate::managers::agent_tools::job_status::JobStatusSource;
use crate::managers::converse_platform::{JobObserverFactory, JobRuntime};
use crate::managers::job_announcer::{Announcer, TtsLowPriorityMouth};
use crate::managers::job_speaker::{idle_dto, snapshot_to_dto, JobEventSink, JobSpeaker};
use crate::managers::job_summary::{
    summary_system_prompt, summary_user_prompt, JobSummaryContext, ResultStore, ResultSummarizer,
    SummaryError, SummaryHandle, SummaryPolicy, SummaryTokens, SUMMARY_DEADLINE,
};
use crate::managers::speech_backends;
use crate::managers::tts::TtsManager;
use crate::settings::{clamp_job_timeout_minutes, get_settings, AppSettings};
use crate::tray;

/// The single shared job objects the app registers as Tauri state.
pub struct JobWiring {
    /// The one job manager for the app.
    pub manager: Arc<JobManager>,
    /// The one idempotent exit coordinator for both quit paths.
    pub exit: Arc<ExitCoordinator>,
    /// The one summary token state, shared with the manager hooks and the speakers.
    pub summary_tokens: Arc<SummaryTokens>,
    /// The latest job result text, read by the status command.
    pub results: Arc<ResultStore>,
}

/// Builds the shared wiring and the runtime handed to the conversation platform.
pub fn build(app: &AppHandle, tts: Arc<TtsManager>) -> (JobWiring, JobRuntime) {
    let manager = JobManager::new();
    let announcer = Arc::new(Announcer::new(TtsLowPriorityMouth::shared(tts)));
    let exit = ExitCoordinator::new();
    let summary_tokens = SummaryTokens::new();
    let results = ResultStore::new();
    install_summary_hooks(&manager, Arc::clone(&summary_tokens));
    let summarizer: Arc<dyn ResultSummarizer> =
        Arc::new(GatewaySummarizer::new(production_complete(app.clone())));
    let observer_factory = observer_factory(
        app.clone(),
        announcer.clone(),
        summarizer,
        results.clone(),
        summary_tokens.clone(),
    );
    let runtime = JobRuntime {
        manager: manager.clone(),
        announcer,
        observer_factory,
    };
    let cancel_manager = manager.clone();
    tray::set_cancel_job_handler(Arc::new(move || {
        cancel_manager.cancel();
    }));
    let wiring = JobWiring {
        manager,
        exit,
        summary_tokens,
        results,
    };
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
fn observer_factory(
    app: AppHandle,
    announcer: Arc<Announcer>,
    summarizer: Arc<dyn ResultSummarizer>,
    results: Arc<ResultStore>,
    tokens: Arc<SummaryTokens>,
) -> JobObserverFactory {
    Arc::new(move |project: &str| {
        let settings_app = app.clone();
        let tts_enabled: Arc<dyn Fn() -> bool + Send + Sync> =
            Arc::new(move || get_settings(&settings_app).tts_enabled);
        let timeout = clamp_job_timeout_minutes(get_settings(&app).agent_jobs_timeout_minutes);
        let sink = Arc::new(AppEventSink { app: app.clone() });
        let policy = summary_policy(settings_getter(app.clone()));
        let speaker = JobSpeaker::new(project.to_string(), announcer.clone(), tts_enabled, sink)
            .with_timeout_minutes(timeout)
            .with_summary(summarizer.clone(), results.clone(), tokens.clone(), policy);
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

/// Budgets for the `RunEvent::Exit` fallback shutdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitFallbackBudgets {
    /// Time between SIGTERM and SIGKILL.
    pub term_grace: Duration,
    /// Output drain budget after the job stops.
    pub drain: Duration,
    /// Overall deadline. Always at least `term_grace` plus the margin, so the SIGKILL
    /// is sent before the fallback gives up and a child cannot outlive a restart.
    pub deadline: Duration,
}

/// Margin added to the grace so the SIGKILL step always runs before the deadline.
pub const EXIT_FALLBACK_MARGIN: Duration = Duration::from_millis(500);

/// Pure budgets for the `RunEvent::Exit` fallback (S5 fix, updater restarts).
pub fn exit_fallback_budgets() -> ExitFallbackBudgets {
    let term_grace = Duration::from_secs(1);
    ExitFallbackBudgets {
        term_grace,
        drain: Duration::from_secs(1),
        deadline: term_grace + EXIT_FALLBACK_MARGIN,
    }
}

/// M4: a completion call that takes (system prompt, user prompt) and returns the answer text.
pub type CompleteFn = Arc<dyn Fn(&str, &str) -> Result<String, ()> + Send + Sync>;

/// M4: production summarizer that runs one gateway completion on a detached worker.
pub struct GatewaySummarizer {
    complete: CompleteFn,
}

impl GatewaySummarizer {
    /// Builds the summarizer over a completion call.
    pub fn new(complete: CompleteFn) -> Self {
        Self { complete }
    }
}

impl ResultSummarizer for GatewaySummarizer {
    fn submit(&self, text: &str, ctx: &JobSummaryContext) -> SummaryHandle {
        let (tx, rx) = mpsc::channel();
        let complete = Arc::clone(&self.complete);
        let system = summary_system_prompt(ctx);
        let user = summary_user_prompt(text, ctx);
        // If the thread cannot start, the closure (and `tx`) is dropped, so the waiter sees a
        // disconnected channel and reports `Failed`.
        let _ = std::thread::Builder::new()
            .name("ask-summary".to_string())
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(|| complete(&system, &user)));
                let answer = match outcome {
                    Ok(Ok(text)) => Ok(text),
                    _ => Err(SummaryError::Failed),
                };
                // The waiter may have gone away; a dropped answer is harmless.
                let _ = tx.send(answer);
            });
        SummaryHandle::new(rx)
    }
}

/// M4: the production completion: the current settings, the shared summary route and the
/// summary deadline. Runs on the summary worker thread, never on an async runtime thread.
fn production_complete(app: AppHandle) -> CompleteFn {
    Arc::new(move |system: &str, user: &str| {
        let settings = get_settings(&app);
        tauri::async_runtime::block_on(speech_backends::complete_summary(
            &settings,
            system,
            user,
            SUMMARY_MAX_TOKENS,
            SUMMARY_DEADLINE,
        ))
        .map_err(|_| ())
    })
}

/// Output budget of one summary answer, in tokens.
const SUMMARY_MAX_TOKENS: u32 = 200;

/// A settings getter that reads the live settings through the app handle on every call.
fn settings_getter(app: AppHandle) -> Arc<dyn Fn() -> AppSettings + Send + Sync> {
    Arc::new(move || get_settings(&app))
}

/// M4: installs the admission and close hooks that feed the summary tokens.
/// Replaces any hooks installed before. The hooks never call back into the manager.
pub fn install_summary_hooks(manager: &JobManager, tokens: Arc<SummaryTokens>) {
    let admit_tokens = Arc::clone(&tokens);
    manager.set_hooks(JobHooks {
        on_admit: Arc::new(move |id| admit_tokens.admit(id)),
        on_close: Arc::new(move || tokens.close()),
    });
}

/// M4: live ask and TTS switches read from the settings getter on every call.
pub fn summary_policy(settings: Arc<dyn Fn() -> AppSettings + Send + Sync>) -> SummaryPolicy {
    let ask_settings = Arc::clone(&settings);
    SummaryPolicy {
        ask_enabled: Arc::new(move || ask_settings().agent_jobs_ask_enabled),
        tts_enabled: Arc::new(move || settings().tts_enabled),
    }
}

/// M4: the job status DTO with the stored result for the current or most recent job.
/// An idle manager reports `idle` with no result.
pub fn status_dto_with(manager: &JobManager, results: &ResultStore) -> AgentJobStatusDto {
    match manager.snapshot() {
        Some(snap) => {
            let mut dto = snapshot_to_dto(&snap);
            dto.result = results.get_for(snap.id);
            dto
        }
        None => idle_dto(),
    }
}

/// M4: invalidates every outstanding summary ticket. `None` is a no-op.
pub fn invalidate_summaries(tokens: Option<&Arc<SummaryTokens>>) {
    if let Some(tokens) = tokens {
        tokens.invalidate();
    }
}

#[cfg(test)]
#[path = "job_runtime_tests.rs"]
mod job_runtime_tests;

#[cfg(test)]
#[path = "job_runtime_m4_tests.rs"]
mod job_runtime_m4_tests;
