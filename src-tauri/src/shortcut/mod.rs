//! Keyboard shortcut management module
//!
//! This module provides a unified interface for keyboard shortcuts with
//! multiple backend implementations:
//!
//! - `tauri`: Uses Tauri's built-in global-shortcut plugin
//! - `handy_keys`: Uses the handy-keys library for more control
//!
//! The active implementation is determined by the `keyboard_implementation`
//! setting and can be changed at runtime.

mod handler;
pub mod handy_keys;
pub mod tauri_impl;

use log::{debug, error, info, warn};
use serde::Serialize;
use specta::Type;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use crate::settings::APPLE_INTELLIGENCE_DEFAULT_MODEL_ID;
use crate::settings::{
    self, get_settings, AutoSubmitKey, ChineseScript, ClipboardHandling, KeyboardImplementation,
    LLMPrompt, OverlayPosition, OverlayStyle, PasteMethod, ShortcutActivation, ShortcutBinding,
    SoundTheme, Theme, TypingTool, VadBackend, APPLE_INTELLIGENCE_PROVIDER_ID,
};
use crate::tray;

// Note: Commands are accessed via shortcut::handy_keys:: in lib.rs

/// Initialize shortcuts using the configured implementation
pub fn init_shortcuts(app: &AppHandle) {
    let user_settings = settings::load_or_create_app_settings(app);

    // Check which implementation to use
    match user_settings.keyboard_implementation {
        KeyboardImplementation::Tauri => {
            tauri_impl::init_shortcuts(app);
        }
        KeyboardImplementation::HandyKeys => {
            if let Err(e) = handy_keys::init_shortcuts(app) {
                error!("Failed to initialize handy-keys shortcuts: {}", e);
                // Fall back to Tauri implementation and persist this fallback
                warn!("Falling back to Tauri global shortcut implementation and saving fallback to settings");

                // Update settings to persist the fallback so we don't retry HandyKeys on next launch
                let mut settings = settings::get_settings(app);
                settings.keyboard_implementation = KeyboardImplementation::Tauri;
                settings::write_settings(app, settings);

                tauri_impl::init_shortcuts(app);
            }
        }
    }
}

/// Whether the recording lifecycle currently wants the cancel shortcut.
/// Written synchronously by start/stop, so it always holds the latest request.
static CANCEL_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Whether the cancel shortcut is actually registered with the backend.
/// The lock also serializes reconciliation passes.
#[cfg(not(target_os = "linux"))]
static CANCEL_REGISTERED: std::sync::Mutex<bool> = std::sync::Mutex::new(false);

/// The command mutex for the commands that change a setting and announce it in `settings-changed`
/// (update checks, automatic updates). Two quick toggles then announce in the order they were
/// applied, and each announces the value read back from the settings.
///
/// Lock order (outermost first, the same everywhere): command mutex -> update coordinator -> settings gate -> store.
/// Nothing takes the command mutex while holding a lower lock, and no command takes it twice.
static SETTINGS_ANNOUNCE_ORDER: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Takes the command mutex of the settings announcements. Recovers from poisoning.
pub fn settings_announce_order() -> std::sync::MutexGuard<'static, ()> {
    SETTINGS_ANNOUNCE_ORDER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Runs `write`, reads the value back, announces it and returns it, all under the command mutex.
pub fn write_read_back_and_announce<T>(
    write: impl FnOnce(),
    read_back: impl FnOnce() -> T,
    announce: impl FnOnce(&T),
) -> T {
    let _order = settings_announce_order();
    write();
    let value = read_back();
    announce(&value);
    value
}

/// Register the cancel shortcut (called when recording starts)
pub fn register_cancel_shortcut(app: &AppHandle) {
    // Track recording lifecycle independently of the current implementation so
    // switching implementations mid-recording cannot leave stale fallback state.
    crate::secure_input::register_cancel_fallback(app);

    CANCEL_REQUESTED.store(true, Ordering::SeqCst);
    schedule_cancel_reconcile(app);
}

/// Unregister the cancel shortcut (called when recording stops)
pub fn unregister_cancel_shortcut(app: &AppHandle) {
    crate::secure_input::unregister_cancel_fallback(app);

    CANCEL_REQUESTED.store(false, Ordering::SeqCst);
    schedule_cancel_reconcile(app);
}

/// Apply the requested cancel state off the calling thread. Start and stop run
/// on the shortcut handler thread, which must not block on registration, so the
/// work is spawned. Spawned tasks may run in any order (a very short recording
/// can see its unregister task run before its register task), so each pass
/// applies the latest request instead of the action that scheduled it.
fn schedule_cancel_reconcile(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        reconcile_cancel_shortcut(&app);
    });
}

fn reconcile_cancel_shortcut(app: &AppHandle) {
    // Cancel shortcut is disabled on Linux due to instability with dynamic shortcut registration
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        return;
    }

    #[cfg(not(target_os = "linux"))]
    {
        let mut registered = CANCEL_REGISTERED.lock().unwrap_or_else(|e| e.into_inner());
        let requested = CANCEL_REQUESTED.load(Ordering::SeqCst);
        if requested == *registered {
            return;
        }

        let Some(cancel_binding) = get_settings(app).bindings.get("cancel").cloned() else {
            return;
        };

        if requested {
            match register_shortcut(app, cancel_binding) {
                Ok(()) => *registered = true,
                Err(e) => error!("Failed to register cancel shortcut: {}", e),
            }
        } else {
            match unregister_shortcut(app, cancel_binding) {
                Ok(()) => *registered = false,
                Err(e) => error!("Failed to unregister cancel shortcut: {}", e),
            }
        }
    }
}

/// Register a shortcut using the appropriate implementation
pub fn register_shortcut(app: &AppHandle, binding: ShortcutBinding) -> Result<(), String> {
    let settings = get_settings(app);
    match settings.keyboard_implementation {
        KeyboardImplementation::Tauri => tauri_impl::register_shortcut(app, binding),
        KeyboardImplementation::HandyKeys => handy_keys::register_shortcut(app, binding),
    }
}

/// Unregister a shortcut using the appropriate implementation
pub fn unregister_shortcut(app: &AppHandle, binding: ShortcutBinding) -> Result<(), String> {
    let settings = get_settings(app);
    match settings.keyboard_implementation {
        KeyboardImplementation::Tauri => tauri_impl::unregister_shortcut(app, binding),
        KeyboardImplementation::HandyKeys => handy_keys::unregister_shortcut(app, binding),
    }
}

// ============================================================================
// Binding Management Commands
// ============================================================================

#[derive(Serialize, Type)]
pub struct BindingResponse {
    success: bool,
    binding: Option<ShortcutBinding>,
    error: Option<String>,
}

#[tauri::command]
#[specta::specta]
pub fn change_binding(
    app: AppHandle,
    id: String,
    binding: String,
) -> Result<BindingResponse, String> {
    // Reject empty bindings — every shortcut should have a value
    if binding.trim().is_empty() {
        return Err("Binding cannot be empty".to_string());
    }

    let mut settings = settings::get_settings(&app);

    // Get the binding to modify, or create it from defaults if it doesn't exist
    let binding_to_modify = match settings.bindings.get(&id) {
        Some(binding) => binding.clone(),
        None => {
            // Try to get the default binding for this id
            let default_settings = settings::get_default_settings();
            match default_settings.bindings.get(&id) {
                Some(default_binding) => {
                    warn!(
                        "Binding '{}' not found in settings, creating from defaults",
                        id
                    );
                    default_binding.clone()
                }
                None => {
                    let error_msg = format!("Binding with id '{}' not found in defaults", id);
                    warn!("change_binding error: {}", error_msg);
                    return Ok(BindingResponse {
                        success: false,
                        binding: None,
                        error: Some(error_msg),
                    });
                }
            }
        }
    };

    // If this is the cancel binding, just update the settings and return
    // It's managed dynamically, so we don't register/unregister here
    if id == "cancel" {
        if let Some(mut b) = settings.bindings.get(&id).cloned() {
            b.current_binding = binding;
            settings.bindings.insert(id.clone(), b.clone());
            settings::write_settings(&app, settings);
            crate::secure_input::reconcile_fallback(&app);
            return Ok(BindingResponse {
                success: true,
                binding: Some(b.clone()),
                error: None,
            });
        }
    }

    // Unregister the existing binding
    if let Err(e) = unregister_shortcut(&app, binding_to_modify.clone()) {
        let error_msg = format!("Failed to unregister shortcut: {}", e);
        error!("change_binding error: {}", error_msg);
    }

    // Validate the new shortcut for the current keyboard implementation
    if let Err(e) = validate_shortcut_for_implementation(&binding, settings.keyboard_implementation)
    {
        warn!("change_binding validation error: {}", e);
        restore_registration(&app, &binding_to_modify);
        return Err(e);
    }

    // Create an updated binding
    let mut updated_binding = binding_to_modify.clone();
    updated_binding.current_binding = binding;

    // Register the new binding
    if let Err(e) = register_shortcut(&app, updated_binding.clone()) {
        let error_msg = format!("Failed to register shortcut: {}", e);
        error!("change_binding error: {}", error_msg);
        restore_registration(&app, &binding_to_modify);
        return Ok(BindingResponse {
            success: false,
            binding: None,
            error: Some(error_msg),
        });
    }

    // Update the binding in the settings
    settings.bindings.insert(id, updated_binding.clone());

    // Save the settings and synchronize any active Secure Input shadows.
    settings::write_settings(&app, settings);
    crate::secure_input::reconcile_fallback(&app);

    // Return the updated binding
    Ok(BindingResponse {
        success: true,
        binding: Some(updated_binding),
        error: None,
    })
}

/// Best-effort re-register of the previous binding after a failed change,
/// so a failure leaves the user's shortcut working exactly as before.
fn restore_registration(app: &AppHandle, binding: &ShortcutBinding) {
    if let Err(e) = register_shortcut(app, binding.clone()) {
        error!(
            "Failed to restore previous binding '{}' ({}): {}",
            binding.id, binding.current_binding, e
        );
    }
}

#[tauri::command]
#[specta::specta]
pub fn reset_binding(app: AppHandle, id: String) -> Result<BindingResponse, String> {
    let binding = settings::get_stored_binding(&settings::get_settings(&app), &id)?;
    change_binding(app, id, binding.default_binding)
}

/// Unregister every binding while the user is recording a new shortcut in
/// the UI, so no existing shortcut can fire — or swallow the keystrokes —
/// mid-capture. The "cancel" binding is untouched: it is managed dynamically
/// by the recording lifecycle.
pub fn suspend_all_shortcuts(app: &AppHandle) {
    for (id, binding) in settings::get_bindings(app) {
        if id == "cancel" {
            continue;
        }
        if let Err(e) = unregister_shortcut(app, binding) {
            debug!(
                "suspend_all_shortcuts: could not unregister '{}': {}",
                id, e
            );
        }
    }
}

/// Re-register every binding from settings after shortcut recording ends.
/// Registering an already-registered shortcut fails cleanly in both
/// implementations, so this is idempotent and safe on every exit path.
pub fn resume_all_shortcuts(app: &AppHandle) {
    let settings = get_settings(app);
    for (id, binding) in &settings.bindings {
        if id == "cancel" {
            continue;
        }
        if id == "transcribe_with_post_process" && !settings.post_process_enabled {
            continue;
        }
        if let Err(e) = register_shortcut(app, binding.clone()) {
            debug!("resume_all_shortcuts: could not register '{}': {}", id, e);
        }
    }
}

/// Temporarily unregister all bindings while the user is recording a
/// shortcut in the UI. This avoids firing actions while keys are recorded.
#[tauri::command]
#[specta::specta]
pub fn suspend_all_bindings(app: AppHandle) -> Result<(), String> {
    suspend_all_shortcuts(&app);
    Ok(())
}

/// Re-register all bindings after the user has finished recording.
#[tauri::command]
#[specta::specta]
pub fn resume_all_bindings(app: AppHandle) -> Result<(), String> {
    resume_all_shortcuts(&app);
    Ok(())
}

// ============================================================================
// Keyboard Implementation Switching
// ============================================================================

/// Result of changing keyboard implementation
#[derive(Serialize, Type)]
pub struct ImplementationChangeResult {
    pub success: bool,
    /// List of binding IDs that were reset to defaults due to incompatibility
    pub reset_bindings: Vec<String>,
}

/// Change the keyboard implementation with runtime switching.
/// This will unregister all shortcuts from the old implementation,
/// validate shortcuts for the new implementation (resetting invalid ones to defaults),
/// and register them with the new implementation.
#[tauri::command]
#[specta::specta]
pub fn change_keyboard_implementation_setting(
    app: AppHandle,
    implementation: String,
) -> Result<ImplementationChangeResult, String> {
    let current_settings = settings::get_settings(&app);
    let current_impl = current_settings.keyboard_implementation;
    let new_impl = parse_keyboard_implementation(&implementation);

    // If same implementation, nothing to do
    if current_impl == new_impl {
        return Ok(ImplementationChangeResult {
            success: true,
            reset_bindings: vec![],
        });
    }

    info!(
        "Switching keyboard implementation from {:?} to {:?}",
        current_impl, new_impl
    );

    // Unregister all shortcuts from the current implementation
    unregister_all_shortcuts(&app, current_impl);

    // Update the setting
    let mut settings = settings::get_settings(&app);
    settings.keyboard_implementation = new_impl;
    settings::write_settings(&app, settings);

    // Carbon fallback registrations use the Tauri plugin. Remove them before
    // registering the full Tauri implementation to avoid duplicate conflicts.
    if new_impl == KeyboardImplementation::Tauri {
        crate::secure_input::reconcile_fallback(&app);
    }

    // Initialize new implementation if needed (HandyKeys needs state)
    if new_impl == KeyboardImplementation::HandyKeys && initialize_handy_keys_with_rollback(&app)? {
        // Shortcuts already registered during init.
        crate::secure_input::reconcile_fallback(&app);
        return Ok(ImplementationChangeResult {
            success: true,
            reset_bindings: vec![],
        });
    }

    // Register all shortcuts with new implementation, resetting invalid ones
    let reset_bindings = register_all_shortcuts_for_implementation(&app, new_impl);
    crate::secure_input::reconcile_fallback(&app);

    // Emit event to notify frontend of the change
    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "keyboard_implementation",
            "value": implementation,
            "reset_bindings": reset_bindings
        }),
    );

    info!("Keyboard implementation switched to {:?}", new_impl);

    Ok(ImplementationChangeResult {
        success: true,
        reset_bindings,
    })
}

/// Get the current keyboard implementation
#[tauri::command]
#[specta::specta]
pub fn get_keyboard_implementation(app: AppHandle) -> String {
    let settings = settings::get_settings(&app);
    match settings.keyboard_implementation {
        KeyboardImplementation::Tauri => "tauri".to_string(),
        KeyboardImplementation::HandyKeys => "handy_keys".to_string(),
    }
}

// ============================================================================
// Validation Helpers
// ============================================================================

/// Validate a shortcut for a specific implementation
fn validate_shortcut_for_implementation(
    raw: &str,
    implementation: KeyboardImplementation,
) -> Result<(), String> {
    match implementation {
        KeyboardImplementation::Tauri => tauri_impl::validate_shortcut(raw),
        KeyboardImplementation::HandyKeys => handy_keys::validate_shortcut(raw),
    }
}

/// Parse a keyboard implementation string into the enum
fn parse_keyboard_implementation(s: &str) -> KeyboardImplementation {
    match s {
        "tauri" => KeyboardImplementation::Tauri,
        "handy_keys" => KeyboardImplementation::HandyKeys,
        other => {
            warn!(
                "Invalid keyboard implementation '{}', defaulting to tauri",
                other
            );
            KeyboardImplementation::Tauri
        }
    }
}

/// Unregister all shortcuts for the current implementation
fn unregister_all_shortcuts(app: &AppHandle, implementation: KeyboardImplementation) {
    let bindings = settings::get_bindings(app);

    for (id, binding) in bindings {
        // Skip cancel shortcut as it's dynamically registered
        if id == "cancel" {
            continue;
        }

        let result = match implementation {
            KeyboardImplementation::Tauri => tauri_impl::unregister_shortcut(app, binding),
            KeyboardImplementation::HandyKeys => handy_keys::unregister_shortcut(app, binding),
        };

        if let Err(e) = result {
            warn!(
                "Failed to unregister shortcut '{}' during switch: {}",
                id, e
            );
        }
    }
}

/// Register all shortcuts for a specific implementation, validating and resetting invalid ones
fn register_all_shortcuts_for_implementation(
    app: &AppHandle,
    implementation: KeyboardImplementation,
) -> Vec<String> {
    let mut reset_bindings = Vec::new();
    let default_bindings = settings::get_default_settings().bindings;
    let mut current_settings = settings::get_settings(app);

    for (id, default_binding) in &default_bindings {
        // Skip cancel shortcut as it's dynamically registered
        if id == "cancel" {
            continue;
        }

        // Skip post-processing shortcut when the feature is disabled
        if id == "transcribe_with_post_process" && !current_settings.post_process_enabled {
            continue;
        }

        let mut binding = current_settings
            .bindings
            .get(id)
            .cloned()
            .unwrap_or_else(|| default_binding.clone());

        // Validate the shortcut for the target implementation
        if let Err(e) =
            validate_shortcut_for_implementation(&binding.current_binding, implementation)
        {
            info!(
                "Shortcut '{}' ({}) is invalid for {:?}: {}. Resetting to default.",
                id, binding.current_binding, implementation, e
            );

            // Reset to default
            binding.current_binding = default_binding.current_binding.clone();
            current_settings
                .bindings
                .insert(id.clone(), binding.clone());
            reset_bindings.push(id.clone());
        }

        // Register with the appropriate implementation
        let result = match implementation {
            KeyboardImplementation::Tauri => tauri_impl::register_shortcut(app, binding),
            KeyboardImplementation::HandyKeys => handy_keys::register_shortcut(app, binding),
        };

        if let Err(e) = result {
            error!(
                "Failed to register shortcut '{}' for {:?}: {}",
                id, implementation, e
            );
        }
    }

    // Save settings if any bindings were reset
    if !reset_bindings.is_empty() {
        settings::write_settings(app, current_settings);
    }

    reset_bindings
}

/// Initialize HandyKeys if not already initialized, with rollback on failure
fn initialize_handy_keys_with_rollback(app: &AppHandle) -> Result<bool, String> {
    if app.try_state::<handy_keys::HandyKeysState>().is_some() {
        return Ok(false); // Already initialized, caller should continue
    }

    if let Err(e) = handy_keys::init_shortcuts(app) {
        error!("Failed to initialize HandyKeys: {}", e);
        // Rollback to Tauri
        let mut settings = settings::get_settings(app);
        settings.keyboard_implementation = KeyboardImplementation::Tauri;
        settings::write_settings(app, settings);
        crate::secure_input::reconcile_fallback(app);
        tauri_impl::init_shortcuts(app);
        return Err(format!(
            "Failed to initialize HandyKeys: {}. Reverted to Tauri.",
            e
        ));
    }

    // init_shortcuts already registered shortcuts
    Ok(true)
}

// ============================================================================
// General Settings Commands
// ============================================================================

#[tauri::command]
#[specta::specta]
pub fn change_shortcut_activation_setting(
    app: AppHandle,
    activation: ShortcutActivation,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.shortcut_activation = activation;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_hold_threshold_ms_setting(app: AppHandle, ms: u64) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.hold_threshold_ms = ms;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_audio_feedback_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.audio_feedback = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_audio_feedback_volume_setting(app: AppHandle, volume: f32) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.audio_feedback_volume = volume;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_sound_theme_setting(app: AppHandle, theme: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let parsed = match theme.as_str() {
        "marimba" => SoundTheme::Marimba,
        "pop" => SoundTheme::Pop,
        "custom" => SoundTheme::Custom,
        other => {
            warn!("Invalid sound theme '{}', defaulting to marimba", other);
            SoundTheme::Marimba
        }
    };
    settings.sound_theme = parsed;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn change_theme_setting(app: AppHandle, theme: String) -> Result<(), String> {
    let parsed = parse_theme_setting(&theme);
    // One gated commit: persists the theme, bumps the appearance rev and publishes the snapshot
    // to the consumer windows and the legacy `theme-changed` event (appearance.rs).
    crate::appearance::commit_appearance(&app, |fields| fields.theme = parsed);
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    apply_native_theme_on_main(&app, parsed);
    Ok(())
}

/// Validates and persists a custom color palette, re-applying native window
/// chrome if `Custom` is the currently active theme. `colors` is already
/// guaranteed to contain only valid `#rrggbb` hex strings by its own
/// `Deserialize` impl (see `settings::CustomThemeColors`) -- IPC deserialization
/// rejects an invalid payload before this command body ever runs, so there is
/// nothing left to re-validate here.
#[tauri::command]
#[specta::specta]
pub async fn update_custom_theme_colors(
    app: AppHandle,
    colors: settings::CustomThemeColors,
) -> Result<(), String> {
    // One gated commit, as for `change_theme_setting`; the snapshot carries the colours only
    // while the theme is `Custom`.
    crate::appearance::commit_appearance(&app, |fields| fields.custom_theme_colors = colors);
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    if settings::get_settings(&app).theme == Theme::Custom {
        apply_native_theme_on_main(&app, Theme::Custom);
    }
    Ok(())
}

/// Applies the native window chrome for `theme` on the main thread, where the window calls
/// run. The custom background is read from the store.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn apply_native_theme_on_main(app: &AppHandle, theme: Theme) {
    let background = settings::get_settings(app).custom_theme_colors.background;
    let handle = app.clone();
    if let Err(e) = app.run_on_main_thread(move || apply_window_theme(&handle, theme, &background))
    {
        log::warn!("native window theme could not be scheduled: {e}");
    }
}

/// Pure parser for the `theme` setting string -> `Theme`. Extracted from
/// `change_theme_setting` so the mapping (including the fallback-to-System
/// behavior for an unrecognized value) is unit-testable without an `AppHandle`.
fn parse_theme_setting(theme: &str) -> Theme {
    match theme {
        "system" => Theme::System,
        "light" => Theme::Light,
        "dark" => Theme::Dark,
        "aurora" => Theme::Aurora,
        "custom" => Theme::Custom,
        other => {
            warn!("Invalid theme '{}', defaulting to system", other);
            Theme::System
        }
    }
}

/// Applies the appearance setting to the native window chrome (title bar), which
/// CSS `data-theme` cannot reach. `System` clears the override so the window
/// follows the OS. Call this on startup and whenever the setting changes to keep
/// the title bar in sync with the in-app palette.
///
/// On Windows this themes the title bar only. On macOS `set_theme` sets
/// `NSApp.appearance` app-wide, which is what we want here: it darkens the title
/// bar and keeps the overlay in step. Linux is left to `data-theme` alone, since
/// its window theming is backend-dependent and unreliable.
///
/// `custom_background_hex` is only consulted for `Theme::Custom` (every other
/// variant ignores it) -- pass `&settings.custom_theme_colors.background`.
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub fn apply_window_theme(app: &AppHandle, theme: Theme, custom_background_hex: &str) {
    let window_theme = window_theme_for(theme, custom_background_hex);
    if let Some(window) = app.get_webview_window("main") {
        if let Err(e) = window.set_theme(window_theme) {
            warn!("Failed to apply window theme: {}", e);
        }
    }
}

/// Pure mapping from the app's `Theme` setting to the native window chrome
/// theme. Extracted from `apply_window_theme` so it's unit-testable without an
/// `AppHandle`/real window. `System` clears the override (native chrome follows
/// the OS); `Aurora` is a dark-based palette (see theme.css) so it forces the
/// matching native chrome rather than leaving it to follow the OS, same as
/// `Dark`. `Custom` can be either light- or dark-based depending on what the
/// user picked, so it's resolved from the actual background color's luminance
/// (see `settings::native_chrome_for_hex_background`) instead of a fixed arm.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn window_theme_for(theme: Theme, custom_background_hex: &str) -> Option<tauri::Theme> {
    match theme {
        Theme::System => None,
        Theme::Light => Some(tauri::Theme::Light),
        Theme::Dark => Some(tauri::Theme::Dark),
        Theme::Aurora => Some(tauri::Theme::Dark),
        Theme::Custom => Some(settings::native_chrome_for_hex_background(
            custom_background_hex,
        )),
    }
}

#[tauri::command]
#[specta::specta]
pub fn change_translate_to_english_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.translate_to_english = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_selected_language_setting(app: AppHandle, language: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.selected_language = language;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_overlay_position_setting(app: AppHandle, position: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let parsed = match position.as_str() {
        // "none" is retired (visibility is overlay_style now); fold legacy callers
        // onto Bottom rather than warn.
        "none" | "bottom" => OverlayPosition::Bottom,
        "top" => OverlayPosition::Top,
        other => {
            warn!("Invalid overlay position '{}', defaulting to bottom", other);
            OverlayPosition::Bottom
        }
    };
    settings.overlay_position = parsed;
    settings::write_settings(&app, settings);

    // Whether the overlay shows at all is owned by overlay_style now; position
    // only ever toggles Top/Bottom, so the enabled cache is untouched here.
    // Update overlay position without recreating window
    crate::utils::update_overlay_position(&app);

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_overlay_style_setting(app: AppHandle, style: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let parsed = match style.as_str() {
        "none" => OverlayStyle::None,
        "minimal" => OverlayStyle::Minimal,
        "live" => OverlayStyle::Live,
        other => {
            warn!("Invalid overlay style '{}', defaulting to minimal", other);
            OverlayStyle::Minimal
        }
    };
    settings.overlay_style = parsed;
    settings::write_settings(&app, settings);

    // Keep the cached overlay-enabled flag in sync so emit_levels stops (or
    // resumes) emitting on the next audio callback.
    crate::overlay::update_overlay_enabled_cache(parsed != OverlayStyle::None);

    // Reposition in case the window needs to re-center for the new style.
    crate::utils::update_overlay_position(&app);

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_debug_mode_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.debug_mode = enabled;
    settings::write_settings(&app, settings);

    // Keep webview log streaming in sync: the live log viewer only exists in
    // debug mode, so logs are forwarded to the frontend only while it is on.
    crate::WEBVIEW_LOG_STREAMING.store(enabled, std::sync::atomic::Ordering::Relaxed);

    // Emit event to notify frontend of debug mode change
    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "debug_mode",
            "value": enabled
        }),
    );

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_start_hidden_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.start_hidden = enabled;
    settings::write_settings(&app, settings);

    // Notify frontend
    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "start_hidden",
            "value": enabled
        }),
    );

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_autostart_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.autostart_enabled = enabled;
    settings::write_settings(&app, settings);

    // Apply the autostart setting immediately
    crate::autostart::apply_autostart(&app, enabled);

    // Notify frontend
    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "autostart_enabled",
            "value": enabled
        }),
    );

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn change_update_checks_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    // `write_settings` takes the update coordinator when this setting changes, and a commit can
    // hold it for seconds: run on a blocking thread so the main thread never waits for it.
    tauri::async_runtime::spawn_blocking(move || apply_update_checks_setting(app, enabled))
        .await
        .map_err(|_| "The update checks setting could not be changed.".to_string())?
}

/// The blocking body of [`change_update_checks_setting`].
fn apply_update_checks_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    if settings::update_checks_forced_disabled() {
        return Err(
            "Update checks are disabled by system configuration (HANDY_DISABLE_UPDATER)".into(),
        );
    }

    // Written and announced under the command mutex; the announced value is read back.
    let applied = write_read_back_and_announce(
        || {
            let mut settings = settings::get_settings(&app);
            settings.update_checks_enabled = enabled;
            settings::write_settings(&app, settings);
        },
        || settings::get_settings(&app).update_checks_enabled,
        |value| {
            let _ = app.emit(
                "settings-changed",
                serde_json::json!({
                    "setting": "update_checks_enabled",
                    "value": value
                }),
            );
        },
    );

    if applied {
        // Check right away instead of waiting for the next scheduled run.
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = crate::managers::source_update::check_and_publish(&app2, true).await;
        });
    } else {
        // Turning checks off hides any cached "update available" immediately.
        crate::managers::source_update::clear(&app);
    }

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_show_whats_new_on_update_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.show_whats_new_on_update = enabled;
    settings::write_settings(&app, settings);

    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "show_whats_new_on_update",
            "value": enabled
        }),
    );

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_whats_new_last_seen_version_setting(
    app: AppHandle,
    version: String,
) -> Result<(), String> {
    let version = version.trim().to_string();
    let mut settings = settings::get_settings(&app);
    settings.whats_new_last_seen_version = version.clone();
    settings::write_settings(&app, settings);

    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "whats_new_last_seen_version",
            "value": version
        }),
    );

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn update_custom_words(app: AppHandle, words: Vec<String>) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.custom_words = words;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_word_correction_threshold_setting(
    app: AppHandle,
    threshold: f64,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.word_correction_threshold = threshold;
    settings::write_settings(&app, settings);
    Ok(())
}

/// Validates and stores the wake-word confidence threshold. A rejected value
/// leaves the stored setting untouched.
#[tauri::command]
#[specta::specta]
pub fn change_wake_word_confidence_threshold_setting(
    app: AppHandle,
    threshold: f32,
) -> Result<(), String> {
    let threshold = settings::validate_wake_word_confidence_threshold(threshold)?;
    let mut settings = settings::get_settings(&app);
    settings.wake_word_confidence_threshold = threshold;
    settings::write_settings(&app, settings);
    Ok(())
}

/// Validates and stores the wake-word cooldown. A rejected value leaves the
/// stored setting untouched.
#[tauri::command]
#[specta::specta]
pub fn change_wake_word_cooldown_ms_setting(
    app: AppHandle,
    cooldown_ms: u32,
) -> Result<(), String> {
    let cooldown_ms = settings::validate_wake_word_cooldown_ms(cooldown_ms)?;
    let mut settings = settings::get_settings(&app);
    settings.wake_word_cooldown_ms = cooldown_ms;
    settings::write_settings(&app, settings);
    Ok(())
}

/// Validates and stores the stored-only wake-word silence timeout. No UI control and no
/// code path reads it any more; kept so the field round-trips for downgrades.
/// A rejected value leaves the stored setting untouched.
#[tauri::command]
#[specta::specta]
pub fn change_wake_word_silence_timeout_ms_setting(
    app: AppHandle,
    timeout_ms: u32,
) -> Result<(), String> {
    let timeout_ms = settings::validate_wake_word_silence_timeout_ms(timeout_ms)?;
    let mut settings = settings::get_settings(&app);
    settings.wake_word_silence_timeout_ms = timeout_ms;
    settings::write_settings(&app, settings);
    Ok(())
}

/// Validates and stores the ambient wake name (trimmed). A rejected value leaves
/// the stored setting untouched. Takes effect after restart.
#[tauri::command]
#[specta::specta]
pub fn change_ambient_wake_name_setting(app: AppHandle, name: String) -> Result<(), String> {
    let name = settings::validate_ambient_wake_name(&name)?;
    let mut settings = settings::get_settings(&app);
    settings.ambient_wake_name = name;
    settings::write_settings(&app, settings);
    Ok(())
}

/// Validates and stores the ambient conversation window in milliseconds. A rejected
/// value leaves the stored setting untouched. Takes effect after restart.
#[tauri::command]
#[specta::specta]
pub fn change_ambient_window_ms_setting(app: AppHandle, window_ms: u64) -> Result<(), String> {
    let window_ms = settings::validate_ambient_window_ms(window_ms)?;
    let mut settings = settings::get_settings(&app);
    settings.ambient_window_ms = window_ms;
    settings::write_settings(&app, settings);
    Ok(())
}

/// Validates and stores the custom filler word list. `None` restores the built-in lists,
/// `Some([])` turns filler removal off. A rejected list leaves the stored setting untouched.
#[tauri::command]
#[specta::specta]
pub fn change_custom_filler_words_setting(
    app: AppHandle,
    words: Option<Vec<String>>,
) -> Result<(), String> {
    let words = settings::validate_custom_filler_words(words)?;
    let mut settings = settings::get_settings(&app);
    settings.custom_filler_words = words;
    settings::write_settings(&app, settings);
    Ok(())
}

/// Validates and stores the agent bridge prompt flag. The flag must have a valid shape and
/// must differ from the effective agent flag. A rejected flag leaves the stored setting
/// untouched.
#[tauri::command]
#[specta::specta]
pub fn change_agent_bridge_prompt_flag_setting(app: AppHandle, flag: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let flag = settings::validate_agent_bridge_prompt_flag(&settings, &flag)?;
    settings.agent_bridge_prompt_flag = flag;
    settings::write_settings(&app, settings);
    Ok(())
}

/// Retires the managed ACP agent child, if one is managed. `shutdown` retires the child and then
/// waits for it (`Conn::terminate`, up to about 2 s), so the settings generation stays odd for that
/// time. A launch prepared or admitted in that window is refused with a transient "settings
/// changing" error; that is acceptable and safe. The shutdown runs inside the gate's update, after
/// the setting is applied in memory, and before `write_settings_gated` writes the store; the order
/// is kept because the store write lives in `settings.rs`, outside this closure.
fn shutdown_managed_acp_agent(app: &AppHandle) {
    if let Some(manager) = app.try_state::<Arc<crate::managers::acp_agent::AcpAgentManager>>() {
        manager.inner().shutdown();
    }
}

/// Enables or disables the persistent ACP agent client. Turning it off also retires the
/// running agent so it does not linger until the idle timeout.
#[tauri::command]
#[specta::specta]
pub fn change_acp_agent_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    // Why: the shutdown runs inside the gate's update (F3), so a launch prepared under the new
    // generation cannot be retired by a trailing shutdown.
    settings::write_settings_gated(&app, |settings| {
        settings.acp_agent_enabled = enabled;
        if !enabled {
            shutdown_managed_acp_agent(&app);
        }
    });
    Ok(())
}

/// Turns "Send dictation to the agent" on or off. This is a plain setting write: it never
/// starts or retires the agent process (only the agent switch does that). Main window only.
#[tauri::command]
#[specta::specta]
pub fn change_acp_dictation_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::write_settings_gated(&app, |settings| {
        settings.acp_dictation_enabled = enabled;
    });
    Ok(())
}

/// Validates and stores the ACP agent command. Blank or missing means unset. A rejected
/// command leaves the stored setting untouched.
#[tauri::command]
#[specta::specta]
pub fn change_acp_agent_command_setting(
    app: AppHandle,
    command: Option<String>,
) -> Result<(), String> {
    let normalized = settings::normalize_acp_agent_command(command.as_deref().unwrap_or(""))?;
    let changed = settings::get_settings(&app).acp_agent_command != normalized;
    // Why: inside the gate's update (F3); see `change_acp_agent_enabled_setting`.
    settings::write_settings_gated(&app, |settings| {
        settings.acp_agent_command = normalized;
        if changed {
            shutdown_managed_acp_agent(&app);
        }
    });
    Ok(())
}

/// Validates and stores the ACP agent arguments. A rejected value leaves the stored setting
/// untouched.
#[tauri::command]
#[specta::specta]
pub fn change_acp_agent_args_setting(app: AppHandle, args: String) -> Result<(), String> {
    settings::validate_acp_agent_args(&args)?;
    let changed = settings::get_settings(&app).acp_agent_args != args;
    // Why: inside the gate's update (F3); see `change_acp_agent_enabled_setting`.
    settings::write_settings_gated(&app, |settings| {
        settings.acp_agent_args = args;
        if changed {
            shutdown_managed_acp_agent(&app);
        }
    });
    Ok(())
}

/// Validates and stores the ACP agent turn timeout in seconds. A rejected value leaves the
/// stored setting untouched.
#[tauri::command]
#[specta::specta]
pub fn change_acp_agent_turn_timeout_setting(app: AppHandle, secs: u64) -> Result<(), String> {
    let secs = settings::validate_acp_agent_turn_timeout_secs(secs)?;
    settings::write_settings_gated(&app, |settings| settings.acp_agent_turn_timeout_secs = secs);
    Ok(())
}

/// Stores or clears one specialist agent (Stage 4). `step_type` is `research`, `browser`,
/// `code` or `files`. A blank command clears the slot. The command and arguments are validated
/// before any write, so a rejected value leaves the setting and the running agent untouched.
#[tauri::command]
#[specta::specta]
pub fn change_acp_step_agent_setting(
    app: AppHandle,
    step_type: String,
    command: Option<String>,
    args: String,
) -> Result<(), String> {
    // Validate on a copy first: nothing is written and no agent is retired on a rejected value.
    let mut candidate = settings::get_settings(&app);
    let before = candidate.acp_step_agents.clone();
    settings::apply_step_agent_setting(&mut candidate, &step_type, command.as_deref(), &args)?;
    let changed = candidate.acp_step_agents != before;
    settings::write_settings_gated(&app, |settings| {
        // Re-applied to the settings read inside the gate, so a concurrent change to another
        // field is kept. The values were validated above.
        let _ = settings::apply_step_agent_setting(settings, &step_type, command.as_deref(), &args);
        // Why: inside the gate's update (F3); see `change_acp_agent_enabled_setting`.
        if changed {
            shutdown_managed_acp_agent(&app);
        }
    });
    Ok(())
}

/// Spawns the configured ACP agent and completes the handshake, without sending a prompt.
/// Requires the agent to be enabled and configured. Errors are short and user-safe.
#[tauri::command]
#[specta::specta]
pub async fn acp_agent_test_connection(app: AppHandle) -> Result<String, String> {
    const GENERIC_FAILURE: &str = "Could not connect to the ACP agent.";
    let settings = settings::get_settings(&app);
    if !settings.acp_agent_enabled {
        return Err("Turn on the ACP agent before testing the connection.".to_string());
    }
    let cfg = crate::actions::acp_agent_config(&app, &settings)?;
    let manager = app
        .try_state::<Arc<crate::managers::acp_agent::AcpAgentManager>>()
        .map(|state| state.inner().clone())
        .ok_or_else(|| GENERIC_FAILURE.to_string())?;
    let outcome = tauri::async_runtime::spawn_blocking(move || manager.warm_up(&cfg))
        .await
        .map_err(|_| GENERIC_FAILURE.to_string())?;
    match outcome {
        Ok(Some(name)) => {
            let name: String = name.chars().filter(|c| !c.is_control()).take(80).collect();
            let name = name.trim();
            if name.is_empty() {
                Ok("Connected".to_string())
            } else {
                Ok(format!("Connected to {name}"))
            }
        }
        Ok(None) => Ok("Connected".to_string()),
        Err(error) => Err(crate::managers::acp_escalation::user_message_for(&error)
            .unwrap_or_else(|| GENERIC_FAILURE.to_string())),
    }
}

/// Normalizes a voice name submitted from the settings UI.
///
/// Blank or missing input means "system default" (`Ok(None)`). A non-blank
/// name is trimmed and must pass [`crate::managers::tts::sanitize_voice`];
/// anything else is rejected so an unsafe value is never persisted.
pub fn normalize_tts_voice(voice: Option<String>) -> Result<Option<String>, String> {
    let Some(raw) = voice else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    crate::managers::tts::sanitize_voice(&raw)
        .map(Some)
        .ok_or_else(|| "Invalid voice name".to_string())
}

/// Maps a speech error to a short, content-free message for the UI. Speech
/// text and backend output are never included.
pub fn tts_error_message(error: &crate::managers::tts::TtsError) -> String {
    use crate::managers::tts::TtsError;
    match error {
        TtsError::Empty => "Nothing to speak".to_string(),
        TtsError::Recording => "Cannot speak while recording".to_string(),
        TtsError::CancelFailed => "Previous speech could not be stopped".to_string(),
        TtsError::Unsupported => "Spoken replies are not supported on this platform".to_string(),
        TtsError::Backend(_) => "Speech output failed".to_string(),
        TtsError::Superseded => "Replaced by newer speech".to_string(),
    }
}

#[tauri::command]
#[specta::specta]
pub fn change_tts_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.tts_enabled = enabled;
    settings::write_settings(&app, settings);
    // M4: any change, on or off, invalidates the summaries issued under the old switch.
    let summary_tokens = app
        .try_state::<crate::job_runtime::JobWiring>()
        .map(|wiring| wiring.summary_tokens.clone());
    crate::job_runtime::invalidate_summaries(summary_tokens.as_ref());
    if let Some(tts) = app.try_state::<Arc<crate::managers::tts::TtsManager>>() {
        if !enabled {
            tts.stop();
        }
        tts.invalidate();
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_tts_voice_setting(app: AppHandle, voice: Option<String>) -> Result<(), String> {
    let voice = normalize_tts_voice(voice)?;
    let mut settings = settings::get_settings(&app);
    settings.tts_voice = voice;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_tts_rate_setting(app: AppHandle, rate: u32) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.tts_rate_wpm = crate::managers::tts::clamp_rate(rate);
    settings::write_settings(&app, settings);
    Ok(())
}

/// After a spoken-replies engine/voice change: stop current speech and let a failed natural
/// voice retry (best effort; both managers may be absent).
pub(crate) fn reset_spoken_replies(app: &AppHandle) {
    if let Some(tts) = app.try_state::<Arc<crate::managers::tts::TtsManager>>() {
        let _ = tts.stop();
    }
    if let Some(health) = app.try_state::<Arc<crate::managers::natural_engine::NaturalHealth>>() {
        health.bump();
    }
    if let Some(phonon) = app.try_state::<crate::commands::phonon_voice::PhononHealth>() {
        phonon.0.bump();
    }
}

#[tauri::command]
#[specta::specta]
pub fn change_tts_engine_setting(
    app: AppHandle,
    engine: crate::settings::TtsEngine,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    crate::managers::tts_engines::validate_engine_selection(
        settings.tts_engine,
        engine,
        &crate::managers::tts_engines::tts_engine_catalog(),
    )?;
    settings.tts_engine = engine;
    settings::write_settings(&app, settings);
    reset_spoken_replies(&app);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_tts_natural_voice_setting(app: AppHandle, voice: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.tts_natural_voice = crate::managers::natural_voices::normalize_natural_voice(&voice);
    settings::write_settings(&app, settings);
    reset_spoken_replies(&app);
    Ok(())
}

/// Sets whether Tamil text is spoken with the Tamil neural voice on `settings`. Pure, so the
/// stored value can be tested without Tauri. The router reads the live value per burst, so no
/// speech reset is needed (a running Tamil burst is not cut by this switch).
pub fn apply_tts_tamil_enabled(settings: &mut crate::settings::AppSettings, enabled: bool) {
    settings.tts_tamil_enabled = enabled;
}

/// Stores whether Tamil text is spoken with the Tamil neural voice when its pack is installed.
#[tauri::command]
#[specta::specta]
pub fn change_tts_tamil_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    apply_tts_tamil_enabled(&mut settings, enabled);
    settings::write_settings(&app, settings);
    Ok(())
}

/// Sets whether Tamil speech is recognised with the Tamil speech pack on `settings`. Pure, so
/// the stored value can be tested without Tauri. The route reads the live value on every call.
pub fn apply_stt_tamil_enabled(settings: &mut crate::settings::AppSettings, enabled: bool) {
    settings.stt_tamil_enabled = enabled;
}

/// Stores whether Tamil speech is recognised with the Tamil speech pack when it is installed.
#[tauri::command]
#[specta::specta]
pub fn change_stt_tamil_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    apply_stt_tamil_enabled(&mut settings, enabled);
    settings::write_settings(&app, settings);
    Ok(())
}

/// Sets whether the experimental auto-language check is on in `settings`. Pure, so the stored
/// value can be tested without Tauri. Off by default (opt-in).
pub fn apply_stt_auto_language_enabled(settings: &mut crate::settings::AppSettings, enabled: bool) {
    settings.stt_auto_language_enabled = enabled;
}

/// Stores whether the experimental auto-language check is on (opt-in, off by default).
#[tauri::command]
#[specta::specta]
pub fn change_stt_auto_language_enabled_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    apply_stt_auto_language_enabled(&mut settings, enabled);
    settings::write_settings(&app, settings);
    Ok(())
}

/// Speaks a short test phrase with the saved voice and rate. Works even when
/// spoken replies are disabled so the user can audition a voice first. The
/// text is never logged.
#[tauri::command]
#[specta::specta]
pub fn tts_speak_test(app: AppHandle, text: String) -> Result<(), String> {
    let tts = app
        .try_state::<Arc<crate::managers::tts::TtsManager>>()
        .ok_or_else(|| "Spoken replies are not available".to_string())?;
    let settings = settings::get_settings(&app);
    tts.speak(
        &text,
        settings.tts_voice.as_deref(),
        Some(settings.tts_rate_wpm),
    )
    .map_err(|error| tts_error_message(&error))
}

/// Stops any spoken output that is currently playing.
#[tauri::command]
#[specta::specta]
pub fn tts_stop(app: AppHandle) -> Result<(), String> {
    if let Some(tts) = app.try_state::<Arc<crate::managers::tts::TtsManager>>() {
        if !tts.stop() {
            return Err("Speech could not be stopped".to_string());
        }
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_extra_recording_buffer_setting(app: AppHandle, ms: u64) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.extra_recording_buffer_ms = ms;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_paste_delay_ms_setting(app: AppHandle, ms: u64) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.paste_delay_ms = ms;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_paste_delay_after_ms_setting(app: AppHandle, ms: u64) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.paste_delay_after_ms = ms;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_reliable_paste_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.reliable_paste = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_paste_method_setting(app: AppHandle, method: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let parsed = match method.as_str() {
        "ctrl_v" => PasteMethod::CtrlV,
        "direct" => PasteMethod::Direct,
        "none" => PasteMethod::None,
        "shift_insert" => PasteMethod::ShiftInsert,
        "ctrl_shift_v" => PasteMethod::CtrlShiftV,
        "external_script" => PasteMethod::ExternalScript,
        other => {
            warn!("Invalid paste method '{}', defaulting to ctrl_v", other);
            PasteMethod::CtrlV
        }
    };
    settings.paste_method = parsed;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn get_available_typing_tools() -> Vec<String> {
    #[cfg(target_os = "linux")]
    {
        crate::clipboard::get_available_typing_tools()
    }
    #[cfg(not(target_os = "linux"))]
    {
        vec!["auto".to_string()]
    }
}

#[tauri::command]
#[specta::specta]
pub fn change_typing_tool_setting(app: AppHandle, tool: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let parsed = match tool.as_str() {
        "auto" => TypingTool::Auto,
        "wtype" => TypingTool::Wtype,
        "kwtype" => TypingTool::Kwtype,
        "dotool" => TypingTool::Dotool,
        "ydotool" => TypingTool::Ydotool,
        "xdotool" => TypingTool::Xdotool,
        other => {
            warn!("Invalid typing tool '{}', defaulting to auto", other);
            TypingTool::Auto
        }
    };
    settings.typing_tool = parsed;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_external_script_path_setting(
    app: AppHandle,
    path: Option<String>,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.external_script_path = path;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_clipboard_handling_setting(app: AppHandle, handling: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let parsed = match handling.as_str() {
        "dont_modify" => ClipboardHandling::DontModify,
        "copy_to_clipboard" => ClipboardHandling::CopyToClipboard,
        other => {
            warn!(
                "Invalid clipboard handling '{}', defaulting to dont_modify",
                other
            );
            ClipboardHandling::DontModify
        }
    };
    settings.clipboard_handling = parsed;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_auto_submit_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.auto_submit = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_auto_submit_key_setting(app: AppHandle, key: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let parsed = match key.as_str() {
        "enter" => AutoSubmitKey::Enter,
        "ctrl_enter" => AutoSubmitKey::CtrlEnter,
        "cmd_enter" => AutoSubmitKey::CmdEnter,
        other => {
            warn!("Invalid auto submit key '{}', defaulting to enter", other);
            AutoSubmitKey::Enter
        }
    };
    settings.auto_submit_key = parsed;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_post_process_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.post_process_enabled = enabled;
    settings::write_settings(&app, settings.clone());

    // Register or unregister the post-processing shortcut
    if let Some(binding) = settings
        .bindings
        .get("transcribe_with_post_process")
        .cloned()
    {
        if enabled {
            let _ = register_shortcut(&app, binding);
        } else {
            let _ = unregister_shortcut(&app, binding);
        }
    }

    crate::secure_input::reconcile_fallback(&app);
    Ok(())
}

/// Maximum accepted length of the agent binary path setting.
pub(crate) const MAX_AGENT_BINARY_PATH_CHARS: usize = 1024;

/// Normalizes the agent-bridge binary path before it is persisted: trims
/// surrounding whitespace, maps blank to `None`, and rejects control
/// characters (including NUL and newlines) and anything longer than
/// [`MAX_AGENT_BINARY_PATH_CHARS`] characters. Existence is NOT checked here:
/// the user may configure the path before installing the agent, and the bridge
/// already reports a missing binary when it is used.
pub(crate) fn normalize_agent_bridge_binary_path(
    input: Option<String>,
) -> Result<Option<String>, String> {
    let Some(raw) = input else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err("Agent binary path must not contain control characters".to_string());
    }
    if trimmed.chars().count() > MAX_AGENT_BINARY_PATH_CHARS {
        return Err(format!(
            "Agent binary path must be at most {MAX_AGENT_BINARY_PATH_CHARS} characters"
        ));
    }
    Ok(Some(trimmed.to_string()))
}

#[tauri::command]
#[specta::specta]
pub fn change_voice_commands_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.voice_commands_enabled = enabled;
    settings::write_settings(&app, settings);
    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "voice_commands_enabled",
            "value": enabled
        }),
    );
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_memory_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.memory_enabled = enabled;
    settings::write_settings(&app, settings);
    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "memory_enabled",
            "value": enabled
        }),
    );
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_agent_bridge_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.agent_bridge_enabled = enabled;
    settings::write_settings(&app, settings);
    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "agent_bridge_enabled",
            "value": enabled
        }),
    );
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_agent_bridge_binary_path_setting(
    app: AppHandle,
    path: Option<String>,
) -> Result<(), String> {
    // Validate before touching settings so a rejected path leaves the stored
    // value unchanged.
    let normalized = normalize_agent_bridge_binary_path(path)?;
    let mut settings = settings::get_settings(&app);
    settings.agent_bridge_binary_path = normalized.clone();
    settings::write_settings(&app, settings);
    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "agent_bridge_binary_path",
            "value": normalized
        }),
    );
    Ok(())
}

/// What an Experimental switch change does to ambient listening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmbientTransition {
    Stop,
    Start,
    None,
}

/// Pure: the ambient listener's action for an Experimental change. Turning Experimental off
/// stops ambient listening (the stored flag is left as it is). Turning it on starts ambient
/// only when the stored flag is on and the wake word is off. An unchanged value is a no-op.
pub fn ambient_transition_on_experimental_change(
    prev_experimental: bool,
    next_experimental: bool,
    ambient_flag: bool,
    wake_enabled: bool,
) -> AmbientTransition {
    match (prev_experimental, next_experimental) {
        (true, false) => AmbientTransition::Stop,
        (false, true) if ambient_flag && !wake_enabled => AmbientTransition::Start,
        _ => AmbientTransition::None,
    }
}

#[tauri::command]
#[specta::specta]
pub fn change_experimental_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let prev_experimental = settings.experimental_enabled;
    settings.experimental_enabled = enabled;
    let transition = ambient_transition_on_experimental_change(
        prev_experimental,
        enabled,
        settings.ambient_mode_enabled,
        settings.wake_word_enabled,
    );
    settings::write_settings(&app, settings);
    let ambient = app.state::<Arc<crate::managers::ambient_listener::AmbientListener>>();
    match transition {
        AmbientTransition::Stop => ambient.stop(),
        AmbientTransition::Start => ambient.start(),
        AmbientTransition::None => {}
    }
    tray::update_tray_menu(&app);
    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "experimental_enabled",
            "value": enabled
        }),
    );
    Ok(())
}

/// Persists the `voice_media_controls_enabled` setting (opt-in media control
/// phrases such as "pause" or "volume 40"; only effective while Voice
/// Commands is on).
#[tauri::command]
#[specta::specta]
pub fn change_voice_media_controls_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.voice_media_controls_enabled = enabled;
    settings::write_settings(&app, settings);
    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "voice_media_controls_enabled",
            "value": enabled
        }),
    );
    Ok(())
}

/// Persists the `agent_context_enabled` setting. Turning it off also clears the
/// in-memory conversation log so re-enabling cannot resurrect old turns.
#[tauri::command]
#[specta::specta]
pub fn change_agent_context_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.agent_context_enabled = enabled;
    settings::write_settings(&app, settings);
    let log = app
        .try_state::<Arc<crate::managers::conversation::ConversationLog>>()
        .map(|state| state.inner().clone());
    match app.try_state::<Arc<crate::managers::acp_agent::AcpAgentManager>>() {
        Some(manager) => crate::managers::acp_escalation::on_context_setting_changed(
            enabled,
            log.as_deref(),
            &**manager.inner(),
        ),
        None => {
            if !enabled {
                if let Some(log) = log.as_deref() {
                    log.clear();
                }
            }
        }
    }
    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "agent_context_enabled",
            "value": enabled
        }),
    );
    Ok(())
}

/// Clears the in-memory agent conversation history.
#[tauri::command]
#[specta::specta]
pub fn agent_clear_conversation(app: AppHandle) -> Result<(), String> {
    let log = app
        .try_state::<Arc<crate::managers::conversation::ConversationLog>>()
        .map(|state| state.inner().clone());
    match app.try_state::<Arc<crate::managers::acp_agent::AcpAgentManager>>() {
        // Always resets the ACP session, even when no conversation log is managed.
        Some(manager) => {
            crate::managers::acp_escalation::on_history_cleared(log.as_deref(), &**manager.inner())
        }
        None => {
            if let Some(log) = log.as_deref() {
                log.clear();
            }
        }
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_post_process_base_url_setting(
    app: AppHandle,
    provider_id: String,
    base_url: String,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let label = settings
        .post_process_provider(&provider_id)
        .map(|provider| provider.label.clone())
        .ok_or_else(|| format!("Provider '{}' not found", provider_id))?;

    let provider = settings
        .post_process_provider_mut(&provider_id)
        .expect("Provider looked up above must exist");

    if provider.id != "custom" {
        return Err(format!(
            "Provider '{}' does not allow editing the base URL",
            label
        ));
    }

    provider.base_url = base_url;
    settings::write_settings(&app, settings);
    Ok(())
}

/// Generic helper to validate provider exists
fn validate_provider_exists(
    settings: &settings::AppSettings,
    provider_id: &str,
) -> Result<(), String> {
    if !settings
        .post_process_providers
        .iter()
        .any(|provider| provider.id == provider_id)
    {
        return Err(format!("Provider '{}' not found", provider_id));
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_post_process_api_key_setting(
    app: AppHandle,
    provider_id: String,
    api_key: String,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    validate_provider_exists(&settings, &provider_id)?;
    settings.post_process_api_keys.insert(provider_id, api_key);
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_post_process_model_setting(
    app: AppHandle,
    provider_id: String,
    model: String,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    validate_provider_exists(&settings, &provider_id)?;
    settings.post_process_models.insert(provider_id, model);
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn set_post_process_provider(app: AppHandle, provider_id: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    validate_provider_exists(&settings, &provider_id)?;
    settings.post_process_provider_id = provider_id;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn add_post_process_prompt(
    app: AppHandle,
    name: String,
    prompt: String,
) -> Result<LLMPrompt, String> {
    let mut settings = settings::get_settings(&app);

    // Generate unique ID using timestamp and random component
    let id = format!("prompt_{}", chrono::Utc::now().timestamp_millis());

    let new_prompt = LLMPrompt {
        id: id.clone(),
        name,
        prompt,
    };

    settings.post_process_prompts.push(new_prompt.clone());
    settings::write_settings(&app, settings);

    Ok(new_prompt)
}

#[tauri::command]
#[specta::specta]
pub fn update_post_process_prompt(
    app: AppHandle,
    id: String,
    name: String,
    prompt: String,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);

    if let Some(existing_prompt) = settings
        .post_process_prompts
        .iter_mut()
        .find(|p| p.id == id)
    {
        existing_prompt.name = name;
        existing_prompt.prompt = prompt;
        settings::write_settings(&app, settings);
        Ok(())
    } else {
        Err(format!("Prompt with id '{}' not found", id))
    }
}

#[tauri::command]
#[specta::specta]
pub fn delete_post_process_prompt(app: AppHandle, id: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);

    // Don't allow deleting the last prompt
    if settings.post_process_prompts.len() <= 1 {
        return Err("Cannot delete the last prompt".to_string());
    }

    // Find and remove the prompt
    let original_len = settings.post_process_prompts.len();
    settings.post_process_prompts.retain(|p| p.id != id);

    if settings.post_process_prompts.len() == original_len {
        return Err(format!("Prompt with id '{}' not found", id));
    }

    // If the deleted prompt was selected, select the first one or None
    if settings.post_process_selected_prompt_id.as_ref() == Some(&id) {
        settings.post_process_selected_prompt_id =
            settings.post_process_prompts.first().map(|p| p.id.clone());
    }

    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn fetch_post_process_models(
    app: AppHandle,
    provider_id: String,
) -> Result<Vec<String>, String> {
    let settings = settings::get_settings(&app);

    // Find the provider
    let provider = settings
        .post_process_providers
        .iter()
        .find(|p| p.id == provider_id)
        .ok_or_else(|| format!("Provider '{}' not found", provider_id))?;

    if provider.id == APPLE_INTELLIGENCE_PROVIDER_ID {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            return Ok(vec![APPLE_INTELLIGENCE_DEFAULT_MODEL_ID.to_string()]);
        }

        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            return Err("Apple Intelligence is only available on Apple silicon Macs running macOS 15 or later.".to_string());
        }
    }

    // Get API key
    let api_key = settings
        .post_process_api_keys
        .get(&provider_id)
        .cloned()
        .unwrap_or_default();

    // Skip fetching if no API key for providers that typically need one
    if api_key.trim().is_empty() && provider.id != "custom" {
        return Err(format!(
            "API key is required for {}. Please add an API key to list available models.",
            provider.label
        ));
    }

    crate::llm_client::fetch_models(provider, api_key).await
}

#[tauri::command]
#[specta::specta]
pub fn set_post_process_selected_prompt(app: AppHandle, id: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);

    // Verify the prompt exists
    if !settings.post_process_prompts.iter().any(|p| p.id == id) {
        return Err(format!("Prompt with id '{}' not found", id));
    }

    settings.post_process_selected_prompt_id = Some(id);
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_mute_while_recording_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.mute_while_recording = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_append_trailing_space_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.append_trailing_space = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_lazy_stream_close_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.lazy_stream_close = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_vad_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.vad_enabled = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn change_vad_backend_setting(app: AppHandle, backend: VadBackend) -> Result<(), String> {
    if settings::get_settings(&app).vad_backend == backend {
        return Ok(());
    }

    // Construct/swap the detector and, when necessary, reopen cpal away from
    // the webview thread. Persist only after the runtime change succeeds so a
    // rejected in-progress switch or failed microphone reopen rolls back cleanly.
    let manager = app
        .state::<std::sync::Arc<crate::managers::audio::AudioRecordingManager>>()
        .inner()
        .clone();
    tokio::task::spawn_blocking(move || manager.update_vad_backend(backend))
        .await
        .map_err(|e| format!("audio task join failed: {e}"))?
        .map_err(|e| format!("Failed to update VAD backend: {e}"))?;

    let mut current_settings = settings::get_settings(&app);
    current_settings.vad_backend = backend;
    settings::write_settings(&app, current_settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_filler_word_removal_enabled_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.filler_word_removal_enabled = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_chinese_script_setting(app: AppHandle, script: ChineseScript) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.chinese_script = script;
    settings::write_settings(&app, settings);
    Ok(())
}

/// What a wake-word detection starts. Always a conversation; kept registered for bindings stability.
#[tauri::command]
#[specta::specta]
pub fn change_wake_word_action_setting(
    app: AppHandle,
    action: settings::WakeWordAction,
) -> Result<(), String> {
    // A wake word always starts a conversation. The argument is accepted only so the command
    // signature (and the generated bindings) stay put; the stored value is always Conversation.
    let _ = action;
    let mut settings = settings::get_settings(&app);
    settings.wake_word_action = settings::WakeWordAction::Conversation;
    settings::write_settings(&app, settings);
    Ok(())
}

/// Interrupt a spoken reply by talking over it: `headphones` or `off`. Any other value is Off.
#[tauri::command]
#[specta::specta]
pub fn change_converse_barge_in_setting(app: AppHandle, value: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.converse_barge_in = settings::ConverseBargeIn::from_setting_value(&value);
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_app_language_setting(app: AppHandle, language: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.app_language = language.clone();
    settings::write_settings(&app, settings);

    // Refresh the tray menu with the new language
    tray::update_tray_menu(&app);

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_show_tray_icon_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.show_tray_icon = enabled;
    settings::write_settings(&app, settings);

    // Apply change immediately
    tray::set_tray_visibility(&app, enabled);

    Ok(())
}

/// Save accelerator settings and make the next model use reload with them.
/// The currently running transcription, if any, keeps its existing engine.
fn save_accelerator_and_reload_next_use(app: &AppHandle, s: settings::AppSettings) {
    settings::write_settings(app, s);

    let tm = app.state::<std::sync::Arc<crate::managers::transcription::TranscriptionManager>>();
    tm.reload_model_on_next_use();
}

#[tauri::command]
#[specta::specta]
pub fn change_transcribe_accelerator_setting(
    app: AppHandle,
    accelerator: settings::TranscribeAcceleratorSetting,
) -> Result<(), String> {
    let mut s = settings::get_settings(&app);
    s.transcribe_accelerator = accelerator;
    save_accelerator_and_reload_next_use(&app, s);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_ort_accelerator_setting(
    app: AppHandle,
    accelerator: settings::OrtAcceleratorSetting,
) -> Result<(), String> {
    let mut s = settings::get_settings(&app);
    s.ort_accelerator = accelerator;
    save_accelerator_and_reload_next_use(&app, s);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_transcribe_gpu_device(app: AppHandle, device: Option<String>) -> Result<(), String> {
    let mut s = settings::get_settings(&app);
    s.transcribe_gpu_device = device;
    save_accelerator_and_reload_next_use(&app, s);
    Ok(())
}

/// Return which accelerators and GPU devices are available for this build.
///
/// First-call cost is dominated by enumerating GPU devices through the
/// transcribe.cpp Metal/Vulkan backend, which loads dynamic libraries and
/// probes hardware. Run it on the blocking pool so the webview thread
/// stays responsive — see also the startup pre-warm in `lib.rs`.
#[tauri::command]
#[specta::specta]
pub async fn get_available_accelerators() -> crate::managers::transcription::AvailableAccelerators {
    tauri::async_runtime::spawn_blocking(crate::managers::transcription::get_available_accelerators)
        .await
        .expect("get_available_accelerators panicked")
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_agent_bridge_binary_path, parse_theme_setting, MAX_AGENT_BINARY_PATH_CHARS,
    };
    use crate::settings::Theme;
    use handy_keys::Hotkey;
    use tauri_plugin_global_shortcut::Shortcut;

    #[test]
    fn parse_theme_setting_recognizes_all_five_values() {
        assert_eq!(parse_theme_setting("system"), Theme::System);
        assert_eq!(parse_theme_setting("light"), Theme::Light);
        assert_eq!(parse_theme_setting("dark"), Theme::Dark);
        assert_eq!(parse_theme_setting("aurora"), Theme::Aurora);
        assert_eq!(parse_theme_setting("custom"), Theme::Custom);
    }

    #[test]
    fn parse_theme_setting_falls_back_to_system_for_unknown_value() {
        assert_eq!(parse_theme_setting("not-a-real-theme"), Theme::System);
        assert_eq!(parse_theme_setting(""), Theme::System);
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn window_theme_for_maps_aurora_to_native_dark_chrome() {
        use super::window_theme_for;
        // The hex param is irrelevant for every non-Custom variant.
        let unused = "#000000";
        assert_eq!(window_theme_for(Theme::System, unused), None);
        assert_eq!(
            window_theme_for(Theme::Light, unused),
            Some(tauri::Theme::Light)
        );
        assert_eq!(
            window_theme_for(Theme::Dark, unused),
            Some(tauri::Theme::Dark)
        );
        assert_eq!(
            window_theme_for(Theme::Aurora, unused),
            Some(tauri::Theme::Dark)
        );
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn window_theme_for_custom_resolves_from_the_actual_background_luminance() {
        use super::window_theme_for;
        assert_eq!(
            window_theme_for(Theme::Custom, "#ffffff"),
            Some(tauri::Theme::Light)
        );
        assert_eq!(
            window_theme_for(Theme::Custom, "#000000"),
            Some(tauri::Theme::Dark)
        );
    }

    #[test]
    fn compound_shortcut_keys_parse_on_both_backends() {
        for key in [
            "scrolllock",
            "capslock",
            "numlock",
            "pageup",
            "pagedown",
            "printscreen",
        ] {
            assert!(key.parse::<Shortcut>().is_ok(), "Tauri rejected {key}");
            assert!(key.parse::<Hotkey>().is_ok(), "HandyKeys rejected {key}");
        }
    }

    #[test]
    fn agent_binary_path_none_and_blank_normalize_to_none() {
        assert_eq!(normalize_agent_bridge_binary_path(None), Ok(None));
        assert_eq!(
            normalize_agent_bridge_binary_path(Some(String::new())),
            Ok(None)
        );
        assert_eq!(
            normalize_agent_bridge_binary_path(Some("   \t ".into())),
            Ok(None)
        );
    }

    #[test]
    fn agent_binary_path_is_trimmed_and_otherwise_preserved() {
        assert_eq!(
            normalize_agent_bridge_binary_path(Some("  /usr/local/bin/agent  ".into())),
            Ok(Some("/usr/local/bin/agent".into()))
        );
        assert_eq!(
            normalize_agent_bridge_binary_path(Some("/opt/My Agent/bin/agent".into())),
            Ok(Some("/opt/My Agent/bin/agent".into())),
            "interior spaces are legal in paths"
        );
        assert_eq!(
            normalize_agent_bridge_binary_path(Some("/opt/caf\u{e9}/agent".into())),
            Ok(Some("/opt/caf\u{e9}/agent".into()))
        );
        assert_eq!(
            normalize_agent_bridge_binary_path(Some("my-agent".into())),
            Ok(Some("my-agent".into())),
            "a bare command name resolved via PATH is allowed"
        );
    }

    #[test]
    fn agent_binary_path_rejects_control_characters() {
        for bad in [
            "/bin/a\nb",
            "/bin/a\rb",
            "/bin/a\u{0}b",
            "/bin/a\tb",
            "/bin/a\u{7}b",
            "/bin/a\u{1b}[0m",
        ] {
            assert!(
                normalize_agent_bridge_binary_path(Some(bad.to_string())).is_err(),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn agent_binary_path_length_limit_is_in_characters() {
        let ok = format!("/{}", "a".repeat(MAX_AGENT_BINARY_PATH_CHARS - 1));
        assert!(normalize_agent_bridge_binary_path(Some(ok)).is_ok());
        let too_long = format!("/{}", "a".repeat(MAX_AGENT_BINARY_PATH_CHARS));
        assert!(normalize_agent_bridge_binary_path(Some(too_long)).is_err());
        let multibyte = "\u{e9}".repeat(MAX_AGENT_BINARY_PATH_CHARS);
        assert!(
            normalize_agent_bridge_binary_path(Some(multibyte)).is_ok(),
            "counted in chars, not bytes"
        );
    }
}

#[cfg(test)]
mod stt_auto_language_switch_tests {
    use super::apply_stt_auto_language_enabled;

    #[test]
    fn stt_auto_language_switch_stores_true_and_false_and_defaults_off() {
        let mut settings = crate::settings::get_default_settings();
        assert!(!settings.stt_auto_language_enabled);
        apply_stt_auto_language_enabled(&mut settings, true);
        assert!(settings.stt_auto_language_enabled);
        apply_stt_auto_language_enabled(&mut settings, false);
        assert!(!settings.stt_auto_language_enabled);
    }
}

#[cfg(test)]
mod stt_tamil_switch_tests {
    use super::apply_stt_tamil_enabled;

    #[test]
    fn stt_tamil_switch_stores_false_and_true() {
        let mut settings = crate::settings::get_default_settings();
        assert!(settings.stt_tamil_enabled);
        apply_stt_tamil_enabled(&mut settings, false);
        assert!(!settings.stt_tamil_enabled);
        apply_stt_tamil_enabled(&mut settings, true);
        assert!(settings.stt_tamil_enabled);
    }
}

#[cfg(test)]
mod tts_command_tests {
    use super::{apply_tts_tamil_enabled, normalize_tts_voice, tts_error_message};
    use crate::managers::tts::TtsError;

    #[test]
    fn tamil_switch_stores_false_and_true() {
        let mut settings = crate::settings::get_default_settings();
        assert!(settings.tts_tamil_enabled);
        apply_tts_tamil_enabled(&mut settings, false);
        assert!(!settings.tts_tamil_enabled);
        apply_tts_tamil_enabled(&mut settings, true);
        assert!(settings.tts_tamil_enabled);
    }

    #[test]
    fn missing_or_blank_voice_means_system_default() {
        assert_eq!(normalize_tts_voice(None), Ok(None));
        assert_eq!(normalize_tts_voice(Some(String::new())), Ok(None));
        assert_eq!(normalize_tts_voice(Some("   ".to_string())), Ok(None));
    }

    #[test]
    fn valid_voice_is_stored_trimmed() {
        assert_eq!(
            normalize_tts_voice(Some("  Samantha (Enhanced) ".to_string())),
            Ok(Some("Samantha (Enhanced)".to_string()))
        );
    }

    #[test]
    fn unsafe_voice_names_are_rejected() {
        for bad in [
            "-v".to_string(),
            "Samantha; rm -rf".to_string(),
            "Voice/../x".to_string(),
            "a\nb".to_string(),
            "x".repeat(65),
        ] {
            assert_eq!(
                normalize_tts_voice(Some(bad.clone())),
                Err("Invalid voice name".to_string()),
                "accepted unsafe voice {bad:?}"
            );
        }
    }

    #[test]
    fn speech_errors_map_to_content_free_messages() {
        let backend = TtsError::Backend("secret path /tmp/x".to_string());
        let message = tts_error_message(&backend);
        assert_eq!(message, "Speech output failed");
        assert!(!message.contains("secret"));
        assert_eq!(tts_error_message(&TtsError::Empty), "Nothing to speak");
        assert_eq!(
            tts_error_message(&TtsError::Superseded),
            "Replaced by newer speech"
        );
        assert_eq!(
            tts_error_message(&TtsError::Recording),
            "Cannot speak while recording"
        );
    }
}

#[cfg(test)]
mod ambient_experimental_transition_tests {
    use super::{ambient_transition_on_experimental_change as transition, AmbientTransition};

    #[test]
    fn experimental_off_while_ambient_runs_stops_it_whatever_the_flag_and_wake_word() {
        assert_eq!(
            transition(true, false, true, false),
            AmbientTransition::Stop
        );
        assert_eq!(transition(true, false, true, true), AmbientTransition::Stop);
        assert_eq!(
            transition(true, false, false, false),
            AmbientTransition::Stop
        );
        assert_eq!(
            transition(true, false, false, true),
            AmbientTransition::Stop
        );
    }

    #[test]
    fn experimental_on_with_flag_and_wake_word_off_starts_ambient() {
        assert_eq!(
            transition(false, true, true, false),
            AmbientTransition::Start
        );
    }

    #[test]
    fn experimental_on_with_flag_but_wake_word_enabled_does_not_start_ambient() {
        assert_eq!(transition(false, true, true, true), AmbientTransition::None);
    }

    #[test]
    fn experimental_on_without_flag_does_not_start_ambient() {
        assert_eq!(
            transition(false, true, false, false),
            AmbientTransition::None
        );
        assert_eq!(
            transition(false, true, false, true),
            AmbientTransition::None
        );
    }

    #[test]
    fn unchanged_experimental_value_is_a_no_op() {
        assert_eq!(
            transition(false, false, true, false),
            AmbientTransition::None
        );
        assert_eq!(
            transition(false, false, false, false),
            AmbientTransition::None
        );
        assert_eq!(transition(true, true, true, false), AmbientTransition::None);
        assert_eq!(transition(true, true, false, true), AmbientTransition::None);
    }
}

/// B7 minor: the update-checks and automatic-update commands announce a setting in
/// `settings-changed` under one command mutex, so two quick toggles announce in order.
#[cfg(test)]
mod settings_announce_tests {
    use super::*;
    use std::sync::Mutex;

    /// Two sequential toggles announce the value read back after each write, in order.
    #[test]
    fn two_sequential_toggles_announce_the_read_back_value() {
        let stored = Mutex::new(false);
        let announced = Mutex::new(Vec::new());
        for enabled in [true, false] {
            let applied = write_read_back_and_announce(
                || *stored.lock().unwrap() = enabled,
                || *stored.lock().unwrap(),
                |value| announced.lock().unwrap().push(*value),
            );
            assert_eq!(applied, enabled);
        }
        assert_eq!(*announced.lock().unwrap(), vec![true, false]);
        assert!(!*stored.lock().unwrap());
    }

    /// The lock order is written next to the command mutex (outermost first).
    #[test]
    fn lock_order_is_documented_on_the_announce_mutex() {
        let source = include_str!("mod.rs");
        let documented = source.lines().any(|line| {
            line.trim_start().starts_with("/// Lock order")
                && line.contains("command mutex -> update coordinator -> settings gate -> store")
        });
        assert!(documented, "lock order doc comment missing");
    }
}
