//! Tauri commands of the floating orb window and the orb settings (DESIGN.md Rev 5, 'Rust modules
//! and commands'). Window work runs in `async` commands so it never blocks the main thread. The
//! settings commands write through `write_settings_gated` and then apply the change at once.
//! Nothing here logs content or touches the filesystem.

use crate::chat_presentation::Presentation;
use crate::managers::chat_log::ChatLog;
use crate::orb_geometry::OrbSize;
use crate::orb_native;
use crate::settings::write_settings_gated;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::sync::Arc;
use tauri::{AppHandle, Manager};

/// Conversation states that drive their own face. Mirrors `ACTIVE_STATES` in `chatModel.ts`.
const ACTIVE_STATES: [&str; 4] = ["listening", "thinking", "speaking", "acting"];

/// End reasons that make the idle face look concerned. Mirrors `ERROR_ENDS` in `chatModel.ts`.
const ERROR_ENDS: [&str; 3] = ["not_configured", "audio_unavailable", "speech_stuck"];

/// The face the orb shows for a conversation state and end reason. Mirrors `avatarFace` in
/// `chatModel.ts`: an active state shows itself, any other state but idle shows idle, and idle
/// shows sleep, or concerned after an error end.
pub fn avatar_face(state: &str, end: Option<&str>) -> &'static str {
    if let Some(active) = ACTIVE_STATES.iter().copied().find(|s| *s == state) {
        return active;
    }
    if state != "idle" {
        return "idle";
    }
    match end {
        None | Some("") => "idle",
        Some(reason) if ERROR_ENDS.contains(&reason) => "concerned",
        Some(_) => "sleep",
    }
}

/// The orb's current face, as the orb page shows it.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct OrbState {
    /// Face name: `idle`, `sleep`, `concerned`, `listening`, `thinking`, `speaking` or `acting`.
    pub face: String,
}

/// The orb pressed: starts the gesture with id `id` (the page counts its presses).
#[tauri::command]
#[specta::specta]
pub async fn orb_pointer_down(app: AppHandle, id: u64) {
    orb_native::pointer_down(&app, id);
}

/// Keyboard activation of the orb: toggles the conversation surface.
#[tauri::command]
#[specta::specta]
pub async fn orb_activate(app: AppHandle) {
    orb_native::activate(&app);
}

/// Opens the orb's native context menu.
#[tauri::command]
#[specta::specta]
pub async fn orb_menu(app: AppHandle) {
    orb_native::show_menu(&app);
}

/// The face for the live conversation snapshot.
#[tauri::command]
#[specta::specta]
pub async fn orb_get_state(app: AppHandle) -> OrbState {
    let face = match app.try_state::<Arc<ChatLog>>() {
        Some(chat) => {
            let snapshot = chat.snapshot();
            avatar_face(&snapshot.state, snapshot.end.as_deref())
        }
        None => "idle",
    };
    OrbState {
        face: face.to_string(),
    }
}

/// Reports the page's reduced-motion preference. While on, the orb stops gaze and polling.
#[tauri::command]
#[specta::specta]
pub async fn orb_set_reduced_motion(app: AppHandle, reduced: bool) {
    orb_native::set_reduced_motion(&app, reduced);
}

/// Shows or hides the orb and stores the choice.
#[tauri::command]
#[specta::specta]
pub async fn change_orb_visible_setting(app: AppHandle, enabled: bool) {
    write_settings_gated(&app, |settings| settings.orb_visible = Some(enabled));
    orb_native::apply_visibility(&app);
}

/// Stores the orb size preset and resizes the orb at once.
#[tauri::command]
#[specta::specta]
pub async fn change_orb_size_setting(app: AppHandle, size: OrbSize) {
    write_settings_gated(&app, |settings| settings.orb_size = size);
    orb_native::apply_visibility(&app);
}

/// Stores the chat presentation choice. `None` returns to the platform default.
#[tauri::command]
#[specta::specta]
pub async fn change_chat_presentation_setting(app: AppHandle, value: Option<Presentation>) {
    write_settings_gated(&app, |settings| settings.chat_presentation = value);
    orb_native::apply_visibility(&app);
}

/// Forgets the saved orb position and moves the orb to the default corner.
#[tauri::command]
#[specta::specta]
pub async fn reset_orb_position(app: AppHandle) {
    write_settings_gated(&app, |settings| settings.orb_position = None);
    orb_native::apply_visibility(&app);
}

#[cfg(test)]
mod tests {
    use super::avatar_face;

    #[test]
    fn active_states_show_their_own_face_whatever_the_end() {
        for state in ["listening", "thinking", "speaking", "acting"] {
            assert_eq!(avatar_face(state, None), state);
            assert_eq!(avatar_face(state, Some("speech_stuck")), state);
        }
    }

    #[test]
    fn idle_shows_sleep_without_an_end_and_concerned_after_an_error_end() {
        assert_eq!(avatar_face("idle", None), "idle");
        assert_eq!(avatar_face("idle", Some("")), "idle");
        assert_eq!(avatar_face("idle", Some("user_stopped")), "sleep");
        assert_eq!(avatar_face("idle", Some("not_configured")), "concerned");
        assert_eq!(avatar_face("idle", Some("audio_unavailable")), "concerned");
        assert_eq!(avatar_face("idle", Some("speech_stuck")), "concerned");
    }

    #[test]
    fn any_other_state_shows_idle_even_with_an_error_end() {
        assert_eq!(avatar_face("starting", None), "idle");
        assert_eq!(avatar_face("finished", Some("speech_stuck")), "idle");
        assert_eq!(avatar_face("", Some("not_configured")), "idle");
    }
}
