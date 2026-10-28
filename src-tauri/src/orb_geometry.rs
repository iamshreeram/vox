//! Pure geometry for the floating orb and its chat panel (DESIGN.md Rev 4, 'Coordinates' and
//! 'Surface state machine and placement'). Every value is in PHYSICAL pixels in one global
//! desktop space. This module has no Tauri types: callers convert monitor and cursor data into
//! these structs. Nothing here panics; non-finite or negative inputs are sanitised.
#![cfg_attr(not(test), allow(dead_code))]

use serde::{Deserialize, Serialize};
use specta::Type;

/// Gap between the orb and a panel placed beside it, in logical pixels.
const GAP_LOGICAL_PX: f64 = 8.0;
/// Margin kept free around the panel inside the work area, in logical pixels.
const MARGIN_LOGICAL_PX: f64 = 12.0;
/// Preferred panel width, in logical pixels.
const PANEL_WIDTH_LOGICAL_PX: f64 = 340.0;
/// Smallest panel height, in logical pixels.
const MIN_PANEL_HEIGHT_LOGICAL_PX: f64 = 160.0;
/// Distance the tail keeps from the panel corners, in logical pixels.
const TAIL_INSET_LOGICAL_PX: f64 = 24.0;
/// Extra room a new side must have before the panel leaves its current side, in logical pixels.
const HYSTERESIS_SLACK_LOGICAL_PX: f64 = 24.0;
/// Inset of the work-area corner an orb is moved to when it is stranded, in logical pixels.
const ORB_CLAMP_INSET_LOGICAL_PX: f64 = 20.0;

/// Pointer travel, in logical pixels, at or below which a press is a click rather than a drag.
pub const SLOP_LOGICAL_PX: f64 = 4.0;

/// An axis-aligned rectangle in physical pixels (origin top-left, y grows downwards).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

impl Rect {
    /// Right edge (`x + w`).
    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    /// Bottom edge (`y + h`).
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    /// Centre point `(x, y)`.
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// A width and height in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

/// One monitor as seen by the geometry code.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    /// Human-readable monitor name as reported by the windowing system.
    pub name: String,
    /// Stable identity: see [`monitor_key`].
    pub key: String,
    /// Full monitor rectangle in physical pixels.
    pub rect: Rect,
    /// Work area (monitor minus menu bar and dock or taskbar), physical pixels.
    pub work: Rect,
    /// Device scale factor (physical pixels per logical pixel).
    pub scale: f64,
    /// Whether this is the primary monitor.
    pub primary: bool,
}

/// Orb size preset. Stored in settings as `small`, `medium` or `large`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum OrbSize {
    /// 72 logical pixels.
    Small,
    /// 96 logical pixels (default).
    #[default]
    Medium,
    /// 128 logical pixels.
    Large,
}

/// The saved orb position: the monitor it was on plus its centre as a fraction of that monitor's
/// work area. Fractions survive resolution and scale changes; the name and key identify the
/// monitor when it returns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct OrbPosition {
    /// Monitor name at save time.
    pub monitor_name: String,
    /// Monitor key at save time (see [`monitor_key`]).
    pub monitor_key: String,
    /// Horizontal centre as a fraction of the work-area width, in `[0, 1]`.
    pub fx: f64,
    /// Vertical centre as a fraction of the work-area height, in `[0, 1]`.
    pub fy: f64,
}

/// How a pointer press on the orb is classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragClass {
    /// Pointer moved no more than the slop: a click.
    Click,
    /// Pointer moved more than the slop: a drag.
    Drag,
}

/// The side of the orb a panel sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Panel below the orb (preferred).
    Below,
    /// Panel above the orb.
    Above,
    /// Panel to the left of the orb.
    Left,
    /// Panel to the right of the orb.
    Right,
}

/// How the bubble stack hugs its window: `"top"` when the stack sits below the orb, so its
/// newest bubble is nearest the orb and the stack is top-aligned; `"bottom"` otherwise.
pub fn anchor_for_side(side: Side) -> &'static str {
    match side {
        Side::Below => "top",
        Side::Above | Side::Left | Side::Right => "bottom",
    }
}

/// Result of [`place_panel`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Chosen side.
    pub side: Side,
    /// Panel rectangle, always inside the work area.
    pub rect: Rect,
    /// Tail position along the shared axis, measured from the panel's left (or top) edge.
    pub tail_offset: f64,
}

/// Stable identity for a monitor: `name|x|y|w|h` with integer-rounded physical numbers.
pub fn monitor_key(name: &str, rect: &Rect) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        name,
        rect.x.round() as i64,
        rect.y.round() as i64,
        rect.w.round() as i64,
        rect.h.round() as i64
    )
}

/// Whether a monitor can be used: its rectangle and work area are finite with a positive size.
/// Monitors that fail this are ignored by every lookup in this module.
fn is_usable(m: &MonitorInfo) -> bool {
    [m.rect, m.work]
        .iter()
        .all(|r| [r.x, r.y, r.w, r.h].iter().all(|v| v.is_finite()) && r.w > 0.0 && r.h > 0.0)
}

/// The monitor containing `point`, otherwise the nearest one by distance to its rectangle.
/// Monitors with a non-finite or empty rectangle or work area are ignored. Returns `None` when
/// no usable monitor exists.
pub fn monitor_for(point: (f64, f64), monitors: &[MonitorInfo]) -> Option<&MonitorInfo> {
    let (px, py) = point;
    let usable = || monitors.iter().filter(|m| is_usable(m));
    let inside = usable()
        .find(|m| px >= m.rect.x && px < m.rect.right() && py >= m.rect.y && py < m.rect.bottom());
    if inside.is_some() {
        return inside;
    }
    let mut best: Option<(&MonitorInfo, f64)> = None;
    for m in usable() {
        let dx = (m.rect.x - px).max(0.0).max(px - m.rect.right());
        let dy = (m.rect.y - py).max(0.0).max(py - m.rect.bottom());
        let distance = dx.hypot(dy);
        if best.map_or(true, |(_, current)| distance < current) {
            best = Some((m, distance));
        }
    }
    best.map(|(m, _)| m)
}

/// Smallest free orb size, in logical pixels.
pub const ORB_PX_MIN: u16 = 48;
/// Largest free orb size, in logical pixels.
pub const ORB_PX_MAX: u16 = 192;

/// Orb edge length of a size preset, in whole logical pixels.
pub fn preset_px(size: OrbSize) -> u16 {
    match size {
        OrbSize::Small => 72,
        OrbSize::Medium => 96,
        OrbSize::Large => 128,
    }
}

/// Orb edge length in logical pixels for a size preset.
pub fn orb_logical_px(size: OrbSize) -> f64 {
    f64::from(preset_px(size))
}

/// A free orb size clamped to `[ORB_PX_MIN, ORB_PX_MAX]`.
pub fn clamp_orb_px(px: u16) -> u16 {
    px.clamp(ORB_PX_MIN, ORB_PX_MAX)
}

/// The orb edge in logical pixels: the clamped free size when set, else the preset.
pub fn effective_orb_logical_px(size: OrbSize, px: Option<u16>) -> f64 {
    match px {
        Some(value) => f64::from(clamp_orb_px(value)),
        None => orb_logical_px(size),
    }
}

/// The preset nearest to a free size: Small up to 84, Medium up to 112, Large above.
pub fn nearest_preset(px: u16) -> OrbSize {
    if px <= 84 {
        OrbSize::Small
    } else if px <= 112 {
        OrbSize::Medium
    } else {
        OrbSize::Large
    }
}

/// Orb edge length in physical pixels for a logical length at `scale`, rounded to a whole pixel.
pub fn orb_px_logical(logical_px: f64, scale: f64) -> f64 {
    (logical_px * sane_scale(scale)).round()
}

/// Orb edge length in physical pixels at `scale`, rounded to a whole pixel.
pub fn orb_px(size: OrbSize, scale: f64) -> f64 {
    orb_px_logical(orb_logical_px(size), scale)
}

/// Resizes a square orb to `new_side` (physical pixels) about its centre, then moves it into
/// `work`. The side is first limited to the work area (each axis separately). A non-finite or
/// non-positive `new_side` keeps the current width.
pub fn resize_keep_center(rect: Rect, new_side: f64, work: Rect) -> Rect {
    let rect = sanitize_rect(rect);
    let work = sanitize_rect(work);
    let side = if new_side.is_finite() && new_side > 0.0 {
        new_side
    } else {
        rect.w
    };
    let w = side.min(work.w);
    let h = side.min(work.h);
    let (cx, cy) = rect.center();
    Rect {
        x: clamp_axis(cx - w / 2.0, work.x, work.right() - w),
        y: clamp_axis(cy - h / 2.0, work.y, work.bottom() - h),
        w,
        h,
    }
}

/// Classifies a press by how far the pointer moved (physical pixels) at `scale`.
pub fn classify_drag(moved_physical_px: f64, scale: f64) -> DragClass {
    if moved_physical_px <= SLOP_LOGICAL_PX * sane_scale(scale) {
        DragClass::Click
    } else {
        DragClass::Drag
    }
}

/// Keeps the orb usable after a monitor change. If at least half of the orb lies inside some
/// usable work area it is returned unchanged. Otherwise it moves to the work-area corner nearest
/// its centre (inset 20 logical px) of the monitor nearest that centre. With no usable monitors
/// the orb is returned unchanged.
pub fn clamp_orb(orb: Rect, monitors: &[MonitorInfo]) -> Rect {
    let orb = sanitize_rect(orb);
    let half_area = orb.w * orb.h / 2.0;
    if monitors
        .iter()
        .filter(|m| is_usable(m))
        .any(|m| intersection_area(&orb, &m.work) >= half_area)
    {
        return orb;
    }
    let (cx, cy) = orb.center();
    let Some(monitor) = monitor_for((cx, cy), monitors) else {
        return orb;
    };
    let work = monitor.work;
    let inset = ORB_CLAMP_INSET_LOGICAL_PX * sane_scale(monitor.scale);
    // Each candidate: orb top-left, then the work-area corner it sits in.
    let candidates = [
        (work.x + inset, work.y + inset, work.x, work.y),
        (
            work.right() - inset - orb.w,
            work.y + inset,
            work.right(),
            work.y,
        ),
        (
            work.x + inset,
            work.bottom() - inset - orb.h,
            work.x,
            work.bottom(),
        ),
        (
            work.right() - inset - orb.w,
            work.bottom() - inset - orb.h,
            work.right(),
            work.bottom(),
        ),
    ];
    let mut chosen = candidates[0];
    let mut chosen_distance = f64::INFINITY;
    for candidate in candidates {
        let distance = (candidate.2 - cx).hypot(candidate.3 - cy);
        if distance < chosen_distance {
            chosen = candidate;
            chosen_distance = distance;
        }
    }
    Rect {
        x: chosen.0,
        y: chosen.1,
        w: orb.w,
        h: orb.h,
    }
}

/// The saved position for an orb rectangle: the monitor containing its centre and the centre as
/// a fraction of that monitor's work area, clamped to `[0, 1]`. `None` when there are no
/// monitors.
pub fn position_from_rect(orb: Rect, monitors: &[MonitorInfo]) -> Option<OrbPosition> {
    let orb = sanitize_rect(orb);
    let (cx, cy) = orb.center();
    let monitor = monitor_for((cx, cy), monitors)?;
    Some(OrbPosition {
        monitor_name: monitor.name.clone(),
        monitor_key: monitor.key.clone(),
        fx: fraction(cx - monitor.work.x, monitor.work.w),
        fy: fraction(cy - monitor.work.y, monitor.work.h),
    })
}

/// The orb rectangle for a saved position. The monitor is found by key, then by name, then the
/// primary monitor, then the first monitor, considering usable monitors only. The orb is sized
/// for that monitor's scale, clamped against that monitor alone with [`clamp_orb`], and then
/// moved fully inside its work area so it never lands on a neighbouring monitor. `None` when
/// there is no usable monitor.
pub fn restore_position(
    saved: &OrbPosition,
    size: OrbSize,
    monitors: &[MonitorInfo],
) -> Option<Rect> {
    restore_position_px(saved, orb_logical_px(size), monitors)
}

/// [`restore_position`] for an orb whose logical edge is `logical_px` (a free size or a preset).
pub fn restore_position_px(
    saved: &OrbPosition,
    logical_px: f64,
    monitors: &[MonitorInfo],
) -> Option<Rect> {
    let usable: Vec<MonitorInfo> = monitors.iter().filter(|m| is_usable(m)).cloned().collect();
    let monitor = usable
        .iter()
        .find(|m| m.key == saved.monitor_key)
        .or_else(|| usable.iter().find(|m| m.name == saved.monitor_name))
        .or_else(|| usable.iter().find(|m| m.primary))
        .or_else(|| usable.first())?;
    let side = orb_px_logical(logical_px, monitor.scale);
    let work = sanitize_rect(monitor.work);
    let cx = work.x + unit_fraction(saved.fx) * work.w;
    let cy = work.y + unit_fraction(saved.fy) * work.h;
    let orb = Rect {
        x: cx - side / 2.0,
        y: cy - side / 2.0,
        w: side,
        h: side,
    };
    let clamped = clamp_orb(orb, std::slice::from_ref(monitor));
    Some(Rect {
        x: clamp_axis(clamped.x, work.x, work.right() - side),
        y: clamp_axis(clamped.y, work.y, work.bottom() - side),
        w: side,
        h: side,
    })
}

/// Effective panel size in physical pixels for a requested logical height at `scale`.
///
/// Width is `min(340, work.w - 2 margin)` logical and height is
/// `clamp(requested, min(160, avail), avail)` where `avail = work.h - 2 margin`. A dimension
/// smaller than `2 margin + 1` takes the work size itself. The result is always finite,
/// non-negative and no larger than the work area.
pub fn panel_size(work: Rect, requested_h_logical: f64, scale: f64) -> Size {
    let scale = sane_scale(scale);
    let work = sanitize_rect(work);
    let margin = MARGIN_LOGICAL_PX * scale;
    let requested = if requested_h_logical.is_finite() {
        requested_h_logical.max(0.0) * scale
    } else {
        0.0
    };
    let width = if work.w < 2.0 * margin + 1.0 {
        work.w
    } else {
        (PANEL_WIDTH_LOGICAL_PX * scale).min(work.w - 2.0 * margin)
    };
    let height = if work.h < 2.0 * margin + 1.0 {
        work.h
    } else {
        let avail = work.h - 2.0 * margin;
        requested
            .max(MIN_PANEL_HEIGHT_LOGICAL_PX * scale)
            .min(avail)
    };
    Size {
        w: width.max(0.0),
        h: height.max(0.0),
    }
}

/// Places the chat panel beside the orb.
///
/// Priority: Below if it fits. Otherwise the horizontal side that fits with more room (a tie
/// goes to Right). Otherwise Above if it fits. With a `current` side that still fits, the panel
/// stays on it unless Below has at least 24 logical px to spare, or a horizontal side has at
/// least 24 logical px more room. When nothing fits, the side with the most room is used. The
/// rectangle is then clamped into `work`, and the tail is clamped to the panel corners. Leaving
/// an Above panel for a horizontal side needs that side to fit with 24 logical px of spare space
/// (room minus gap minus panel width).
pub fn place_panel(
    orb: Rect,
    size: Size,
    work: Rect,
    scale: f64,
    current: Option<Side>,
) -> Placement {
    let scale = sane_scale(scale);
    let orb = sanitize_rect(orb);
    let work = sanitize_rect(work);
    let w = sane_len(size.w).min(work.w);
    let h = sane_len(size.h).min(work.h);
    let gap = GAP_LOGICAL_PX * scale;
    let slack = HYSTERESIS_SLACK_LOGICAL_PX * scale;

    let below = layout(Side::Below, orb, work, w, h, gap);
    let above = layout(Side::Above, orb, work, w, h, gap);
    let left = layout(Side::Left, orb, work, w, h, gap);
    let right = layout(Side::Right, orb, work, w, h, gap);
    let placed = |side: Side| match side {
        Side::Below => below,
        Side::Above => above,
        Side::Left => left,
        Side::Right => right,
    };

    let best = if below.fits {
        Side::Below
    } else if let Some(horizontal) = horizontal_pick(&left, &right) {
        horizontal
    } else if above.fits {
        Side::Above
    } else {
        // Nothing fits: the side with the most room, first in the order Below, Right, Left, Above.
        let mut fallback = Side::Below;
        let mut most = below.room;
        for (side, room) in [
            (Side::Right, right.room),
            (Side::Left, left.room),
            (Side::Above, above.room),
        ] {
            if room > most {
                fallback = side;
                most = room;
            }
        }
        fallback
    };

    // A horizontal side is worth leaving Above for only when it fits with the slack to spare.
    let horizontal_ok = |side: Side| {
        let l = placed(side);
        matches!(side, Side::Left | Side::Right) && l.fits && l.room - gap - w >= slack
    };

    let chosen = match current {
        Some(Side::Above) if above.fits && best != Side::Below => {
            if horizontal_ok(best) {
                best
            } else {
                match (horizontal_ok(Side::Left), horizontal_ok(Side::Right)) {
                    (true, true) => {
                        if left.room > right.room {
                            Side::Left
                        } else {
                            Side::Right
                        }
                    }
                    (true, false) => Side::Left,
                    (false, true) => Side::Right,
                    (false, false) => Side::Above,
                }
            }
        }
        Some(current) if current != best && placed(current).fits => {
            let switch = if best == Side::Below {
                work.bottom() - below.rect.bottom() >= slack
            } else {
                placed(best).room >= placed(current).room + slack
            };
            if switch {
                best
            } else {
                current
            }
        }
        _ => best,
    };

    let mut rect = placed(chosen).rect;
    rect.x = clamp_axis(rect.x, work.x, work.right() - w);
    rect.y = clamp_axis(rect.y, work.y, work.bottom() - h);

    let tail_offset = match chosen {
        Side::Below | Side::Above => {
            let (cx, _) = orb.center();
            tail_along(cx - rect.x, rect.w, scale)
        }
        Side::Left | Side::Right => {
            let (_, cy) = orb.center();
            tail_along(cy - rect.y, rect.h, scale)
        }
    };

    Placement {
        side: chosen,
        rect,
        tail_offset,
    }
}

/// One candidate side: its rectangle, whether it fits, and the room on that side.
#[derive(Debug, Clone, Copy)]
struct Layout {
    rect: Rect,
    fits: bool,
    room: f64,
}

/// Lays out the panel on one side of the orb. Below and Above are centred on the orb
/// horizontally, Left and Right vertically; the shared axis is clamped into the work area.
fn layout(side: Side, orb: Rect, work: Rect, w: f64, h: f64, gap: f64) -> Layout {
    let (cx, cy) = orb.center();
    match side {
        Side::Below => {
            let rect = Rect {
                x: clamp_axis(cx - w / 2.0, work.x, work.right() - w),
                y: orb.bottom() + gap,
                w,
                h,
            };
            Layout {
                rect,
                fits: rect.bottom() <= work.bottom(),
                room: work.bottom() - orb.bottom(),
            }
        }
        Side::Above => {
            let rect = Rect {
                x: clamp_axis(cx - w / 2.0, work.x, work.right() - w),
                y: orb.y - gap - h,
                w,
                h,
            };
            Layout {
                rect,
                fits: rect.y >= work.y,
                room: orb.y - work.y,
            }
        }
        Side::Right => {
            let rect = Rect {
                x: orb.right() + gap,
                y: clamp_axis(cy - h / 2.0, work.y, work.bottom() - h),
                w,
                h,
            };
            Layout {
                rect,
                fits: rect.right() <= work.right(),
                room: work.right() - orb.right(),
            }
        }
        Side::Left => {
            let rect = Rect {
                x: orb.x - gap - w,
                y: clamp_axis(cy - h / 2.0, work.y, work.bottom() - h),
                w,
                h,
            };
            Layout {
                rect,
                fits: rect.x >= work.x,
                room: orb.x - work.x,
            }
        }
    }
}

/// The fitting horizontal side with more room; a tie goes to Right.
fn horizontal_pick(left: &Layout, right: &Layout) -> Option<Side> {
    match (left.fits, right.fits) {
        (true, true) => Some(if left.room > right.room {
            Side::Left
        } else {
            Side::Right
        }),
        (false, true) => Some(Side::Right),
        (true, false) => Some(Side::Left),
        (false, false) => None,
    }
}

/// Tail position along a panel edge: the orb centre minus the panel origin, kept at least 24
/// logical px from both corners. A panel narrower than 48 logical px uses its half extent.
fn tail_along(offset: f64, extent: f64, scale: f64) -> f64 {
    let inset = TAIL_INSET_LOGICAL_PX * scale;
    let (lo, hi) = if extent < 2.0 * inset {
        (extent / 2.0, extent / 2.0)
    } else {
        (inset, extent - inset)
    };
    clamp_axis(offset, lo, hi)
}

/// Clamps `v` into `[lo, hi]`. When `hi < lo` the result is `lo`, so it never panics.
fn clamp_axis(v: f64, lo: f64, hi: f64) -> f64 {
    if hi < lo {
        lo
    } else {
        v.max(lo).min(hi)
    }
}

/// Area of the overlap between two rectangles; zero when they do not overlap.
fn intersection_area(a: &Rect, b: &Rect) -> f64 {
    let w = (a.right().min(b.right()) - a.x.max(b.x)).max(0.0);
    let h = (a.bottom().min(b.bottom()) - a.y.max(b.y)).max(0.0);
    w * h
}

/// Fraction of `extent` covered by `offset`, clamped to `[0, 1]`; `0` for a degenerate extent.
fn fraction(offset: f64, extent: f64) -> f64 {
    if extent > 0.0 && extent.is_finite() && offset.is_finite() {
        (offset / extent).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// A saved fraction clamped to `[0, 1]`; a non-finite value becomes the centre.
fn unit_fraction(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.5
    }
}

/// A scale factor that is finite and positive; anything else becomes 1.
fn sane_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// A length that is finite and non-negative; anything else becomes 0.
fn sane_len(v: f64) -> f64 {
    if v.is_finite() {
        v.max(0.0)
    } else {
        0.0
    }
}

/// A coordinate that is finite; anything else becomes 0.
fn sane_coord(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

/// A rectangle with finite origin and non-negative size.
fn sanitize_rect(r: Rect) -> Rect {
    Rect {
        x: sane_coord(r.x),
        y: sane_coord(r.y),
        w: sane_len(r.w),
        h: sane_len(r.h),
    }
}

/// Full size of the bubble stack window, logical pixels (DESIGN A7-3, A8-3).
pub const BUBBLE_W_LOGICAL: f64 = 320.0;
/// Full height of the bubble stack window, logical pixels.
pub const BUBBLE_H_LOGICAL: f64 = 240.0;
/// Smallest usable bubble height, logical pixels: one maximal 4-line bubble plus padding.
pub const MIN_BUBBLE_H_LOGICAL: f64 = 120.0;
/// Smallest usable bubble width, logical pixels.
pub const MIN_BUBBLE_W_LOGICAL: f64 = 200.0;

/// Tolerance when comparing a computed bubble length with its minimum.
const FIT_EPS: f64 = 1e-9;

/// Where a bubble stack sits beside the orb.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BubblePlacement {
    /// The stack rectangle, inside the work area and never overlapping the orb.
    pub rect: Rect,
    /// The side of the orb the stack is on.
    pub side: Side,
    /// Orb centre along the stack edge as a 0..=1 fraction (the tail position).
    pub tail: f64,
}

/// Places a bubble stack beside the orb (DESIGN A7-3, A8-3). Sides are tried in the order
/// Above, Below, Right, Left. Above and Below keep the full width when the work area allows and
/// shrink the height to the room beside the orb; Right and Left shrink the width. A side is
/// usable only when both of its dimensions reach `min`; when no side is usable the result is
/// `None`. All values are physical pixels, so the caller converts logical gap, size and minimum
/// with the monitor scale and this function stays scale-free.
pub fn place_bubbles(
    orb: Rect,
    work: Rect,
    size: Size,
    gap: f64,
    min: Size,
) -> Option<BubblePlacement> {
    // A non-finite input has no meaningful placement: refuse it rather than sanitise it.
    let inputs = [
        orb.x, orb.y, orb.w, orb.h, work.x, work.y, work.w, work.h, size.w, size.h, gap, min.w,
        min.h,
    ];
    if inputs.iter().any(|value| !value.is_finite()) {
        return None;
    }
    let orb = sanitize_rect(orb);
    let work = sanitize_rect(work);
    let full = Size {
        w: sane_len(size.w),
        h: sane_len(size.h),
    };
    let min = Size {
        w: sane_len(min.w),
        h: sane_len(min.h),
    };
    let gap = sane_len(gap);
    [Side::Above, Side::Below, Side::Right, Side::Left]
        .into_iter()
        .find_map(|side| bubble_on(side, orb, work, full, gap, min))
}

/// Private: one side of [`place_bubbles`], or `None` when that side cannot reach `min`.
fn bubble_on(
    side: Side,
    orb: Rect,
    work: Rect,
    full: Size,
    gap: f64,
    min: Size,
) -> Option<BubblePlacement> {
    let rect = match side {
        Side::Above | Side::Below => vertical_bubble(side, orb, work, full, gap, min)?,
        Side::Left | Side::Right => horizontal_bubble(side, orb, work, full, gap, min)?,
    };
    let tail = match side {
        Side::Above | Side::Below => fraction(orb.center().0 - rect.x, rect.w),
        Side::Left | Side::Right => fraction(orb.center().1 - rect.y, rect.h),
    };
    Some(BubblePlacement { rect, side, tail })
}

/// Private: a stack above or below the orb, or `None` when its height or width is below `min`.
fn vertical_bubble(
    side: Side,
    orb: Rect,
    work: Rect,
    full: Size,
    gap: f64,
    min: Size,
) -> Option<Rect> {
    let (edge, room) = if side == Side::Above {
        let edge = (orb.y - gap).min(work.bottom());
        (edge, edge - work.y)
    } else {
        let edge = (orb.bottom() + gap).max(work.y);
        (edge, work.bottom() - edge)
    };
    let h = full.h.min(room).min(work.h);
    let w = full.w.min(work.w);
    if !fits(h, min.h) || !fits(w, min.w) {
        return None;
    }
    let x = clamp_axis(orb.center().0 - w / 2.0, work.x, work.right() - w);
    let top = if side == Side::Above { edge - h } else { edge };
    Some(Rect {
        x,
        y: clamp_axis(top, work.y, work.bottom() - h),
        w,
        h,
    })
}

/// Private: a stack right of or left of the orb, or `None` when its width or height is below
/// `min`.
fn horizontal_bubble(
    side: Side,
    orb: Rect,
    work: Rect,
    full: Size,
    gap: f64,
    min: Size,
) -> Option<Rect> {
    let (edge, room) = if side == Side::Right {
        let edge = (orb.right() + gap).max(work.x);
        (edge, work.right() - edge)
    } else {
        let edge = (orb.x - gap).min(work.right());
        (edge, edge - work.x)
    };
    let w = full.w.min(room).min(work.w);
    let h = full.h.min(work.h);
    if !fits(w, min.w) || !fits(h, min.h) {
        return None;
    }
    let left = if side == Side::Right { edge } else { edge - w };
    let y = clamp_axis(orb.center().1 - h / 2.0, work.y, work.bottom() - h);
    Some(Rect {
        x: clamp_axis(left, work.x, work.right() - w),
        y,
        w,
        h,
    })
}

/// Private: a positive length that reaches `min`.
fn fits(len: f64, min: f64) -> bool {
    len > 0.0 && len + FIT_EPS >= min
}
