//! Tauri commands for the opt-in conversation records and vault context (DESIGN Rev 3, 3.5).
//!
//! Each setter is a thin read-modify-write of one field. The vault path goes through the pure
//! [`validate_vault_path`], and the record list reads through the managed conversation store.

use crate::managers::conversation_store::{ConversationStore, MAX_LIST};
use crate::settings::{self, AppSettings};
use serde::Serialize;
use specta::Type;
use std::sync::Arc;
use tauri::{AppHandle, Manager};

/// Largest accepted vault path, in bytes (UTF-8).
pub const MAX_VAULT_PATH_BYTES: usize = 4096;

/// Fixed error for a rejected vault path. Never echoes the input.
pub const MSG_VAULT_PATH_INVALID: &str = "The vault folder path is not valid.";

/// Validates the vault folder path from the settings form.
///
/// Trims surrounding whitespace. An empty result means "unset" and is allowed. Otherwise the path
/// must be at most [`MAX_VAULT_PATH_BYTES`] bytes and must not contain a NUL or a newline.
/// Returns the trimmed path, or the fixed error message.
pub fn validate_vault_path(raw: &str) -> Result<String, &'static str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed.len() > MAX_VAULT_PATH_BYTES {
        return Err(MSG_VAULT_PATH_INVALID);
    }
    if trimmed.chars().any(|c| c == '\0' || c == '\n' || c == '\r') {
        return Err(MSG_VAULT_PATH_INVALID);
    }
    Ok(trimmed.to_string())
}

/// Read-modify-write of one boolean conversation-record field, then persist.
fn persist_bool(app: &AppHandle, apply: impl FnOnce(&mut AppSettings, bool), value: bool) {
    let mut current = settings::get_settings(app);
    apply(&mut current, value);
    settings::write_settings(app, current);
}

/// Turns conversation records (SQLite) on or off. Off by default.
#[tauri::command]
#[specta::specta]
pub fn change_conversation_records_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    persist_bool(&app, |s, v| s.conversation_records_enabled = v, enabled);
    Ok(())
}

/// Also write a Markdown note into the vault for each saved record. Off by default.
#[tauri::command]
#[specta::specta]
pub fn change_conversation_records_to_vault_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    persist_bool(&app, |s, v| s.conversation_records_to_vault = v, enabled);
    Ok(())
}

/// Use the vault's context pack as background for the conversation. Off by default.
#[tauri::command]
#[specta::specta]
pub fn change_vault_context_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    persist_bool(&app, |s, v| s.vault_context_enabled = v, enabled);
    Ok(())
}

/// Sets the vault folder path. Empty clears it. Invalid input is refused with a fixed message.
#[tauri::command]
#[specta::specta]
pub fn change_vault_path_setting(app: AppHandle, path: String) -> Result<(), String> {
    let validated = validate_vault_path(&path).map_err(str::to_string)?;
    let mut current = settings::get_settings(&app);
    current.vault_path = validated;
    settings::write_settings(&app, current);
    Ok(())
}

/// One saved conversation as listed in the UI. The transcript is not included.
#[derive(Debug, Clone, Serialize, Type)]
pub struct ConversationRecordSummaryDto {
    pub id: i64,
    pub started_at: i64,
    pub ended_at: i64,
    pub end_reason: String,
    pub summary: String,
    pub turns: u32,
    pub vault_note: Option<String>,
}

/// Lists saved conversations, newest first. `limit` is clamped by the store (1..=200, default 50).
#[tauri::command]
#[specta::specta]
pub fn list_conversation_records(
    app: AppHandle,
    limit: Option<u32>,
) -> Result<Vec<ConversationRecordSummaryDto>, String> {
    let store = app
        .try_state::<Arc<ConversationStore>>()
        .ok_or_else(|| "Conversation records are not available.".to_string())?;
    let requested = limit.map_or(50, |n| usize::try_from(n).unwrap_or(MAX_LIST));
    let rows = store
        .list(requested)
        .map_err(|_| "Could not read conversation records.".to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| ConversationRecordSummaryDto {
            id: row.id,
            started_at: row.started_at,
            ended_at: row.ended_at,
            end_reason: row.end_reason,
            summary: row.summary,
            turns: u32::try_from(row.turns).unwrap_or(u32::MAX),
            vault_note: row.vault_note,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_whitespace_mean_unset() {
        assert_eq!(validate_vault_path(""), Ok(String::new()));
        assert_eq!(validate_vault_path("   "), Ok(String::new()));
        assert_eq!(validate_vault_path("\t \t"), Ok(String::new()));
    }

    #[test]
    fn nul_and_newlines_are_refused() {
        assert_eq!(
            validate_vault_path("/tmp/a\0b"),
            Err(MSG_VAULT_PATH_INVALID)
        );
        assert_eq!(
            validate_vault_path("/tmp/a\nb"),
            Err(MSG_VAULT_PATH_INVALID)
        );
        assert_eq!(
            validate_vault_path("/tmp/a\rb"),
            Err(MSG_VAULT_PATH_INVALID)
        );
    }

    #[test]
    fn too_long_is_refused_and_the_limit_itself_is_accepted() {
        let at_limit = "a".repeat(MAX_VAULT_PATH_BYTES);
        assert!(validate_vault_path(&at_limit).is_ok());
        let over = "a".repeat(MAX_VAULT_PATH_BYTES + 1);
        assert_eq!(validate_vault_path(&over), Err(MSG_VAULT_PATH_INVALID));
    }

    #[test]
    fn normal_paths_are_trimmed_and_kept() {
        assert_eq!(
            validate_vault_path("  /Users/someone/notes  "),
            Ok("/Users/someone/notes".to_string())
        );
    }

    #[test]
    fn unicode_paths_are_accepted_and_the_byte_limit_counts_bytes() {
        assert_eq!(
            validate_vault_path("/tmp/notas-é-日本"),
            Ok("/tmp/notas-é-日本".to_string())
        );
        // 'é' is two bytes, so this is over the limit even though it has fewer chars.
        let over_by_bytes = "é".repeat(MAX_VAULT_PATH_BYTES / 2 + 1);
        assert_eq!(
            validate_vault_path(&over_by_bytes),
            Err(MSG_VAULT_PATH_INVALID)
        );
    }
}
