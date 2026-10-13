//! Tauri commands for the Experimental "Realtime conversation" group (design R9).
//! Thin glue: every write goes through `managers::realtime_config`'s validated setters.
//! Values are never logged and error strings are static.

use crate::managers::realtime_config::{
    apply_exp_rt_bool, apply_exp_rt_text, ExpRtBool, ExpRtText,
};
use crate::settings;
use tauri::AppHandle;

/// Persist one realtime toggle. Read-modify-write through `apply_exp_rt_bool`.
#[tauri::command]
#[specta::specta]
pub fn change_exp_rt_bool_setting(
    app: AppHandle,
    key: ExpRtBool,
    value: bool,
) -> Result<(), String> {
    let mut current = settings::get_settings(&app);
    apply_exp_rt_bool(&mut current, key, value);
    settings::write_settings(&app, current);
    Ok(())
}

/// Persist one realtime text field. Read-modify-write through `apply_exp_rt_text`,
/// which trims and validates; on refusal the settings are left unchanged.
#[tauri::command]
#[specta::specta]
pub fn change_exp_rt_text_setting(
    app: AppHandle,
    key: ExpRtText,
    value: String,
) -> Result<(), String> {
    let mut current = settings::get_settings(&app);
    apply_exp_rt_text(&mut current, key, &value).map_err(|message| message.to_string())?;
    settings::write_settings(&app, current);
    Ok(())
}
