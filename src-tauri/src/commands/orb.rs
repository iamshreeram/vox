//! Tauri commands of the floating orb window and the orb settings (DESIGN.md Rev 5, 'Rust modules
//! and commands'). Window work runs in `async` commands so it never blocks the main thread. The
//! settings commands write through `write_settings_gated` and then apply the change at once.
//! Nothing here logs content or touches the filesystem.

use crate::bubble_shell_core::{session_surface, SessionSurface};
use crate::chat_presentation::{
    current_os, orb_support_for, LinuxBackend, OrbSupport, PlatformEnv, Presentation,
};
use crate::managers::chat_log::ChatLog;
use crate::orb_geometry::{preset_px, OrbSize};
use crate::orb_native;
use crate::settings::write_settings_gated;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// Conversation states that drive their own face. Mirrors `ACTIVE_STATES` in `chatModel.ts`.
const ACTIVE_STATES: [&str; 4] = ["listening", "thinking", "speaking", "acting"];

/// End reasons that make the idle face look concerned. Mirrors `ERROR_ENDS` in `chatModel.ts`.
const ERROR_ENDS: [&str; 3] = ["not_configured", "audio_unavailable", "speech_stuck"];

/// The face the orb shows for a conversation state and end reason. Mirrors `avatarFace` in
/// `chatModel.ts`: an active state shows itself, any other state but idle shows idle, and
/// idle with no end shows idle, idle after a normal end shows sleep, and idle after an error end
/// shows concerned.
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

/// Keyboard activation of the orb: starts or stops a voice conversation.
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

/// Whether the floating orb can be used on this system. Reads the platform the same way the orb
/// does: on Linux the GDK display is read on the main thread, where GTK is initialised. If that
/// read cannot be made, Linux is reported with an unidentified backend.
#[tauri::command]
#[specta::specta]
pub async fn get_orb_support(app: AppHandle) -> OrbSupport {
    let (tx, rx) = std::sync::mpsc::channel::<PlatformEnv>();
    let scheduled = app.run_on_main_thread(move || {
        let _ = tx.send(orb_native::current_env());
    });
    let read = match scheduled {
        Ok(()) => tauri::async_runtime::spawn_blocking(move || {
            rx.recv_timeout(Duration::from_secs(2)).ok()
        })
        .await
        .ok()
        .flatten(),
        Err(_) => None,
    };
    let env = read.unwrap_or(PlatformEnv {
        os: current_os(),
        linux_backend: LinuxBackend::Unknown,
        composited: false,
    });
    orb_support_for(&env)
}

/// Shows or hides the orb and stores the choice.
#[tauri::command]
#[specta::specta]
pub async fn change_orb_visible_setting(app: AppHandle, enabled: bool) {
    write_settings_gated(&app, |settings| settings.orb_visible = Some(enabled));
    orb_native::apply_visibility(&app);
}

/// Stores the free orb size in logical pixels (clamped to 48..=192; `None` returns to the
/// preset) and resizes the orb at once. Main window only.
#[tauri::command]
#[specta::specta]
pub async fn change_orb_size_px_setting(app: AppHandle, px: Option<u16>) {
    orb_native::set_orb_size(&app, px);
}

/// Stores the orb size preset and resizes the orb at once. Kept for older callers; the preset is
/// written through the same setter as the free size.
#[tauri::command]
#[specta::specta]
pub async fn change_orb_size_setting(app: AppHandle, size: OrbSize) {
    orb_native::set_orb_size(&app, Some(preset_px(size)));
}

/// Stores the chat presentation choice. `None` returns to the platform default.
#[tauri::command]
#[specta::specta]
pub async fn change_chat_presentation_setting(app: AppHandle, value: Option<Presentation>) {
    write_settings_gated(&app, |settings| settings.chat_presentation = value);
    // Switching to the classic window destroys the bubble window (A7-2).
    if session_surface(value, &orb_native::current_env()) == SessionSurface::Window {
        crate::bubble_native::destroy_bubbles(&app);
    }
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
