//! Desktop approval UI for agent permission requests (DESIGN.md Revision 3, S4).
//!
//! The broker decides when a prompt is presented and closed. This module only turns those
//! calls into Tauri events and maps the UI answer back to the broker's single atomic
//! `resolve`. Titles, details and tokens are never logged.

use crate::managers::acp_agent::AcpAgentManager;
use crate::managers::acp_permission::PermissionMode;
use crate::managers::acp_permission::UserChoice;
use crate::managers::acp_permission::{OperationDetails, ToolKind};
use crate::managers::acp_permission_broker::{
    PermissionBroker, PermissionPrompter, PermissionView,
};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

/// Event emitted when a permission prompt is shown.
pub const EVENT_REQUEST: &str = "acp-permission-request";
/// Event emitted when a permission prompt is hidden, whatever the reason.
pub const EVENT_CLOSED: &str = "acp-permission-closed";
/// Event emitted (no payload) at most once per launch, when an agent run starts under
/// `AllowAll`, so the user sees that approvals are off.
pub const EVENT_ALLOW_ALL_ACTIVE: &str = "acp-allow-all-active";

/// Set once the `acp-allow-all-active` event has been emitted in this process.
static ALLOW_ALL_NOTICE_SENT: AtomicBool = AtomicBool::new(false);

/// Payload of [`EVENT_REQUEST`]. Title and details are untrusted text for display only.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequestPayload {
    /// Opaque prompt token used by the answer command.
    pub token: String,
    /// Sanitised title reported by the agent.
    pub title: String,
    /// Tool kind name, for example `edit` or `execute`.
    pub kind: &'static str,
    /// Bounded operation summary, or `None` when the agent gave none.
    pub details: Option<String>,
    /// Whether `details` was cut short.
    pub truncated: bool,
    /// Seconds this prompt waits before it is rejected.
    pub timeout_secs: u64,
    /// Requests still waiting behind this one.
    pub queued: usize,
}

/// Payload of [`EVENT_CLOSED`].
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PermissionClosedPayload {
    /// Token of the prompt that closed.
    pub token: String,
}

/// Maps a tool kind to the stable name used on the wire.
fn kind_name(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Delete => "delete",
        ToolKind::Move => "move",
        ToolKind::Search => "search",
        ToolKind::Execute => "execute",
        ToolKind::Think => "think",
        ToolKind::Fetch => "fetch",
        ToolKind::SwitchMode => "switch_mode",
        ToolKind::Other => "other",
    }
}

/// Builds the request event payload from a broker view and the configured timeout.
pub fn build_request_payload(view: &PermissionView, timeout_secs: u64) -> PermissionRequestPayload {
    let (details, truncated) = match &view.details {
        Some(OperationDetails { summary, truncated }) => (Some(summary.clone()), *truncated),
        None => (None, false),
    };
    PermissionRequestPayload {
        token: view.token.clone(),
        title: view.title.clone(),
        kind: kind_name(view.kind),
        details,
        truncated,
        timeout_secs,
        queued: view.queued,
    }
}

/// Builds the close event payload for the prompt `token`.
pub fn build_closed_payload(token: &str) -> PermissionClosedPayload {
    PermissionClosedPayload {
        token: token.to_string(),
    }
}

/// Applies the user's answer to the prompt `token`.
///
/// Allow selects an allow-once option and Deny selects a reject-once option, as the broker
/// decides. Returns `false` when the token is unknown, already answered or invalidated.
pub fn respond(broker: &PermissionBroker, token: &str, allow: bool) -> bool {
    let choice = if allow {
        UserChoice::Allow
    } else {
        UserChoice::Reject
    };
    broker.resolve(token, choice)
}

/// Shows approval prompts as Tauri events. It presents and hides only; answers come back
/// through [`respond`].
pub struct TauriPermissionPrompter {
    app: AppHandle,
    timeout_secs: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl TauriPermissionPrompter {
    /// Creates a prompter. `timeout_secs` is read for each prompt, so a settings change applies
    /// to the next request.
    pub fn new(app: AppHandle, timeout_secs: Arc<dyn Fn() -> u64 + Send + Sync>) -> Self {
        Self { app, timeout_secs }
    }
}

impl PermissionPrompter for TauriPermissionPrompter {
    fn present(&self, view: &PermissionView) {
        // The dialog countdown shows the real time left: the configured timeout, capped by what
        // remains of this request's deadline (queued requests may already have waited).
        let remaining_secs = view.remaining.as_secs();
        let payload = build_request_payload(view, (self.timeout_secs)().min(remaining_secs));
        // Vox lives in the tray, so the window may be hidden. Show and focus it so the
        // approval is visible before it times out. Window calls run on the main thread, and
        // failures are ignored: the prompt still waits for an answer or times out.
        let app = self.app.clone();
        if let Err(err) = self.app.run_on_main_thread(move || {
            crate::show_main_window(&app);
        }) {
            log::debug!("permission window show not scheduled: {err}");
        }
        if let Err(err) = self.app.emit(EVENT_REQUEST, payload) {
            log::debug!("permission prompt event not delivered: {err}");
        }
    }

    fn close(&self, token: &str) {
        if let Err(err) = self.app.emit(EVENT_CLOSED, build_closed_payload(token)) {
            log::debug!("permission close event not delivered: {err}");
        }
    }
}

/// Calls `emit` when `mode` is `AllowAll` and no notice was sent through `sent` yet. Returns
/// whether it emitted. `sent` and `emit` are parameters so tests need neither a static nor an
/// `AppHandle`.
pub fn notify_allow_all_once_with(
    sent: &AtomicBool,
    mode: PermissionMode,
    emit: impl FnOnce(),
) -> bool {
    if mode != PermissionMode::AllowAll {
        return false;
    }
    if sent.swap(true, Ordering::SeqCst) {
        return false;
    }
    emit();
    true
}

/// Tells the frontend, once per launch, that every agent action is allowed without asking.
/// Called right before an agent run starts. Does nothing for other modes.
pub fn notify_allow_all_once(app: &AppHandle, mode: PermissionMode) {
    notify_allow_all_once_with(&ALLOW_ALL_NOTICE_SENT, mode, || {
        if let Err(err) = app.emit(EVENT_ALLOW_ALL_ACTIVE, ()) {
            log::debug!("allow-all notice not delivered: {err}");
        }
    });
}

/// Returns the broker of the managed [`AcpAgentManager`], or `None` when permissions are not
/// configured.
pub fn broker_from_app(app: &AppHandle) -> Option<Arc<PermissionBroker>> {
    app.try_state::<Arc<AcpAgentManager>>()
        .and_then(|manager| manager.permission_broker())
}
