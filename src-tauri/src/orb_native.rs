//! Native side of the floating orb (DESIGN.md Rev 5, 'Coordinates', 'Gesture engine owned by Rust',
//! 'Rust modules and commands'). This file creates and moves the orb window, runs the drag and
//! gaze ticker, and carries out the calls the pure drivers in `orb_runtime` produce. The window
//! work runs on the main thread. Runtime state is only locked for pure driver calls, never while a
//! window call is waiting for the main thread, so the two can never wait on each other.
//! Nothing here writes files or console output; logs name operations and errors only.

use crate::appearance;
use crate::chat_presentation::{
    current_os, floating_supported, LinuxBackend, PlatformEnv, ORB_VISIBLE_DEFAULT,
};
use crate::chat_surface;
use crate::chat_window::{is_app_navigation, CHAT_WINDOW_LABEL};
use crate::orb_gaze::Gaze;
use crate::orb_geometry::{
    clamp_orb, clamp_orb_px, effective_orb_logical_px, monitor_for, nearest_preset, orb_px_logical,
    position_from_rect, resize_keep_center, restore_position_px, MonitorInfo, Rect,
};
use crate::orb_gesture::GestureResult;
use crate::orb_runtime::{
    monitors_from_raw, orb_menu_items_for_px, tick_interval_ms, ButtonProbe, CursorSource,
    GazeDriver, GeometryGuard, GestureDriver, MenuEntry, OrbHost, PendingResize, RawMonitor,
};
use crate::orb_window::{orb_window_spec_px, ORB_WINDOW_LABEL};
use crate::settings::{self, write_settings_gated, AppSettings};
use crate::show_main_window;
use crate::tray_i18n::{get_tray_translations, TrayStrings};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::webview::NewWindowResponse;
#[cfg(not(target_os = "macos"))]
use tauri::WebviewWindowBuilder;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WindowEvent,
};

#[cfg(target_os = "macos")]
use objc2::class;
#[cfg(target_os = "macos")]
use tauri::{LogicalSize, Size};
#[cfg(target_os = "macos")]
use tauri_nspanel::{
    tauri_panel, CollectionBehavior, ManagerExt, PanelBuilder, PanelLevel, StyleMask,
};

#[cfg(target_os = "windows")]
use crate::overlay::force_overlay_topmost;

/// Bundled entry page of the orb window (Vite input `orb`).
const ORB_ENTRY_URL: &str = "src/orb/index.html";

/// Gap between the orb and the screen corner of the default position, in logical pixels.
const DEFAULT_INSET_LOGICAL_PX: f64 = 20.0;

/// How often the ticker compares the monitor list, in milliseconds.
const MONITOR_CHECK_MS: u64 = 2_000;

/// Event that carries a gesture result to the orb page.
const GESTURE_EVENT: &str = "orb-gesture";

/// Event that carries a gaze direction to the orb page.
const GAZE_EVENT: &str = "orb-gaze";

#[cfg(target_os = "macos")]
tauri_panel! {
    panel!(OrbPanel {
        config: {
            can_become_key_window: false,
            is_floating_panel: true
        }
    })
}

/// Runtime state of the orb: the drivers, the reduced-motion flag and the running ticker.
#[derive(Default)]
pub struct OrbRuntime {
    gesture: GestureDriver,
    gaze: GazeDriver,
    reduced_motion: bool,
    ticker: Option<Arc<TickerSignal>>,
    geometry: GeometryGuard,
    pending_resize: PendingResize,
}

/// The orb runtime as managed by Tauri. The lock is held only for pure driver calls. The atomic
/// records whether the orb window is shown: it is set on the main thread and read by the ticker,
/// which therefore never calls a window or panel method to find out.
pub struct OrbRuntimeCell(Mutex<OrbRuntime>, AtomicBool);

impl OrbRuntimeCell {
    /// A runtime with nothing running and the orb hidden.
    fn new() -> Self {
        Self(Mutex::new(OrbRuntime::default()), AtomicBool::new(false))
    }

    /// Locks the runtime. A poisoned lock is recovered, since the state is plain data.
    fn lock(&self) -> MutexGuard<'_, OrbRuntime> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether the orb window is shown.
    fn shown(&self) -> bool {
        self.1.load(Ordering::SeqCst)
    }

    /// Records whether the orb window is shown.
    fn set_shown(&self, shown: bool) {
        self.1.store(shown, Ordering::SeqCst);
    }
}

/// Stop and wake signal of one ticker thread. The ticker sleeps on it between passes, so a press
/// or a stop ends the sleep at once instead of after a fixed interval.
struct TickerSignal {
    stop: AtomicBool,
    pending: Mutex<bool>,
    cvar: Condvar,
}

impl TickerSignal {
    fn new() -> Self {
        Self {
            stop: AtomicBool::new(false),
            pending: Mutex::new(false),
            cvar: Condvar::new(),
        }
    }

    /// Whether this ticker has been told to stop.
    fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// Tells the ticker to stop and wakes it so it can exit.
    fn request_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.wake();
    }

    /// Ends the ticker's current or next wait early.
    fn wake(&self) {
        *lock_pending(&self.pending) = true;
        self.cvar.notify_all();
    }

    /// Waits until woken or until `timeout` passes, then clears the wake.
    fn wait(&self, timeout: Duration) {
        let mut pending = lock_pending(&self.pending);
        if !*pending {
            pending = match self.cvar.wait_timeout(pending, timeout) {
                Ok((guard, _)) => guard,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
        *pending = false;
    }
}

/// Locks the wake flag of a ticker. A poisoned lock is recovered, since the flag is plain data.
fn lock_pending(pending: &Mutex<bool>) -> MutexGuard<'_, bool> {
    pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Current pointer and orb geometry, read before the runtime lock is taken.
#[derive(Clone, Copy)]
struct Frame {
    cursor: Option<(f64, f64)>,
    orb: Option<Rect>,
    scale: f64,
}

/// One effect the drivers asked for. Carried out after the runtime lock is released.
enum Call {
    Move(f64, f64),
    Gesture(u64, GestureResult),
    Click,
    Save(Rect, u64),
    Gaze(Gaze),
}

/// Host that records the drivers' calls. The orb geometry it reports is the frame read before
/// the lock, updated by its own moves.
struct Recorder {
    orb: Option<Rect>,
    scale: f64,
    calls: Vec<Call>,
}

impl Recorder {
    /// A recorder for one driver call on `orb` at `scale`.
    fn new(orb: Option<Rect>, scale: f64) -> Self {
        Self {
            orb,
            scale,
            calls: Vec::new(),
        }
    }
}

impl OrbHost for Recorder {
    fn orb_rect(&self) -> Option<Rect> {
        self.orb
    }
    fn scale(&self) -> f64 {
        self.scale
    }
    fn move_to(&mut self, x: f64, y: f64) {
        if let Some(rect) = self.orb.as_mut() {
            rect.x = x;
            rect.y = y;
        }
        self.calls.push(Call::Move(x, y));
    }
    fn emit_gesture(&mut self, id: u64, result: GestureResult) {
        self.calls.push(Call::Gesture(id, result));
    }
    fn click(&mut self) {
        self.calls.push(Call::Click);
    }
    fn save_position(&mut self, final_orb: Rect) {
        // The revision is stamped under the runtime lock by `stamp_saves`.
        self.calls.push(Call::Save(final_orb, 0));
    }
    fn emit_gaze(&mut self, gaze: Gaze) {
        self.calls.push(Call::Gaze(gaze));
    }
}

/// Cursor position captured for one driver call.
struct Snapshot(Option<(f64, f64)>);

impl CursorSource for Snapshot {
    fn cursor(&self) -> Option<(f64, f64)> {
        self.0
    }
}

/// Left button state captured for one driver call.
struct Down(bool);

impl ButtonProbe for Down {
    fn left_down(&self) -> bool {
        self.0
    }
}

/// Payload of the `orb-gesture` event.
#[derive(Serialize, Clone)]
struct GesturePayload {
    id: u64,
    result: &'static str,
}

/// Payload of the `orb-gaze` event.
#[derive(Serialize, Clone)]
struct GazePayload {
    x: f64,
    y: f64,
}

/// Monotonic milliseconds since the first call.
fn now_ms() -> u64 {
    static CLOCK: OnceLock<Instant> = OnceLock::new();
    CLOCK.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Makes sure the runtime state is managed before it is used.
fn ensure_runtime(app: &AppHandle) {
    if app.try_state::<OrbRuntimeCell>().is_none() {
        app.manage(OrbRuntimeCell::new());
    }
}

/// The orb rectangle of `window` in physical pixels.
pub(crate) fn window_rect(window: &WebviewWindow) -> Option<Rect> {
    let position = window.outer_position().ok()?;
    let size = window.outer_size().ok()?;
    Some(Rect {
        x: f64::from(position.x),
        y: f64::from(position.y),
        w: f64::from(size.width),
        h: f64::from(size.height),
    })
}

/// Reads the pointer and the orb geometry. Window queries wait for the main thread, so this is
/// never called while the runtime lock is held.
fn read_frame(app: &AppHandle) -> Frame {
    let cursor = app.cursor_position().ok().map(|p| (p.x, p.y));
    let (orb, scale) = match app.get_webview_window(ORB_WINDOW_LABEL) {
        Some(window) => (window_rect(&window), window.scale_factor().unwrap_or(1.0)),
        None => (None, 1.0),
    };
    Frame { cursor, orb, scale }
}

/// Whether a gesture is in progress. Locks the runtime briefly and makes no window call.
fn gesture_active(app: &AppHandle) -> bool {
    app.try_state::<OrbRuntimeCell>()
        .map(|cell| cell.lock().gesture.is_active())
        .unwrap_or(false)
}

/// The monitors as the orb sees them: work areas, scales and the primary monitor.
pub(crate) fn monitors_for(app: &AppHandle) -> Vec<MonitorInfo> {
    let listed = app.available_monitors().unwrap_or_default();
    let primary = app.primary_monitor().ok().flatten();
    let raw: Vec<RawMonitor> = listed
        .iter()
        .map(|monitor| {
            let work = monitor.work_area();
            RawMonitor {
                name: monitor.name().cloned(),
                position: (monitor.position().x, monitor.position().y),
                size: (monitor.size().width, monitor.size().height),
                work_position: (work.position.x, work.position.y),
                work_size: (work.size.width, work.size.height),
                scale: monitor.scale_factor(),
            }
        })
        .collect();
    let primary_index = primary.and_then(|p| {
        listed
            .iter()
            .position(|m| m.position() == p.position() && m.size() == p.size())
    });
    monitors_from_raw(&raw, primary_index)
}

/// Default orb rectangle: the bottom-right corner of the primary work area, inset, then clamped.
fn default_rect(logical_px: f64, monitors: &[MonitorInfo]) -> Option<Rect> {
    let primary = monitors
        .iter()
        .find(|m| m.primary)
        .or_else(|| monitors.first())?;
    let side = orb_px_logical(logical_px, primary.scale);
    let scale = if primary.scale.is_finite() && primary.scale > 0.0 {
        primary.scale
    } else {
        1.0
    };
    let inset = DEFAULT_INSET_LOGICAL_PX * scale;
    let work = primary.work;
    let rect = Rect {
        x: work.right() - inset - side,
        y: work.bottom() - inset - side,
        w: side,
        h: side,
    };
    Some(clamp_orb(rect, monitors))
}

/// The rectangle the orb should occupy: the saved position when it restores, else the default.
fn target_rect(settings: &AppSettings, monitors: &[MonitorInfo]) -> Option<Rect> {
    let logical = effective_logical(settings);
    settings
        .orb_position
        .as_ref()
        .and_then(|saved| restore_position_px(saved, logical, monitors))
        .or_else(|| default_rect(logical, monitors))
}

/// The effective orb edge in logical pixels: the free size when set, else the legacy preset.
fn effective_logical(settings: &AppSettings) -> f64 {
    effective_orb_logical_px(settings.orb_size, settings.orb_size_px)
}

/// The platform facts the orb decision needs. On Linux the display type and compositing come
/// from GDK; elsewhere the backend is unknown and compositing is assumed.
pub(crate) fn current_env() -> PlatformEnv {
    let (linux_backend, composited) = display_facts();
    PlatformEnv {
        os: current_os(),
        linux_backend,
        composited,
    }
}

/// Linux display facts from GDK. Runs on the main thread, where GTK is initialised.
#[cfg(target_os = "linux")]
fn display_facts() -> (LinuxBackend, bool) {
    use gtk::prelude::*;
    let Some(display) = gtk::gdk::Display::default() else {
        return (LinuxBackend::Unknown, false);
    };
    let type_name = display.type_().name();
    let composited = gtk::gdk::Screen::default()
        .map(|screen| screen.is_composited())
        .unwrap_or(false);
    (
        crate::chat_presentation::linux_backend_from_display_type(Some(type_name)),
        composited,
    )
}

/// Non-Linux platforms: the backend does not apply, and floating windows are assumed to work.
#[cfg(not(target_os = "linux"))]
fn display_facts() -> (LinuxBackend, bool) {
    (LinuxBackend::Unknown, true)
}

/// Shows or hides the orb to match the settings, and keeps its position and size right. Safe to
/// call from any thread; the work runs on the main thread. With the orb disabled nothing is built.
pub fn apply_visibility(app: &AppHandle) {
    let job_app = app.clone();
    if let Err(e) = app.run_on_main_thread(move || apply_on_main(&job_app)) {
        log::warn!("orb update could not be scheduled: {e}");
    }
}

/// Builds the orb window hidden when it does not exist yet. Main thread. Returns whether the
/// window exists afterwards.
pub(crate) fn ensure_orb_window(app: &AppHandle) -> bool {
    if app.get_webview_window(ORB_WINDOW_LABEL).is_some() {
        return true;
    }
    let settings = settings::get_settings(app);
    match build_orb(app, effective_logical(&settings)) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("orb build failed: {e}");
            false
        }
    }
}

/// Sets whether the wake-appear session wants the orb (A9-2). Read by `apply_on_main`.
pub(crate) fn set_orb_transient(on: bool) {
    ORB_TRANSIENT.store(on, Ordering::SeqCst);
}

/// Main-thread body of `apply_visibility`. The orb is shown when the saved setting asks for it,
/// or while a wake-appear session wants it on a platform that supports floating.
pub(crate) fn apply_on_main(app: &AppHandle) {
    ensure_runtime(app);
    let settings = settings::get_settings(app);
    let env = current_env();
    record_platform_offer(app, &env);
    let persisted = settings.orb_visible.unwrap_or(ORB_VISIBLE_DEFAULT);
    let share = ORB_TRANSIENT.load(Ordering::SeqCst);
    if !orb_wanted(persisted, share, floating_supported(&env).is_ok()) {
        hide_orb(app);
        return;
    }
    let monitors = monitors_for(app);
    let Some(rect) = target_rect(&settings, &monitors) else {
        log::warn!("orb not shown: no monitor found");
        return;
    };
    if app.get_webview_window(ORB_WINDOW_LABEL).is_none() {
        if let Err(e) = build_orb(app, effective_logical(&settings)) {
            log::warn!("orb build failed: {e}");
            return;
        }
    }
    let Some(window) = app.get_webview_window(ORB_WINDOW_LABEL) else {
        return;
    };
    apply_geometry(&window, rect);
    show_orb(app, &window);
    if let Some(cell) = app.try_state::<OrbRuntimeCell>() {
        cell.set_shown(true);
        let mut rt = cell.lock();
        let reduced = rt.reduced_motion;
        rt.gaze.set_visible(true);
        rt.gaze.set_reduced_motion(reduced);
    }
    crate::bubble_native::orb_visibility_changed(app, true);
    start_ticker(app);
}

/// Main thread. The bubble executor's entry point: shows or hides the orb, and places its window
/// only when `executor_needs_geometry` says so. A shown orb, or one under a gesture, is never
/// re-targeted here, so a drag is never fought (MAJOR 2 of G4b.fix).
pub(crate) fn show_hide_only(app: &AppHandle, want: bool) {
    if !want {
        hide_orb(app);
        return;
    }
    ensure_runtime(app);
    let exists = app.get_webview_window(ORB_WINDOW_LABEL).is_some();
    let shown = app
        .try_state::<OrbRuntimeCell>()
        .is_some_and(|cell| cell.shown());
    if executor_needs_geometry(exists, shown, gesture_active(app)) {
        let settings = settings::get_settings(app);
        let monitors = monitors_for(app);
        let Some(rect) = target_rect(&settings, &monitors) else {
            log::warn!("orb not shown: no monitor found");
            return;
        };
        if !exists {
            if let Err(e) = build_orb(app, effective_logical(&settings)) {
                log::warn!("orb build failed: {e}");
                return;
            }
        }
        let Some(window) = app.get_webview_window(ORB_WINDOW_LABEL) else {
            return;
        };
        apply_geometry(&window, rect);
    }
    let Some(window) = app.get_webview_window(ORB_WINDOW_LABEL) else {
        return;
    };
    present(app, &window);
}

/// Shows the orb window, marks it shown for the ticker and the gaze, tells the bubbles and starts
/// the ticker. Main thread. Places nothing; the caller decides the geometry.
fn present(app: &AppHandle, window: &WebviewWindow) {
    show_orb(app, window);
    if let Some(cell) = app.try_state::<OrbRuntimeCell>() {
        cell.set_shown(true);
        let mut rt = cell.lock();
        let reduced = rt.reduced_motion;
        rt.gaze.set_visible(true);
        rt.gaze.set_reduced_motion(reduced);
    }
    crate::bubble_native::orb_visibility_changed(app, true);
    start_ticker(app);
}

/// Remembers whether floating is blocked on this system. When that changes the tray is resynced,
/// so its orb item follows. Runs on the main thread.
fn record_platform_offer(app: &AppHandle, env: &PlatformEnv) {
    let blocked = floating_supported(env).is_err();
    let previous = FLOATING_BLOCKED.swap(blocked, Ordering::SeqCst);
    let mut last = LAST_ENV
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *last = Some(*env);
    drop(last);
    if previous != blocked {
        crate::tray::sync_tray(app);
    }
}

/// The platform facts as the last main-thread apply read them. Off the main thread (the converse
/// session hook) the GDK display must not be read, so the session decision uses this copy. Before
/// any apply it assumes an unidentified Linux backend, which blocks floating.
pub(crate) fn cached_env() -> PlatformEnv {
    let last = *LAST_ENV
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    last.unwrap_or(PlatformEnv {
        os: current_os(),
        linux_backend: LinuxBackend::Unknown,
        composited: false,
    })
}

/// The platform facts as last recorded on the main thread. Written by `record_platform_offer`.
static LAST_ENV: Mutex<Option<PlatformEnv>> = Mutex::new(None);

/// Whether the tray should offer the orb item: false where floating is unsupported, as found by
/// the last main-thread apply. Reads only an atomic, so the tray may call it from any thread.
pub fn floating_offered() -> bool {
    !FLOATING_BLOCKED.load(Ordering::SeqCst)
}

/// Floating is blocked on this system, as recorded by the last main-thread apply.
static FLOATING_BLOCKED: AtomicBool = AtomicBool::new(false);

/// Whether the wake-appear session currently owns a transient show of the orb (set by the bubble
/// executor to the session's own share only; the saved setting is never stored here).
static ORB_TRANSIENT: AtomicBool = AtomicBool::new(false);

/// Whether the orb must be shown: the saved setting or the session's share, on a platform that
/// supports floating. Pure, so the apply condition is tested without a window.
pub(crate) fn orb_wanted(persisted: bool, share: bool, supported: bool) -> bool {
    supported && (persisted || share)
}

/// Whether the bubble executor must place the orb window: when the window does not exist, or it
/// is hidden and no gesture is in progress. A shown orb, or one under a gesture, keeps its place
/// so a drag is never fought (MAJOR 2 of G4b.fix).
pub(crate) fn executor_needs_geometry(
    orb_window_exists: bool,
    orb_shown: bool,
    gesture_active: bool,
) -> bool {
    !orb_window_exists || (!orb_shown && !gesture_active)
}

/// Hides the orb: cancels any gesture, stops the ticker and hides the window.
fn hide_orb(app: &AppHandle) {
    stop_orb(app);
    hide_window(app);
}

/// Cancels any gesture (emitting Cancel, never a toggle), stops the ticker and marks the orb as
/// not shown. Does not touch the window.
fn stop_orb(app: &AppHandle) {
    let mut calls = Recorder::new(None, 1.0);
    if let Some(cell) = app.try_state::<OrbRuntimeCell>() {
        cell.set_shown(false);
        let mut rt = cell.lock();
        if let Some(ticker) = rt.ticker.take() {
            ticker.request_stop();
        }
        rt.gesture.cancel(&mut calls);
        rt.gaze.set_visible(false);
    }
    execute(app, calls.calls);
    crate::bubble_native::orb_visibility_changed(app, false);
}

/// The orb window was destroyed: a gesture in progress is cancelled, never completed.
fn on_destroyed(app: &AppHandle) {
    cancel_gesture(app);
    stop_orb(app);
}

/// Cancels the gesture in progress, if any. Emits Cancel and never toggles the chat.
fn cancel_gesture(app: &AppHandle) {
    let mut calls = Recorder::new(None, 1.0);
    if let Some(cell) = app.try_state::<OrbRuntimeCell>() {
        cell.lock().gesture.cancel(&mut calls);
    }
    execute(app, calls.calls);
}

/// Places the orb window at `rect` in physical pixels: size first, then position.
fn apply_geometry(window: &WebviewWindow, rect: Rect) {
    if let Err(e) = window.set_size(PhysicalSize::new(
        rect.w.round() as u32,
        rect.h.round() as u32,
    )) {
        log::warn!("orb resize failed: {e}");
    }
    if let Err(e) = window.set_position(PhysicalPosition::new(
        rect.x.round() as i32,
        rect.y.round() as i32,
    )) {
        log::warn!("orb move failed: {e}");
    }
}

/// Re-fits the orb after a move or a scale change: size for the monitor under its centre, then
/// clamp into a work area. Moves and resizes only when the rectangle actually changes.
fn fit_orb(app: &AppHandle) {
    let Some(window) = app.get_webview_window(ORB_WINDOW_LABEL) else {
        return;
    };
    let Some(current) = window_rect(&window) else {
        return;
    };
    let settings = settings::get_settings(app);
    let monitors = monitors_for(app);
    let (cx, cy) = current.center();
    let Some(monitor) = monitor_for((cx, cy), &monitors) else {
        return;
    };
    let side = orb_px_logical(effective_logical(&settings), monitor.scale);
    let wanted = Rect {
        x: cx - side / 2.0,
        y: cy - side / 2.0,
        w: side,
        h: side,
    };
    let target = clamp_orb(wanted, &monitors);
    if target != current {
        apply_geometry(&window, target);
    }
}

/// Handler for the window's move and scale events. Skips the work while a gesture is in progress,
/// so a drag is never fought, and while the runtime is busy.
fn on_geometry_event(app: &AppHandle) {
    // The bubble stack follows the orb even during a drag; only the orb's own refit is skipped.
    crate::bubble_native::orb_geometry_changed(app);
    if let Some(cell) = app.try_state::<OrbRuntimeCell>() {
        match cell.0.try_lock() {
            Ok(rt) => {
                if rt.gesture.is_active() {
                    return;
                }
            }
            Err(_) => return,
        }
    }
    fit_orb(app);
}

/// The orb page URL: the entry page plus the appearance seed, so the first paint is right.
fn orb_page_url(app: &AppHandle) -> String {
    appearance::with_ap_param(ORB_ENTRY_URL, &appearance::current_snapshot(app))
}

/// Creates the orb window hidden. Its geometry is set by the caller before it is shown.
#[cfg(target_os = "macos")]
fn build_orb(app: &AppHandle, logical_px: f64) -> tauri::Result<()> {
    let spec = orb_window_spec_px(logical_px);
    let side = spec.logical_px;
    let transparent = spec.transparent;
    let decorations = spec.decorations;
    let focusable = spec.focusable;
    let panel = PanelBuilder::<_, OrbPanel>::new(app, ORB_WINDOW_LABEL)
        .url(WebviewUrl::App(orb_page_url(app).into()))
        .title("Vox orb")
        .level(PanelLevel::Status)
        .size(Size::Logical(LogicalSize {
            width: side,
            height: side,
        }))
        .has_shadow(spec.shadow)
        .transparent(transparent)
        .no_activate(true)
        .corner_radius(0.0)
        .style_mask(StyleMask::empty().borderless().nonactivating_panel())
        .with_window(move |w| {
            w.decorations(decorations)
                .transparent(transparent)
                .focusable(focusable)
                .on_navigation(is_app_navigation)
                .on_new_window(|_url, _features| NewWindowResponse::Deny)
        })
        .collection_behavior(
            CollectionBehavior::new()
                .can_join_all_spaces()
                .full_screen_auxiliary(),
        )
        .build()?;
    panel.hide();
    attach(app);
    Ok(())
}

/// Creates the orb window hidden. Windows and Linux X11 use the same builder with the native
/// topmost and all-workspaces flags from the spec.
#[cfg(not(target_os = "macos"))]
fn build_orb(app: &AppHandle, logical_px: f64) -> tauri::Result<()> {
    let spec = orb_window_spec_px(logical_px);
    let builder = WebviewWindowBuilder::new(
        app,
        ORB_WINDOW_LABEL,
        WebviewUrl::App(orb_page_url(app).into()),
    )
    .title("Vox orb")
    .inner_size(spec.logical_px, spec.logical_px)
    .resizable(spec.resizable)
    .decorations(spec.decorations)
    .transparent(spec.transparent)
    .always_on_top(spec.always_on_top)
    .skip_taskbar(spec.skip_taskbar)
    .shadow(spec.shadow)
    .focusable(spec.focusable)
    .visible(false)
    .focused(false)
    .on_navigation(is_app_navigation)
    .on_new_window(|_url, _features| NewWindowResponse::Deny);
    #[cfg(target_os = "linux")]
    let builder = builder.visible_on_all_workspaces(spec.visible_on_all_workspaces);
    builder.build()?;
    attach(app);
    Ok(())
}

/// Attaches the orb window's move, scale and menu handlers after it is built.
fn attach(app: &AppHandle) {
    // Every orb build ends here, so the shared replay worker starts for the orb too.
    crate::chat_window::start_replay_worker(app);
    let Some(window) = app.get_webview_window(ORB_WINDOW_LABEL) else {
        return;
    };
    let geometry_app = app.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. } => {
            on_geometry_event(&geometry_app);
        }
        WindowEvent::Destroyed => on_destroyed(&geometry_app),
        _ => {}
    });
    let menu_app = app.clone();
    window.on_menu_event(move |_window, event| handle_menu(&menu_app, event.id.as_ref()));
}

/// Shows the orb without taking focus: a non-key panel on macOS, a plain show elsewhere.
fn show_orb(app: &AppHandle, window: &WebviewWindow) {
    appearance::emit_current(app);
    #[cfg(target_os = "macos")]
    {
        let _ = window;
        match app.get_webview_panel(ORB_WINDOW_LABEL) {
            Ok(panel) => panel.order_front_regardless(),
            Err(e) => log::warn!("orb show failed: {e:?}"),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        if let Err(e) = window.show() {
            log::warn!("orb show failed: {e}");
        }
        #[cfg(target_os = "windows")]
        force_overlay_topmost(window);
    }
}

/// Hides the orb window.
fn hide_window(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        if let Ok(panel) = app.get_webview_panel(ORB_WINDOW_LABEL) {
            panel.hide();
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(window) = app.get_webview_window(ORB_WINDOW_LABEL) {
            if let Err(e) = window.hide() {
                log::warn!("orb hide failed: {e}");
            }
        }
    }
}

/// Starts the ticker thread. A ticker already running is told to stop first.
fn start_ticker(app: &AppHandle) {
    let signal = Arc::new(TickerSignal::new());
    if let Some(cell) = app.try_state::<OrbRuntimeCell>() {
        let mut rt = cell.lock();
        if let Some(old) = rt.ticker.replace(signal.clone()) {
            old.request_stop();
        }
    }
    let ticker_app = app.clone();
    if let Err(e) = thread::Builder::new()
        .name("orb-ticker".into())
        .spawn(move || run_ticker(ticker_app, signal))
    {
        log::warn!("orb ticker not started: {e}");
    }
}

/// The ticker loop: reads the pointer, steps the drivers, carries out their calls, and re-fits
/// the orb when the monitor list changes. It reads only the shown flag, never a window or panel
/// call to learn whether the orb is visible. Under reduced motion with no gesture it reads no
/// pointer or window state and only waits for a wake. Exits when stopped, when the orb is hidden
/// or when the runtime is gone.
fn run_ticker(app: AppHandle, signal: Arc<TickerSignal>) {
    let Some(cell) = app.try_state::<OrbRuntimeCell>() else {
        return;
    };
    let mut last_monitors: Option<Vec<MonitorInfo>> = None;
    let mut last_check_ms = now_ms();
    loop {
        if signal.is_stopped() || !cell.shown() {
            break;
        }
        let (active, reduced) = {
            let rt = cell.lock();
            (rt.gesture.is_active(), rt.reduced_motion)
        };
        if active || !reduced {
            let frame = read_frame(&app);
            let now = now_ms();
            let down = active && left_button_down(&app);
            let mut calls = Recorder::new(frame.orb, frame.scale);
            let stamped = {
                let mut rt = cell.lock();
                rt.gesture
                    .step(now, &Down(down), &Snapshot(frame.cursor), &mut calls);
                rt.gaze.step(now, &Snapshot(frame.cursor), &mut calls);
                stamp_saves(&mut rt, calls.calls)
            };
            execute(&app, stamped);
            if now.saturating_sub(last_check_ms) >= MONITOR_CHECK_MS {
                last_check_ms = now;
                let monitors = monitors_for(&app);
                let changed = last_monitors.as_ref().is_some_and(|prev| *prev != monitors);
                last_monitors = Some(monitors);
                if changed {
                    if gesture_active(&app) {
                        cancel_gesture(&app);
                    }
                    fit_orb(&app);
                }
            }
        }
        resume_deferred_resize(&app, &cell);
        signal.wait(Duration::from_millis(next_wait_ms(&cell)));
    }
}

/// How long the ticker may sleep before its next pass, from the runtime state.
fn next_wait_ms(cell: &OrbRuntimeCell) -> u64 {
    let now = now_ms();
    let rt = cell.lock();
    tick_interval_ms(
        rt.gesture.is_active(),
        rt.gaze.next_interval_ms(now),
        rt.reduced_motion,
        true,
    )
}

/// Carries out the calls the drivers recorded, in order.
fn execute(app: &AppHandle, calls: Vec<Call>) {
    for call in calls {
        match call {
            Call::Move(x, y) => move_orb(app, x, y),
            Call::Gesture(id, result) => {
                let _ = app.emit_to(
                    ORB_WINDOW_LABEL,
                    GESTURE_EVENT,
                    GesturePayload {
                        id,
                        result: result_name(result),
                    },
                );
            }
            Call::Click => toggle_chat(app),
            Call::Save(rect, rev) => save_position(app, rect, rev),
            Call::Gaze(gaze) => {
                let _ = app.emit_to(
                    ORB_WINDOW_LABEL,
                    GAZE_EVENT,
                    GazePayload {
                        x: gaze.x,
                        y: gaze.y,
                    },
                );
            }
        }
    }
}

/// The page-side name of a gesture result.
fn result_name(result: GestureResult) -> &'static str {
    match result {
        GestureResult::Click => "click",
        GestureResult::Drag => "drag",
        GestureResult::Cancel => "cancel",
    }
}

/// Moves the orb window to physical `(x, y)`.
fn move_orb(app: &AppHandle, x: f64, y: f64) {
    if let Some(window) = app.get_webview_window(ORB_WINDOW_LABEL) {
        if let Err(e) =
            window.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32))
        {
            log::warn!("orb move failed: {e}");
        }
    }
}

/// Toggles the conversation surface through the single dispatcher.
fn toggle_chat(app: &AppHandle) {
    if let Err(e) = chat_surface::toggle(app) {
        log::warn!("orb conversation toggle failed: {e}");
    }
}

/// Saves the orb position computed against geometry revision `rev`: the monitor and its
/// work-area fraction. A save superseded by a newer resize or drag is dropped.
fn save_position(app: &AppHandle, final_orb: Rect, rev: u64) {
    if !geometry_accepts(app, rev) {
        log::debug!("orb position save dropped: superseded by a newer change");
        return;
    }
    let monitors = monitors_for(app);
    match position_from_rect(final_orb, &monitors) {
        Some(position) => {
            write_settings_gated(app, |settings| settings.orb_position = Some(position))
        }
        None => log::warn!("orb position not saved: no monitor found"),
    }
}

/// Starts a gesture for a press with id `id`, from the page. Wakes the ticker so the first
/// gesture tick does not wait out its idle sleep.
pub fn pointer_down(app: &AppHandle, id: u64) {
    let frame = read_frame(app);
    let Some(cell) = app.try_state::<OrbRuntimeCell>() else {
        return;
    };
    let mut calls = Recorder::new(frame.orb, frame.scale);
    let (ticker, stamped) = {
        let mut rt = cell.lock();
        rt.gesture
            .pointer_down(id, now_ms(), &Snapshot(frame.cursor), &mut calls);
        (rt.ticker.clone(), stamp_saves(&mut rt, calls.calls))
    };
    execute(app, stamped);
    if let Some(signal) = ticker {
        signal.wake();
    }
}

/// Keyboard activation: toggles the conversation surface.
pub fn activate(app: &AppHandle) {
    toggle_chat(app);
}

/// Records the page's reduced-motion preference. Wakes the ticker so a change takes effect at once.
pub fn set_reduced_motion(app: &AppHandle, reduced: bool) {
    if let Some(cell) = app.try_state::<OrbRuntimeCell>() {
        let ticker = {
            let mut rt = cell.lock();
            rt.reduced_motion = reduced;
            rt.gaze.set_reduced_motion(reduced);
            rt.ticker.clone()
        };
        if let Some(signal) = ticker {
            signal.wake();
        }
    }
}

/// Flips the orb between shown and hidden, using the effective setting (`ORB_VISIBLE_DEFAULT`
/// when unset). Used by the tray item.
pub fn toggle_visibility(app: &AppHandle) {
    let enabled = settings::get_settings(app)
        .orb_visible
        .unwrap_or(ORB_VISIBLE_DEFAULT);
    write_settings_gated(app, |settings| settings.orb_visible = Some(!enabled));
    if enabled {
        crate::bubble_native::user_hid_orb(app);
    } else {
        crate::bubble_native::user_showed_orb(app);
    }
    apply_visibility(app);
}

/// Shows the orb's native context menu at the pointer. Runs on the main thread.
pub fn show_menu(app: &AppHandle) {
    let job_app = app.clone();
    if let Err(e) = app.run_on_main_thread(move || popup_menu(&job_app)) {
        log::warn!("orb menu could not be scheduled: {e}");
    }
}

/// Main-thread body of `show_menu`.
fn popup_menu(app: &AppHandle) {
    let Some(window) = app.get_webview_window(ORB_WINDOW_LABEL) else {
        return;
    };
    let settings = settings::get_settings(app);
    let chat_open = app
        .get_webview_window(CHAT_WINDOW_LABEL)
        .and_then(|chat| chat.is_visible().ok())
        .unwrap_or(false);
    let strings = get_tray_translations(Some(settings.app_language.clone()));
    let menu = match build_menu(app, &strings, effective_logical(&settings), chat_open) {
        Ok(menu) => menu,
        Err(e) => {
            log::warn!("orb menu build failed: {e}");
            return;
        }
    };
    if let Err(e) = window.popup_menu(&menu) {
        log::warn!("orb menu popup failed: {e}");
    }
}

/// Builds the native menu from the pure menu description and the translated labels.
fn build_menu(
    app: &AppHandle,
    strings: &TrayStrings,
    logical_px: f64,
    chat_open: bool,
) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    for entry in orb_menu_items_for_px(logical_px, chat_open) {
        match entry {
            MenuEntry::Separator => menu.append(&PredefinedMenuItem::separator(app)?)?,
            MenuEntry::Item { id, checked: None } => menu.append(&MenuItem::with_id(
                app,
                id,
                menu_label(strings, id),
                true,
                None::<&str>,
            )?)?,
            MenuEntry::Item {
                id,
                checked: Some(on),
            } => menu.append(&CheckMenuItem::with_id(
                app,
                id,
                menu_label(strings, id),
                true,
                on,
                None::<&str>,
            )?)?,
        }
    }
    Ok(menu)
}

/// The translated label for a menu item id.
fn menu_label(strings: &TrayStrings, id: &str) -> String {
    match id {
        "orb_toggle_chat" => strings.chat.clone(),
        "orb_size_small" => strings.orb_size_small.clone(),
        "orb_size_medium" => strings.orb_size_medium.clone(),
        "orb_size_large" => strings.orb_size_large.clone(),
        "orb_hide" => strings.orb_hide.clone(),
        "orb_settings" => strings.settings.clone(),
        other => other.to_string(),
    }
}

/// Carries out a chosen menu item.
fn handle_menu(app: &AppHandle, id: &str) {
    match id {
        "orb_toggle_chat" => toggle_chat(app),
        "orb_size_small" => set_orb_size(app, Some(72)),
        "orb_size_medium" => set_orb_size(app, Some(96)),
        "orb_size_large" => set_orb_size(app, Some(128)),
        "orb_hide" => {
            crate::bubble_native::user_hid_orb(app);
            write_settings_gated(app, |settings| settings.orb_visible = Some(false));
            apply_visibility(app);
        }
        "orb_settings" => show_main_window(app),
        _ => {}
    }
}

/// The one writer of the orb size. Clamps a free size to 48..=192 (`None` returns to the preset),
/// stores it with the nearest preset as the legacy `orb_size`, then resizes the orb on the main
/// thread. A resize that arrives during a gesture is deferred until the gesture ends.
pub fn set_orb_size(app: &AppHandle, px: Option<u16>) {
    let clamped = px.map(|value| clamp_orb_px(value));
    write_settings_gated(app, |settings| {
        settings.orb_size_px = clamped;
        if let Some(value) = clamped {
            settings.orb_size = nearest_preset(value);
        }
    });
    let job_app = app.clone();
    if let Err(e) = app.run_on_main_thread(move || resize_on_main(&job_app, clamped)) {
        log::warn!("orb resize could not be scheduled: {e}");
    }
}

/// Main-thread entry of a size change: applies it now, or keeps it pending while a gesture is
/// active. The latest value wins.
fn resize_on_main(app: &AppHandle, px: Option<u16>) {
    ensure_runtime(app);
    let next = match app.try_state::<OrbRuntimeCell>() {
        Some(cell) => {
            let mut rt = cell.lock();
            rt.pending_resize.defer(px);
            let active = rt.gesture.is_active();
            rt.pending_resize.take(active)
        }
        None => Some(px),
    };
    if let Some(value) = next {
        apply_resize(app, value);
    }
}

/// Resizes the orb about its centre to the effective size and moves it into its work area, then
/// saves its position once for the new geometry revision. Runs on the main thread.
fn apply_resize(app: &AppHandle, px: Option<u16>) {
    let Some(window) = app.get_webview_window(ORB_WINDOW_LABEL) else {
        return;
    };
    let Some(current) = window_rect(&window) else {
        return;
    };
    let settings = settings::get_settings(app);
    let logical = effective_orb_logical_px(settings.orb_size, px);
    let monitors = monitors_for(app);
    let Some(monitor) = monitor_for(current.center(), &monitors) else {
        return;
    };
    let side = orb_px_logical(logical, monitor.scale);
    let target = resize_keep_center(current, side, monitor.work);
    let rev = bump_geometry(app);
    apply_geometry(&window, target);
    save_position(app, target, rev);
}

/// Applies a deferred size change once no gesture is active. Called by the ticker.
fn resume_deferred_resize(app: &AppHandle, cell: &OrbRuntimeCell) {
    let next = {
        let mut rt = cell.lock();
        let active = rt.gesture.is_active();
        rt.pending_resize.take(active)
    };
    if let Some(px) = next {
        let job_app = app.clone();
        if let Err(e) = app.run_on_main_thread(move || apply_resize(&job_app, px)) {
            log::warn!("orb resize could not be scheduled: {e}");
        }
    }
}

/// Gives each drag-end save a new geometry revision. Runs under the runtime lock, in the same
/// critical section as the driver call that produced the save.
fn stamp_saves(rt: &mut OrbRuntime, calls: Vec<Call>) -> Vec<Call> {
    calls
        .into_iter()
        .map(|call| match call {
            Call::Save(rect, _) => Call::Save(rect, rt.geometry.bump()),
            other => other,
        })
        .collect()
}

/// Starts a new geometry revision and returns it.
fn bump_geometry(app: &AppHandle) -> u64 {
    match app.try_state::<OrbRuntimeCell>() {
        Some(cell) => {
            let mut rt = cell.lock();
            rt.geometry.bump()
        }
        None => 0,
    }
}

/// Whether a position computed against revision `rev` is still current.
fn geometry_accepts(app: &AppHandle, rev: u64) -> bool {
    match app.try_state::<OrbRuntimeCell>() {
        Some(cell) => {
            let rt = cell.lock();
            rt.geometry.accept(rev)
        }
        None => true,
    }
}

/// Left mouse button state on macOS: bit 0 of `NSEvent.pressedMouseButtons`. Readable off the
/// main thread.
#[cfg(target_os = "macos")]
fn left_button_down(_app: &AppHandle) -> bool {
    let mask: usize = unsafe { msg_send![class!(NSEvent), pressedMouseButtons] };
    mask & 1 != 0
}

/// Left mouse button state on Windows: the high bit of `GetAsyncKeyState(VK_LBUTTON)`.
#[cfg(target_os = "windows")]
fn left_button_down(_app: &AppHandle) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    unsafe { GetAsyncKeyState(i32::from(VK_LBUTTON.0)) < 0 }
}

/// Left mouse button state on Linux: asked on the main thread, with a timeout. A failure reads as
/// not pressed.
#[cfg(target_os = "linux")]
fn left_button_down(app: &AppHandle) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    let scheduled = app.run_on_main_thread(move || {
        let _ = tx.send(linux_button_down_on_main());
    });
    if scheduled.is_err() {
        return false;
    }
    rx.recv_timeout(Duration::from_millis(100)).unwrap_or(false)
}

/// The GDK pointer state of the default seat. Runs on the main thread.
#[cfg(target_os = "linux")]
fn linux_button_down_on_main() -> bool {
    use gtk::prelude::*;
    let Some(display) = gtk::gdk::Display::default() else {
        return false;
    };
    let Some(seat) = display.default_seat() else {
        return false;
    };
    let Some(pointer) = seat.pointer() else {
        return false;
    };
    let Some(root) = gtk::gdk::Screen::default().and_then(|screen| screen.root_window()) else {
        return false;
    };
    let (_, _, _, mask) = root.device_position(&pointer);
    mask.contains(gtk::gdk::ModifierType::BUTTON1_MASK)
}
