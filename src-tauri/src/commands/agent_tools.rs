//! Tauri commands for the agent tools settings (DESIGN Rev 4, R4-7). Each setter is a thin
//! read-modify-write of one field, modelled on `conversation_records.rs`. The project folder
//! list goes through the pure [`validate_roots`].

use crate::settings::{self, AppSettings};
use tauri::AppHandle;

/// Largest accepted root path, in bytes (UTF-8).
pub const MAX_ROOT_BYTES: usize = 1024;

/// Most project folders accepted in one save.
pub const MAX_ROOTS: usize = 20;

/// Fixed error for a rejected root. It names the index only, never the path text.
pub const MSG_ROOT_INVALID: &str = "A project folder entry is not valid.";

/// Fixed error when there are too many folders. It names no path.
pub const MSG_TOO_MANY_ROOTS: &str = "Too many project folders (at most 20).";

/// Validates the project folder list from the settings form.
///
/// Each entry is trimmed; entries that are empty after trimming are dropped. Every remaining
/// entry must be an absolute path (`std::path::Path::is_absolute`), at most
/// [`MAX_ROOT_BYTES`] bytes, and must not contain a NUL. At most [`MAX_ROOTS`] entries are
/// allowed. Duplicates are removed, keeping the first occurrence. On error the message names
/// the index of the bad entry only, never the path.
pub fn validate_roots(raw: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for (index, entry) in raw.iter().enumerate() {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.len() > MAX_ROOT_BYTES
            || trimmed.contains('\0')
            || !std::path::Path::new(trimmed).is_absolute()
        {
            return Err(format!("{MSG_ROOT_INVALID} (entry {})", index + 1));
        }
        if !out.iter().any(|existing| existing == trimmed) {
            out.push(trimmed.to_string());
        }
    }
    if out.len() > MAX_ROOTS {
        return Err(MSG_TOO_MANY_ROOTS.to_string());
    }
    Ok(out)
}

/// Read-modify-write of one boolean agent-tools field, then persist.
fn persist_bool(app: &AppHandle, apply: impl FnOnce(&mut AppSettings, bool), value: bool) {
    let mut current = settings::get_settings(app);
    apply(&mut current, value);
    settings::write_settings(app, current);
}

/// Turns the read-only voice agent tools on or off. Off by default.
#[tauri::command]
#[specta::specta]
pub fn change_agent_tools_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    persist_bool(&app, |s, v| s.agent_tools_enabled = v, enabled);
    Ok(())
}

/// Blocks model-planned paste and command after project data was read. On by default.
#[tauri::command]
#[specta::specta]
pub fn change_agent_tools_guard_actions_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    persist_bool(&app, |s, v| s.agent_tools_guard_actions = v, enabled);
    Ok(())
}

/// Sets the project folder list. Invalid input is refused with a fixed message.
#[tauri::command]
#[specta::specta]
pub fn change_agent_project_roots_setting(
    app: AppHandle,
    roots: Vec<String>,
) -> Result<(), String> {
    let validated = validate_roots(&roots)?;
    let mut current = settings::get_settings(&app);
    current.agent_project_roots = validated;
    settings::write_settings(&app, current);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abs() -> String {
        if cfg!(windows) {
            "C:\\Users\\example\\src".to_string()
        } else {
            "/Users/example/src".to_string()
        }
    }

    #[test]
    fn trims_and_drops_empty_entries() {
        let got = validate_roots(&[format!("  {}  ", abs()), "   ".into(), String::new()]).unwrap();
        assert_eq!(got, vec![abs()]);
    }

    #[test]
    fn relative_path_is_refused_and_names_only_the_index() {
        let err = validate_roots(&[abs(), "relative/secret-folder".into()]).unwrap_err();
        assert!(err.contains("entry 2"), "index missing: {err}");
        assert!(
            !err.contains("relative/secret-folder"),
            "path leaked: {err}"
        );
    }

    #[test]
    fn nul_is_refused() {
        let err = validate_roots(&[format!("{}\0x", abs())]).unwrap_err();
        assert!(err.contains("entry 1"));
        assert!(!err.contains('\0'));
    }

    #[test]
    fn path_over_1024_bytes_is_refused() {
        let long = format!("/{}", "a".repeat(1024));
        assert!(long.len() > MAX_ROOT_BYTES);
        let err = validate_roots(&[long]).unwrap_err();
        assert!(err.contains("entry 1"));
    }

    #[test]
    fn path_of_exactly_1024_bytes_is_accepted() {
        let exact = format!("/{}", "a".repeat(MAX_ROOT_BYTES - 1));
        assert_eq!(exact.len(), MAX_ROOT_BYTES);
        assert_eq!(
            validate_roots(std::slice::from_ref(&exact)).unwrap(),
            vec![exact]
        );
    }

    #[test]
    fn twenty_one_roots_are_refused_and_twenty_accepted() {
        let roots: Vec<String> = (0..21).map(|i| format!("/tmp/root-{i}")).collect();
        assert_eq!(validate_roots(&roots[..20]).unwrap().len(), 20);
        assert_eq!(validate_roots(&roots).unwrap_err(), MSG_TOO_MANY_ROOTS);
    }

    #[test]
    fn duplicates_keep_the_first_occurrence_in_order() {
        let got = validate_roots(&[
            "/tmp/b".into(),
            "/tmp/a".into(),
            " /tmp/b ".into(),
            "/tmp/a".into(),
        ])
        .unwrap();
        assert_eq!(got, vec!["/tmp/b".to_string(), "/tmp/a".to_string()]);
    }

    #[test]
    fn windows_style_path_only_accepted_on_windows() {
        let got = validate_roots(&["C:\\Users\\example\\src".into()]);
        if cfg!(windows) {
            assert!(got.is_ok());
        } else {
            assert!(got.is_err(), "a drive path is not absolute on unix");
        }
    }

    #[test]
    fn unicode_folder_name_is_accepted() {
        let p = format!("/Users/example/projets/{}", "\u{00e9}t\u{00e9}-\u{1f680}");
        assert_eq!(validate_roots(std::slice::from_ref(&p)).unwrap(), vec![p]);
    }

    #[test]
    fn error_text_has_index_but_not_path() {
        let err = validate_roots(&[abs(), "/ok".into(), "not-absolute-XYZ".into()]).unwrap_err();
        assert!(err.contains("entry 3"));
        assert!(!err.contains("XYZ"));
    }
}
