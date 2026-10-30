//! Pure runtime drivers for the floating orb (DESIGN.md Rev 5, 'Gesture engine owned by Rust' and
//! 'Rust modules and commands'). The drivers join the gesture engine and the gaze scheduler to
//! small traits, so the shell can be replaced by fakes in tests. Nothing here touches the window
//! system: the native shell supplies the cursor, the button state and the host, and carries out
//! the calls the host receives.
#![cfg_attr(not(test), allow(dead_code))]

use crate::orb_gaze::{Gaze, GazeScheduler};
use crate::orb_geometry::{monitor_key, orb_logical_px, MonitorInfo, OrbSize, Rect};
use crate::orb_gesture::{GestureAction, GestureEngine, GestureEvent, GestureResult};

/// Gaze reach in logical pixels: a cursor this far from the orb centre looks fully to one side.
pub const GAZE_REACH_LOGICAL: f64 = 500.0;

/// Ticker interval while a gesture is active, in milliseconds.
pub const GESTURE_TICK_MS: u64 = 16;

/// Ticker interval while no gesture is active and no gaze is due, in milliseconds. Also the
/// floor under reduced motion, where only a wake ends the wait.
pub const IDLE_TICK_MS: u64 = 250;

/// Ticker wait while the orb is hidden, in milliseconds. A wake still ends it early.
pub const HIDDEN_WAIT_MS: u64 = 1_000;

/// How long the ticker may sleep before its next pass, in milliseconds. An active gesture always
/// ticks at `GESTURE_TICK_MS`, whatever the gaze or reduced-motion state. Otherwise a hidden orb
/// waits `HIDDEN_WAIT_MS`, reduced motion waits at least `IDLE_TICK_MS`, and the gaze interval is
/// honoured (or `IDLE_TICK_MS` when no gaze is due).
pub fn tick_interval_ms(
    gesture_active: bool,
    gaze_interval: Option<u64>,
    reduced_motion: bool,
    visible: bool,
) -> u64 {
    if gesture_active {
        return GESTURE_TICK_MS;
    }
    if !visible {
        return HIDDEN_WAIT_MS;
    }
    if reduced_motion {
        return IDLE_TICK_MS;
    }
    gaze_interval.unwrap_or(IDLE_TICK_MS)
}

/// Reports whether the primary mouse button is currently held.
pub trait ButtonProbe {
    /// Whether the left button is down right now.
    fn left_down(&self) -> bool;
}

/// Reports the pointer position.
pub trait CursorSource {
    /// Pointer position in physical pixels, or `None` when it cannot be read.
    fn cursor(&self) -> Option<(f64, f64)>;
}

/// The shell side of the orb: geometry queries, window moves and outgoing events.
pub trait OrbHost {
    /// The orb rectangle in physical pixels, or `None` when there is no orb window.
    fn orb_rect(&self) -> Option<Rect>;
    /// Device scale factor of the monitor under the orb.
    fn scale(&self) -> f64;
    /// Moves the orb so its origin is at `(x, y)`, physical pixels.
    fn move_to(&mut self, x: f64, y: f64);
    /// Reports how gesture `id` ended.
    fn emit_gesture(&mut self, id: u64, result: GestureResult);
    /// Starts or stops a voice conversation. Called for a click only, as Enter and Space do.
    fn click(&mut self);
    /// Saves the orb position. Called exactly once per finished drag, never for a click or cancel.
    fn save_position(&mut self, final_orb: Rect);
    /// Reports a new gaze direction.
    fn emit_gaze(&mut self, gaze: Gaze);
}

/// Turns presses into one gesture at a time and reports each finished gesture exactly once.
#[derive(Debug, Default)]
pub struct GestureDriver {
    engine: GestureEngine,
}

impl GestureDriver {
    /// A driver with no gesture in progress.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a gesture is in progress.
    pub fn is_active(&self) -> bool {
        self.engine.is_active()
    }

    /// A press with id `id`. A press with no cursor or no orb rectangle is cancelled at once and
    /// arms nothing. Any earlier gesture is cancelled first.
    pub fn pointer_down(
        &mut self,
        id: u64,
        now_ms: u64,
        source: &impl CursorSource,
        host: &mut impl OrbHost,
    ) {
        match (source.cursor(), host.orb_rect()) {
            (Some(cursor), Some(orb)) => {
                let actions = self.engine.on_event(GestureEvent::PointerDown {
                    id,
                    cursor,
                    orb,
                    scale: host.scale(),
                    now_ms,
                });
                self.apply(actions, host);
            }
            _ => {
                self.cancel(host);
                host.emit_gesture(id, GestureResult::Cancel);
            }
        }
    }

    /// One poll of the gesture. Does nothing when idle, or when the cursor cannot be read.
    pub fn step(
        &mut self,
        now_ms: u64,
        probe: &impl ButtonProbe,
        source: &impl CursorSource,
        host: &mut impl OrbHost,
    ) {
        if !self.engine.is_active() {
            return;
        }
        let Some(cursor) = source.cursor() else {
            return;
        };
        let actions = self.engine.on_event(GestureEvent::Tick {
            cursor,
            button_down: probe.left_down(),
            now_ms,
        });
        self.apply(actions, host);
    }

    /// Cancels the gesture in progress, if any. A cancel never toggles the chat.
    pub fn cancel(&mut self, host: &mut impl OrbHost) {
        let actions = self.engine.on_event(GestureEvent::Cancel);
        self.apply(actions, host);
    }

    /// Carries out the engine's actions on the host, in order.
    fn apply(&mut self, actions: Vec<GestureAction>, host: &mut impl OrbHost) {
        for action in actions {
            match action {
                GestureAction::MoveTo { x, y } => host.move_to(x, y),
                GestureAction::Finish {
                    id,
                    result,
                    final_orb,
                } => match result {
                    GestureResult::Click => {
                        host.emit_gesture(id, GestureResult::Click);
                        host.click();
                    }
                    GestureResult::Drag => {
                        host.save_position(final_orb);
                        host.emit_gesture(id, GestureResult::Drag);
                    }
                    GestureResult::Cancel => host.emit_gesture(id, GestureResult::Cancel),
                },
            }
        }
    }
}

/// Feeds the gaze scheduler the orb centre and the cursor, and reports changed gaze values.
#[derive(Debug, Default)]
pub struct GazeDriver {
    scheduler: GazeScheduler,
}

impl GazeDriver {
    /// A driver for a hidden orb with reduced motion off.
    pub fn new() -> Self {
        Self::default()
    }

    /// Shows or hides the orb for the scheduler.
    pub fn set_visible(&mut self, visible: bool) {
        self.scheduler.set_visible(visible);
    }

    /// Turns reduced motion on or off. While on, nothing is polled or reported.
    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.scheduler.set_reduced_motion(reduced);
    }

    /// Milliseconds until the next poll at `now_ms`, or `None` when polling is off.
    pub fn next_interval_ms(&self, now_ms: u64) -> Option<u64> {
        self.scheduler.poll_interval_ms(now_ms)
    }

    /// One poll: reports the gaze when it changed. Silent without an orb rectangle or a cursor.
    pub fn step(&mut self, now_ms: u64, source: &impl CursorSource, host: &mut impl OrbHost) {
        let Some(orb) = host.orb_rect() else {
            return;
        };
        let Some(cursor) = source.cursor() else {
            return;
        };
        let reach_px = GAZE_REACH_LOGICAL * host.scale();
        if let Some(gaze) = self
            .scheduler
            .observe(now_ms, cursor, orb.center(), reach_px)
        {
            host.emit_gaze(gaze);
        }
    }
}

/// One monitor as reported by the windowing system, before it is converted.
#[derive(Debug, Clone, PartialEq)]
pub struct RawMonitor {
    /// Monitor name, if the system reports one.
    pub name: Option<String>,
    /// Full monitor origin, physical pixels.
    pub position: (i32, i32),
    /// Full monitor size, physical pixels.
    pub size: (u32, u32),
    /// Work-area origin, physical pixels.
    pub work_position: (i32, i32),
    /// Work-area size, physical pixels.
    pub work_size: (u32, u32),
    /// Device scale factor.
    pub scale: f64,
}

/// Converts raw monitors into monitor infos. A missing name becomes `monitor-<index>`. The primary
/// monitor is `primary_index`, or the first monitor when that is `None`.
pub fn monitors_from_raw(raw: &[RawMonitor], primary_index: Option<usize>) -> Vec<MonitorInfo> {
    let primary = primary_index.or(if raw.is_empty() { None } else { Some(0) });
    raw.iter()
        .enumerate()
        .map(|(index, monitor)| {
            let name = monitor
                .name
                .clone()
                .unwrap_or_else(|| format!("monitor-{index}"));
            let rect = Rect {
                x: f64::from(monitor.position.0),
                y: f64::from(monitor.position.1),
                w: f64::from(monitor.size.0),
                h: f64::from(monitor.size.1),
            };
            let work = Rect {
                x: f64::from(monitor.work_position.0),
                y: f64::from(monitor.work_position.1),
                w: f64::from(monitor.work_size.0),
                h: f64::from(monitor.work_size.1),
            };
            MonitorInfo {
                key: monitor_key(&name, &rect),
                name,
                rect,
                work,
                scale: monitor.scale,
                primary: primary == Some(index),
            }
        })
        .collect()
}

/// One row of the orb's native context menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuEntry {
    /// A clickable item. `checked` is `None` for a plain item, else the check state.
    Item {
        /// Stable item id, used by the menu handler.
        id: &'static str,
        /// Check state, or `None` for a plain item.
        checked: Option<bool>,
    },
    /// A separator line.
    Separator,
}

/// The orb context menu, in display order. The size items are checked for the current size; the
/// conversation item is checked while the conversation surface is open.
pub fn orb_menu_items(size: OrbSize, chat_open: bool) -> Vec<MenuEntry> {
    orb_menu_items_for_px(orb_logical_px(size), chat_open)
}

/// [`orb_menu_items`] for an effective logical size. A preset item is checked only when the
/// effective size equals that preset exactly, so a custom size checks no item.
pub fn orb_menu_items_for_px(logical_px: f64, chat_open: bool) -> Vec<MenuEntry> {
    let is_preset = |size: OrbSize| logical_px == orb_logical_px(size);
    vec![
        MenuEntry::Item {
            id: "orb_toggle_chat",
            checked: Some(chat_open),
        },
        MenuEntry::Separator,
        MenuEntry::Item {
            id: "orb_size_small",
            checked: Some(is_preset(OrbSize::Small)),
        },
        MenuEntry::Item {
            id: "orb_size_medium",
            checked: Some(is_preset(OrbSize::Medium)),
        },
        MenuEntry::Item {
            id: "orb_size_large",
            checked: Some(is_preset(OrbSize::Large)),
        },
        MenuEntry::Separator,
        MenuEntry::Item {
            id: "orb_hide",
            checked: None,
        },
        MenuEntry::Item {
            id: "orb_settings",
            checked: None,
        },
    ]
}

/// Geometry revision guard. Every resize and every finished drag bumps the revision; a saved
/// position is accepted only when it was computed against the latest revision, so a save that
/// was captured before a newer change can never overwrite it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GeometryGuard {
    rev: u64,
}

impl GeometryGuard {
    /// A guard at revision 0.
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a new revision and returns it.
    pub fn bump(&mut self) -> u64 {
        self.rev = self.rev.wrapping_add(1);
        self.rev
    }

    /// Whether a position computed against revision `rev` is still current.
    pub fn accept(&self, rev: u64) -> bool {
        rev == self.rev
    }
}

/// A free-size change that arrived during a gesture. The latest deferred value wins and is
/// returned once, after the gesture has ended.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PendingResize {
    pending: Option<Option<u16>>,
}

impl PendingResize {
    /// Nothing pending.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `px` (`None` for the preset) as the value to apply, replacing any earlier one.
    pub fn defer(&mut self, px: Option<u16>) {
        self.pending = Some(px);
    }

    /// The deferred value once no gesture is active, clearing it. While a gesture is active
    /// nothing is returned and the value stays pending.
    pub fn take(&mut self, gesture_active: bool) -> Option<Option<u16>> {
        if gesture_active {
            return None;
        }
        self.pending.take()
    }
}
