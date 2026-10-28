//! Native side of the wake-appear orb and its bubbles (DESIGN.md Rev 6 R6-3 and R6-4, A7-2,
//! A7-4, A8-1, A9-2 and Revision 10).
//!
//! This file owns the one shell core, the bubble window, the session timer and the main-thread
//! executor that makes the real windows match the core. Every hook takes the core lock briefly
//! and never holds it across a window call or an emit. Window calls run on the main thread. The
//! executor is level-triggered: it reads the desired state under the lock and applies only the
//! difference, so a stale request cannot act. Logs carry names, counts and ids only, never
//! message text or positions.

use crate::bubble_hub::{BubbleRole, HubEntry, Snapshot};
use crate::bubble_shell_core::{
    BubbleShellCore, Effects, SessionSurface, ShellInputs, READY_TIMEOUT_MS,
};
use crate::chat_presentation::{floating_supported, ORB_VISIBLE_DEFAULT};
use crate::chat_window::{self, is_app_navigation};
use crate::managers::chat_log::{ChatLog, GateState};
use crate::orb_geometry::{
    monitor_for, place_bubbles, BubblePlacement, Rect, Size, BUBBLE_H_LOGICAL, BUBBLE_W_LOGICAL,
    MIN_BUBBLE_H_LOGICAL, MIN_BUBBLE_W_LOGICAL,
};
use crate::orb_native;
use crate::orb_window::ORB_WINDOW_LABEL;
use crate::settings;
use crate::window_commands::BUBBLES_WINDOW_LABEL;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tauri::webview::NewWindowResponse;
#[cfg(not(target_os = "macos"))]
use tauri::WebviewWindowBuilder;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
};

#[cfg(target_os = "macos")]
use tauri_nspanel::{
    tauri_panel, CollectionBehavior, ManagerExt, PanelBuilder, PanelLevel, StyleMask,
};

/// Bundled entry page of the bubble window (Vite input `bubbles`).
const BUBBLES_ENTRY_URL: &str = "src/bubbles/index.html";

/// Event that carries a projection revision to the bubble page.
const BUBBLE_EVENT: &str = "bubbles-state";

/// Gap between the orb and the bubble stack, in logical pixels.
const BUBBLE_GAP_LOGICAL: f64 = 8.0;

/// The session timer wakes this often (a fraction of the readiness timeout).
const TICK_MS: u64 = READY_TIMEOUT_MS / 8;

#[cfg(target_os = "macos")]
tauri_panel! {
    panel!(BubblesPanel {
        config: {
            can_become_key_window: false,
            is_floating_panel: true
        }
    })
}

/// Wake signal of the session timer: a wait that a stop or a new session can end early.
struct Wake {
    lock: Mutex<()>,
    cv: Condvar,
}

/// The managed shell state. The core is the single owner of the session and bubble state.
pub struct BubbleShell {
    core: Arc<Mutex<BubbleShellCore>>,
    /// Stop flag of the running timer thread, `None` while no timer runs.
    timer: Mutex<Option<Arc<AtomicBool>>>,
    wake: Wake,
    /// One pending main-thread executor at most; cleared when it starts.
    scheduled: AtomicBool,
    /// Whether the bubble stack had a placement beside the orb at the last check.
    placement_ok: AtomicBool,
    /// Whether the classic conversation window is visible.
    classic_visible: AtomicBool,
    /// Whether the bubble window is shown (set on the main thread).
    bubbles_shown: AtomicBool,
}

impl BubbleShell {
    fn new(recording: bool) -> BubbleShell {
        BubbleShell {
            core: Arc::new(Mutex::new(BubbleShellCore::new(recording))),
            timer: Mutex::new(None),
            wake: Wake {
                lock: Mutex::new(()),
                cv: Condvar::new(),
            },
            scheduled: AtomicBool::new(false),
            placement_ok: AtomicBool::new(true),
            classic_visible: AtomicBool::new(false),
            bubbles_shown: AtomicBool::new(false),
        }
    }
}

/// Locks a plain mutex. The guarded data is plain state, so a poisoned lock is recovered.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_core(shell: &BubbleShell) -> MutexGuard<'_, BubbleShellCore> {
    lock(&shell.core)
}

/// Monotonic milliseconds since the first call in this process.
fn now_ms() -> u64 {
    static CLOCK: OnceLock<Instant> = OnceLock::new();
    let start = CLOCK.get_or_init(Instant::now);
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// The facts read from the settings and the windows before the core lock is taken.
#[derive(Debug, Clone, Copy)]
struct Facts {
    persisted: bool,
    supported: bool,
    classic: bool,
    placement: bool,
}

fn facts(app: &AppHandle, shell: &BubbleShell) -> Facts {
    let settings = settings::get_settings(app);
    // The copy the last main-thread apply recorded: the GDK display is never read off the main
    // thread, and the converse hooks run off it.
    let env = orb_native::cached_env();
    Facts {
        persisted: settings.orb_visible.unwrap_or(ORB_VISIBLE_DEFAULT),
        supported: floating_supported(&env).is_ok(),
        classic: shell.classic_visible.load(Ordering::SeqCst),
        placement: shell.placement_ok.load(Ordering::SeqCst),
    }
}

/// The admission tuple of the chat log, read now. Called only while the core lock is held, so
/// the tuple and the decision are taken at one moment (A9-1).
fn gate_state(app: &AppHandle) -> GateState {
    match app.try_state::<Arc<ChatLog>>() {
        Some(chat) => chat.gate().state(),
        None => GateState {
            session_token: 0,
            rec_epoch: 0,
            recording: false,
        },
    }
}

/// The inputs for one core step. Reads the gate and the clock, so call it under the lock.
fn inputs(app: &AppHandle, facts: Facts) -> ShellInputs {
    ShellInputs {
        now: now_ms(),
        gate: gate_state(app),
        orb_persisted_visible: facts.persisted,
        supported: facts.supported,
        classic_visible: facts.classic,
        placement_ok: facts.placement,
    }
}

/// Runs one core step under the lock, then applies its effects with the lock released.
fn update(app: &AppHandle, step: impl FnOnce(&mut BubbleShellCore, &ShellInputs) -> Effects) {
    let Some(shell) = app.try_state::<BubbleShell>() else {
        return;
    };
    let facts = facts(app, &shell);
    let effects = {
        let mut core = lock_core(&shell);
        let inp = inputs(app, facts);
        step(&mut core, &inp)
    };
    apply_effects(app, &shell, effects);
}

/// Sends the snapshot, opens the classic window when asked, and schedules the executor when
/// anything changed. Never called with the core lock held.
fn apply_effects(app: &AppHandle, shell: &BubbleShell, effects: Effects) {
    let changed = effects.snapshot.is_some()
        || effects.orb.is_some()
        || effects.bubbles_visible.is_some()
        || effects.open_classic;
    if let Some(snapshot) = effects.snapshot.as_ref() {
        emit_snapshot(app, snapshot);
    }
    if effects.open_classic {
        log::info!("bubble window unavailable: the classic conversation window opens");
        chat_window::open_chat_window_on_session_start(app);
        // The fallback destroys the bubble window and clears the session state (M1 of G4b.fix).
        destroy_bubbles(app);
    }
    if changed {
        schedule(app, shell);
    }
}

/// Wire form of one bubble on `bubbles-state`.
#[derive(Clone, Serialize)]
struct BubbleItemPayload {
    id: u64,
    role: &'static str,
    text: String,
    age_ms: u64,
    ttl_ms: u64,
}

/// Wire form of a projection revision on `bubbles-state`.
#[derive(Clone, Serialize)]
struct BubblesPayload {
    instance: u64,
    rev: u64,
    items: Vec<BubbleItemPayload>,
}

/// Sends a snapshot to the bubble window only.
fn emit_snapshot(app: &AppHandle, snapshot: &Snapshot) {
    let items = snapshot
        .items
        .iter()
        .map(|item| BubbleItemPayload {
            id: item.id,
            role: match item.role {
                BubbleRole::User => "user",
                BubbleRole::Assistant => "assistant",
            },
            text: item.text.clone(),
            age_ms: item.age_ms,
            ttl_ms: item.ttl_ms,
        })
        .collect();
    let payload = BubblesPayload {
        instance: snapshot.instance,
        rev: snapshot.rev,
        items,
    };
    if app
        .emit_to(BUBBLES_WINDOW_LABEL, BUBBLE_EVENT, payload)
        .is_err()
    {
        log::debug!("bubble snapshot not delivered");
    }
}

/// Queues one executor run on the main thread. At most one run is pending: a request made
/// while one is queued is absorbed, and the run reads the latest desired state when it starts.
fn schedule(app: &AppHandle, shell: &BubbleShell) {
    if shell.scheduled.swap(true, Ordering::SeqCst) {
        return;
    }
    let job_app = app.clone();
    let queued = app.run_on_main_thread(move || {
        if let Some(shell) = job_app.try_state::<BubbleShell>() {
            shell.scheduled.store(false, Ordering::SeqCst);
        }
        execute_on_main(&job_app);
    });
    if let Err(e) = queued {
        shell.scheduled.store(false, Ordering::SeqCst);
        log::warn!("bubble update could not be scheduled: {e}");
    }
}

/// Main-thread executor. Makes the orb, the placement and the bubble window match the core's
/// desired state. Holds no lock while a window call runs.
fn execute_on_main(app: &AppHandle) {
    let Some(shell) = app.try_state::<BubbleShell>() else {
        return;
    };
    // 1. The orb: the total desired visibility decides show or hide (A9-2). Only the session's
    //    own share is stored as transient, never the saved setting (MAJOR 1). The window is shown
    //    or hidden without re-targeting it, so a drag is never fought (MAJOR 2).
    let (desired, share) = {
        let facts = facts(app, &shell);
        let core = lock_core(&shell);
        let inp = inputs(app, facts);
        (core.desired_orb(&inp), core.orb_share())
    };
    orb_native::set_orb_transient(share);
    orb_native::show_hide_only(app, desired);

    // 2. The placement beside the orb. A change re-evaluates the projection.
    let placement = placement_for_orb(app);
    let placement_ok = placement.is_some();
    if shell.placement_ok.swap(placement_ok, Ordering::SeqCst) != placement_ok {
        update(app, |core, inp| core.refresh(inp));
    }

    // 3. The bubble window: shown only while the projection has bubbles and a placement exists.
    let wanted = lock_core(&shell).bubbles_wanted(now_ms());
    match placement {
        Some(placement) if wanted => show_bubbles(app, &shell, placement),
        _ => hide_bubbles(app, &shell),
    }
}

/// Where the bubble stack goes beside the orb: physical pixels, scaled from the logical sizes
/// of the monitor under the orb.
fn placement_for_orb(app: &AppHandle) -> Option<BubblePlacement> {
    let window = app.get_webview_window(ORB_WINDOW_LABEL)?;
    let orb = orb_native::window_rect(&window)?;
    let monitors = orb_native::monitors_for(app);
    let (cx, cy) = orb.center();
    let monitor = monitor_for((cx, cy), &monitors)?;
    let scale = monitor.scale;
    place_bubbles(
        orb,
        monitor.work,
        Size {
            w: BUBBLE_W_LOGICAL * scale,
            h: BUBBLE_H_LOGICAL * scale,
        },
        BUBBLE_GAP_LOGICAL * scale,
        Size {
            w: MIN_BUBBLE_W_LOGICAL * scale,
            h: MIN_BUBBLE_H_LOGICAL * scale,
        },
    )
}

/// Shows the bubble window at the placement, building it first when needed. Main thread.
fn show_bubbles(app: &AppHandle, shell: &BubbleShell, placement: BubblePlacement) {
    if app.get_webview_window(BUBBLES_WINDOW_LABEL).is_none() && !ensure_bubble_window(app, shell) {
        return;
    }
    let Some(window) = app.get_webview_window(BUBBLES_WINDOW_LABEL) else {
        return;
    };
    apply_rect(&window, placement.rect);
    show_bubble_window(app);
    shell.bubbles_shown.store(true, Ordering::SeqCst);
}

/// Hides the bubble window; the window itself is kept. Main thread.
fn hide_bubbles(app: &AppHandle, shell: &BubbleShell) {
    if shell.bubbles_shown.swap(false, Ordering::SeqCst) {
        hide_bubble_window(app);
    }
}

/// Builds the bubble window when it does not exist yet. Takes a new window instance from the
/// core, so the page can be told apart from an older one. Returns whether the window exists.
fn ensure_bubble_window(app: &AppHandle, shell: &BubbleShell) -> bool {
    if app.get_webview_window(BUBBLES_WINDOW_LABEL).is_some() {
        return true;
    }
    let instance = lock_core(shell).window_created(now_ms());
    match build_bubbles(app, instance) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("bubble window build failed: {e}");
            false
        }
    }
}

/// Creates the bubble window hidden, with its page for `instance`. The window is transparent,
/// undecorated, never focusable and click-through.
#[cfg(target_os = "macos")]
fn build_bubbles(app: &AppHandle, instance: u64) -> tauri::Result<()> {
    let url = format!("{BUBBLES_ENTRY_URL}?instance={instance}");
    let panel = PanelBuilder::<_, BubblesPanel>::new(app, BUBBLES_WINDOW_LABEL)
        .url(WebviewUrl::App(url.into()))
        .title("Vox bubbles")
        .level(PanelLevel::Status)
        .size(tauri::Size::Logical(tauri::LogicalSize {
            width: BUBBLE_W_LOGICAL,
            height: BUBBLE_H_LOGICAL,
        }))
        .has_shadow(false)
        .transparent(true)
        .no_activate(true)
        .corner_radius(0.0)
        .style_mask(StyleMask::empty().borderless().nonactivating_panel())
        .with_window(|w| {
            w.decorations(false)
                .transparent(true)
                .focusable(false)
                .always_on_top(true)
                .skip_taskbar(true)
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
    make_click_through(app);
    Ok(())
}

/// Creates the bubble window hidden on Windows and Linux, with the same flags.
#[cfg(not(target_os = "macos"))]
fn build_bubbles(app: &AppHandle, instance: u64) -> tauri::Result<()> {
    let url = format!("{BUBBLES_ENTRY_URL}?instance={instance}");
    let builder = WebviewWindowBuilder::new(app, BUBBLES_WINDOW_LABEL, WebviewUrl::App(url.into()))
        .title("Vox bubbles")
        .inner_size(BUBBLE_W_LOGICAL, BUBBLE_H_LOGICAL)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .focusable(false)
        .visible(false)
        .focused(false)
        .on_navigation(is_app_navigation)
        .on_new_window(|_url, _features| NewWindowResponse::Deny);
    #[cfg(target_os = "linux")]
    let builder = builder.visible_on_all_workspaces(true);
    builder.build()?;
    make_click_through(app);
    Ok(())
}

/// Makes the bubble window ignore the pointer, so it can never block or take a click.
fn make_click_through(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(BUBBLES_WINDOW_LABEL) {
        if let Err(e) = window.set_ignore_cursor_events(true) {
            log::warn!("bubble window click-through not set: {e}");
        }
    }
}

/// Places the bubble window at `rect` in physical pixels: size first, then position.
fn apply_rect(window: &WebviewWindow, rect: Rect) {
    if let Err(e) = window.set_size(PhysicalSize::new(
        rect.w.round() as u32,
        rect.h.round() as u32,
    )) {
        log::warn!("bubble resize failed: {e}");
    }
    if let Err(e) = window.set_position(PhysicalPosition::new(
        rect.x.round() as i32,
        rect.y.round() as i32,
    )) {
        log::warn!("bubble move failed: {e}");
    }
}

/// Shows the bubble window without taking focus: a non-key panel on macOS, a plain show elsewhere.
#[cfg(target_os = "macos")]
fn show_bubble_window(app: &AppHandle) {
    match app.get_webview_panel(BUBBLES_WINDOW_LABEL) {
        Ok(panel) => panel.order_front_regardless(),
        Err(e) => log::warn!("bubble show failed: {e:?}"),
    }
}

#[cfg(not(target_os = "macos"))]
fn show_bubble_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(BUBBLES_WINDOW_LABEL) {
        if let Err(e) = window.show() {
            log::warn!("bubble show failed: {e}");
        }
        #[cfg(target_os = "windows")]
        crate::overlay::force_overlay_topmost(&window);
    }
}

/// Hides the bubble window. The window stays and can be shown again.
#[cfg(target_os = "macos")]
fn hide_bubble_window(app: &AppHandle) {
    if let Ok(panel) = app.get_webview_panel(BUBBLES_WINDOW_LABEL) {
        panel.hide();
    }
}

#[cfg(not(target_os = "macos"))]
fn hide_bubble_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(BUBBLES_WINDOW_LABEL) {
        if let Err(e) = window.hide() {
            log::warn!("bubble hide failed: {e}");
        }
    }
}

/// Starts the session timer when none runs. The thread exits by itself when the core is Idle.
fn ensure_timer(app: &AppHandle, shell: &BubbleShell) {
    let mut slot = lock(&shell.timer);
    if slot.is_some() {
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let thread_app = app.clone();
    let thread_stop = Arc::clone(&stop);
    match thread::Builder::new()
        .name("bubble-timer".into())
        .spawn(move || run_timer(thread_app, thread_stop))
    {
        Ok(_) => *slot = Some(stop),
        Err(e) => log::warn!("bubble timer could not start: {e}"),
    }
}

/// Stops the running timer thread, if any, and wakes it.
fn stop_timer(shell: &BubbleShell) {
    if let Some(stop) = lock(&shell.timer).take() {
        stop.store(true, Ordering::SeqCst);
    }
    shell.wake.cv.notify_all();
}

/// Waits one tick, or until woken.
fn wait_tick(wake: &Wake) {
    let guard = lock(&wake.lock);
    let _ = wake.cv.wait_timeout(guard, Duration::from_millis(TICK_MS));
}

/// Timer thread body. Ticks the core (session deadline and readiness timeout) and applies the
/// result. Exits when stopped or when the core is Idle, releasing its slot only if it still holds
/// it.
fn run_timer(app: AppHandle, stop: Arc<AtomicBool>) {
    let Some(shell) = app.try_state::<BubbleShell>() else {
        return;
    };
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        wait_tick(&shell.wake);
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let facts = facts(&app, &shell);
        let outcome = {
            let mut slot = lock(&shell.timer);
            let mut core = lock_core(&shell);
            if core.is_idle() {
                if slot.as_ref().is_some_and(|held| Arc::ptr_eq(held, &stop)) {
                    *slot = None;
                }
                None
            } else {
                let inp = inputs(&app, facts);
                Some(core.tick(&inp))
            }
        };
        match outcome {
            Some(effects) => apply_effects(&app, &shell, effects),
            None => break,
        }
    }
}

/// Initialises the shell state at startup from the saved recording setting.
pub fn init(app: &AppHandle) {
    let recording = settings::get_settings(app).converse_chat_log;
    let _ = app.manage(BubbleShell::new(recording));
}

/// A conversation session started. Runs on the converse thread: the core's synchronous part (the
/// session machine, the hub arming and the orb effects) happens here under the shell lock, so an
/// end or a line that follows is never lost (MAJOR 3). The windows are built on the main thread
/// afterwards and reported to the core. The classic surface opens its window on the main thread.
pub fn session_begin(app: &AppHandle, sid: u64, surface: SessionSurface) {
    let Some(shell) = app.try_state::<BubbleShell>() else {
        open_classic_on_main(app);
        return;
    };
    let facts = facts(app, &shell);
    let effects = {
        let mut core = lock_core(&shell);
        let inp = inputs(app, facts);
        core.session_begin(sid, surface, &inp)
    };
    apply_effects(app, &shell, effects);
    if surface == SessionSurface::OrbBubbles {
        schedule_windows(app, sid);
    } else {
        open_classic_on_main(app);
    }
    ensure_timer(app, &shell);
}

/// Opens the classic conversation window on the main thread.
fn open_classic_on_main(app: &AppHandle) {
    let job_app = app.clone();
    if app
        .run_on_main_thread(move || chat_window::open_chat_window_on_session_start(&job_app))
        .is_err()
    {
        log::warn!("classic conversation window could not be scheduled");
    }
}

/// Queues the main-thread build of the orb and bubble windows for session `sid`. When the job
/// cannot be queued, the core is told the windows failed, so the session falls back at once.
fn schedule_windows(app: &AppHandle, sid: u64) {
    let job_app = app.clone();
    let queued = app.run_on_main_thread(move || build_session_windows(&job_app, sid));
    if let Err(e) = queued {
        log::warn!("bubble windows could not be scheduled: {e}");
        update(app, move |core, inp| core.windows_ready(sid, false, inp));
    }
}

/// Main thread. Builds the orb and the bubble window for session `sid` and reports the result.
fn build_session_windows(app: &AppHandle, sid: u64) {
    let Some(shell) = app.try_state::<BubbleShell>() else {
        return;
    };
    let orb_ok = orb_native::ensure_orb_window(app);
    let windows_ok = orb_ok && ensure_bubble_window(app, &shell);
    let facts = facts(app, &shell);
    let effects = {
        let mut core = lock_core(&shell);
        let inp = inputs(app, facts);
        core.windows_ready(sid, windows_ok, &inp)
    };
    apply_effects(app, &shell, effects);
}

/// The conversation session ended (terminal state with a reason). The session is identified by
/// the chat log's current session token.
pub fn session_ended(app: &AppHandle) {
    update(app, |core, inp| {
        core.session_ended(inp.gate.session_token, inp)
    });
}

/// A transcript line reached the hub. Admitted only against the chat log's current tuple.
pub fn chat_entry(app: &AppHandle, entry: HubEntry) {
    update(app, move |core, inp| core.chat_entry(entry, inp));
}

/// The transcript was reset for a new session.
pub fn chat_reset(app: &AppHandle) {
    update(app, |core, inp| core.chat_reset(inp));
}

/// The user cleared the transcript.
pub fn chat_clear(app: &AppHandle) {
    update(app, |core, inp| core.chat_clear(inp));
}

/// The recording setting changed. Call after the chat log's gate has changed (A9-1).
pub fn recording_changed(app: &AppHandle, on: bool) {
    update(app, move |core, inp| core.recording_changed(on, inp));
}

/// The orb became shown or hidden in the real window.
pub fn orb_visibility_changed(app: &AppHandle, shown: bool) {
    update(app, move |core, inp| {
        core.orb_visibility_changed(shown, inp)
    });
}

/// The orb moved, was dragged or was resized: the bubble stack follows while it is shown.
pub fn orb_geometry_changed(app: &AppHandle) {
    let Some(shell) = app.try_state::<BubbleShell>() else {
        return;
    };
    if shell.bubbles_shown.load(Ordering::SeqCst) {
        schedule(app, &shell);
    }
}

/// The classic conversation window became shown or hidden. Bubbles yield to it.
pub fn classic_visibility_changed(app: &AppHandle, visible: bool) {
    if let Some(shell) = app.try_state::<BubbleShell>() {
        shell.classic_visible.store(visible, Ordering::SeqCst);
    }
    update(app, move |core, inp| {
        core.classic_visibility_changed(visible, inp)
    });
}

/// The user hid the orb from the tray or the menu.
pub fn user_hid_orb(app: &AppHandle) {
    update(app, |core, inp| core.user_hid_orb(inp));
}

/// The user showed the orb from the tray or the menu.
pub fn user_showed_orb(app: &AppHandle) {
    update(app, |core, inp| core.user_showed_orb(inp));
}

/// The window instance the bubble page must present, if a bubble window exists.
pub fn current_instance(app: &AppHandle) -> Option<u64> {
    let shell = app.try_state::<BubbleShell>()?;
    let instance = lock_core(&shell).instance();
    (instance != 0).then_some(instance)
}

/// The bubble page of `instance` reported ready. Sends the current snapshot to it and lets the
/// executor show the window when there is something to show.
pub fn page_ready(app: &AppHandle, instance: u64) {
    let Some(shell) = app.try_state::<BubbleShell>() else {
        return;
    };
    let facts = facts(app, &shell);
    let snapshot = {
        let mut core = lock_core(&shell);
        let inp = inputs(app, facts);
        core.page_ready(instance, &inp)
    };
    if let Some(snapshot) = snapshot.as_ref() {
        emit_snapshot(app, snapshot);
    }
    schedule(app, &shell);
}

/// Destroys the bubble window, resets the session state and stops the timer. Called on app exit
/// and when the presentation switches to the classic window.
pub fn destroy_bubbles(app: &AppHandle) {
    let Some(shell) = app.try_state::<BubbleShell>() else {
        return;
    };
    stop_timer(&shell);
    shell.bubbles_shown.store(false, Ordering::SeqCst);
    let job_app = app.clone();
    let queued = app.run_on_main_thread(move || {
        if let Some(window) = job_app.get_webview_window(BUBBLES_WINDOW_LABEL) {
            if let Err(e) = window.close() {
                log::warn!("bubble window close failed: {e}");
            }
        }
    });
    if let Err(e) = queued {
        log::warn!("bubble window close could not be scheduled: {e}");
    }
    update(app, |core, inp| core.reset(inp));
}
