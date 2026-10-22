//! Personas: named presets of reply style and voice (design D5, A6).
//!
//! The style cap is `composer_rules::MAX_STYLE_CHARS` (the same cap
//! `converse_plan::build_system` truncates to). Voice ids are validated against
//! `natural_voices::NATURAL_VOICES` / `is_known_voice`.
use crate::managers::composer_rules::{DEFAULT_STYLE, MAX_STYLE_CHARS};
use crate::managers::natural_voices::is_known_voice;
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

/// Prefix of user-defined (custom) persona ids.
const CUSTOM_PREFIX: &str = "custom-";

fn builtin(id: &str, name: &str, style: &str, voice: Option<&str>) -> Persona {
    Persona {
        id: id.to_string(),
        name: name.to_string(),
        style: style.to_string(),
        voice: voice.map(str::to_string),
    }
}

/// The five built-in personas (in code, never persisted).
///
/// Voices (all curated, all distinct): concise = `af_sky`, coach = `am_adam`,
/// narrator = `bf_emma`, dry = `bm_lewis`. The default persona has no voice, so
/// applying it leaves the user's current natural voice untouched.
pub fn builtins() -> Vec<Persona> {
    vec![
        builtin("builtin-default", "Default", DEFAULT_STYLE, None),
        builtin(
            "builtin-concise",
            "Concise",
            "Answer first, in one or two short sentences. No filler and no preamble.",
            Some("af_sky"),
        ),
        builtin(
            "builtin-coach",
            "Coach",
            "Be encouraging and practical. Keep it brief, and you may end with one guiding question.",
            Some("am_adam"),
        ),
        builtin(
            "builtin-narrator",
            "Narrator",
            "Speak with a warm, vivid storytelling cadence. Use short, calm sentences with gentle pauses.",
            Some("bf_emma"),
        ),
        builtin(
            "builtin-dry",
            "Dry wit",
            "Be understated and dry in tone. Never mock the user, and always stay accurate.",
            Some("bm_lewis"),
        ),
    ]
}

fn has_control(s: &str) -> bool {
    s.chars().any(char::is_control)
}

/// Trim name and style, then check every rule. Returns the trimmed persona.
pub fn validate(p: &Persona) -> Result<Persona, PersonaError> {
    let name = p.name.trim();
    if name.is_empty() {
        return Err(PersonaError::NameEmpty);
    }
    if has_control(name) {
        return Err(PersonaError::ControlChars);
    }
    if name.chars().count() > NAME_MAX_CHARS {
        return Err(PersonaError::NameTooLong);
    }

    let style = p.style.trim();
    if style.is_empty() {
        return Err(PersonaError::StyleEmpty);
    }
    if has_control(style) {
        return Err(PersonaError::ControlChars);
    }
    if style.chars().count() > MAX_STYLE_CHARS {
        return Err(PersonaError::StyleTooLong);
    }

    if let Some(voice) = p.voice.as_deref() {
        if !is_known_voice(voice) {
            return Err(PersonaError::UnknownVoice);
        }
    }

    Ok(Persona {
        id: p.id.clone(),
        name: name.to_string(),
        style: style.to_string(),
        voice: p.voice.clone(),
    })
}

fn is_builtin_id(id: &str) -> bool {
    id.starts_with(BUILTIN_PREFIX)
}

/// `custom-<n>` where n = 1 + the largest numeric suffix currently in the list.
/// Every existing numeric suffix is <= the maximum, so the new id cannot collide.
fn next_custom_id(custom: &[Persona]) -> Result<String, PersonaError> {
    let largest = custom
        .iter()
        .filter_map(|p| p.id.strip_prefix(CUSTOM_PREFIX)?.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    let next = largest.checked_add(1).ok_or(PersonaError::LimitReached)?;
    Ok(format!("{CUSTOM_PREFIX}{next}"))
}

/// Validate and insert or replace a custom persona. Empty id => new `custom-<n>`
/// id appended; existing custom id => replaced in place; built-in id =>
/// `BuiltinImmutable`; other non-empty unknown id => `ReservedId`.
pub fn save(custom: &mut Vec<Persona>, p: Persona) -> Result<Persona, PersonaError> {
    let valid = validate(&p)?;

    if p.id.is_empty() {
        if custom.len() >= MAX_CUSTOM {
            return Err(PersonaError::LimitReached);
        }
        let saved = Persona {
            id: next_custom_id(custom)?,
            ..valid
        };
        custom.push(saved.clone());
        return Ok(saved);
    }

    if is_builtin_id(&p.id) {
        return Err(PersonaError::BuiltinImmutable);
    }

    let saved = Persona {
        id: p.id.clone(),
        ..valid
    };
    match custom.iter_mut().find(|c| c.id == p.id) {
        Some(slot) => {
            *slot = saved.clone();
            Ok(saved)
        }
        None => Err(PersonaError::ReservedId),
    }
}

/// Remove a custom persona by id.
pub fn delete(custom: &mut Vec<Persona>, id: &str) -> Result<(), PersonaError> {
    if is_builtin_id(id) {
        return Err(PersonaError::BuiltinImmutable);
    }
    match custom.iter().position(|c| c.id == id) {
        Some(index) => {
            custom.remove(index);
            Ok(())
        }
        None => Err(PersonaError::NotFound),
    }
}

/// Built-ins first, then customs in stored order.
pub fn all(custom: &[Persona]) -> Vec<Persona> {
    builtins()
        .into_iter()
        .chain(custom.iter().cloned())
        .collect()
}

/// Copy-on-apply: sets `tts_composer_style`, and `tts_natural_voice` only when
/// the persona has a curated voice. Touches nothing else.
pub fn apply(settings: &mut AppSettings, p: &Persona) {
    settings.tts_composer_style = p.style.clone();
    if let Some(voice) = p.voice.as_deref() {
        if is_known_voice(voice) {
            settings.tts_natural_voice = voice.to_string();
        }
    }
}

/// Id of the first persona in `all(&settings.personas)` matching the current
/// style and (if the persona has one) voice; `None` otherwise.
pub fn active_id(settings: &AppSettings) -> Option<String> {
    all(&settings.personas)
        .into_iter()
        .find(|p| {
            p.style == settings.tts_composer_style
                && match &p.voice {
                    None => true,
                    Some(voice) => *voice == settings.tts_natural_voice,
                }
        })
        .map(|p| p.id)
}

#[cfg(test)]
#[path = "persona_tests.rs"]
mod tests;
