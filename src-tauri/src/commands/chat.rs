//! Tauri commands and the event emitter of the chat window transcript (DESIGN-CHAT 2.1, 2.3).
//! The log itself lives in `managers::chat_log`; this file only adapts it to the app. Nothing
//! here writes text to disk or to the log output.

use crate::chat_window::CHAT_WINDOW_LABEL;
use crate::managers::chat_log::{
    ChatEmitter, ChatEvent, ChatLog, ChatSnapshot, ClearError, CHAT_EVENT,
};
use crate::settings;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

/// Emits every chat event to the conversation window only (label `chat`), as the
/// `converse-chat` Tauri event, so transcript text never reaches other webviews. Delivery
/// errors are ignored so the conversation never notices them. The window hydrates from the
/// snapshot when it opens, so events sent while it is closed can be dropped safely.
pub struct TauriChatEmitter {
    /// The app whose conversation window receives the events.
    pub app: AppHandle,
}

impl ChatEmitter for TauriChatEmitter {
    fn emit(&self, event: &ChatEvent) {
        let _ = self.app.emit_to(CHAT_WINDOW_LABEL, CHAT_EVENT, event);
    }
}

/// Short fixed text for a failed Clear. Never includes transcript content.
fn clear_error_text(err: ClearError) -> &'static str {
    match err {
        ClearError::Busy => "The chat log is busy. Try again.",
        ClearError::Timeout => "The chat log did not clear in time.",
        ClearError::Dead => "The chat log is unavailable.",
    }
}

/// Authoritative snapshot of the transcript, state and recording flag (brief lock, no wait).
#[tauri::command]
#[specta::specta]
pub fn get_chat_snapshot(chat: State<'_, Arc<ChatLog>>) -> ChatSnapshot {
    chat.snapshot()
}

/// Empties the transcript. Runs on a blocking thread because the log waits up to two seconds
/// for its worker to apply the wipe.
#[tauri::command]
#[specta::specta]
pub async fn clear_chat_log(chat: State<'_, Arc<ChatLog>>) -> Result<(), String> {
    let log = chat.inner().clone();
    match tauri::async_runtime::spawn_blocking(move || log.clear()).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(clear_error_text(err).to_string()),
        Err(_) => Err(clear_error_text(ClearError::Dead).to_string()),
    }
}

/// Turns recording of the chat transcript on or off and persists the choice.
#[tauri::command]
#[specta::specta]
pub fn change_converse_chat_log_setting(app: AppHandle, enabled: bool) {
    let mut current = settings::get_settings(&app);
    current.converse_chat_log = enabled;
    settings::write_settings(&app, current);
    if let Some(chat) = app.try_state::<Arc<ChatLog>>() {
        chat.set_recording(enabled);
    }
}

/// Turns auto-opening of the conversation window at conversation start on or off and persists
/// the choice. Main window only (not on the chat allow-list).
#[tauri::command]
#[specta::specta]
pub fn change_converse_chat_open_on_start_setting(app: AppHandle, enabled: bool) {
    let mut current = settings::get_settings(&app);
    current.converse_chat_open_on_start = enabled;
    settings::write_settings(&app, current);
}

/// Shows the conversation window, or hides it when it is visible and focused. The settings
/// button calls this; the hotkey and the tray item use the same path.
#[tauri::command]
#[specta::specta]
pub async fn toggle_chat_window_command(app: AppHandle) -> Result<(), String> {
    crate::chat_window::toggle_chat_window(&app).map_err(|err| err.to_string())
}

/// Sends one typed message to the conversation (DESIGN-TYPE Rev 4). The chat window is the only
/// caller (the allow-list refuses it elsewhere). An idle conversation starts a text session; a
/// running one takes the message in its loop. The text is never logged or echoed: the error is
/// the stable code of the reason only.
#[tauri::command]
#[specta::specta]
pub fn send_chat_message(app: AppHandle, text: String) -> Result<(), String> {
    let controller = app.state::<Arc<crate::managers::converse_runtime::ConverseController>>();
    controller
        .send_typed(&text)
        .map_err(|err| err.code().to_string())
}
