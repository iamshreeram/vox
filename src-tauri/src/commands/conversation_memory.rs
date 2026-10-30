//! Tauri commands for conversation memory (DESIGN.md Revision 4, D4).
//!
//! The store is managed by `lib.rs` only when it opened. The commands tolerate its absence:
//! clear and count report 0 and a setting change still persists.

use crate::managers::conversation_memory::ConversationMemory;
use crate::settings;
use std::sync::Arc;
use tauri::{AppHandle, Manager};

/// Deletes every remembered summary. Returns the rows deleted; 0 when there is no store or
/// the store reports an error.
pub fn clear_memory(mem: Option<&ConversationMemory>) -> usize {
    mem.and_then(|memory| memory.clear().ok()).unwrap_or(0)
}

/// Number of remembered summaries; 0 when there is no store or the store reports an error.
pub fn memory_count(mem: Option<&ConversationMemory>) -> usize {
    mem.and_then(|memory| memory.count().ok()).unwrap_or(0)
}

/// Called on every change of the setting, on or off. Bumps the epoch so that a session started
/// before the change can neither inject nor save. Deletes nothing.
pub fn apply_setting_change(mem: Option<&ConversationMemory>) {
    if let Some(memory) = mem {
        memory.bump_epoch();
    }
}

fn managed_memory(app: &AppHandle) -> Option<Arc<ConversationMemory>> {
    app.try_state::<Arc<ConversationMemory>>()
        .map(|state| Arc::clone(&*state))
}

#[tauri::command]
#[specta::specta]
pub fn clear_conversation_memory(app: AppHandle) -> usize {
    clear_memory(managed_memory(&app).as_deref())
}

#[tauri::command]
#[specta::specta]
pub fn conversation_memory_count(app: AppHandle) -> usize {
    memory_count(managed_memory(&app).as_deref())
}

/// Persists the opt-in setting, then bumps the epoch (after the write, on every call).
#[tauri::command]
#[specta::specta]
pub fn change_conversation_memory_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut current = settings::get_settings(&app);
    current.conversation_memory_enabled = enabled;
    settings::write_settings(&app, current);
    apply_setting_change(managed_memory(&app).as_deref());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{apply_setting_change, clear_memory, memory_count};
    use crate::managers::conversation_memory::{ConversationMemory, MemoryError};

    fn store() -> ConversationMemory {
        ConversationMemory::open_in_memory().expect("in-memory memory")
    }

    /// Adds one row with the live epoch.
    fn put(store: &ConversationMemory, ended: i64, text: &str) {
        store
            .add(store.epoch(), ended - 10, ended, 2, text, false)
            .expect("row added");
    }

    #[test]
    fn clear_returns_the_row_count_and_empties_the_store() {
        let mem = store();
        put(&mem, 1_000, "first summary");
        put(&mem, 2_000, "second summary");
        assert_eq!(clear_memory(Some(&mem)), 2);
        assert_eq!(mem.count().unwrap(), 0);
    }

    #[test]
    fn clear_bumps_the_epoch_so_a_pre_clear_write_is_stale() {
        let mem = store();
        let before = mem.epoch();
        clear_memory(Some(&mem));
        assert_eq!(
            mem.add(before, 0, 1_000, 2, "late write", false),
            Err(MemoryError::Stale)
        );
    }

    #[test]
    fn clear_without_a_store_returns_zero() {
        assert_eq!(clear_memory(None), 0);
    }

    #[test]
    fn count_matches_the_rows_and_is_zero_without_a_store() {
        let mem = store();
        assert_eq!(memory_count(Some(&mem)), 0);
        put(&mem, 1_000, "one");
        put(&mem, 2_000, "two");
        assert_eq!(memory_count(Some(&mem)), 2);
        assert_eq!(memory_count(None), 0);
    }

    #[test]
    fn setting_change_bumps_the_epoch_and_deletes_nothing() {
        let mem = store();
        put(&mem, 1_000, "kept");
        let before = mem.epoch();
        apply_setting_change(Some(&mem));
        assert_eq!(mem.epoch(), before + 1);
        assert_eq!(mem.count().unwrap(), 1);
    }

    #[test]
    fn setting_change_without_a_store_does_not_panic() {
        apply_setting_change(None);
    }

    #[test]
    fn two_setting_changes_bump_twice() {
        let mem = store();
        let before = mem.epoch();
        apply_setting_change(Some(&mem));
        apply_setting_change(Some(&mem));
        assert_eq!(mem.epoch(), before + 2);
    }

    #[test]
    fn a_write_captured_before_the_change_is_refused_afterwards() {
        let mem = store();
        let captured = mem.epoch();
        apply_setting_change(Some(&mem));
        assert_eq!(
            mem.add(captured, 0, 1_000, 2, "pre-change write", false),
            Err(MemoryError::Stale)
        );
        assert_eq!(mem.count().unwrap(), 0);
    }
}
