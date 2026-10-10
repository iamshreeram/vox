use crate::managers::voice_shortcuts::{validate_all, VoiceShortcut};
use crate::settings::{get_settings, write_settings};
use tauri::AppHandle;

/// Validates and persists the full list of voice shortcuts.
///
/// The whole list is validated before anything is written: if any entry is
/// invalid (or the list has duplicate phrases or exceeds the cap), `Err` is
/// returned and the stored settings are left untouched.
#[tauri::command]
#[specta::specta]
pub fn set_voice_shortcuts(app: AppHandle, shortcuts: Vec<VoiceShortcut>) -> Result<(), String> {
    validate_all(&shortcuts)?;
    let mut settings = get_settings(&app);
    settings.voice_shortcuts = shortcuts;
    write_settings(&app, settings);
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::managers::voice_shortcuts::{validate_all, VoiceShortcut, VoiceShortcutAction};

    fn url(phrase: &str, url: &str) -> VoiceShortcut {
        VoiceShortcut {
            phrase: phrase.into(),
            action: VoiceShortcutAction::OpenUrl { url: url.into() },
        }
    }

    #[test]
    fn invalid_list_is_rejected_before_any_write() {
        let bad = vec![
            url("open docs", "https://example.com"),
            url("javascript bomb", "javascript:alert(1)"),
        ];
        assert!(validate_all(&bad).is_err());
    }

    #[test]
    fn valid_list_passes_validation() {
        let good = vec![url("open docs", "https://example.com")];
        assert!(validate_all(&good).is_ok());
    }
}
