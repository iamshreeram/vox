//! Tauri-backed ports for the read-only agent tools (DESIGN D4, R4-8): the settings
//! navigation port and the recent-conversation source. Nothing here changes any data.

use std::sync::Arc;

use chrono::TimeZone;
use tauri::{AppHandle, Emitter, Manager};

use crate::job_runtime::{JobWiring, ManagerStatusSource};
use crate::managers::agent_tools::project::ProjectResolver;
use crate::managers::agent_tools::tools::{
    ConversationSource, ConversationSummary, DefaultToolExecutor, DEFAULT_TOOL_TIMEOUT,
};
use crate::managers::agent_tools::ui_port::{UiPort, OPEN_SETTINGS_SECTIONS};
use crate::managers::conversation_store::{ConversationRow, ConversationStore};
use crate::managers::tool_loop::ToolExecutor;
use crate::settings::AppSettings;

/// Tauri event that asks the settings window to show one section. Payload: one section id.
pub const OPEN_SETTINGS_EVENT: &str = "agent-open-settings";

/// Navigation port over the main window. Validates the section again (defense in depth).
pub struct TauriUiPort {
    app: AppHandle,
}

impl TauriUiPort {
    /// Wraps the app handle used to show the window and emit the event.
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl UiPort for TauriUiPort {
    fn open_settings_section(&self, section: &str) {
        if !OPEN_SETTINGS_SECTIONS.contains(&section) {
            return;
        }
        let app = self.app.clone();
        let section = section.to_string();
        // Window calls run on the main thread, like the tray menu does.
        let queued = self.app.run_on_main_thread(move || {
            crate::show_main_window(&app);
            if app.emit(OPEN_SETTINGS_EVENT, section).is_err() {
                log::debug!("agent tools: settings navigation event not delivered");
            }
        });
        if queued.is_err() {
            log::debug!("agent tools: settings navigation not scheduled");
        }
    }
}

/// Recent conversation summaries over the managed conversation store (read-only).
pub struct StoreConversations {
    app: AppHandle,
}

impl StoreConversations {
    /// Holds the app handle and looks the store up on each call.
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl ConversationSource for StoreConversations {
    fn recent(&self, limit: usize) -> Result<Vec<ConversationSummary>, String> {
        let store = self
            .app
            .try_state::<Arc<ConversationStore>>()
            .ok_or_else(|| "conversation records are not available".to_string())?;
        let rows = store
            .list(limit)
            .map_err(|_| "conversation records could not be read".to_string())?;
        Ok(rows.into_iter().map(summary_of_row).collect())
    }
}

/// Keeps only the date, a turn count and the stored summary; never transcripts.
fn summary_of_row(row: ConversationRow) -> ConversationSummary {
    ConversationSummary {
        date: local_date(row.started_at),
        title: format!("{} turns", row.turns),
        summary: row.summary,
    }
}

/// `YYYY-MM-DD` in local time for a millisecond timestamp; empty when out of range.
fn local_date(ms: i64) -> String {
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// Builds the default read-only executor for one turn from the current settings and the
/// Tauri-backed ports (DESIGN D4). Recent conversations are offered only when records are on.
/// The `job_status` tool is offered only when coding jobs are enabled (DESIGN D7).
pub fn build_tauri_executor(app: &AppHandle, settings: &AppSettings) -> Arc<dyn ToolExecutor> {
    let resolver = ProjectResolver::new(&settings.agent_project_roots);
    let conversations: Arc<dyn ConversationSource> = Arc::new(StoreConversations::new(app.clone()));
    let executor = DefaultToolExecutor::with_backends(
        resolver,
        Box::new(TauriUiPort::new(app.clone())),
        settings.conversation_records_enabled,
        Some(conversations),
        DEFAULT_TOOL_TIMEOUT,
    );
    let executor = match (settings.agent_jobs_enabled, app.try_state::<JobWiring>()) {
        (true, Some(wiring)) => {
            executor.with_job_status(Arc::new(ManagerStatusSource(wiring.manager.clone())))
        }
        _ => executor,
    };
    Arc::new(executor)
}
