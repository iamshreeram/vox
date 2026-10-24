//! Tauri command for the agent permission approval dialog (DESIGN.md Revision 3, S4).

use crate::managers::acp_permission::PermissionMode;
use crate::managers::acp_permission_ui::{broker_from_app, respond};
use crate::settings::{clamp_permission_timeout_secs, get_settings, write_settings, AppSettings};
use tauri::AppHandle;

/// Answers the approval prompt `token`. `allow` selects an allow-once option, otherwise the
/// request is rejected. Returns `false` when there is no broker or the token is not pending
/// (unknown, already answered or invalidated).
#[tauri::command]
#[specta::specta]
pub fn acp_permission_respond(app: AppHandle, token: String, allow: bool) -> bool {
    broker_from_app(&app).is_some_and(|broker| respond(&broker, &token, allow))
}

/// Stores the approval policy for agent permission requests. The permission manager reads
/// the settings on every request, so the change applies to the next request without a restart.
#[tauri::command]
#[specta::specta]
pub fn change_acp_permission_mode_setting(
    app: AppHandle,
    mode: PermissionMode,
) -> Result<(), String> {
    let mut settings = get_settings(&app);
    apply_acp_permission_mode(&mut settings, mode);
    write_settings(&app, settings);
    Ok(())
}

/// Stores the approval timeout in seconds, clamped to the accepted range. Unlike the agent turn
/// timeout, an out-of-range value is clamped rather than rejected.
#[tauri::command]
#[specta::specta]
pub fn change_acp_permission_timeout_secs_setting(app: AppHandle, secs: u64) -> Result<(), String> {
    let mut settings = get_settings(&app);
    apply_acp_permission_timeout(&mut settings, secs);
    write_settings(&app, settings);
    Ok(())
}

/// Sets the approval policy on `settings`. Pure, so the stored value can be tested without Tauri.
pub fn apply_acp_permission_mode(settings: &mut AppSettings, mode: PermissionMode) {
    settings.acp_permission_mode = mode;
}

/// Sets the approval timeout on `settings`, clamped to the accepted range. Returns nothing;
/// read `settings.acp_permission_timeout_secs` for the stored value.
pub fn apply_acp_permission_timeout(settings: &mut AppSettings, secs: u64) {
    settings.acp_permission_timeout_secs = clamp_permission_timeout_secs(secs);
}
