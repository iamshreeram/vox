//! System tray icon and menu.
//!
//! The tray is driven by a single *desired state* snapshot ([`TrayDesired`])
//! that callers update through [`set_tray_state`], [`refresh_tray_icon`] and
//! [`update_tray_menu`]. Every such call just records intent and schedules a
//! single applier on the main thread, which diffs the desired snapshot against
//! what is currently displayed and touches the native tray only for the parts
//! that actually changed. Requests that arrive while an apply is pending are
//! coalesced into it, so bursts of state changes never queue up native work.
//!
//! Why: native tray updates are the lever we control for the macOS tray
//! disappearance bug (tauri-apps/tauri#12060, Vox #1948). Before this, every
//! recording cycle rebuilt the full menu 3-6 times from several threads, and
//! concurrent rebuilds could interleave and leave a stale menu behind.
//!
//! Exception: [`set_tray_visibility`] and [`recreate_tray_icon`] call the tray
//! directly. Visibility is a separate attribute that never participates in the
//! icon/menu diff, both are rare and user-initiated, and Tauri marshals them
//! onto the main thread so they serialize with the applier anyway. Re-showing
//! a hidden tray relies on tray-icon recreating it from the last applied
//! icon/menu/tooltip, so those must only ever be set through the applier.

use crate::managers::history::{HistoryEntry, HistoryManager};
use crate::managers::model::ModelManager;
use crate::managers::source_update_runner::{RunPhase, UpdateProgress};
use crate::managers::transcription::TranscriptionManager;
use crate::settings;
use crate::tray_i18n::get_tray_translations;
use log::{debug, error, info, trace, warn};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIcon;
use tauri::{AppHandle, Manager, Theme};
use tauri_plugin_clipboard_manager::ClipboardExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayIconState {
    Idle,
    Recording,
    Transcribing,
}

impl TrayIconState {
    /// Recording and Transcribing share the same menu ("Cancel" instead of the
    /// model submenu), so only the idle/busy distinction matters for the menu.
    fn is_busy(self) -> bool {
        self != TrayIconState::Idle
    }
}

/// Background update progress as the tray shows it (DESIGN D4, R2-7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayUpdateProgress {
    /// Progress, 0..=100.
    pub percent: u8,
    /// True when the update failed; the item then only opens the window.
    pub failed: bool,
}

/// Everything the tray *menu* (and tooltip) depends on. When two snapshots
/// compare equal the menu is not rebuilt.
#[derive(Clone, Debug, PartialEq, Eq)]
struct MenuInputs {
    busy: bool,
    warning: bool,
    model_loaded: bool,
    selected_model: String,
    /// `(id, name)` of downloaded models, sorted by name.
    downloaded_models: Vec<(String, String)>,
    locale: String,
    update_checks_enabled: bool,
    /// A source-build check found a newer `main` (see `source_update`).
    update_available: bool,
    /// Background update progress, if one is running or has just failed.
    update_progress: Option<TrayUpdateProgress>,
    wake_word_enabled: bool,
    ambient_mode_enabled: bool,
    experimental_enabled: bool,
    /// Floating orb is supported here (see `orb_native::floating_offered`). Unsupported systems
    /// get no orb item.
    orb_offered: bool,
}

/// Complete description of what the tray should look like.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TrayDesired {
    icon_path: &'static str,
    menu: MenuInputs,
}

struct TrayInner {
    /// Intent set by [`set_tray_state`].
    icon_state: TrayIconState,
    /// Latest computed snapshot, waiting to be (or just) applied.
    desired: Option<TrayDesired>,
    /// Icon the native tray currently shows. Only updated when `set_icon`
    /// succeeds, so a failed update is retried on the next sync.
    applied_icon: Option<&'static str>,
    /// Inputs the native menu was last successfully built from. The tooltip
    /// is derived from the same inputs and set best-effort alongside the menu;
    /// it is not tracked separately.
    applied_menu: Option<MenuInputs>,
    /// An apply is scheduled on the main thread.
    pending: bool,
    /// Decoded icons by resource path so the main thread never touches disk.
    icons: HashMap<&'static str, Image<'static>>,
    /// Handed out to each sync request in trigger order, so a slow request
    /// can't overwrite the snapshot of one that was triggered after it.
    next_seq: u64,
    /// Sequence number of the request that produced `desired`.
    desired_seq: u64,
    /// A coding job is running (set by [`set_job_running`]). Kept out of `MenuInputs` so the
    /// menu inputs stay unchanged; the applier diffs it through `applied_cancel_enabled`.
    job_running: bool,
    /// Enabled state the native "Cancel coding job" item was last built with.
    applied_cancel_enabled: Option<bool>,
}

/// Tauri managed state owning the tray's desired/applied snapshots.
pub struct TrayState(Mutex<TrayInner>);

impl TrayState {
    pub fn new() -> Self {
        Self(Mutex::new(TrayInner {
            icon_state: TrayIconState::Idle,
            desired: None,
            applied_icon: None,
            applied_menu: None,
            pending: false,
            icons: HashMap::new(),
            next_seq: 0,
            desired_seq: 0,
            job_running: false,
            applied_cancel_enabled: None,
        }))
    }

    fn lock(&self) -> MutexGuard<'_, TrayInner> {
        self.0.lock().unwrap_or_else(|poisoned| {
            warn!("Tray state mutex was poisoned, recovering");
            poisoned.into_inner()
        })
    }
}

impl Default for TrayState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AppTheme {
    Dark,
    Light,
    Colored, // Pink/colored theme for Linux
}

/// Gets the current app theme, with Linux defaulting to Colored theme
pub fn get_current_theme(app: &AppHandle) -> AppTheme {
    if cfg!(target_os = "linux") {
        // On Linux, always use the colored theme
        AppTheme::Colored
    } else {
        // On Windows the tray icon sits on the taskbar, which follows the
        // *system* theme (SystemUsesLightTheme), not the app theme. With the
        // "Custom" personalization mode the two can differ (e.g. dark taskbar
        // + light apps), and the window theme would pick an icon that is
        // invisible against the taskbar.
        #[cfg(target_os = "windows")]
        if let Some(theme) = windows_taskbar_theme() {
            return theme;
        }

        // On other platforms, map system theme to our app theme
        if let Some(main_window) = app.get_webview_window("main") {
            match main_window.theme().unwrap_or(Theme::Dark) {
                Theme::Light => AppTheme::Light,
                Theme::Dark => AppTheme::Dark,
                _ => AppTheme::Dark, // Default fallback
            }
        } else {
            AppTheme::Dark
        }
    }
}

/// Reads the Windows taskbar theme from the registry.
///
/// Returns None if the value is missing (older Windows 10 builds default to a
/// dark taskbar there, but falling back to the window theme is safer than
/// guessing).
#[cfg(target_os = "windows")]
fn windows_taskbar_theme() -> Option<AppTheme> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let personalize = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
        .ok()?;
    let system_uses_light: u32 = personalize.get_value("SystemUsesLightTheme").ok()?;
    Some(if system_uses_light == 1 {
        AppTheme::Light
    } else {
        AppTheme::Dark
    })
}

/// Gets the appropriate icon path for the given theme and state.
///
/// `warning` overlays a badge on the idle icon while keyboard shortcuts are
/// blocked (macOS Secure Input); recording/transcribing states keep their
/// normal icons so in-flight activity stays recognizable.
pub fn get_icon_path(theme: AppTheme, state: TrayIconState, warning: bool) -> &'static str {
    if warning && state == TrayIconState::Idle {
        return match theme {
            AppTheme::Dark => "resources/tray_idle_warning.png",
            AppTheme::Light => "resources/tray_idle_warning_dark.png",
            // Linux never sets the warning flag (Secure Input is macOS-only),
            // but fall back to the normal icon just in case.
            AppTheme::Colored => "resources/handy.png",
        };
    }
    match (theme, state) {
        // Dark theme uses light icons
        (AppTheme::Dark, TrayIconState::Idle) => "resources/tray_idle.png",
        (AppTheme::Dark, TrayIconState::Recording) => "resources/tray_recording.png",
        (AppTheme::Dark, TrayIconState::Transcribing) => "resources/tray_transcribing.png",
        // Light theme uses dark icons
        (AppTheme::Light, TrayIconState::Idle) => "resources/tray_idle_dark.png",
        (AppTheme::Light, TrayIconState::Recording) => "resources/tray_recording_dark.png",
        (AppTheme::Light, TrayIconState::Transcribing) => "resources/tray_transcribing_dark.png",
        // Colored theme uses pink icons (for Linux)
        (AppTheme::Colored, TrayIconState::Idle) => "resources/handy.png",
        (AppTheme::Colored, TrayIconState::Recording) => "resources/recording.png",
        (AppTheme::Colored, TrayIconState::Transcribing) => "resources/transcribing.png",
    }
}

/// Sets the recording state shown by the tray (icon + Cancel/model menu).
pub fn set_tray_state(app: &AppHandle, state: TrayIconState) {
    sync_tray_with(app, |inner| inner.icon_state = state);
}

/// Callback run when the user clicks "Cancel coding job" (M3 S3c-2, DESIGN D6). Registered
/// by a later step; until then the item stays disabled.
type CancelJobHandler = Arc<dyn Fn() + Send + Sync>;

/// The registered cancel handler, if any.
static CANCEL_JOB_HANDLER: Mutex<Option<CancelJobHandler>> = Mutex::new(None);

/// Registers the handler the "Cancel coding job" item calls. Replaces any earlier handler.
/// Call [`sync_tray`] afterwards so the item's enabled state is refreshed.
#[allow(dead_code)]
pub fn set_cancel_job_handler(handler: CancelJobHandler) {
    let mut slot = CANCEL_JOB_HANDLER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *slot = Some(handler);
}

/// Runs the registered cancel handler. Returns `false` when none is registered.
#[allow(dead_code)]
pub fn invoke_cancel_job_handler() -> bool {
    let handler = CANCEL_JOB_HANDLER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    match handler {
        Some(run) => {
            run();
            true
        }
        None => false,
    }
}

/// Records whether a coding job is running and re-syncs the tray so the cancel item
/// follows it (M3 S3c-2, DESIGN D6).
#[allow(dead_code)]
pub fn set_job_running(app: &AppHandle, running: bool) {
    sync_tray_with(app, |inner| inner.job_running = running);
}

/// Whether the cancel handler is registered.
fn cancel_job_handler_registered() -> bool {
    CANCEL_JOB_HANDLER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_some()
}

/// Re-syncs the tray after something other than the recording state changed
/// (theme, Secure Input warning). The recording state itself is preserved.
pub fn refresh_tray_icon(app: &AppHandle) {
    sync_tray(app);
}

/// Re-syncs the tray after something the menu depends on changed (model
/// list/selection/loaded state, language, settings).
pub fn update_tray_menu(app: &AppHandle) {
    sync_tray(app);
}

/// Records the current desired tray state and schedules one apply on the main
/// thread (or lets an already-pending apply pick it up). Never blocks on the
/// main thread.
///
/// The snapshot (settings, model list, loaded state) is computed on the
/// *calling* thread on purpose: the main-thread applier must not take manager
/// locks that a worker may hold across slow work (see #1716).
pub fn sync_tray(app: &AppHandle) {
    sync_tray_with(app, |_| {});
}

fn sync_tray_with(app: &AppHandle, update: impl FnOnce(&mut TrayInner)) {
    let Some(state) = app.try_state::<TrayState>() else {
        return;
    };

    // Record intent and claim a sequence number in one critical section, so
    // sequence order == the order in which state changes were requested.
    let (seq, icon_state) = {
        let mut inner = state.lock();
        update(&mut inner);
        inner.next_seq += 1;
        (inner.next_seq, inner.icon_state)
    };

    // Tray not built yet (early secure-input monitor callbacks). The intent
    // is kept and picked up by the first sync after the tray exists.
    if app.try_state::<TrayIcon>().is_none() {
        return;
    }

    let desired = compute_desired(app, icon_state);

    // Decode the icon off the main thread, once per path, outside the lock.
    let needs_icon = !state.lock().icons.contains_key(desired.icon_path);
    let loaded_icon = if needs_icon {
        match load_tray_icon(
            app.path()
                .resolve(desired.icon_path, tauri::path::BaseDirectory::Resource),
        ) {
            Ok(image) => Some(image),
            Err(err) => {
                error!("Failed to load tray icon '{}': {err}", desired.icon_path);
                None
            }
        }
    } else {
        None
    };

    let schedule = {
        let mut inner = state.lock();
        if let Some(image) = loaded_icon {
            inner.icons.insert(desired.icon_path, image);
        }
        if seq < inner.desired_seq {
            // A request triggered after this one already stored its snapshot
            // (and scheduled an apply). Ours is stale; drop it.
            trace!(
                "tray sync: request {seq} superseded by {}",
                inner.desired_seq
            );
            return;
        }
        inner.desired = Some(desired);
        inner.desired_seq = seq;
        // If an apply is already pending it will read the snapshot we just
        // stored; otherwise schedule one.
        !std::mem::replace(&mut inner.pending, true)
    };

    if schedule {
        post_apply(app);
    } else {
        trace!("tray sync: apply already pending");
    }
}

fn compute_desired(app: &AppHandle, icon_state: TrayIconState) -> TrayDesired {
    let settings = settings::get_settings(app);
    let theme = get_current_theme(app);
    let warning = crate::secure_input::tray_warning_active(app);
    let model_loaded = app.state::<Arc<TranscriptionManager>>().is_model_loaded();

    let mut downloaded_models: Vec<(String, String)> = app
        .state::<Arc<ModelManager>>()
        .get_available_models()
        .into_iter()
        .filter(|m| m.is_downloaded)
        .map(|m| (m.id, m.name))
        .collect();
    downloaded_models.sort_by(|a, b| a.1.cmp(&b.1));

    TrayDesired {
        icon_path: get_icon_path(theme, icon_state, warning),
        menu: MenuInputs {
            busy: icon_state.is_busy(),
            warning,
            model_loaded,
            selected_model: settings.selected_model,
            downloaded_models,
            locale: settings.app_language,
            update_checks_enabled: settings.update_checks_enabled,
            update_available: crate::managers::source_update::update_available(app),
            update_progress: crate::managers::source_update::progress(app)
                .as_ref()
                .and_then(tray_progress_from),
            wake_word_enabled: settings.wake_word_enabled,
            ambient_mode_enabled: settings.ambient_mode_enabled,
            experimental_enabled: settings.experimental_enabled,
            orb_offered: crate::orb_native::floating_offered(),
        },
    }
}

fn post_apply(app: &AppHandle) {
    let handle = app.clone();
    if let Err(err) = app.run_on_main_thread(move || apply_on_main(&handle)) {
        // Event loop is gone (shutdown). Clear `pending` so a later call, if
        // any, doesn't wait forever for an apply that will never run.
        error!("Failed to dispatch tray update to the main thread: {err}");
        if let Some(state) = app.try_state::<TrayState>() {
            state.lock().pending = false;
        }
    }
}

/// The single writer to the native tray. Runs on the main thread.
fn apply_on_main(app: &AppHandle) {
    let Some(state) = app.try_state::<TrayState>() else {
        return;
    };
    let Some(tray) = app.try_state::<TrayIcon>() else {
        return;
    };

    let started = Instant::now();
    let (desired, icon, icon_changed, menu_changed, cancel_enabled) = {
        let mut inner = state.lock();
        inner.pending = false;
        let Some(desired) = inner.desired.clone() else {
            return;
        };
        let cancel_enabled =
            cancel_job_item_enabled(inner.job_running) && cancel_job_handler_registered();
        let icon_changed = inner.applied_icon != Some(desired.icon_path);
        let cancel_changed = inner.applied_cancel_enabled != Some(cancel_enabled);
        let menu_changed = inner.applied_menu.as_ref() != Some(&desired.menu) || cancel_changed;
        if !icon_changed && !menu_changed {
            trace!("tray apply: nothing changed");
            return;
        }
        let icon = inner.icons.get(desired.icon_path).cloned();
        (desired, icon, icon_changed, menu_changed, cancel_enabled)
    };

    // Each part is recorded as applied only if its native call succeeded, so a
    // transient failure is retried on the next sync instead of being
    // remembered as displayed.
    let mut icon_ok = false;
    if icon_changed {
        match icon {
            Some(image) => match tray.set_icon_with_as_template(Some(image), false) {
                Ok(()) => icon_ok = true,
                Err(err) => error!("Failed to update tray icon '{}': {err}", desired.icon_path),
            },
            None => error!("Tray icon '{}' is not loaded", desired.icon_path),
        }
    }

    let mut menu_ok = false;
    if menu_changed {
        match build_menu(app, &desired.menu, cancel_enabled) {
            Ok((menu, tooltip)) => match tray.set_menu(Some(menu)) {
                Ok(()) => {
                    menu_ok = true;
                    // Best-effort: logged, not retried. The tooltip is cosmetic
                    // and can only fail on Windows, where a failing
                    // Shell_NotifyIcon call means the icon is failing too.
                    // Gating `menu_ok` on it would re-run the full menu
                    // rebuild on every sync for the cheapest mutation.
                    if let Err(err) = tray.set_tooltip(Some(tooltip)) {
                        error!("Failed to set tray tooltip: {err}");
                    }
                }
                Err(err) => error!("Failed to set tray menu: {err}"),
            },
            Err(err) => error!("Failed to build tray menu: {err}"),
        }
    }

    {
        let mut inner = state.lock();
        if icon_ok {
            inner.applied_icon = Some(desired.icon_path);
        }
        if menu_ok {
            inner.applied_menu = Some(desired.menu.clone());
            inner.applied_cancel_enabled = Some(cancel_enabled);
        }
    }

    debug!(
        "tray apply: icon={} menu={} busy={} took={:?}",
        if icon_changed {
            desired.icon_path
        } else {
            "unchanged"
        },
        if menu_changed { "rebuilt" } else { "unchanged" },
        desired.menu.busy,
        started.elapsed()
    );
}

pub(crate) fn load_tray_icon(
    resolved_icon_path: tauri::Result<PathBuf>,
) -> tauri::Result<Image<'static>> {
    let resolved_icon_path = resolved_icon_path?;
    Image::from_path(&resolved_icon_path).map(Image::to_owned)
}

pub fn tray_tooltip() -> String {
    version_label()
}

fn version_label() -> String {
    if cfg!(debug_assertions) {
        format!("Vox v{} (Dev)", env!("CARGO_PKG_VERSION"))
    } else {
        format!("Vox v{}", env!("CARGO_PKG_VERSION"))
    }
}

/// Builds the tray menu and tooltip for the given inputs. Pure with respect
/// to app state: everything it depends on is in `inputs`, plus the
/// process-constant `HANDY_DISABLE_UPDATER` env flag behind
/// `update_checks_forced_disabled()`, which cannot change during a run.
fn build_menu(
    app: &AppHandle,
    inputs: &MenuInputs,
    cancel_job_enabled: bool,
) -> tauri::Result<(Menu<tauri::Wry>, String)> {
    let strings = get_tray_translations(Some(inputs.locale.clone()));

    // Secure Input warning entry (macOS): clicking opens the settings window
    // where the full warning banner explains the situation. Locales that
    // haven't translated the key yet get the English string rather than a
    // blank menu item (build.rs emits "" for missing keys).
    let secure_input_warning = if inputs.warning {
        let label = if strings.secure_input_warning.is_empty() {
            get_tray_translations(Some("en".to_string())).secure_input_warning
        } else {
            strings.secure_input_warning.clone()
        };
        Some(MenuItem::with_id(
            app,
            "secure_input_warning",
            &label,
            true,
            None::<&str>,
        )?)
    } else {
        None
    };

    let listening_mode_submenu =
        if let Some((hotkey, wake_word, ambient)) = listening_mode_checked_states(inputs) {
            let submenu =
                Submenu::with_id(app, "listening_mode_submenu", &strings.listening_mode, true)?;
            for (id, label, checked) in [
                (
                    "listening_mode:hotkey",
                    strings.listening_mode_hotkey_only.as_str(),
                    hotkey,
                ),
                (
                    "listening_mode:wake_word",
                    strings.listening_mode_wake_word.as_str(),
                    wake_word,
                ),
                (
                    "listening_mode:ambient",
                    strings.listening_mode_ambient.as_str(),
                    ambient,
                ),
            ] {
                submenu.append(&CheckMenuItem::with_id(
                    app,
                    id,
                    label,
                    true,
                    checked,
                    None::<&str>,
                )?)?;
            }
            Some(submenu)
        } else {
            None
        };

    // Platform-specific accelerators
    #[cfg(target_os = "macos")]
    let (settings_accelerator, quit_accelerator) = (Some("Cmd+,"), Some("Cmd+Q"));
    #[cfg(not(target_os = "macos"))]
    let (settings_accelerator, quit_accelerator) = (Some("Ctrl+,"), Some("Ctrl+Q"));

    // Create common menu items
    let version_label = version_label();
    let version_i = MenuItem::with_id(app, "version", &version_label, false, None::<&str>)?;
    let settings_i = MenuItem::with_id(
        app,
        "settings",
        &strings.settings,
        true,
        settings_accelerator,
    )?;
    let check_updates_i = MenuItem::with_id(
        app,
        "check_updates",
        &strings.check_updates,
        inputs.update_checks_enabled,
        None::<&str>,
    )?;
    let copy_last_transcript_i = MenuItem::with_id(
        app,
        "copy_last_transcript",
        &strings.copy_last_transcript,
        true,
        None::<&str>,
    )?;
    // Conversation toggle. The key is English-only in the locale files, so
    // other locales fall back to the English label (see localized_or_english).
    let converse_label = localized_or_english(&strings.converse, &english_converse());
    let converse_i = MenuItem::with_id(app, "converse", &converse_label, true, None::<&str>)?;
    // Conversation window toggle (DESIGN-CHAT A5). Same English-fallback rule as converse.
    let chat_label = localized_or_english(&strings.chat, &english_chat());
    let chat_i = MenuItem::with_id(app, "chat", &chat_label, true, None::<&str>)?;
    // Floating orb show/hide (G3b). Same English-fallback rule as the other labels.
    let orb_label = localized_or_english(&strings.orb_toggle, &english_orb_toggle());
    let orb_i = MenuItem::with_id(app, "orb", &orb_label, true, None::<&str>)?;
    // "Cancel coding job" (M3 S3c-2, DESIGN D6): English-only label, enabled only while a job
    // runs and a cancel handler is registered (see `cancel_job_item_enabled`).
    let cancel_job_i = MenuItem::with_id(
        app,
        "cancel_job",
        CANCEL_JOB_LABEL,
        cancel_job_enabled,
        None::<&str>,
    )?;
    let quit_i = MenuItem::with_id(app, "quit", &strings.quit, true, quit_accelerator)?;
    let separator = || PredefinedMenuItem::separator(app);

    // "Update available" sits directly above "Check for Updates..." and is only
    // present while a source-build check found a newer commit. It never starts
    // an install; clicking it just opens the window.
    let update_item = match update_slot(inputs, settings::update_checks_forced_disabled()) {
        UpdateSlot::Hidden => None,
        UpdateSlot::Available => {
            let label =
                localized_or_english(&strings.update_available, &english_update_available());
            Some(MenuItem::with_id(
                app,
                "update_available",
                &label,
                true,
                None::<&str>,
            )?)
        }
        UpdateSlot::Progress(percent) => {
            let base = label_or(
                &strings.updating_vox,
                &english_updating_vox(),
                "Updating Vox…",
            );
            Some(MenuItem::with_id(
                app,
                "update_progress",
                progress_label(&base, percent),
                false,
                None::<&str>,
            )?)
        }
        UpdateSlot::Failed => {
            let label = label_or(
                &strings.update_failed,
                &english_update_failed(),
                "Update failed",
            );
            Some(MenuItem::with_id(
                app,
                "update_failed",
                &label,
                true,
                None::<&str>,
            )?)
        }
    };

    let menu = if inputs.busy {
        let cancel_i = MenuItem::with_id(app, "cancel", &strings.cancel, true, None::<&str>)?;
        Menu::with_items(
            app,
            &[
                &version_i,
                &separator()?,
                &cancel_i,
                &separator()?,
                &copy_last_transcript_i,
                &converse_i,
                &chat_i,
                &orb_i,
                &separator()?,
                &settings_i,
                &check_updates_i,
                &separator()?,
                &quit_i,
            ],
        )?
    } else {
        // Build model submenu — label is the active model name
        let submenu_label = inputs
            .downloaded_models
            .iter()
            .find(|(id, _)| *id == inputs.selected_model)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| strings.model.clone());

        let model_submenu = Submenu::with_id(app, "model_submenu", &submenu_label, true)?;
        for (id, name) in &inputs.downloaded_models {
            let is_active = *id == inputs.selected_model;
            let item_id = format!("model_select:{}", id);
            let item = CheckMenuItem::with_id(app, &item_id, name, true, is_active, None::<&str>)?;
            model_submenu.append(&item)?;
        }

        let unload_model_i = MenuItem::with_id(
            app,
            "unload_model",
            &strings.unload_model,
            inputs.model_loaded,
            None::<&str>,
        )?;

        Menu::with_items(
            app,
            &[
                &version_i,
                &separator()?,
                &copy_last_transcript_i,
                &converse_i,
                &chat_i,
                &orb_i,
                &cancel_job_i,
                &separator()?,
                &model_submenu,
                &unload_model_i,
                &separator()?,
                &settings_i,
                &check_updates_i,
                &separator()?,
                &quit_i,
            ],
        )?
    };

    // The listening-mode group is present only when experimental features are
    // enabled. Insert it before settings in either menu layout.
    if let Some(submenu) = &listening_mode_submenu {
        let insert_at = if inputs.busy { 7 } else { 9 };
        menu.insert(submenu, insert_at)?;
        menu.insert(&separator()?, insert_at + 1)?;
    }

    // The orb item is dropped where floating is unsupported. It is removed after the listening-mode
    // group is inserted, so the insert indices above still match the full layout.
    if !inputs.orb_offered {
        menu.remove(&orb_i)?;
    }

    // When update checks are forced off (e.g. HANDY_DISABLE_UPDATER, set by
    // the Nix package), the item is dropped from the menu rather than shown
    // disabled — it can never do anything in that case, and a disabled item
    // still shifts every entry below it by one position. A manually-disabled
    // toggle in Debug Settings keeps the old greyed-out behavior via the
    // enabled flag.
    if let Some(update_item) = &update_item {
        let items = menu.items()?;
        if let Some(index) = items
            .iter()
            .position(|item| item.id().as_ref() == "check_updates")
        {
            menu.insert(update_item, index)?;
        }
    }

    if settings::update_checks_forced_disabled() {
        menu.remove(&check_updates_i)?;
    }

    // Both layouts start with [version, separator, ...]; slot the warning in
    // right below the version line so it's the first actionable thing seen.
    let mut tooltip = version_label;
    if let Some(warning_item) = secure_input_warning {
        menu.insert(&warning_item, 2)?;
        menu.insert(&separator()?, 3)?;
        tooltip = format!("{} — {}", tooltip, warning_item.text().unwrap_or_default());
    }

    Ok((menu, tooltip))
}

/// English label for the "Cancel coding job" tray item. English only, like the "Update
/// available" fallback: the locale files do not carry this key yet (M3 S3c-2).
const CANCEL_JOB_LABEL: &str = "Cancel coding job";

/// Whether the "Cancel coding job" tray item is enabled (M3 S3c-2, DESIGN D6): only while a
/// coding job is running.
fn cancel_job_item_enabled(job_running: bool) -> bool {
    job_running
}

/// Whether the "Update available" item is shown: only while an update is
/// known, checks are on, and checks are not forced off by the environment.
fn show_update_available_item(inputs: &MenuInputs, forced_disabled: bool) -> bool {
    inputs.update_available && inputs.update_checks_enabled && !forced_disabled
}

/// Which item fills the update slot above "Check for Updates...". A running or
/// failed background update takes the slot before "Update available" would.
#[derive(Debug, Clone, PartialEq, Eq)]
enum UpdateSlot {
    /// No item.
    Hidden,
    /// "Update available" (clickable; opens the window).
    Available,
    /// "Updating Vox… N%" (disabled) at this percent.
    Progress(u8),
    /// "Update failed" (clickable; opens the window).
    Failed,
}

fn update_slot(inputs: &MenuInputs, forced_disabled: bool) -> UpdateSlot {
    match &inputs.update_progress {
        Some(progress) if progress.failed => UpdateSlot::Failed,
        Some(progress) => UpdateSlot::Progress(progress.percent),
        None if show_update_available_item(inputs, forced_disabled) => UpdateSlot::Available,
        None => UpdateSlot::Hidden,
    }
}

/// Tray view of a progress: none once done, failed or progress otherwise.
pub fn tray_progress_from(progress: &UpdateProgress) -> Option<TrayUpdateProgress> {
    match progress.phase {
        RunPhase::Done => None,
        RunPhase::Failed => Some(TrayUpdateProgress {
            percent: progress.percent,
            failed: true,
        }),
        _ => Some(TrayUpdateProgress {
            percent: progress.percent,
            failed: false,
        }),
    }
}

/// The progress item text: the base label followed by the percent.
fn progress_label(base: &str, percent: u8) -> String {
    format!("{base} {percent}%")
}

/// The label to use: `localized` when present, else `english`, else `literal`.
fn label_or(localized: &str, english: &str, literal: &str) -> String {
    if !localized.is_empty() {
        localized.to_string()
    } else if !english.is_empty() {
        english.to_string()
    } else {
        literal.to_string()
    }
}

/// The label to use: the localized string when present, else the English one
/// (build.rs emits an empty string for locales missing the key), else a fixed
/// literal so the item is never blank.
fn localized_or_english(localized: &str, english_fallback: &str) -> String {
    label_or(localized, english_fallback, "Update available")
}

/// English conversation label from the generated translations.
fn english_converse() -> String {
    crate::tray_i18n::TRANSLATIONS
        .get("en")
        .map(|strings| strings.converse.clone())
        .unwrap_or_default()
}

/// English conversation-window label from the generated translations.
fn english_chat() -> String {
    crate::tray_i18n::TRANSLATIONS
        .get("en")
        .map(|strings| strings.chat.clone())
        .unwrap_or_default()
}

/// English floating-orb toggle label from the generated translations.
fn english_orb_toggle() -> String {
    crate::tray_i18n::TRANSLATIONS
        .get("en")
        .map(|strings| strings.orb_toggle.clone())
        .unwrap_or_default()
}

/// English "Update available" label from the generated translations.
fn english_update_available() -> String {
    crate::tray_i18n::TRANSLATIONS
        .get("en")
        .map(|strings| strings.update_available.clone())
        .unwrap_or_default()
}

/// English "Updating Vox…" label from the generated translations.
fn english_updating_vox() -> String {
    crate::tray_i18n::TRANSLATIONS
        .get("en")
        .map(|strings| strings.updating_vox.clone())
        .unwrap_or_default()
}

/// English "Update failed" label from the generated translations.
fn english_update_failed() -> String {
    crate::tray_i18n::TRANSLATIONS
        .get("en")
        .map(|strings| strings.update_failed.clone())
        .unwrap_or_default()
}

fn listening_mode_checked_states(inputs: &MenuInputs) -> Option<(bool, bool, bool)> {
    if !inputs.experimental_enabled {
        return None;
    }
    let ambient = inputs.ambient_mode_enabled;
    let wake_word = inputs.wake_word_enabled && !ambient;
    let hotkey = !ambient && !inputs.wake_word_enabled;
    Some((hotkey, wake_word, ambient))
}

fn last_transcript_text(entry: &HistoryEntry) -> &str {
    entry
        .post_processed_text
        .as_deref()
        .unwrap_or(&entry.transcription_text)
}

pub fn set_tray_visibility(app: &AppHandle, visible: bool) {
    let tray = app.state::<TrayIcon>();
    if let Err(e) = tray.set_visible(visible) {
        error!("Failed to set tray visibility: {}", e);
    } else {
        info!("Tray visibility set to: {}", visible);
    }
}

/// Recovery for the macOS tray-disappearance bug (#1948, tauri-apps/tauri#12060):
/// the `NSStatusItem` can silently vanish with no error surfaced to the app.
/// Hiding and re-showing the tray recreates it with its current icon, menu and
/// tooltip. Called when the user "relaunches" Vox while it is already running
/// (`RunEvent::Reopen` for Spotlight/Finder/Dock, the single-instance callback
/// for a second process) — the natural "where did my icon go?" moment — so a
/// relaunch brings the icon back without a full quit.
#[cfg(target_os = "macos")]
pub fn recreate_tray_icon(app: &AppHandle) {
    let no_tray = app
        .try_state::<crate::cli::CliArgs>()
        .map(|args| args.no_tray)
        .unwrap_or(false);
    if no_tray || !settings::get_settings(app).show_tray_icon {
        return;
    }
    let Some(tray) = app.try_state::<TrayIcon>() else {
        return;
    };
    info!("Recreating tray icon on relaunch");
    if let Err(e) = tray.set_visible(false).and_then(|_| tray.set_visible(true)) {
        error!("Failed to recreate tray icon: {}", e);
    }
}

pub fn copy_last_transcript(app: &AppHandle) {
    let history_manager = app.state::<Arc<HistoryManager>>();
    let entry = match history_manager.get_latest_completed_entry() {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            warn!("No completed transcription history entries available for tray copy.");
            return;
        }
        Err(err) => {
            error!(
                "Failed to fetch last completed transcription entry: {}",
                err
            );
            return;
        }
    };

    let text = last_transcript_text(&entry);
    if text.trim().is_empty() {
        warn!("Last completed transcription is empty; skipping tray copy.");
        return;
    }

    if let Err(err) = app.clipboard().write_text(text) {
        error!("Failed to copy last transcript to clipboard: {}", err);
        return;
    }

    info!("Copied last transcript to clipboard via tray.");
}

#[cfg(test)]
mod tests {
    use super::{
        last_transcript_text, listening_mode_checked_states, load_tray_icon, MenuInputs,
        TrayDesired, TrayIconState,
    };
    use crate::managers::history::HistoryEntry;

    fn build_entry(transcription: &str, post_processed: Option<&str>) -> HistoryEntry {
        HistoryEntry {
            id: 1,
            file_name: "handy-1.wav".to_string(),
            timestamp: 0,
            saved: false,
            title: "Recording".to_string(),
            transcription_text: transcription.to_string(),
            post_processed_text: post_processed.map(|text| text.to_string()),
            post_process_prompt: None,
            post_process_requested: false,
        }
    }

    fn inputs(busy: bool) -> MenuInputs {
        MenuInputs {
            busy,
            warning: false,
            model_loaded: true,
            selected_model: "small".to_string(),
            downloaded_models: vec![("small".to_string(), "Small".to_string())],
            locale: "en".to_string(),
            update_checks_enabled: true,
            update_available: false,
            update_progress: None,
            wake_word_enabled: false,
            ambient_mode_enabled: false,
            experimental_enabled: true,
            orb_offered: true,
        }
    }

    #[test]
    fn listening_mode_submenu_is_experimental_gated_and_checks_one_mode() {
        let mut menu_inputs = inputs(false);
        menu_inputs.experimental_enabled = false;
        assert_eq!(listening_mode_checked_states(&menu_inputs), None);

        menu_inputs.experimental_enabled = true;
        assert_eq!(
            listening_mode_checked_states(&menu_inputs),
            Some((true, false, false))
        );

        menu_inputs.wake_word_enabled = true;
        assert_eq!(
            listening_mode_checked_states(&menu_inputs),
            Some((false, true, false))
        );

        menu_inputs.wake_word_enabled = false;
        menu_inputs.ambient_mode_enabled = true;
        assert_eq!(
            listening_mode_checked_states(&menu_inputs),
            Some((false, false, true))
        );
    }

    #[test]
    fn all_listening_mode_transitions_produce_exact_setting_pairs() {
        use crate::commands::ambient::update_ambient_mode_enabled;
        use crate::commands::wakeword::update_wake_word_enabled;
        use crate::settings::get_default_settings;

        let initial_states = [
            ("hotkey", false, false),
            ("wake_word", true, false),
            ("ambient", false, true),
        ];
        let clicked_modes = ["hotkey", "wake_word", "ambient"];

        for (initial_name, initial_wake, initial_ambient) in initial_states {
            for clicked in clicked_modes {
                let mut settings = get_default_settings();
                settings.wake_word_enabled = initial_wake;
                settings.ambient_mode_enabled = initial_ambient;
                match clicked {
                    "hotkey" => {
                        update_wake_word_enabled(&mut settings, false);
                        update_ambient_mode_enabled(&mut settings, false);
                    }
                    "wake_word" => update_wake_word_enabled(&mut settings, true),
                    "ambient" => update_ambient_mode_enabled(&mut settings, true),
                    _ => unreachable!(),
                }

                let expected = match clicked {
                    "hotkey" => (false, false),
                    "wake_word" => (true, false),
                    "ambient" => (false, true),
                    _ => unreachable!(),
                };
                assert_eq!(
                    (settings.wake_word_enabled, settings.ambient_mode_enabled),
                    expected,
                    "initial {initial_name} state after clicking {clicked}"
                );
            }
        }
    }

    #[test]
    fn uses_post_processed_text_when_available() {
        let entry = build_entry("raw", Some("processed"));
        assert_eq!(last_transcript_text(&entry), "processed");
    }

    #[test]
    fn falls_back_to_raw_transcription() {
        let entry = build_entry("raw", None);
        assert_eq!(last_transcript_text(&entry), "raw");
    }

    #[test]
    fn tray_icon_resolution_failure_is_returned_instead_of_panicking() {
        assert!(load_tray_icon(Err(tauri::Error::UnknownPath)).is_err());
    }

    #[test]
    fn tray_icon_returns_err_when_file_does_not_exist() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let missing = dir.path().join("does_not_exist.png");
        assert!(load_tray_icon(Ok(missing)).is_err());
    }

    #[test]
    fn recording_and_transcribing_share_a_menu() {
        // The icon differs but the menu inputs are identical, so a
        // Recording -> Transcribing transition must not rebuild the menu.
        let recording = TrayDesired {
            icon_path: "resources/tray_recording.png",
            menu: inputs(TrayIconState::Recording.is_busy()),
        };
        let transcribing = TrayDesired {
            icon_path: "resources/tray_transcribing.png",
            menu: inputs(TrayIconState::Transcribing.is_busy()),
        };
        assert_ne!(recording.icon_path, transcribing.icon_path);
        assert_eq!(recording.menu, transcribing.menu);
    }

    #[test]
    fn idle_and_busy_menus_differ() {
        assert_ne!(inputs(false), inputs(true));
    }
}

#[cfg(test)]
mod update_available_item_tests {
    use super::{localized_or_english, show_update_available_item, MenuInputs};

    fn base(update_available: bool, update_checks_enabled: bool) -> MenuInputs {
        MenuInputs {
            busy: false,
            warning: false,
            model_loaded: false,
            selected_model: String::new(),
            downloaded_models: Vec::new(),
            locale: "en".to_string(),
            update_checks_enabled,
            update_available,
            update_progress: None,
            wake_word_enabled: false,
            ambient_mode_enabled: false,
            experimental_enabled: false,
            orb_offered: true,
        }
    }

    #[test]
    fn item_is_shown_only_when_update_available_and_checks_enabled() {
        for update_available in [false, true] {
            for checks_enabled in [false, true] {
                for forced in [false, true] {
                    let expected = update_available && checks_enabled && !forced;
                    assert_eq!(
                        show_update_available_item(&base(update_available, checks_enabled), forced),
                        expected,
                        "available={update_available} enabled={checks_enabled} forced={forced}"
                    );
                }
            }
        }
    }

    #[test]
    fn update_available_changes_the_menu_inputs() {
        assert_ne!(base(false, true), base(true, true));
    }

    #[test]
    fn localized_label_wins_over_english_fallback() {
        assert_eq!(
            localized_or_english("Mise a jour", "Update available"),
            "Mise a jour"
        );
    }

    #[test]
    fn missing_localized_label_falls_back_to_english() {
        assert_eq!(
            localized_or_english("", "Update available"),
            "Update available"
        );
    }

    #[test]
    fn missing_everything_falls_back_to_literal() {
        assert_eq!(localized_or_english("", ""), "Update available");
    }
}

/// Tests for the background-update slot: progress label, the hidden "Update available"
/// during a run, the failed item, and the done state.
#[cfg(test)]
mod update_progress_item_tests {
    use super::{
        label_or, progress_label, tray_progress_from, update_slot, MenuInputs, TrayUpdateProgress,
        UpdateSlot,
    };
    use crate::managers::source_update_runner::{RunPhase, UpdateProgress};

    fn inputs(update_available: bool, update_progress: Option<TrayUpdateProgress>) -> MenuInputs {
        MenuInputs {
            busy: false,
            warning: false,
            model_loaded: false,
            selected_model: String::new(),
            downloaded_models: Vec::new(),
            locale: "en".to_string(),
            update_checks_enabled: true,
            update_available,
            update_progress,
            wake_word_enabled: false,
            ambient_mode_enabled: false,
            experimental_enabled: false,
            orb_offered: true,
        }
    }

    fn progress(phase: RunPhase, percent: u8) -> UpdateProgress {
        UpdateProgress {
            run_id: "0123456789abcdef".to_string(),
            phase,
            percent,
            target: "0123456".to_string(),
            message: String::new(),
            estimated: false,
        }
    }

    #[test]
    fn progress_label_shows_the_percent_for_each_boundary() {
        assert_eq!(progress_label("Updating Vox…", 0), "Updating Vox… 0%");
        assert_eq!(progress_label("Updating Vox…", 7), "Updating Vox… 7%");
        assert_eq!(progress_label("Updating Vox…", 100), "Updating Vox… 100%");
    }

    #[test]
    fn progress_label_uses_the_localized_base_when_present() {
        assert_eq!(label_or("Mise à jour", "Updating Vox…", "x"), "Mise à jour");
        assert_eq!(label_or("", "Updating Vox…", "x"), "Updating Vox…");
        assert_eq!(label_or("", "", "Updating Vox…"), "Updating Vox…");
    }

    #[test]
    fn update_available_is_replaced_while_an_update_runs() {
        let running = Some(TrayUpdateProgress {
            percent: 42,
            failed: false,
        });
        assert_eq!(
            update_slot(&inputs(true, running), false),
            UpdateSlot::Progress(42)
        );
        assert_eq!(
            update_slot(&inputs(true, None), false),
            UpdateSlot::Available
        );
    }

    #[test]
    fn failed_update_shows_the_failed_item_and_only_that() {
        let failed = Some(TrayUpdateProgress {
            percent: 37,
            failed: true,
        });
        assert_eq!(
            update_slot(&inputs(true, failed.clone()), false),
            UpdateSlot::Failed
        );
        assert_eq!(
            update_slot(&inputs(false, failed), false),
            UpdateSlot::Failed
        );
    }

    #[test]
    fn no_progress_and_no_update_hides_the_slot() {
        assert_eq!(update_slot(&inputs(false, None), false), UpdateSlot::Hidden);
        assert_eq!(update_slot(&inputs(true, None), true), UpdateSlot::Hidden);
    }

    #[test]
    fn running_progress_maps_to_a_tray_progress() {
        assert_eq!(
            tray_progress_from(&progress(RunPhase::Compiling, 42)),
            Some(TrayUpdateProgress {
                percent: 42,
                failed: false
            })
        );
    }

    #[test]
    fn done_has_no_tray_item() {
        assert_eq!(tray_progress_from(&progress(RunPhase::Done, 100)), None);
    }

    #[test]
    fn failed_maps_to_a_failed_tray_progress() {
        assert_eq!(
            tray_progress_from(&progress(RunPhase::Failed, 37)),
            Some(TrayUpdateProgress {
                percent: 37,
                failed: true
            })
        );
    }
}

#[cfg(test)]
#[path = "tray_job_tests.rs"]
mod tray_job_tests;
