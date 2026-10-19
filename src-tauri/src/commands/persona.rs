//! Tauri commands for personas (design D5). Each command is a thin read-modify-write
//! over a pure function that takes `&mut AppSettings`. A refused operation never
//! writes settings: the custom list is cloned, changed, and committed only on success.

use crate::managers::persona::{self as store, Persona, PersonaError};
use crate::settings::{self, AppSettings};
use serde::Serialize;
use specta::Type;
use tauri::AppHandle;

/// One entry of the Personas card: the persona plus what the UI needs to show.
#[derive(Serialize, Debug, Clone, PartialEq, Eq, Type)]
pub struct PersonaView {
    pub persona: Persona,
    /// True for the five built-ins (read-only).
    pub builtin: bool,
    /// True for the single persona that matches the current settings, if any.
    pub active: bool,
}

/// Built-ins first, then customs in stored order. At most one entry is active.
pub fn persona_views(settings: &AppSettings) -> Vec<PersonaView> {
    let active = store::active_id(settings);
    let builtin_ids: Vec<String> = store::builtins().into_iter().map(|p| p.id).collect();
    store::all(&settings.personas)
        .into_iter()
        .map(|persona| {
            let builtin = builtin_ids.contains(&persona.id);
            let active = active.as_deref() == Some(persona.id.as_str());
            PersonaView {
                persona,
                builtin,
                active,
            }
        })
        .collect()
}

/// Validate and insert or replace a custom persona. Settings are unchanged on error.
pub fn save_in(settings: &mut AppSettings, persona: Persona) -> Result<Persona, PersonaError> {
    let mut custom = settings.personas.clone();
    let saved = store::save(&mut custom, persona)?;
    settings.personas = custom;
    Ok(saved)
}

/// Remove a custom persona. Settings are unchanged on error.
pub fn delete_in(settings: &mut AppSettings, id: &str) -> Result<(), PersonaError> {
    let mut custom = settings.personas.clone();
    store::delete(&mut custom, id)?;
    settings.personas = custom;
    Ok(())
}

/// Copy-on-apply of the persona `id` (built-in or custom). `NotFound` leaves settings unchanged.
pub fn apply_in(settings: &mut AppSettings, id: &str) -> Result<(), PersonaError> {
    let persona = store::all(&settings.personas)
        .into_iter()
        .find(|p| p.id == id)
        .ok_or(PersonaError::NotFound)?;
    store::apply(settings, &persona);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn list_personas(app: AppHandle) -> Vec<PersonaView> {
    persona_views(&settings::get_settings(&app))
}

#[tauri::command]
#[specta::specta]
pub fn save_persona(app: AppHandle, persona: Persona) -> Result<Persona, PersonaError> {
    let mut current = settings::get_settings(&app);
    let saved = save_in(&mut current, persona)?;
    settings::write_settings(&app, current);
    Ok(saved)
}

#[tauri::command]
#[specta::specta]
pub fn delete_persona(app: AppHandle, id: String) -> Result<(), PersonaError> {
    let mut current = settings::get_settings(&app);
    delete_in(&mut current, &id)?;
    settings::write_settings(&app, current);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn apply_persona(app: AppHandle, id: String) -> Result<(), PersonaError> {
    let mut current = settings::get_settings(&app);
    apply_in(&mut current, &id)?;
    settings::write_settings(&app, current);
    Ok(())
}

#[cfg(test)]
#[path = "persona_cmd_tests.rs"]
mod tests;
