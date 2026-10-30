//! macOS-only native chrome of the conversation window (orb-click D1, A-C5).
//!
//! The window keeps its native frame (resize edges, shadow handling, drag through the title-bar
//! strip). This module only hides the close, miniaturize and zoom buttons. It is idempotent, runs
//! on the main thread, changes no other window property and logs only fixed reasons.

use tauri::{Manager, WebviewWindow};

/// Hides the three standard window buttons of the chat window. The work runs on the main thread
/// and never blocks; a failure is logged with a fixed reason and otherwise ignored.
pub fn hide_window_buttons(window: &WebviewWindow) {
    let target = window.clone();
    let dispatched = window
        .app_handle()
        .run_on_main_thread(move || hide_on_main_thread(&target));
    if let Err(e) = dispatched {
        log::warn!("chat window buttons not hidden: {e}");
    }
}

/// Main-thread body: looks up the native window and hides each standard button.
fn hide_on_main_thread(window: &WebviewWindow) {
    let raw = match window.ns_window() {
        Ok(raw) => raw,
        Err(_) => {
            log::warn!("chat window buttons not hidden: no native window");
            return;
        }
    };
    if raw.is_null() {
        log::warn!("chat window buttons not hidden: native window is null");
        return;
    }
    // SAFETY: `ns_window` returns the NSWindow that backs this webview window; it stays alive while
    // the window exists, and this function runs on the main thread, the only thread AppKit allows.
    let ns_window: &objc2_app_kit::NSWindow = unsafe { &*raw.cast::<objc2_app_kit::NSWindow>() };
    let buttons = [
        objc2_app_kit::NSWindowButton::CloseButton,
        objc2_app_kit::NSWindowButton::MiniaturizeButton,
        objc2_app_kit::NSWindowButton::ZoomButton,
    ];
    for kind in buttons {
        match ns_window.standardWindowButton(kind) {
            Some(button) => button.setHidden(true),
            None => log::warn!("chat window button not found; not hidden"),
        }
    }
}
