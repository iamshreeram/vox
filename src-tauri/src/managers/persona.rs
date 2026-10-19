//! Personas: named presets of reply style and voice (design D5, A6).
//!
//! Contract stubs (step T4, red): every logic function is `todo!()` and is
//! filled in by the implementation step. The authoritative tests live in
//! `persona_tests.rs`.
//!
//! The style cap is `composer_rules::MAX_STYLE_CHARS` (the same cap
//! `converse_plan::build_system` truncates to). Voice ids are validated against
//! `natural_voices::NATURAL_VOICES` / `is_known_voice`.
use crate::settings::AppSettings;
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

/// Maximum number of user-defined (custom) personas.
pub const MAX_CUSTOM: usize = 12;
/// Maximum name length in characters (after trimming).
pub const NAME_MAX_CHARS: usize = 40;
/// Prefix reserved for built-in persona ids.
pub const BUILTIN_PREFIX: &str = "builtin-";

/// Why a persona operation was rejected. Carries no text payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub enum PersonaError {
    NameEmpty,
    NameTooLong,
    StyleEmpty,
    StyleTooLong,
    ControlChars,
    UnknownVoice,
    ReservedId,
    BuiltinImmutable,
    LimitReached,
    NotFound,
}

/// The five built-in personas (in code, never persisted).
pub fn builtins() -> Vec<Persona> {
    todo!("persona: builtins() not implemented (T4 red stub)")
}

/// Trim name and style, then check every rule. Returns the trimmed persona.
pub fn validate(_p: &Persona) -> Result<Persona, PersonaError> {
    todo!("persona: validate() not implemented (T4 red stub)")
}

/// Validate and insert or replace a custom persona. Empty id => new `custom-<n>`
/// id appended; existing custom id => replaced in place; built-in id =>
/// `BuiltinImmutable`; other non-empty unknown id => `ReservedId`.
pub fn save(_custom: &mut Vec<Persona>, _p: Persona) -> Result<Persona, PersonaError> {
    todo!("persona: save() not implemented (T4 red stub)")
}

/// Remove a custom persona by id.
pub fn delete(_custom: &mut Vec<Persona>, _id: &str) -> Result<(), PersonaError> {
    todo!("persona: delete() not implemented (T4 red stub)")
}

/// Built-ins first, then customs in stored order.
pub fn all(_custom: &[Persona]) -> Vec<Persona> {
    todo!("persona: all() not implemented (T4 red stub)")
}

/// Copy-on-apply: sets `tts_composer_style`, and `tts_natural_voice` only when
/// the persona has a curated voice. Touches nothing else.
pub fn apply(_settings: &mut AppSettings, _p: &Persona) {
    todo!("persona: apply() not implemented (T4 red stub)")
}

/// Id of the first persona in `all(&settings.personas)` matching the current
/// style and (if the persona has one) voice; `None` otherwise.
pub fn active_id(_settings: &AppSettings) -> Option<String> {
    todo!("persona: active_id() not implemented (T4 red stub)")
}

#[cfg(test)]
#[path = "persona_tests.rs"]
mod tests;
