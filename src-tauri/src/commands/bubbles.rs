//! Tauri command of the bubble window (DESIGN.md A7-2): the page's readiness handshake. The page
//! calls it once after its listener is registered, with the instance id from its URL. Only the
//! bubble window may call it, and only for the instance that is current. Nothing here logs
//! content.

use crate::bubble_native;
use crate::window_commands::BUBBLES_WINDOW_LABEL;
use tauri::{Manager, WebviewWindow};

/// Readiness handshake of the bubble page. Sends the current snapshot to that page only.
#[tauri::command]
#[specta::specta]
pub async fn bubbles_ready(window: WebviewWindow, instance: u64) -> Result<(), String> {
    let from_bubbles = window.label() == BUBBLES_WINDOW_LABEL;
    if !from_bubbles {
        return Err("bubbles_ready is only for the bubble window".to_string());
    }
    let app = window.app_handle().clone();
    if bubble_native::current_instance(&app) == Some(instance) {
        bubble_native::page_ready(&app, instance);
        Ok(())
    } else {
        Err("stale bubble window instance".to_string())
    }
}
