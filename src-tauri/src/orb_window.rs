//! Pure description of the floating orb window (DESIGN.md Rev 5, 'Orb window'). This module only
//! states what the window must look like; creating it is a later step and needs no Tauri types
//! here.
#![cfg_attr(not(test), allow(dead_code))]

use crate::orb_geometry::{orb_logical_px, OrbSize};

/// The window label used for the orb.
pub const ORB_WINDOW_LABEL: &str = "orb";

/// The properties the orb window must be created with.
#[derive(Debug, Clone, PartialEq)]
pub struct OrbWindowSpec {
    /// Window label.
    pub label: &'static str,
    /// Edge length of the square window, in logical pixels.
    pub logical_px: f64,
    /// Whether the window background is transparent.
    pub transparent: bool,
    /// Whether the window has native decorations.
    pub decorations: bool,
    /// Whether the window stays above other windows.
    pub always_on_top: bool,
    /// Whether the window is hidden from the taskbar or dock.
    pub skip_taskbar: bool,
    /// Whether the window can be resized.
    pub resizable: bool,
    /// Whether the window has a native shadow.
    pub shadow: bool,
    /// Whether the window can take keyboard focus.
    pub focusable: bool,
    /// Whether the window appears on every virtual desktop or workspace.
    pub visible_on_all_workspaces: bool,
}

/// The orb window spec for a size preset.
pub fn orb_window_spec(size: OrbSize) -> OrbWindowSpec {
    orb_window_spec_px(orb_logical_px(size))
}

/// The orb window spec for an edge length in logical pixels (a free size or a preset).
pub fn orb_window_spec_px(logical_px: f64) -> OrbWindowSpec {
    OrbWindowSpec {
        label: ORB_WINDOW_LABEL,
        logical_px,
        transparent: true,
        decorations: false,
        always_on_top: true,
        skip_taskbar: true,
        resizable: false,
        shadow: false,
        focusable: false,
        visible_on_all_workspaces: true,
    }
}
