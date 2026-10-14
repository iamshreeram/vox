// M2 stub: bodies arrive in M2-1..M2-3, this allow is removed in M2-3
#![allow(dead_code)]

//! UI navigation port (DESIGN D4 `open_settings`, R4-6). Navigation only, no data effect.
//! The real implementation lives in lib.rs (M2-3); tests use a fake.

/// Settings section ids accepted by `open_settings`: general, history, models, advanced,
/// postprocessing, experimental, about (NOT debug).
pub const OPEN_SETTINGS_SECTIONS: &[&str] = &[
    "general",
    "history",
    "models",
    "advanced",
    "postprocessing",
    "experimental",
    "about",
];

pub trait UiPort: Send + Sync {
    /// Emits the navigation event and shows the main window.
    fn open_settings_section(&self, section: &str);
}
