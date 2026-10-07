use crate::managers::memory::{Fact, MemoryManager};
use std::sync::Arc;
use tauri::{AppHandle, State};

fn gate_if_enabled<T>(enabled: bool, operation: impl FnOnce() -> T) -> Result<T, String> {
    if !enabled {
        return Err("Memory is disabled".to_string());
    }
    Ok(operation())
}

fn update_memory_enabled(settings: &mut crate::settings::AppSettings, enabled: bool) {
    settings.memory_enabled = enabled;
}

fn memory_enabled(app: &AppHandle) -> bool {
    crate::settings::get_settings(app).memory_enabled
}

#[tauri::command]
#[specta::specta]
pub async fn memory_remember(
    app: AppHandle,
    manager: State<'_, Arc<MemoryManager>>,
    text: String,
) -> Result<i64, String> {
    let manager = Arc::clone(&manager);
    gate_if_enabled(memory_enabled(&app), || {
        tauri::async_runtime::spawn_blocking(move || {
            let fact = MemoryManager::extract_fact(&text).unwrap_or_else(|| text.clone());
            manager
                .remember(&fact, &text)
                .map_err(|error| error.to_string())
        })
    })?
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
#[specta::specta]
pub async fn memory_recall(
    app: AppHandle,
    manager: State<'_, Arc<MemoryManager>>,
    query: String,
    limit: u32,
) -> Result<Vec<Fact>, String> {
    let manager = Arc::clone(&manager);
    gate_if_enabled(memory_enabled(&app), || {
        tauri::async_runtime::spawn_blocking(move || {
            manager
                .recall(&query, limit as usize)
                .map_err(|error| error.to_string())
        })
    })?
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
#[specta::specta]
pub async fn memory_list_all(
    app: AppHandle,
    manager: State<'_, Arc<MemoryManager>>,
) -> Result<Vec<Fact>, String> {
    let manager = Arc::clone(&manager);
    gate_if_enabled(memory_enabled(&app), || {
        tauri::async_runtime::spawn_blocking(move || {
            manager.list_all().map_err(|error| error.to_string())
        })
    })?
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
#[specta::specta]
pub async fn memory_forget(
    app: AppHandle,
    manager: State<'_, Arc<MemoryManager>>,
    id: i64,
) -> Result<(), String> {
    let manager = Arc::clone(&manager);
    gate_if_enabled(memory_enabled(&app), || {
        tauri::async_runtime::spawn_blocking(move || {
            manager.forget(id).map_err(|error| error.to_string())
        })
    })?
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
#[specta::specta]
pub fn memory_set_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = crate::settings::get_settings(&app);
    update_memory_enabled(&mut settings, enabled);
    crate::settings::write_settings(&app, settings);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{gate_if_enabled, update_memory_enabled};
    use crate::managers::memory::MemoryManager;
    use crate::settings::get_default_settings;

    #[test]
    fn disabled_gate_returns_clear_error_without_writing_a_fact() {
        let dir = tempfile::tempdir().unwrap();
        let manager = MemoryManager::open(dir.path().join("memory.db")).unwrap();
        let result = gate_if_enabled(false, || manager.remember("secret fact", "source").unwrap());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Memory is disabled");
        assert!(manager.list_all().unwrap().is_empty());
    }

    #[test]
    fn disabling_then_enabling_allows_memory_operation() {
        let mut settings = get_default_settings();
        update_memory_enabled(&mut settings, false);
        let disabled = gate_if_enabled(settings.memory_enabled, || "saved");
        assert_eq!(disabled.unwrap_err(), "Memory is disabled");
        update_memory_enabled(&mut settings, true);
        assert_eq!(
            gate_if_enabled(settings.memory_enabled, || "saved").unwrap(),
            "saved"
        );
    }

    #[test]
    fn enabling_memory_updates_the_setting() {
        let mut settings = get_default_settings();
        update_memory_enabled(&mut settings, true);
        assert!(settings.memory_enabled);
    }

    #[test]
    fn disabling_memory_updates_the_setting() {
        let mut settings = get_default_settings();
        update_memory_enabled(&mut settings, true);
        update_memory_enabled(&mut settings, false);
        assert!(!settings.memory_enabled);
    }

    #[test]
    fn enabled_gate_runs_operation() {
        let result = gate_if_enabled(true, || "stored");
        assert_eq!(result.unwrap(), "stored");
    }
}
