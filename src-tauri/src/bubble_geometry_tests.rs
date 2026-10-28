//! G4a spec: bubble placement (DESIGN.md A7-3 and A8-3; the R6-4 placement text is superseded
//! where it conflicts). Authoritative: do not weaken an assertion; if a test is wrong, report it.
//!
//! Contract (module `crate::orb_geometry`, new):
//!   fn place_bubbles(orb: Rect, work: Rect, size: Size, gap: f64, min: Size) -> Option<BubblePlacement>
//!   struct BubblePlacement { rect: Rect, side: Side, tail: f64 }   (Debug, Clone, Copy, PartialEq)
//!   consts MIN_BUBBLE_H_LOGICAL = 120.0, MIN_BUBBLE_W_LOGICAL = 200.0,
//!          BUBBLE_W_LOGICAL = 320.0, BUBBLE_H_LOGICAL = 240.0
//! All numbers are PHYSICAL pixels; callers multiply logical values by the monitor scale, so this
//! function is scale-free. Side order: Above, Below, Right, Left. Above/Below reduce the height
//! (the room along the main axis); Left/Right reduce the width. The cross dimension never goes
//! below `min`. A side needs both dimensions >= min. The rect never overlaps the orb and always
//! lies fully inside `work`. `tail` = orb centre along the bubble edge as a 0..1 fraction.

use crate::orb_geometry::{
    place_bubbles, BubblePlacement, Rect, Side, Size, BUBBLE_H_LOGICAL, BUBBLE_W_LOGICAL,
    MIN_BUBBLE_H_LOGICAL, MIN_BUBBLE_W_LOGICAL,
};

const GAP_LOGICAL: f64 = 8.0;
const EPS: f64 = 1e-6;
const SCALES: [f64; 3] = [1.0, 1.5, 2.0];

fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { x, y, w, h }
}

fn full(s: f64) -> Size {
    Size {
        w: BUBBLE_W_LOGICAL * s,
        h: BUBBLE_H_LOGICAL * s,
    }
}

fn min_size(s: f64) -> Size {
    Size {
        w: MIN_BUBBLE_W_LOGICAL * s,
        h: MIN_BUBBLE_H_LOGICAL * s,
    }
}

/// Placement with the caller's conversion: logical gap, full size and minimum scaled by `s`.
fn place(orb: Rect, work: Rect, s: f64) -> Option<BubblePlacement> {
    place_bubbles(orb, work, full(s), GAP_LOGICAL * s, min_size(s))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < EPS
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x < b.right() - EPS
        && b.x < a.right() - EPS
        && a.y < b.bottom() - EPS
        && b.y < a.bottom() - EPS
}

fn inside(inner: Rect, outer: Rect) -> bool {
    inner.x >= outer.x - EPS
        && inner.y >= outer.y - EPS
        && inner.right() <= outer.right() + EPS
        && inner.bottom() <= outer.bottom() + EPS
}

fn assert_valid(p: &BubblePlacement, orb: Rect, work: Rect, s: f64) {
    assert!(
        inside(p.rect, work),
        "{:?} escapes the work area {:?}",
        p.rect,
        work
    );
    assert!(
        !overlaps(p.rect, orb),
        "{:?} overlaps the orb {:?}",
        p.rect,
        orb
    );
    assert!(
        p.rect.w <= BUBBLE_W_LOGICAL * s + EPS && p.rect.h <= BUBBLE_H_LOGICAL * s + EPS,
        "{:?} is larger than the full size",
        p.rect
    );
    assert!(
        p.rect.w >= MIN_BUBBLE_W_LOGICAL * s - EPS && p.rect.h >= MIN_BUBBLE_H_LOGICAL * s - EPS,
        "{:?} is below the minimum",
        p.rect
    );
    assert!(
        (0.0..=1.0).contains(&p.tail),
        "tail {} outside 0..1",
        p.tail
    );
}

/// Work area in which only Above has room: `room` logical px between the work top and the orb
/// after the gap. Left and right are too narrow and the orb's bottom leaves no room below.
fn above_only(s: f64, room: f64) -> (Rect, Rect) {
    let orb_y = room * s + GAP_LOGICAL * s;
    let work = r(0.0, 0.0, 300.0 * s, orb_y + 96.0 * s);
    let orb = r(100.0 * s, orb_y, 96.0 * s, 96.0 * s);
    (orb, work)
}

/// Work area in which only Left has room: `room` logical px between the work's left edge and the
/// orb after the gap. The orb is as tall as the work (no room above or below) and only 100
/// logical px remain on the right.
fn left_only(s: f64, room: f64) -> (Rect, Rect) {
    let orb_x = room * s + GAP_LOGICAL * s;
    let work = r(0.0, 0.0, orb_x + 96.0 * s + 100.0 * s, 900.0 * s);
    let orb = r(orb_x, 0.0, 96.0 * s, 900.0 * s);
    (orb, work)
}

// ---- constants and basic placement --------------------------------------------------------

#[test]
fn bubble_constants_match_the_spec() {
    assert_eq!(MIN_BUBBLE_H_LOGICAL, 120.0);
    assert_eq!(MIN_BUBBLE_W_LOGICAL, 200.0);
    assert_eq!(BUBBLE_W_LOGICAL, 320.0);
    assert_eq!(BUBBLE_H_LOGICAL, 240.0);
}

#[test]
fn centred_orb_prefers_above_at_full_size() {
    // A7-3: Above is first in the side order and is centred on the orb.
    for s in SCALES {
        let work = r(0.0, 0.0, 1440.0 * s, 900.0 * s);
        let orb = r(600.0 * s, 400.0 * s, 96.0 * s, 96.0 * s);
        let p = place(orb, work, s).expect("room above");
        assert_eq!(p.side, Side::Above, "scale {s}");
        assert!(
            close(p.rect.w, 320.0 * s) && close(p.rect.h, 240.0 * s),
            "full size: {:?}",
            p.rect
        );
        assert!(
            close(p.rect.bottom(), orb.y - GAP_LOGICAL * s),
            "Above bottom edge = orb.top - gap"
        );
        assert!(
            close(p.rect.center().0, orb.center().0),
            "centred horizontally on the orb"
        );
        assert!(
            close(p.tail, 0.5),
            "a centred bubble has the tail in the middle"
        );
        assert_valid(&p, orb, work, s);
    }
}

#[test]
fn orb_at_the_top_gets_below() {
    // A7-3: Above has no room, so Below is used; Below top = orb.bottom + gap.
    for s in SCALES {
        let work = r(0.0, 0.0, 1440.0 * s, 900.0 * s);
        let orb = r(600.0 * s, 0.0, 96.0 * s, 96.0 * s);
        let p = place(orb, work, s).expect("room below");
        assert_eq!(p.side, Side::Below, "scale {s}");
        assert!(
            close(p.rect.y, orb.bottom() + GAP_LOGICAL * s),
            "Below top = orb.bottom + gap"
        );
        assert_valid(&p, orb, work, s);
    }
}

#[test]
fn orb_on_the_right_edge_column_gets_right() {
    // A7-3: Above and Below have no room (tall orb), Left has less than the minimum, Right fits.
    for s in SCALES {
        let work = r(0.0, 0.0, 1440.0 * s, 900.0 * s);
        let orb = r(100.0 * s, 0.0, 96.0 * s, 900.0 * s);
        let p = place(orb, work, s).expect("room right");
        assert_eq!(p.side, Side::Right, "scale {s}");
        assert!(
            close(p.rect.x, orb.right() + GAP_LOGICAL * s),
            "Right left = orb.right + gap"
        );
        assert!(close(p.rect.w, 320.0 * s), "full width when it fits");
        assert_valid(&p, orb, work, s);
    }
}

#[test]
fn orb_on_the_left_edge_column_gets_left() {
    // A7-3: only Left has room with the full size.
    for s in SCALES {
        let work = r(0.0, 0.0, 1440.0 * s, 900.0 * s);
        let orb = r(1244.0 * s, 0.0, 96.0 * s, 900.0 * s);
        let p = place(orb, work, s).expect("room left");
        assert_eq!(p.side, Side::Left, "scale {s}");
        assert!(
            close(p.rect.right(), orb.x - GAP_LOGICAL * s),
            "Left right = orb.left - gap"
        );
        assert!(close(p.rect.w, 320.0 * s));
        assert_valid(&p, orb, work, s);
    }
}

// ---- reduced sizes and the minimum --------------------------------------------------------

#[test]
fn heights_from_96_to_119_logical_have_no_placement_when_height_is_the_only_room() {
    // A8-3: below the 120 logical minimum the side is unavailable -> None.
    for s in SCALES {
        for room in 96..=119 {
            let (orb, work) = above_only(s, room as f64);
            assert!(
                place(orb, work, s).is_none(),
                "scale {s}: room {room} logical must give None"
            );
        }
    }
}

#[test]
fn exactly_120_logical_is_accepted_and_reduces_the_height() {
    // A8-3: the minimum itself is accepted; the bubble is cut to the room.
    for s in SCALES {
        let (orb, work) = above_only(s, 120.0);
        let p = place(orb, work, s).expect("120 logical accepted");
        assert_eq!(p.side, Side::Above, "scale {s}");
        assert!(
            close(p.rect.h, 120.0 * s),
            "height reduced to the room: {:?}",
            p.rect
        );
        assert!(
            close(p.rect.bottom(), orb.y - GAP_LOGICAL * s),
            "bottom = orb.top - gap"
        );
        assert_valid(&p, orb, work, s);
    }
}

#[test]
fn room_between_minimum_and_full_height_is_used_in_full() {
    // A7-3: height = min(full, room) when room >= min.
    for s in SCALES {
        let (orb, work) = above_only(s, 180.0);
        let p = place(orb, work, s).expect("180 logical fits");
        assert!(close(p.rect.h, 180.0 * s), "scale {s}: {:?}", p.rect);
        assert_valid(&p, orb, work, s);

        let (orb, work) = above_only(s, 300.0);
        let p = place(orb, work, s).expect("300 logical fits");
        assert!(close(p.rect.h, 240.0 * s), "full height when room allows");
        assert_valid(&p, orb, work, s);
    }
}

#[test]
fn width_minimum_is_200_logical_and_is_accepted_exactly() {
    // A8-3: the width analogue for Left: 200 logical of room is accepted, 199 is not.
    for s in SCALES {
        let (orb, work) = left_only(s, 200.0);
        let p = place(orb, work, s).expect("200 logical accepted");
        assert_eq!(p.side, Side::Left, "scale {s}");
        assert!(
            close(p.rect.w, 200.0 * s),
            "width reduced to the room: {:?}",
            p.rect
        );
        assert!(close(p.rect.right(), orb.x - GAP_LOGICAL * s));
        assert_valid(&p, orb, work, s);

        let (orb, work) = left_only(s, 199.0);
        assert!(
            place(orb, work, s).is_none(),
            "scale {s}: 199 logical must give None"
        );
    }
}

#[test]
fn tiny_work_areas_return_none() {
    // A36: total function; no side can reach the minimum.
    for s in SCALES {
        let orb = r(20.0 * s, 20.0 * s, 40.0 * s, 40.0 * s);
        assert!(
            place(orb, r(0.0, 0.0, 150.0 * s, 100.0 * s), s).is_none(),
            "scale {s}: 150x100"
        );
        assert!(
            place(
                r(2.0 * s, 2.0 * s, 4.0 * s, 4.0 * s),
                r(0.0, 0.0, 10.0 * s, 10.0 * s),
                s
            )
            .is_none(),
            "scale {s}: 10x10"
        );
        assert!(place(r(0.0, 0.0, 96.0 * s, 96.0 * s), r(0.0, 0.0, 0.0, 0.0), s).is_none());
    }
}

#[test]
fn narrow_but_tall_work_is_none_because_the_cross_dimension_cannot_reach_the_minimum() {
    // A8-3: Above/Below need 200 logical width; Left/Right have too little room beside the orb.
    for s in SCALES {
        let work = r(0.0, 0.0, 150.0 * s, 2000.0 * s);
        let orb = r(27.0 * s, 900.0 * s, 96.0 * s, 96.0 * s);
        assert!(place(orb, work, s).is_none(), "scale {s}");
    }
}

// ---- clamping, gap, determinism -----------------------------------------------------------

#[test]
fn horizontal_position_is_centred_then_clamped_into_the_work_area() {
    // A7-3: centred on the orb, then clamped; tail follows the orb centre.
    for s in SCALES {
        let work = r(0.0, 0.0, 1440.0 * s, 900.0 * s);

        let orb = r(0.0, 400.0 * s, 96.0 * s, 96.0 * s);
        let p = place(orb, work, s).expect("room above");
        assert_eq!(p.side, Side::Above);
        assert!(
            close(p.rect.x, 0.0),
            "clamped to the left edge: {:?}",
            p.rect
        );
        assert!(
            close(p.tail, 48.0 / 320.0),
            "tail at the orb centre: {}",
            p.tail
        );
        assert_valid(&p, orb, work, s);

        let orb = r(1344.0 * s, 400.0 * s, 96.0 * s, 96.0 * s);
        let p = place(orb, work, s).expect("room above");
        assert!(
            close(p.rect.right(), 1440.0 * s),
            "clamped to the right edge: {:?}",
            p.rect
        );
        assert!(close(p.tail, (1392.0 - 1120.0) / 320.0), "tail: {}", p.tail);
        assert_valid(&p, orb, work, s);
    }
}

#[test]
fn the_caller_gap_is_used_as_given() {
    // A7-3: the gap is a physical value passed by the caller.
    for s in SCALES {
        let work = r(0.0, 0.0, 1440.0 * s, 900.0 * s);
        let orb = r(600.0 * s, 400.0 * s, 96.0 * s, 96.0 * s);
        let p = place_bubbles(orb, work, full(s), 20.0 * s, min_size(s)).expect("room above");
        assert_eq!(p.side, Side::Above);
        assert!(
            close(p.rect.bottom(), orb.y - 20.0 * s),
            "gap 20 used as given"
        );
    }
}

#[test]
fn placement_is_deterministic() {
    let s = 1.5;
    let work = r(-1440.0 * s, -900.0 * s, 1440.0 * s, 900.0 * s);
    let orb = r(-768.0 * s, -498.0 * s, 96.0 * s, 96.0 * s);
    let a = place(orb, work, s).expect("room");
    let b = place(orb, work, s).expect("room");
    assert_eq!(a.side, b.side);
    assert_eq!(a.rect, b.rect);
    assert!(close(a.tail, b.tail));
}

#[test]
fn negative_work_origins_are_respected() {
    // A36: work areas may have negative origins (secondary monitors left of or above the main).
    for s in SCALES {
        let work = r(-1440.0 * s, -900.0 * s, 1440.0 * s, 900.0 * s);
        let orb = r(-768.0 * s, -498.0 * s, 96.0 * s, 96.0 * s);
        let p = place(orb, work, s).expect("room above");
        assert_eq!(p.side, Side::Above, "scale {s}");
        assert!(
            close(p.rect.center().0, orb.center().0),
            "centred on the negative origin orb"
        );
        assert_valid(&p, orb, work, s);
    }
}

#[test]
fn every_orb_position_on_a_grid_gets_a_valid_placement() {
    // A36 property sweep: corners, edges and centre of a 1440x900 logical work area, at scales
    // 1, 1.5 and 2, with a zero origin and a negative origin. A 96 logical orb always leaves room
    // on some side, so every result must be Some and valid.
    for s in SCALES {
        for (ox, oy) in [(0.0, 0.0), (-1440.0 * s, -900.0 * s)] {
            let work = r(ox, oy, 1440.0 * s, 900.0 * s);
            let ow = 96.0 * s;
            let xs = [work.x, work.x + (work.w - ow) / 2.0, work.right() - ow];
            let ys = [work.y, work.y + (work.h - ow) / 2.0, work.bottom() - ow];
            for x in xs {
                for y in ys {
                    let orb = r(x, y, ow, ow);
                    let p = place(orb, work, s).unwrap_or_else(|| {
                        panic!("scale {s}: orb {orb:?} in {work:?} has no room")
                    });
                    assert_valid(&p, orb, work, s);
                }
            }
        }
    }
}
