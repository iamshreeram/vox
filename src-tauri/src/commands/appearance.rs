//! Commands that save the accent, the conversation window theme and the visual style (UI
//! enterprise S1, DESIGN Revision 3, contract 3). Main window only (`window_commands`). Each one
//! goes through `appearance::commit_appearance`, so it writes once and publishes once.

use crate::appearance::commit_appearance;
use crate::settings::{Accent, ChatTheme, VisualStyle};
use tauri::AppHandle;

/// Saves the accent and publishes the committed snapshot to every consumer window.
#[tauri::command]
#[specta::specta]
pub async fn change_accent_setting(app: AppHandle, accent: Accent) {
    commit_appearance(&app, |fields| fields.accent = accent);
}

/// Saves the conversation window theme and publishes the committed snapshot.
#[tauri::command]
#[specta::specta]
pub async fn change_chat_theme_setting(app: AppHandle, chat_theme: ChatTheme) {
    commit_appearance(&app, |fields| fields.chat_theme = chat_theme);
}

/// Saves the visual style and publishes the committed snapshot.
#[tauri::command]
#[specta::specta]
pub async fn change_visual_style_setting(app: AppHandle, visual_style: VisualStyle) {
    commit_appearance(&app, |fields| fields.visual_style = visual_style);
}
