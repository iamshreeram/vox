//! Standalone conversation window (DESIGN-CHAT Rev 6: A3, A5, A8). Stub: signatures only, bodies
//! are `unimplemented!()` until the implementation step lands. `chat_window_tests.rs` is the
//! authoritative spec.
#![allow(dead_code)]

/// Window label of the standalone conversation window.
pub const CHAT_WINDOW_LABEL: &str = "chat";

/// Bundled entry page of the conversation window (Vite input `chat`).
pub const CHAT_ENTRY_URL: &str = "src/chat/index.html";

/// What the chat-window toggle does for the current window state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleAction {
    Show,
    Hide,
}

/// Pure toggle decision: hide only when the window is visible AND focused; otherwise show
/// (which also brings a visible but unfocused window to the front).
pub fn toggle_decision(visible: bool, focused: bool) -> ToggleAction {
    let _ = (visible, focused);
    unimplemented!("toggle_decision: not implemented yet (red spec)")
}

/// Command allow-list for the window that invoked a command (A8, single choke point).
pub fn chat_command_allowed(window_label: &str, command: &str) -> bool {
    let _ = (window_label, command);
    unimplemented!("chat_command_allowed: not implemented yet (red spec)")
}

#[cfg(test)]
#[path = "chat_window_tests.rs"]
mod chat_window_tests;
