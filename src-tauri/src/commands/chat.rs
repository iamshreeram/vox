//! Tauri commands and the event emitter of the chat window transcript (DESIGN-CHAT 2.1, 2.3).
//! The log itself lives in `managers::chat_log`; this file only adapts it to the app. Nothing
//! here writes text to disk or to the log output.

use crate::bubble_hub::{BubbleRole, HubEntry};
use crate::bubble_native;
use crate::chat_window::CHAT_WINDOW_LABEL;
use crate::managers::chat_log::{
    ChatEmitter, ChatEvent, ChatEventKind, ChatLog, ChatRole, ChatSnapshot, ClearError, Stamp,
    CHAT_EVENT,
};
use crate::settings;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

/// Emits every chat event to the conversation window only (label `chat`), as the
/// `converse-chat` Tauri event, so transcript text never reaches other webviews. Delivery
/// errors are ignored so the conversation never notices them. The window hydrates from the
/// snapshot when it opens, so events sent while it is closed can be dropped safely. The same
/// events also feed the bubble shell in Rust, which never sends transcript text to a webview
/// other than the bubble window.
pub struct TauriChatEmitter {
    /// The app whose conversation window receives the events.
    pub app: AppHandle,
}

/// The bubble role of a transcript speaker. System notes never become bubbles.
fn bubble_role(role: ChatRole) -> Option<BubbleRole> {
    match role {
        ChatRole::User => Some(BubbleRole::User),
        ChatRole::Assistant => Some(BubbleRole::Assistant),
        ChatRole::System => None,
    }
}

impl ChatEmitter for TauriChatEmitter {
    fn emit(&self, event: &ChatEvent) {
        let _ = self.app.emit_to(CHAT_WINDOW_LABEL, CHAT_EVENT, event);
        match &event.kind {
            ChatEventKind::Entry { entry } => {
                if let (Some(stamp), Some(role)) = (event.stamp, bubble_role(entry.role)) {
                    bubble_native::chat_entry(
                        &self.app,
                        HubEntry {
                            id: entry.id,
                            role,
                            text: entry.text.clone(),
                            stamp,
                        },
                    );
                }
            }
            ChatEventKind::Reset => bubble_native::chat_reset(&self.app),
            ChatEventKind::Clear => bubble_native::chat_clear(&self.app),
            ChatEventKind::State { .. } | ChatEventKind::Config { .. } => {}
        }
    }

    /// Speech marks only feed the bubble shell. Nothing is sent to the chat window.
    fn speech(&self, scope: u64, on: bool, stamp: Option<Stamp>) {
        match (on, stamp) {
            (true, Some(stamp)) => bubble_native::speech_begin(&self.app, scope, stamp),
            (true, None) => {}
            (false, _) => bubble_native::speech_end(&self.app, scope),
        }
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
    // After the chat log's gate has changed, so the bubbles judge lines by the new tuple (A9-1).
    bubble_native::recording_changed(&app, enabled);
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
    crate::chat_surface::toggle(&app).map_err(|err| err.to_string())
}

/// Hides the conversation window when the chat window asks for it (Escape, orb-click D1b). The
/// allow-list admits only the chat window; any other calling label is a no-op. Hiding an already
/// hidden window is harmless, so the command is idempotent.
#[tauri::command]
#[specta::specta]
pub fn chat_hide_window(app: AppHandle, window: tauri::WebviewWindow) -> Result<(), String> {
    if !crate::chat_window::hide_request_applies(window.label()) {
        return Ok(());
    }
    crate::chat_window::hide_existing(&app, &window);
    Ok(())
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
