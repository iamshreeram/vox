//! Standalone conversation window (DESIGN-CHAT Rev 6: A3, A5, A8).
//!
//! The window is built lazily, starts hidden, and is only shown by an explicit user action
//! (hotkey, tray item, settings button or command). Closing it hides it so the in-memory
//! transcript survives and reopening is instant. Creation and show/hide run on the main thread.
//! Nothing here touches the filesystem or writes text to any log.

use crate::appearance;
use crate::chat_opacity::{page_snapshot, replay_lifecycle, CHAT_OPACITY, CHAT_OPACITY_REPLAY};
use crate::cli::CliArgs;
use crate::commands::chat_opacity as opacity_command;
use crate::settings;
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

/// Window label of the standalone conversation window.
pub const CHAT_WINDOW_LABEL: &str = "chat";

/// Bundled entry page of the conversation window (Vite input `chat`).
pub const CHAT_ENTRY_URL: &str = "src/chat/index.html";

/// Commands only the chat window may call (A8). Every other app command is refused from it.
/// `send_chat_message` is the typing command (DESIGN-TYPE Rev 4); it is refused everywhere else.
const CHAT_ALLOWED_COMMANDS: [&str; 3] =
    ["get_chat_snapshot", "clear_chat_log", "send_chat_message"];

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
pub(crate) fn is_app_navigation(url: &tauri::Url) -> bool {
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
        if let Err(e) = toggle_on_main_thread(&handle) {
            log::warn!("chat window toggle failed: {e}");
        }
    })
}

/// Logs a window operation that failed. Logs the operation name and the error only, never
/// window content.
fn log_window_result(op: &str, result: tauri::Result<()>) {
    if let Err(e) = result {
        log::warn!("chat window {op} failed: {e}");
    }
}

/// Main-thread body of `toggle_chat_window`. Logs one line with the window state and the
/// chosen action.
fn toggle_on_main_thread(app: &AppHandle) -> tauri::Result<()> {
    let existing = app.get_webview_window(CHAT_WINDOW_LABEL);
    let exists = existing.is_some();
    let visible = existing
        .as_ref()
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    let focused = existing
        .as_ref()
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    let minimized = existing
        .as_ref()
        .and_then(|w| w.is_minimized().ok())
        .unwrap_or(false);
    let action = toggle_decision(visible, focused);
    log::info!(
        "chat window toggle: exists={exists} visible={visible} focused={focused} minimized={minimized} action={action:?}"
    );
    match existing {
        Some(window) => match action {
            ToggleAction::Show => show_existing(app, &window),
            ToggleAction::Hide => hide_existing(app, &window),
        },
        None => {
            let window = build_chat_window(app)?;
            show_existing(app, &window);
        }
    }
    Ok(())
}

/// Builds the hidden chat window from the bundled entry page. Logs the build result.
fn build_chat_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    // The first opacity travels in the page URL, so the first paint is already right (A-T3).
    // The URL carries the effective value (100 where compositing is unavailable): the page never
    // corrects itself from a replay of the same revision. This runs on the main thread.
    let committed = CHAT_OPACITY.seed(i64::from(settings::get_settings(app).chat_opacity_pct));
    let initial = page_snapshot(committed, &crate::orb_native::current_env());
    let url = appearance::with_ap_param(
        &format!(
            "{CHAT_ENTRY_URL}?opacity={}&rev={}",
            initial.pct, initial.rev
        ),
        &appearance::current_snapshot(app),
    );
    let builder = WebviewWindowBuilder::new(app, CHAT_WINDOW_LABEL, WebviewUrl::App(url.into()))
        .title("Vox Conversation")
        .inner_size(480.0, 720.0)
        .min_inner_size(360.0, 420.0)
        .theme(Some(tauri::Theme::Dark))
        .resizable(true)
        .decorations(true)
        .transparent(true)
        .visible(false)
        .focused(false)
        .on_navigation(is_app_navigation)
        .on_new_window(|_url, _features| NewWindowResponse::Deny);
    #[cfg(target_os = "macos")]
    let builder = builder
        .shadow(false)
        .title_bar_style(tauri::TitleBarStyle::Transparent);
    let built = builder.build();
    let window = match built {
        Ok(window) => {
            log::info!("chat window built");
            window
        }
        Err(e) => {
            log::warn!("chat window build failed: {e}");
            return Err(e);
        }
    };

    let close_app = app.clone();
    let focus_app = app.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            if let Some(chat) = close_app.get_webview_window(CHAT_WINDOW_LABEL) {
                hide_existing(&close_app, &chat);
            }
        }
        // Gaining focus re-sends the opacity, so a page that missed an event converges (A-T4).
        WindowEvent::Focused(true) => {
            emit_current_opacity(&focus_app);
            appearance::emit_current(&focus_app);
        }
        _ => {}
    });
    start_replay_worker(app);
    Ok(window)
}

/// Re-sends the latest opacity snapshot to the conversation window. Runs on show and on focus.
pub fn emit_current_opacity(app: &AppHandle) {
    opacity_command::emit_snapshot(app, CHAT_OPACITY.snapshot());
}

/// Starts the single shared replay worker (A-T4, UI enterprise A2). It serves every consumer
/// window (chat, orb, bubbles): while any of them exists it re-emits the chat opacity and the
/// current appearance snapshot every replay interval, so a listener that subscribes late still
/// converges. The gate allows one worker at a time; the worker releases it when no consumer is
/// left. Called by every consumer build.
pub(crate) fn start_replay_worker(app: &AppHandle) {
    if !CHAT_OPACITY_REPLAY.try_start() {
        return;
    }
    let worker_app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("consumer-replay".to_string())
        .spawn(move || {
            // Replays until no consumer is left; a window built in the gap gets one worker.
            replay_lifecycle(
                &CHAT_OPACITY_REPLAY,
                || appearance::any_consumer_exists(&worker_app),
                || {
                    appearance::run_consumer_replay(
                        || appearance::any_consumer_exists(&worker_app),
                        std::thread::sleep,
                        || appearance::replay_tick(&worker_app),
                    );
                },
            );
        });
    if let Err(e) = spawned {
        CHAT_OPACITY_REPLAY.finish();
        log::warn!("chat opacity replay worker did not start: {e}");
    }
}

/// What opening the chat window on conversation start must do for the current window state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenPlan {
    Build,
    Show,
    Nothing,
}

/// Pure open-on-start decision (DESIGN-UI). A minimized window is shown (unminimized) even if
/// it still reports visible.
pub fn open_plan(exists: bool, visible: bool, minimized: bool) -> OpenPlan {
    if !exists {
        OpenPlan::Build
    } else if !visible || minimized {
        OpenPlan::Show
    } else {
        OpenPlan::Nothing
    }
}

/// Queues the open-on-start job and returns at once. Only the boxed closure is built here;
/// `eligible` (which reads the settings store) and `open` run later, inside the job.
pub fn queue_open_on_start<D, E, A>(dispatch: D, eligible: E, open: A)
where
    D: FnOnce(Box<dyn FnOnce() + Send + 'static>),
    E: FnOnce() -> bool + Send + 'static,
    A: FnOnce() + Send + 'static,
{
    let job: Box<dyn FnOnce() + Send + 'static> = Box::new(move || {
        if eligible() {
            open();
        }
    });
    dispatch(job);
}

/// Called from the conversation session-start hook. Queues the auto-open; the settings check
/// and the window work happen on the main thread (DESIGN-UI).
pub fn open_chat_window_on_session_start(app: &AppHandle) {
    let dispatch_app = app.clone();
    let eligible_app = app.clone();
    let open_app = app.clone();
    queue_open_on_start(
        move |job| {
            let _ = dispatch_app.run_on_main_thread(job);
        },
        move || settings::should_open_chat_on_start(&settings::get_settings(&eligible_app)),
        move || open_on_main_thread(&open_app),
    );
}

/// Main-thread body of the auto-open. Builds the window if needed, or shows it without
/// taking focus. Errors are logged and otherwise ignored: auto-open must never disturb the conversation.
fn open_on_main_thread(app: &AppHandle) {
    let existing = app.get_webview_window(CHAT_WINDOW_LABEL);
    let exists = existing.is_some();
    let visible = existing
        .as_ref()
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    let minimized = existing
        .as_ref()
        .and_then(|w| w.is_minimized().ok())
        .unwrap_or(false);
    match open_plan(exists, visible, minimized) {
        OpenPlan::Build => match build_chat_window(app) {
            Ok(window) => show_without_focus(app, &window),
            Err(e) => log::warn!("chat window auto-open skipped, build failed: {e}"),
        },
        OpenPlan::Show => {
            if let Some(window) = existing {
                show_without_focus(app, &window);
            }
        }
        OpenPlan::Nothing => {}
    }
}

/// Shows the window without activating the app: the foreground app keeps focus, so the user's
/// dictation or typing target is not disturbed.
fn show_without_focus(app: &AppHandle, window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
    }
    log_window_result("unminimize", window.unminimize());
    log_window_result("show", window.show());
    crate::bubble_native::classic_visibility_changed(app, true);
    emit_current_opacity(app);
    appearance::emit_current(app);
}

/// Shows and focuses the window and promotes the macOS activation policy so the window can
/// appear while the app is a tray-only accessory app (mirrors `show_main_window`). The policy
/// is set first, so the window is not shown while the app is still accessory.
fn show_existing(app: &AppHandle, window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        log_window_result(
            "activation policy",
            app.set_activation_policy(tauri::ActivationPolicy::Regular),
        );
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
    }
    log_window_result("unminimize", window.unminimize());
    log_window_result("show", window.show());
    log_window_result("focus", window.set_focus());
    crate::bubble_native::classic_visibility_changed(app, true);
    emit_current_opacity(app);
    appearance::emit_current(app);
}

/// Hides the window. Only after a successful hide, on macOS, when the main window is hidden
/// and the tray is available, the app returns to accessory mode (no Dock icon), as the main
/// window's close does.
fn hide_existing(app: &AppHandle, window: &WebviewWindow) {
    if let Err(e) = window.hide() {
        log::warn!("chat window hide failed: {e}");
        return;
    }
    crate::bubble_native::classic_visibility_changed(app, false);
    #[cfg(target_os = "macos")]
    {
        let main_visible = app
            .get_webview_window(MAIN_WINDOW_LABEL)
            .and_then(|main| main.is_visible().ok())
            .unwrap_or(false);
        let tray_visible =
            settings::get_settings(app).show_tray_icon && !app.state::<CliArgs>().no_tray;
        // The policy owner decides. This window has just been hidden, so it is not visible.
        let policy = crate::activation_policy::desired(&crate::activation_policy::Inputs {
            main_visible,
            classic_chat_visible: false,
            panel_needs_focus: false,
            tray_present: tray_visible,
        });
        if policy == crate::activation_policy::Policy::Accessory {
            log_window_result(
                "activation policy",
                app.set_activation_policy(tauri::ActivationPolicy::Accessory),
            );
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
