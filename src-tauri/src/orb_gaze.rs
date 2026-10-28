//! Pure gaze scheduler for the floating orb's eyes (DESIGN.md Rev 5, 'Gaze'). It decides how
//! often the shell should poll the pointer and which quantised gaze vector to emit. It has no
//! Tauri types, never produces NaN, and emits nothing while hidden or in reduced motion.
#![cfg_attr(not(test), allow(dead_code))]

/// Poll interval, in milliseconds, while the cursor moved recently.
const FAST_POLL_MS: u64 = 33;
/// Poll interval, in milliseconds, while the cursor is still.
const IDLE_POLL_MS: u64 = 500;
/// How long after the last movement the fast poll rate is kept, in milliseconds.
const ACTIVE_WINDOW_MS: u64 = 500;
/// Gaze values are quantised to multiples of `1 / QUANTUM`.
const QUANTUM: f64 = 16.0;

/// A gaze direction: each component is in `[-1, 1]` and a multiple of 1/16.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gaze {
    /// Horizontal component; positive looks right.
    pub x: f64,
    /// Vertical component; positive looks down.
    pub y: f64,
}

/// Decides when to poll the cursor and when the gaze has changed.
#[derive(Debug, Clone, Default)]
pub struct GazeScheduler {
    visible: bool,
    reduced_motion: bool,
    last_cursor: Option<(f64, f64)>,
    last_move_ms: Option<u64>,
    last_emitted: Option<(i64, i64)>,
}

impl GazeScheduler {
    /// A scheduler for a hidden orb with reduced motion off.
    pub fn new() -> Self {
        Self::default()
    }

    /// Shows or hides the orb. Hiding forgets the last cursor, the last movement and the last
    /// emitted value, so the next show emits again.
    pub fn set_visible(&mut self, visible: bool) {
        if !visible {
            self.last_cursor = None;
            self.last_move_ms = None;
            self.last_emitted = None;
        }
        self.visible = visible;
    }

    /// Turns reduced motion on or off. While on, nothing is polled or emitted.
    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
    }

    /// The poll interval to use at `now_ms`: 33 ms while the cursor moved within the last 500 ms,
    /// otherwise 500 ms. `None` when hidden or in reduced motion.
    pub fn poll_interval_ms(&self, now_ms: u64) -> Option<u64> {
        if !self.is_live() {
            return None;
        }
        let moving = self
            .last_move_ms
            .is_some_and(|moved| now_ms.saturating_sub(moved) <= ACTIVE_WINDOW_MS);
        Some(if moving { FAST_POLL_MS } else { IDLE_POLL_MS })
    }

    /// Observes the cursor at `now_ms` and returns the gaze when its quantised value changed
    /// since the last emitted one. The vector is the cursor offset from `orb_center` divided by
    /// `reach_px`, clamped to `[-1, 1]` and rounded to 1/16. A non-positive or non-finite reach
    /// gives the zero vector. A non-finite cursor or centre is ignored. `None` when hidden, in
    /// reduced motion, or unchanged.
    pub fn observe(
        &mut self,
        now_ms: u64,
        cursor: (f64, f64),
        orb_center: (f64, f64),
        reach_px: f64,
    ) -> Option<Gaze> {
        if !self.is_live() || !finite_point(cursor) {
            return None;
        }
        if self.last_cursor != Some(cursor) {
            self.last_move_ms = Some(now_ms);
            self.last_cursor = Some(cursor);
        }
        let (x, y) = if finite_point(orb_center) && reach_px.is_finite() && reach_px > 0.0 {
            (
                (cursor.0 - orb_center.0) / reach_px,
                (cursor.1 - orb_center.1) / reach_px,
            )
        } else {
            (0.0, 0.0)
        };
        let key = (quantise(x), quantise(y));
        if self.last_emitted == Some(key) {
            return None;
        }
        self.last_emitted = Some(key);
        Some(Gaze {
            x: key.0 as f64 / QUANTUM,
            y: key.1 as f64 / QUANTUM,
        })
    }

    /// Whether polling and emission are currently allowed.
    fn is_live(&self) -> bool {
        self.visible && !self.reduced_motion
    }
}

/// Clamps a component to `[-1, 1]` and returns it in sixteenths, rounded. A non-finite value
/// becomes zero.
fn quantise(v: f64) -> i64 {
    if !v.is_finite() {
        return 0;
    }
    (v.clamp(-1.0, 1.0) * QUANTUM).round() as i64
}

/// Whether both coordinates are finite.
fn finite_point(p: (f64, f64)) -> bool {
    p.0.is_finite() && p.1.is_finite()
}
