//! Activation-policy owner (DESIGN.md Rev 4, 'Activation policy owner'). The Dock icon is shown
//! (Regular) whenever a visible window needs it; otherwise the app runs as an accessory with a
//! tray icon. Only a user-initiated open may take focus or change the policy. Pure decisions:
//! nothing in G1 calls these yet.
#![cfg_attr(not(test), allow(dead_code))]

/// The macOS activation policy the app should be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Dock icon visible, normal app.
    Regular,
    /// No Dock icon; reachable through the tray.
    Accessory,
}

/// The facts that decide the activation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inputs {
    /// The main window is visible.
    pub main_visible: bool,
    /// The classic chat window is visible.
    pub classic_chat_visible: bool,
    /// The floating panel is open and needs keyboard focus.
    pub panel_needs_focus: bool,
    /// A tray icon exists to reach the app while it has no Dock icon.
    pub tray_present: bool,
}

/// What caused a window to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenKind {
    /// The user asked for it (hotkey, tray, orb).
    User,
    /// The app opened it on its own (for example a reply arriving).
    Auto,
    /// A passive preview that must not steal attention.
    Peek,
}

/// The policy the current inputs call for. Accessory only when nothing needs the Dock and a tray
/// icon exists to reach the app; Regular in every other case.
pub fn desired(inputs: &Inputs) -> Policy {
    if !inputs.main_visible
        && !inputs.classic_chat_visible
        && !inputs.panel_needs_focus
        && inputs.tray_present
    {
        Policy::Accessory
    } else {
        Policy::Regular
    }
}

/// Whether opening this way may take keyboard focus. Only a user open may.
pub fn open_may_focus(kind: OpenKind) -> bool {
    kind == OpenKind::User
}

/// Whether opening this way may change the activation policy. Only a user open may.
pub fn open_may_change_policy(kind: OpenKind) -> bool {
    kind == OpenKind::User
}
