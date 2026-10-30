//! Command matrix for the orb and chat windows (DESIGN.md Rev 4, 'Rust modules and commands').
//! Each window may call only its own commands. Every other window may call anything except the
//! orb and chat-panel commands. The invoke handler in `lib.rs` uses
//! `window_command_allowed` as the single gate; `chat_window::chat_command_allowed` remains for
//! the classic chat tests.
#![cfg_attr(not(test), allow(dead_code))]

/// Label of the floating orb window.
pub const ORB_LABEL: &str = "orb";

/// Label of the classic chat window.
pub const CHAT_LABEL: &str = "chat";

/// Label of the main window.
pub const MAIN_LABEL: &str = "main";

/// Label of the bubble window that shows the transcript beside the orb (DESIGN A7-2).
pub const BUBBLES_WINDOW_LABEL: &str = "orb-bubbles";

/// Commands the bubble window may invoke. Only its readiness handshake; it has no other command.
pub const BUBBLES_COMMANDS: [&str; 1] = ["bubbles_ready"];

/// Commands the orb window may invoke.
pub const ORB_COMMANDS: [&str; 5] = [
    "orb_pointer_down",
    "orb_activate",
    "orb_menu",
    "orb_get_state",
    "orb_set_reduced_motion",
];

/// Commands the chat window (classic or panel) may invoke.
pub const CHAT_COMMANDS: [&str; 10] = [
    "get_chat_snapshot",
    "clear_chat_log",
    "send_chat_message",
    "chat_panel_ready",
    "chat_panel_resize",
    "chat_panel_close",
    "chat_draft_set",
    "chat_draft_get",
    "chat_draft_clear",
    "chat_draft_flush_ack",
];

/// Commands only the main window may invoke: the conversation opacity setting (Track T) and the
/// three appearance setters (UI enterprise S1). Every other window, the overlay included, is refused.
pub const MAIN_ONLY_COMMANDS: [&str; 5] = [
    "change_chat_opacity_setting",
    "change_accent_setting",
    "change_chat_theme_setting",
    "change_visual_style_setting",
    "change_orb_accent_setting",
];

/// Whether the window labelled `window_label` may invoke `command`.
///
/// The orb window may call only [`ORB_COMMANDS`]; the chat window only [`CHAT_COMMANDS`]. The main
/// window may call anything except the orb, chat and bubble lists. Any other window may call
/// anything that is not in those lists or in [`MAIN_ONLY_COMMANDS`].
pub fn window_command_allowed(window_label: &str, command: &str) -> bool {
    match window_label {
        ORB_LABEL => ORB_COMMANDS.contains(&command),
        CHAT_LABEL => CHAT_COMMANDS.contains(&command),
        BUBBLES_WINDOW_LABEL => BUBBLES_COMMANDS.contains(&command),
        MAIN_LABEL => {
            !ORB_COMMANDS.contains(&command)
                && !CHAT_COMMANDS.contains(&command)
                && !BUBBLES_COMMANDS.contains(&command)
        }
        _ => {
            !ORB_COMMANDS.contains(&command)
                && !CHAT_COMMANDS.contains(&command)
                && !BUBBLES_COMMANDS.contains(&command)
                && !MAIN_ONLY_COMMANDS.contains(&command)
        }
    }
}
