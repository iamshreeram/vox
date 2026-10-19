//! Personas: named presets of reply style and voice (design D5, A6). Logic filled by step S4.
use serde::{Deserialize, Serialize};
use specta::Type;

/// A persona. `style` feeds `tts_composer_style`; `voice` (a curated natural
/// voice id) feeds `tts_natural_voice` when present.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Type)]
pub struct Persona {
    pub id: String,
    pub name: String,
    pub style: String,
    #[serde(default)]
    pub voice: Option<String>,
}
