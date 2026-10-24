//! UI navigation port (DESIGN D4 `open_settings`, R4-6). Navigation only, no data effect.
//! The real implementation lives in lib.rs (M2-3); tests use a fake.

/// Settings section ids accepted by `open_settings`: general, history, models, advanced,
/// postprocessing, agents, experimental, about (NOT debug).
/// `experimental` is a legacy alias that the UI maps to `agents` (it is kept so old
/// tool calls and stored links still resolve). `debug` is deliberately not accepted.
pub const OPEN_SETTINGS_SECTIONS: &[&str] = &[
    "general",
    "history",
    "models",
    "advanced",
    "postprocessing",
    "agents",
    "experimental",
    "about",
];

pub trait UiPort: Send + Sync {
    /// Emits the navigation event and shows the main window.
    fn open_settings_section(&self, section: &str);
}
