//! Command that saves the opacity of the see-through conversation window (Track T, DESIGN
//! Revision 3). Main window only: the chat window may not call it (window_commands matrix).

use crate::chat_opacity::{OpacitySnapshot, CHAT_OPACITY, CHAT_OPACITY_EVENT, CHAT_OPACITY_TARGET};
use crate::chat_window::CHAT_WINDOW_LABEL;
use crate::orb_native;
use crate::settings::write_settings_gated;
use tauri::{AppHandle, Emitter, Manager};

/// Saves the conversation window opacity (clamped to 20..=100), bumps the revision and pushes
/// the new value to the conversation window at once. An absent or hidden window is not an error:
/// the value is saved either way and the window reads it when it next shows.
#[tauri::command]
#[specta::specta]
pub async fn change_chat_opacity_setting(app: AppHandle, pct: i32) {
    let clamped = crate::chat_opacity::clamp_chat_opacity_pct(i64::from(pct));
    write_settings_gated(&app, |settings| settings.chat_opacity_pct = clamped);
    let snapshot = CHAT_OPACITY.commit(i64::from(clamped));
    emit_snapshot(&app, snapshot);
}

/// Schedules one emit of `snapshot` to the conversation window on the main thread, where the
/// platform facts are read. Used by the command, the replay worker and the window show path.
pub fn emit_snapshot(app: &AppHandle, snapshot: OpacitySnapshot) {
    let job_app = app.clone();
    if let Err(e) = app.run_on_main_thread(move || deliver(&job_app, snapshot)) {
        log::warn!("chat opacity could not be scheduled: {e}");
    }
}

/// Main-thread body of `emit_snapshot`. Logs nothing about the value, only failures.
fn deliver(app: &AppHandle, snapshot: OpacitySnapshot) {
    let exists = app.get_webview_window(CHAT_WINDOW_LABEL).is_some();
    let env = orb_native::current_env();
    if let Some((_, payload)) = crate::chat_opacity::plan_emit(exists, snapshot, &env) {
        if let Err(e) = app.emit_to(CHAT_OPACITY_TARGET, CHAT_OPACITY_EVENT, payload) {
            log::warn!("chat opacity emit failed: {e}");
        }
    }
}
