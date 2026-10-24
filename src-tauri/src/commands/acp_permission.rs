//! Tauri command for the agent permission approval dialog (DESIGN.md Revision 3, S4).

use crate::managers::acp_permission_ui::{broker_from_app, respond};
use tauri::AppHandle;

/// Answers the approval prompt `token`. `allow` selects an allow-once option, otherwise the
/// request is rejected. Returns `false` when there is no broker or the token is not pending
/// (unknown, already answered or invalidated).
#[tauri::command]
#[specta::specta]
pub fn acp_permission_respond(app: AppHandle, token: String, allow: bool) -> bool {
    broker_from_app(&app).is_some_and(|broker| respond(&broker, &token, allow))
}
