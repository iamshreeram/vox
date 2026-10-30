//! Appearance model and transport (UI enterprise S1, DESIGN Revision 3, contracts 2-4).
//!
//! One versioned snapshot `{rev, theme, custom_colors, accent, chat_theme, visual_style}` is the
//! only appearance transport. The persisted settings are the authority. Every writer goes through
//! `commit_appearance`, which serialises the write and the publish, bumps the appearance rev
//! (independent of the chat-opacity rev) and emits the full committed snapshot to every existing
//! consumer window. The consumer pages apply only a strictly higher rev (`should_apply`).
//! Nothing here logs colours, paths or conversation content.

use crate::chat_opacity::{replay_interval, CHAT_OPACITY};
use crate::chat_window::CHAT_WINDOW_LABEL;
use crate::orb_window::ORB_WINDOW_LABEL;
use crate::settings::{
    self, Accent, AppSettings, ChatTheme, CustomThemeColors, OrbColor, Theme, VisualStyle,
};
use crate::window_commands::BUBBLES_WINDOW_LABEL;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// Tauri event that carries an `AppearanceSnapshot` to the consumer windows.
pub const APPEARANCE_EVENT: &str = "appearance";

/// The consumer windows, in delivery order: the conversation, the orb and the bubbles.
pub const CONSUMER_LABELS: [&str; 3] = [CHAT_WINDOW_LABEL, ORB_WINDOW_LABEL, BUBBLES_WINDOW_LABEL];

/// Legacy global event kept for the main window and the recording overlay.
const LEGACY_THEME_EVENT: &str = "theme-changed";

/// A committed appearance. `rev` grows on every commit and is independent of the opacity rev.
/// `custom_colors` is present only while the theme is `Custom`, so no other window receives the
/// palette.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub struct AppearanceSnapshot {
    /// Monotonic revision. A consumer applies only a strictly higher value.
    pub rev: u64,
    /// The theme as stored.
    pub theme: Theme,
    /// The custom palette, `Some` only when `theme` is `Custom`.
    pub custom_colors: Option<CustomThemeColors>,
    /// The accent.
    pub accent: Accent,
    /// The conversation window theme.
    pub chat_theme: ChatTheme,
    /// The visual style.
    pub visual_style: VisualStyle,
    /// The orb colour (`auto`, `app` or an accent). Always present on the wire as a string.
    pub orb_accent: OrbColor,
}

/// The appearance fields of the settings, and the only ones a commit may change.
#[derive(Debug, Clone, PartialEq)]
pub struct AppearanceFields {
    pub theme: Theme,
    pub custom_theme_colors: CustomThemeColors,
    pub accent: Accent,
    pub chat_theme: ChatTheme,
    pub visual_style: VisualStyle,
    pub orb_accent: OrbColor,
}

impl AppearanceFields {
    /// Reads the appearance fields of `settings`.
    pub fn from_settings(settings: &AppSettings) -> Self {
        Self {
            theme: settings.theme,
            custom_theme_colors: settings.custom_theme_colors.clone(),
            accent: settings.accent,
            chat_theme: settings.chat_theme,
            visual_style: settings.visual_style,
            orb_accent: settings.orb_accent,
        }
    }

    /// Writes these fields into `settings`. No other field is touched.
    pub fn apply_to(self, settings: &mut AppSettings) {
        settings.theme = self.theme;
        settings.custom_theme_colors = self.custom_theme_colors;
        settings.accent = self.accent;
        settings.chat_theme = self.chat_theme;
        settings.visual_style = self.visual_style;
        settings.orb_accent = self.orb_accent;
    }
}

/// The snapshot of the persisted settings at revision `rev`.
pub fn snapshot_from_settings(rev: u64, settings: &AppSettings) -> AppearanceSnapshot {
    AppearanceSnapshot {
        rev,
        theme: settings.theme,
        custom_colors: (settings.theme == Theme::Custom)
            .then(|| settings.custom_theme_colors.clone()),
        accent: settings.accent,
        chat_theme: settings.chat_theme,
        visual_style: settings.visual_style,
        orb_accent: settings.orb_accent,
    }
}

/// Holds the latest committed appearance. The rev is assigned under the holder lock, so the
/// revision and the content always move together.
pub struct AppearanceHolder {
    rev: AtomicU64,
    current: Mutex<Option<AppearanceSnapshot>>,
}

impl AppearanceHolder {
    /// An empty holder: no snapshot and revision 0.
    pub const fn new() -> Self {
        Self {
            rev: AtomicU64::new(0),
            current: Mutex::new(None),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<AppearanceSnapshot>> {
        self.current.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn next_rev(&self) -> u64 {
        self.rev.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// The highest revision assigned so far.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn revision(&self) -> u64 {
        self.rev.load(Ordering::SeqCst)
    }

    /// Publishes the persisted settings as the next revision and returns the snapshot.
    pub fn publish(&self, settings: &AppSettings) -> AppearanceSnapshot {
        let mut current = self.lock();
        let snapshot = snapshot_from_settings(self.next_rev(), settings);
        *current = Some(snapshot.clone());
        snapshot
    }

    /// The current snapshot. When nothing was committed yet, it is seeded once from `read`.
    pub fn current_or_seed(&self, read: impl FnOnce() -> AppSettings) -> AppearanceSnapshot {
        let mut current = self.lock();
        if let Some(snapshot) = current.as_ref() {
            return snapshot.clone();
        }
        let snapshot = snapshot_from_settings(self.next_rev(), &read());
        *current = Some(snapshot.clone());
        snapshot
    }
}

impl Default for AppearanceHolder {
    fn default() -> Self {
        Self::new()
    }
}

/// The one appearance holder of the app.
pub static APPEARANCE: AppearanceHolder = AppearanceHolder::new();

/// Serialises a commit's write and publish, so the published order is the persisted order.
static COMMIT_SERIAL: Mutex<()> = Mutex::new(());

/// Applies `mutation` to the appearance fields of `settings`, then publishes the result.
/// The pure form of `commit_appearance`. The app path runs the same two steps around the store.
#[cfg_attr(not(test), allow(dead_code))]
pub fn commit_in_memory(
    settings: &mut AppSettings,
    holder: &AppearanceHolder,
    mutation: impl FnOnce(&mut AppearanceFields),
) -> AppearanceSnapshot {
    let mut fields = AppearanceFields::from_settings(settings);
    mutation(&mut fields);
    fields.apply_to(settings);
    holder.publish(settings)
}

/// The one gated commit of every appearance writer. Persists the change under the settings
/// gate, bumps the rev, and delivers the committed snapshot. Returns the snapshot.
pub fn commit_appearance(
    app: &AppHandle,
    mutation: impl FnOnce(&mut AppearanceFields),
) -> AppearanceSnapshot {
    let snapshot = {
        let _serial = COMMIT_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        settings::write_settings_gated(app, |settings| {
            let mut fields = AppearanceFields::from_settings(settings);
            mutation(&mut fields);
            fields.apply_to(settings);
        });
        // Only appearance commits change these fields, and they are serialised by this lock, so
        // the read-back is the persisted value.
        APPEARANCE.publish(&settings::get_settings(app))
    };
    publish_snapshot(app, &snapshot);
    snapshot
}

/// The pure delivery plan: one `(label, payload)` per consumer window that exists. An absent
/// window is skipped, which is success, not an error.
pub fn plan_deliveries(
    snapshot: &AppearanceSnapshot,
    exists: impl Fn(&str) -> bool,
) -> Vec<(&'static str, AppearanceSnapshot)> {
    CONSUMER_LABELS
        .iter()
        .copied()
        .filter(|label| exists(label))
        .map(|label| (label, snapshot.clone()))
        .collect()
}

/// The consumer rule: a delivery applies only when its rev is strictly higher than the applied
/// one. A duplicate or an older, reordered delivery is ignored. The pages apply this rule.
#[cfg_attr(not(test), allow(dead_code))]
pub fn should_apply(applied_rev: u64, incoming_rev: u64) -> bool {
    incoming_rev > applied_rev
}

/// Emits `snapshot` to every existing consumer window. Logs counts only.
pub fn emit_to_consumers(app: &AppHandle, snapshot: &AppearanceSnapshot) {
    let deliveries = plan_deliveries(snapshot, |label| app.get_webview_window(label).is_some());
    let mut delivered = 0usize;
    for (label, payload) in deliveries {
        match app.emit_to(label, APPEARANCE_EVENT, payload) {
            Ok(()) => delivered += 1,
            Err(e) => log::warn!("appearance emit failed: {e}"),
        }
    }
    log::debug!("appearance snapshot delivered to {delivered} windows");
}

/// Publishes a commit: the consumer windows get the snapshot, and the legacy global event goes
/// out for the main window and the overlay.
fn publish_snapshot(app: &AppHandle, snapshot: &AppearanceSnapshot) {
    emit_to_consumers(app, snapshot);
    let _ = app.emit(LEGACY_THEME_EVENT, snapshot.theme);
}

/// The committed snapshot, seeded from the store on first use.
pub fn current_snapshot(app: &AppHandle) -> AppearanceSnapshot {
    APPEARANCE.current_or_seed(|| settings::get_settings(app))
}

/// Re-sends the current snapshot to the consumer windows. Runs on show, focus and creation.
pub fn emit_current(app: &AppHandle) {
    let snapshot = current_snapshot(app);
    emit_to_consumers(app, &snapshot);
}

/// Whether any consumer window exists. The shared worker lives exactly while this holds.
pub fn any_consumer_exists(app: &AppHandle) -> bool {
    CONSUMER_LABELS
        .iter()
        .any(|label| app.get_webview_window(label).is_some())
}

/// One replay tick: the chat-opacity snapshot to the chat window when it exists, then the
/// current appearance snapshot to every existing consumer. Each read is fresh and no lock is
/// held across an emit.
pub fn replay_tick(app: &AppHandle) {
    if app.get_webview_window(CHAT_WINDOW_LABEL).is_some() {
        crate::commands::chat_opacity::emit_snapshot(app, CHAT_OPACITY.snapshot());
    }
    emit_current(app);
}

/// The shared replay loop. While `alive` holds it calls `tick`, then sleeps one replay interval.
/// Returns the number of ticks. The clock, liveness and tick are injected.
pub fn run_consumer_replay(
    mut alive: impl FnMut() -> bool,
    mut sleep: impl FnMut(Duration),
    mut tick: impl FnMut(),
) -> usize {
    let mut ticks = 0;
    while alive() {
        tick();
        ticks += 1;
        sleep(replay_interval());
    }
    ticks
}

/// Percent-encodes `input` for a query value. Only the unreserved characters pass through.
pub fn encode_query_value(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The snapshot as the value of the `ap` query parameter: percent-encoded JSON.
pub fn seed_value(snapshot: &AppearanceSnapshot) -> String {
    match serde_json::to_string(snapshot) {
        Ok(json) => encode_query_value(&json),
        Err(_) => encode_query_value("{}"),
    }
}

/// Appends `ap=<seed>` to a page URL, keeping every parameter already there.
pub fn with_ap_param(base: &str, snapshot: &AppearanceSnapshot) -> String {
    let separator = if base.contains('?') { '&' } else { '?' };
    format!("{base}{separator}ap={}", seed_value(snapshot))
}
