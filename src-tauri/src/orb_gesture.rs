//! Pure gesture engine for the floating orb (DESIGN.md Rev 5, 'Gesture engine owned by Rust').
//! The engine turns press, tick and cancel events into orb moves and a single finish per
//! gesture. It has no Tauri types and never panics; non-finite numbers are sanitised.
#![cfg_attr(not(test), allow(dead_code))]

use crate::orb_geometry::{Rect, SLOP_LOGICAL_PX};

/// Longest a gesture may stay active, in milliseconds, before the watchdog cancels it.
pub const WATCHDOG_MS: u64 = 60_000;

/// How a gesture ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureResult {
    /// The pointer stayed within the slop: a click.
    Click,
    /// The pointer moved beyond the slop: the orb was dragged.
    Drag,
    /// The gesture was cancelled, by request, by a new press or by the watchdog.
    Cancel,
}

/// An input event fed to the engine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GestureEvent {
    /// The orb was pressed.
    PointerDown {
        /// Identity of the press; the same id twice is ignored.
        id: u64,
        /// Pointer position in physical pixels.
        cursor: (f64, f64),
        /// The orb rectangle at the moment of the press, physical pixels.
        orb: Rect,
        /// Device scale factor of the monitor under the orb.
        scale: f64,
        /// Monotonic timestamp in milliseconds.
        now_ms: u64,
    },
    /// A periodic poll of the pointer state.
    Tick {
        /// Pointer position in physical pixels.
        cursor: (f64, f64),
        /// Whether the primary button is still down.
        button_down: bool,
        /// Monotonic timestamp in milliseconds.
        now_ms: u64,
    },
    /// The gesture must be abandoned.
    Cancel,
}

/// An instruction produced by the engine for the shell to carry out.
#[derive(Debug, Clone, PartialEq)]
pub enum GestureAction {
    /// Move the orb so its origin is at `(x, y)`, physical pixels.
    MoveTo {
        /// New left edge.
        x: f64,
        /// New top edge.
        y: f64,
    },
    /// The gesture ended.
    Finish {
        /// Identity of the finished press.
        id: u64,
        /// How it ended.
        result: GestureResult,
        /// The orb rectangle after the gesture.
        final_orb: Rect,
    },
}

/// The state of the one gesture in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Active {
    id: u64,
    press: (f64, f64),
    grab: (f64, f64),
    origin: Rect,
    last_orb: Rect,
    last_cursor: (f64, f64),
    scale: f64,
    press_ms: u64,
    dragging: bool,
}

/// Tracks at most one press at a time and decides click, drag or cancel.
#[derive(Debug, Clone, Default)]
pub struct GestureEngine {
    active: Option<Active>,
}

impl GestureEngine {
    /// An engine with no gesture in progress.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a gesture is in progress.
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// The id of the gesture in progress, if any.
    pub fn active_id(&self) -> Option<u64> {
        self.active.map(|a| a.id)
    }

    /// Applies one event and returns the actions to carry out, in order.
    pub fn on_event(&mut self, event: GestureEvent) -> Vec<GestureAction> {
        match event {
            GestureEvent::PointerDown {
                id,
                cursor,
                orb,
                scale,
                now_ms,
            } => self.press(id, cursor, orb, scale, now_ms),
            GestureEvent::Tick {
                cursor,
                button_down,
                now_ms,
            } => self.tick(cursor, button_down, now_ms),
            GestureEvent::Cancel => self
                .finish_with(GestureResult::Cancel)
                .into_iter()
                .collect(),
        }
    }

    /// Arms a new press. A press during a gesture first cancels that gesture; the same id again
    /// is ignored.
    fn press(
        &mut self,
        id: u64,
        cursor: (f64, f64),
        orb: Rect,
        scale: f64,
        now_ms: u64,
    ) -> Vec<GestureAction> {
        if self.active.is_some_and(|a| a.id == id) {
            return Vec::new();
        }
        let actions: Vec<GestureAction> = self
            .finish_with(GestureResult::Cancel)
            .into_iter()
            .collect();
        let orb = sanitize_rect(orb);
        let cursor = if finite_point(cursor) {
            cursor
        } else {
            (orb.x, orb.y)
        };
        self.active = Some(Active {
            id,
            press: cursor,
            grab: (cursor.0 - orb.x, cursor.1 - orb.y),
            origin: orb,
            last_orb: orb,
            last_cursor: cursor,
            scale: sane_scale(scale),
            press_ms: now_ms,
            dragging: false,
        });
        actions
    }

    /// Advances the gesture on a poll: watchdog, drag, or release.
    fn tick(&mut self, cursor: (f64, f64), button_down: bool, now_ms: u64) -> Vec<GestureAction> {
        let Some(mut active) = self.active else {
            return Vec::new();
        };
        if now_ms.saturating_sub(active.press_ms) > WATCHDOG_MS {
            return self
                .finish_with(GestureResult::Cancel)
                .into_iter()
                .collect();
        }
        // A non-finite cursor counts as no movement.
        let cursor = if finite_point(cursor) {
            cursor
        } else {
            active.last_cursor
        };
        let moved = distance(active.press, cursor);
        let slop = SLOP_LOGICAL_PX * active.scale;
        if !button_down {
            let result = if active.dragging || moved > slop {
                GestureResult::Drag
            } else {
                GestureResult::Click
            };
            return self.finish_with(result).into_iter().collect();
        }
        active.last_cursor = cursor;
        if !active.dragging && moved > slop {
            active.dragging = true;
        }
        if !active.dragging {
            self.active = Some(active);
            return Vec::new();
        }
        let x = cursor.0 - active.grab.0;
        let y = cursor.1 - active.grab.1;
        active.last_orb = Rect {
            x,
            y,
            w: active.origin.w,
            h: active.origin.h,
        };
        self.active = Some(active);
        vec![GestureAction::MoveTo { x, y }]
    }

    /// Ends the gesture in progress with `result`, reporting the last orb position. A click
    /// keeps the original rectangle because the orb never moved.
    fn finish_with(&mut self, result: GestureResult) -> Option<GestureAction> {
        let active = self.active.take()?;
        let final_orb = if result == GestureResult::Click {
            active.origin
        } else {
            active.last_orb
        };
        Some(GestureAction::Finish {
            id: active.id,
            result,
            final_orb,
        })
    }
}

/// Whether both coordinates are finite.
fn finite_point(p: (f64, f64)) -> bool {
    p.0.is_finite() && p.1.is_finite()
}

/// Euclidean distance between two points; non-finite results count as infinitely far.
fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    let d = (b.0 - a.0).hypot(b.1 - a.1);
    if d.is_finite() {
        d
    } else {
        f64::INFINITY
    }
}

/// A scale that is finite and positive; anything else becomes 1.
fn sane_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// A rectangle with finite origin and non-negative size; bad parts become 0.
fn sanitize_rect(r: Rect) -> Rect {
    let coord = |v: f64| if v.is_finite() { v } else { 0.0 };
    let len = |v: f64| if v.is_finite() { v.max(0.0) } else { 0.0 };
    Rect {
        x: coord(r.x),
        y: coord(r.y),
        w: len(r.w),
        h: len(r.h),
    }
}
