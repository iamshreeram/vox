//! Standalone conversation window (DESIGN-CHAT Rev 6: A3, A5, A8).
//!
//! The window is built lazily, starts hidden, and is only shown by an explicit user action
//! (hotkey, tray item, settings button or command). Closing it hides it so the in-memory
//! transcript survives and reopening is instant. Creation and show/hide run on the main thread.
//! Nothing here touches the filesystem or writes text to any log.

use crate::cli::CliArgs;
use crate::settings;
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

/// Window label of the standalone conversation window.
pub const CHAT_WINDOW_LABEL: &str = "chat";

/// Bundled entry page of the conversation window (Vite input `chat`).
pub const CHAT_ENTRY_URL: &str = "src/chat/index.html";

/// Commands only the chat window may call (A8). Every other app command is refused from it.
const CHAT_ALLOWED_COMMANDS: [&str; 2] = ["get_chat_snapshot", "clear_chat_log"];

/// The main window label, used to decide the macOS activation policy on hide.
const MAIN_WINDOW_LABEL: &str = "main";

/// What the chat-window toggle does for the current window state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleAction {
    Show,
    Hide,
}

/// Pure toggle decision: hide only when the window is visible AND focused; otherwise show
/// (which also brings a visible but unfocused window to the front).
pub fn toggle_decision(visible: bool, focused: bool) -> ToggleAction {
    if visible && focused {
        ToggleAction::Hide
    } else {
        ToggleAction::Show
    }
}

/// Command allow-list for the window that invoked a command (A8, single choke point).
///
/// The chat window may call exactly `get_chat_snapshot` and `clear_chat_log`. Those two
/// commands are refused from every other window. All other labels keep access as before.
pub fn chat_command_allowed(window_label: &str, command: &str) -> bool {
    let chat_only = CHAT_ALLOWED_COMMANDS.contains(&command);
    if window_label == CHAT_WINDOW_LABEL {
        return chat_only;
    }
    !chat_only
}

/// Dev server port. Must match `devUrl` in `tauri.conf.json`. Only debug builds accept it.
const DEV_SERVER_PORT: u16 = 1420;

/// Accepts only the app's own pages: the `tauri` scheme (production macOS and Linux), the
/// production `tauri.localhost` host (Windows and Android), and in debug builds the dev
/// server on `localhost:1420`. Any other URL is refused.
fn is_app_navigation(url: &tauri::Url) -> bool {
    match url.scheme() {
        "tauri" => true,
        "http" | "https" => {
            if url.host_str() == Some("tauri.localhost") {
                return true;
            }
            cfg!(debug_assertions)
                && url.host_str() == Some("localhost")
                && url.port() == Some(DEV_SERVER_PORT)
        }
        _ => false,
    }
}

/// Shows or hides the conversation window according to `toggle_decision`. The work runs on
/// the main thread (window creation from another thread can deadlock on Windows).
pub fn toggle_chat_window(app: &AppHandle) -> tauri::Result<()> {
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let _ = toggle_on_main_thread(&handle);
    })
}

/// Main-thread body of `toggle_chat_window`.
fn toggle_on_main_thread(app: &AppHandle) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window(CHAT_WINDOW_LABEL) else {
        let window = build_chat_window(app)?;
        show_existing(app, &window);
        return Ok(());
    };
    let visible = window.is_visible().unwrap_or(false);
    let focused = window.is_focused().unwrap_or(false);
    match toggle_decision(visible, focused) {
        ToggleAction::Show => show_existing(app, &window),
        ToggleAction::Hide => hide_existing(app, &window),
    }
    Ok(())
}

/// Builds the hidden chat window from the bundled entry page.
fn build_chat_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let window = WebviewWindowBuilder::new(
        app,
        CHAT_WINDOW_LABEL,
        WebviewUrl::App(CHAT_ENTRY_URL.into()),
    )
    .title("Vox Conversation")
    .inner_size(440.0, 680.0)
    .min_inner_size(320.0, 360.0)
    .resizable(true)
    .decorations(true)
    .visible(false)
    .focused(false)
    .on_navigation(is_app_navigation)
    .on_new_window(|_url, _features| NewWindowResponse::Deny)
    .build()?;

    let close_app = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            if let Some(chat) = close_app.get_webview_window(CHAT_WINDOW_LABEL) {
                let _ = chat.hide();
            }
        }
    });
    Ok(window)
}

/// Shows and focuses the window and promotes the macOS activation policy so the window can
/// appear while the app is a tray-only accessory app (mirrors `show_main_window`).
fn show_existing(app: &AppHandle, window: &WebviewWindow) {
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
    #[cfg(target_os = "macos")]
    {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
    }
}

/// Hides the window. On macOS, when the main window is hidden and the tray is available,
/// the app returns to accessory mode (no Dock icon), as the main window's close does.
fn hide_existing(app: &AppHandle, window: &WebviewWindow) {
    let _ = window.hide();
    #[cfg(target_os = "macos")]
    {
        let main_visible = app
            .get_webview_window(MAIN_WINDOW_LABEL)
            .and_then(|main| main.is_visible().ok())
            .unwrap_or(false);
        let tray_visible =
            settings::get_settings(app).show_tray_icon && !app.state::<CliArgs>().no_tray;
        if !main_visible && tray_visible {
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
    }
}

#[cfg(test)]
#[path = "chat_window_tests.rs"]
mod chat_window_tests;
