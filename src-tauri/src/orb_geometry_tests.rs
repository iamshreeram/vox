//! G1 spec: orb and panel geometry (DESIGN.md Rev 4: 'Coordinates', 'Surface state machine and
//! placement'). Authoritative: do not weaken an assertion; if a test is wrong, report it.
//! All geometry is in PHYSICAL pixels in one global desktop space. Pure: no Tauri types.
//!
//! Contract (module `crate::orb_geometry`):
//!   struct Rect { x, y, w, h: f64 } (Debug, Clone, Copy, PartialEq) + fn right(), bottom(), center() -> (f64, f64)
//!   struct Size { w, h: f64 }                                         (Debug, Clone, Copy, PartialEq)
//!   struct MonitorInfo { name: String, key: String, rect: Rect, work: Rect, scale: f64, primary: bool } (Debug, Clone, PartialEq)
//!   enum OrbSize { Small, Medium, Large }  (Debug, Clone, Copy, PartialEq, Eq, Default = Medium, serde snake_case, specta Type)
//!   struct OrbPosition { monitor_name: String, monitor_key: String, fx: f64, fy: f64 } (Debug, Clone, PartialEq, serde, specta Type)
//!   const SLOP_LOGICAL_PX: f64 = 4.0
//!   enum DragClass { Click, Drag }
//!   enum Side { Below, Above, Left, Right }                           (Debug, Clone, Copy, PartialEq, Eq)
//!   struct Placement { side: Side, rect: Rect, tail_offset: f64 }
//!   fn monitor_key(name: &str, rect: &Rect) -> String                 // "name|x|y|w|h" with integer-rounded numbers
//!   fn monitor_for(point: (f64, f64), monitors: &[MonitorInfo]) -> Option<&MonitorInfo>  // containing rect, else nearest by distance to rect, None if empty
//!   fn orb_logical_px(OrbSize) -> f64 (72 / 96 / 128);  fn orb_px(OrbSize, scale: f64) -> f64 (logical x scale, rounded)
//!   fn classify_drag(moved_physical_px: f64, scale: f64) -> DragClass // moved <= 4 x scale is a Click
//!   fn clamp_orb(orb: Rect, monitors: &[MonitorInfo]) -> Rect         // >= half of the orb area inside some work area keeps it; else the work-area corner
//!                                                                     // nearest to the orb centre, inset 20 x scale, of the nearest monitor; no monitors: unchanged
//!   fn position_from_rect(orb: Rect, monitors: &[MonitorInfo]) -> Option<OrbPosition> // monitor containing the orb centre; fx, fy = centre fraction of that work area, clamped to [0, 1]
//!   fn restore_position(saved: &OrbPosition, size: OrbSize, monitors: &[MonitorInfo]) -> Option<Rect> // key match, else name match, else primary monitor (else first); orb sized orb_px(size, that monitor's scale); then clamp_orb. None when there are no monitors
//!   fn panel_size(work: Rect, requested_h_logical: f64, scale: f64) -> Size
//!   fn place_panel(orb: Rect, size: Size, work: Rect, scale: f64, current: Option<Side>) -> Placement
//! Constants: panel width 340 logical, margin 12 logical, min height 160 logical, gap 8 logical,
//! tail inset 24 logical, hysteresis slack 24 logical.
use crate::orb_geometry::*;

fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { x, y, w, h }
}

fn assert_rect(actual: Rect, expected: (f64, f64, f64, f64)) {
    let (x, y, w, h) = expected;
    let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
    assert!(
        close(actual.x, x) && close(actual.y, y) && close(actual.w, w) && close(actual.h, h),
        "expected {expected:?}, got {actual:?}"
    );
}

fn mon(name: &str, rect: Rect, work: Rect, scale: f64, primary: bool) -> MonitorInfo {
    MonitorInfo {
        name: name.to_string(),
        key: monitor_key(name, &rect),
        rect,
        work,
        scale,
        primary,
    }
}

fn work() -> Rect {
    r(0.0, 0.0, 1440.0, 900.0)
}

const PANEL: Size = Size { w: 340.0, h: 480.0 };

fn orb_at(x: f64, y: f64) -> Rect {
    r(x, y, 96.0, 96.0)
}

// ---- place_panel: side priority Below, horizontal with more room, Above ----

#[test]
fn below_is_used_when_it_fits_and_is_centred_on_the_orb() {
    let p = place_panel(orb_at(600.0, 100.0), PANEL, work(), 1.0, None);
    assert_eq!(p.side, Side::Below);
    assert_rect(p.rect, (478.0, 204.0, 340.0, 480.0));
    assert!((p.tail_offset - 170.0).abs() < 1e-6, "tail {}", p.tail_offset);
}

#[test]
fn near_the_bottom_a_horizontal_side_that_fits_wins_over_above() {
    let p = place_panel(orb_at(600.0, 700.0), PANEL, work(), 1.0, None);
    assert_eq!(p.side, Side::Right, "right has more room (744 vs 600)");
    assert_rect(p.rect, (704.0, 420.0, 340.0, 480.0));
    assert!((p.tail_offset - 328.0).abs() < 1e-6, "tail {}", p.tail_offset);
}

#[test]
fn only_above_is_used_when_no_other_side_fits() {
    let narrow = r(0.0, 0.0, 420.0, 900.0);
    let p = place_panel(orb_at(160.0, 700.0), PANEL, narrow, 1.0, None);
    assert_eq!(p.side, Side::Above);
    assert_rect(p.rect, (38.0, 212.0, 340.0, 480.0));
    assert!((p.tail_offset - 170.0).abs() < 1e-6);
}

#[test]
fn left_edge_goes_right_and_right_edge_goes_left() {
    let p = place_panel(orb_at(20.0, 500.0), PANEL, work(), 1.0, None);
    assert_eq!(p.side, Side::Right);
    assert_rect(p.rect, (124.0, 420.0, 340.0, 480.0));

    let p = place_panel(orb_at(1330.0, 500.0), PANEL, work(), 1.0, None);
    assert_eq!(p.side, Side::Left);
    assert_rect(p.rect, (982.0, 420.0, 340.0, 480.0));
}

#[test]
fn bottom_right_corner_goes_left() {
    let p = place_panel(orb_at(1344.0, 804.0), PANEL, work(), 1.0, None);
    assert_eq!(p.side, Side::Left);
    assert_rect(p.rect, (996.0, 420.0, 340.0, 480.0));
}

#[test]
fn equal_room_on_both_sides_prefers_right() {
    let p = place_panel(orb_at(672.0, 700.0), PANEL, work(), 1.0, None);
    assert_eq!(p.side, Side::Right);
}

#[test]
fn hysteresis_keeps_the_current_side_until_the_better_one_fits_with_slack() {
    // Below fits with only 10 px to spare: not enough slack to leave Right.
    let tight = orb_at(600.0, 306.0);
    let kept = place_panel(tight, PANEL, work(), 1.0, Some(Side::Right));
    assert_eq!(kept.side, Side::Right);
    // Without a current side the priority order applies.
    assert_eq!(place_panel(tight, PANEL, work(), 1.0, None).side, Side::Below);
    // With 66 px to spare Below wins again.
    let roomy = orb_at(600.0, 250.0);
    assert_eq!(
        place_panel(roomy, PANEL, work(), 1.0, Some(Side::Right)).side,
        Side::Below
    );
    // A current side that no longer fits is abandoned regardless of slack.
    let p = place_panel(orb_at(1330.0, 500.0), PANEL, work(), 1.0, Some(Side::Right));
    assert_eq!(p.side, Side::Left);
}

#[test]
fn the_panel_is_always_inside_the_work_area_for_every_orb_position() {
    let sizes = [PANEL, Size { w: 340.0, h: 160.0 }, Size { w: 276.0, h: 76.0 }];
    for size in sizes {
        let area = if size.w < 300.0 { r(0.0, 0.0, 300.0, 100.0) } else { work() };
        let mut y = area.y - 40.0;
        while y <= area.bottom() + 40.0 {
            let mut x = area.x - 40.0;
            while x <= area.right() + 40.0 {
                for current in [None, Some(Side::Below), Some(Side::Left)] {
                    let p = place_panel(orb_at(x, y), size, area, 1.0, current);
                    assert!(p.rect.x >= area.x - 1e-6 && p.rect.y >= area.y - 1e-6);
                    assert!(p.rect.right() <= area.right() + 1e-6, "{x},{y} {p:?}");
                    assert!(p.rect.bottom() <= area.bottom() + 1e-6, "{x},{y} {p:?}");
                    assert!(p.tail_offset.is_finite() && p.tail_offset >= 0.0);
                }
                x += 97.0;
            }
            y += 83.0;
        }
    }
}

#[test]
fn the_tail_stays_inside_the_panel_corners() {
    // Orb far to the left of a panel clamped to the work edge: the tail cannot leave the panel.
    let p = place_panel(orb_at(0.0, 100.0), PANEL, work(), 1.0, None);
    assert_eq!(p.side, Side::Below);
    assert!(p.tail_offset >= 24.0 - 1e-6 && p.tail_offset <= PANEL.w - 24.0 + 1e-6);
}

#[test]
fn placement_scales_with_the_monitor() {
    let w2 = r(0.0, 0.0, 2880.0, 1800.0);
    let size = Size { w: 680.0, h: 960.0 };
    let p = place_panel(r(1200.0, 200.0, 192.0, 192.0), size, w2, 2.0, None);
    assert_eq!(p.side, Side::Below);
    // top = 200 + 192 + gap (8 x 2) = 408
    assert!((p.rect.y - 408.0).abs() < 1e-6, "{p:?}");
}

// ---- panel_size ----

#[test]
fn panel_size_follows_the_request_within_bounds() {
    let s = panel_size(work(), 480.0, 1.0);
    assert_eq!((s.w, s.h), (340.0, 480.0));
    assert_eq!(panel_size(work(), 2000.0, 1.0).h, 876.0, "work height minus 2 margins");
    assert_eq!(panel_size(work(), 50.0, 1.0).h, 160.0, "minimum height");
}

#[test]
fn panel_size_in_tiny_work_areas_is_finite_non_negative_and_contained() {
    let tiny = r(0.0, 0.0, 300.0, 100.0);
    let s = panel_size(tiny, 480.0, 1.0);
    assert_eq!((s.w, s.h), (276.0, 76.0));
    let smaller = r(0.0, 0.0, 20.0, 20.0);
    let s = panel_size(smaller, 480.0, 1.0);
    assert_eq!((s.w, s.h), (20.0, 20.0), "smaller than the margins: the work size itself");
    for area in [tiny, smaller, r(0.0, 0.0, 0.0, 0.0), work()] {
        for req in [0.0, 1.0, 480.0, 1e9] {
            let s = panel_size(area, req, 1.0);
            assert!(s.w.is_finite() && s.h.is_finite() && s.w >= 0.0 && s.h >= 0.0);
            assert!(s.w <= area.w + 1e-6 && s.h <= area.h + 1e-6);
        }
    }
}

#[test]
fn panel_size_uses_physical_pixels_on_a_scaled_monitor() {
    let w2 = r(0.0, 0.0, 2880.0, 1800.0);
    let s = panel_size(w2, 480.0, 2.0);
    assert_eq!((s.w, s.h), (680.0, 960.0));
}

// ---- monitors, clamp, restore ----

fn dual() -> Vec<MonitorInfo> {
    vec![
        mon("Built-in", r(0.0, 0.0, 1920.0, 1080.0), r(0.0, 25.0, 1920.0, 1055.0), 1.0, true),
        mon("DELL", r(1920.0, 0.0, 2560.0, 1440.0), r(1920.0, 0.0, 2560.0, 1440.0), 2.0, false),
        mon("Left", r(-1280.0, 0.0, 1280.0, 1024.0), r(-1280.0, 0.0, 1280.0, 1024.0), 1.0, false),
    ]
}

#[test]
fn monitor_key_is_stable_and_rounded() {
    assert_eq!(monitor_key("DELL", &r(1920.0, 0.0, 2560.0, 1440.0)), "DELL|1920|0|2560|1440");
}

#[test]
fn monitor_for_picks_the_containing_monitor_else_the_nearest() {
    let m = dual();
    assert_eq!(monitor_for((2000.0, 100.0), &m).map(|x| x.name.as_str()), Some("DELL"));
    assert_eq!(monitor_for((-5.0, 5.0), &m).map(|x| x.name.as_str()), Some("Left"));
    assert_eq!(monitor_for((6000.0, 100.0), &m).map(|x| x.name.as_str()), Some("DELL"));
    assert!(monitor_for((0.0, 0.0), &[]).is_none());
}

#[test]
fn orb_sizes_scale() {
    assert_eq!(orb_logical_px(OrbSize::Small), 72.0);
    assert_eq!(orb_logical_px(OrbSize::Medium), 96.0);
    assert_eq!(orb_logical_px(OrbSize::Large), 128.0);
    assert_eq!(orb_px(OrbSize::Medium, 2.0), 192.0);
    assert_eq!(orb_px(OrbSize::Small, 1.5), 108.0);
    assert_eq!(OrbSize::default(), OrbSize::Medium);
}

#[test]
fn drag_slop_boundaries() {
    assert_eq!(classify_drag(0.0, 1.0), DragClass::Click);
    assert_eq!(classify_drag(4.0, 1.0), DragClass::Click);
    assert_eq!(classify_drag(5.0, 1.0), DragClass::Drag);
    assert_eq!(classify_drag(8.0, 2.0), DragClass::Click);
    assert_eq!(classify_drag(9.0, 2.0), DragClass::Drag);
}

#[test]
fn clamp_keeps_a_half_visible_orb_and_moves_a_stray_one_to_the_nearest_corner() {
    let m = vec![dual().remove(0)];
    let half = orb_at(1872.0, 500.0);
    assert_rect(clamp_orb(half, &m), (1872.0, 500.0, 96.0, 96.0));
    // Slightly less than half visible: nearest work corner (top-right here) inset by 20.
    assert_rect(clamp_orb(orb_at(1873.0, 500.0), &m), (1804.0, 45.0, 96.0, 96.0));
    // Far away: bottom-right corner inset 20.
    assert_rect(clamp_orb(orb_at(5000.0, 5000.0), &m), (1804.0, 964.0, 96.0, 96.0));
    // No monitors: unchanged.
    assert_rect(clamp_orb(orb_at(5000.0, 5000.0), &[]), (5000.0, 5000.0, 96.0, 96.0));
}

#[test]
fn an_orb_on_a_negative_origin_monitor_is_left_alone() {
    let m = dual();
    assert_rect(clamp_orb(orb_at(-1200.0, 100.0), &m), (-1200.0, 100.0, 96.0, 96.0));
}

#[test]
fn position_round_trips_on_a_scaled_secondary_monitor() {
    let m = dual();
    let orb = r(3104.0, 624.0, 192.0, 192.0);
    let pos = position_from_rect(orb, &m).expect("monitor found");
    assert_eq!(pos.monitor_name, "DELL");
    assert_eq!(pos.monitor_key, "DELL|1920|0|2560|1440");
    assert!((pos.fx - 0.5).abs() < 1e-9 && (pos.fy - 0.5).abs() < 1e-9, "{pos:?}");
    let back = restore_position(&pos, OrbSize::Medium, &m).expect("restored");
    assert_rect(back, (3104.0, 624.0, 192.0, 192.0));
}

#[test]
fn restore_uses_the_name_when_the_key_changed() {
    let pos = OrbPosition {
        monitor_name: "DELL".into(),
        monitor_key: "DELL|1920|0|2560|1440".into(),
        fx: 0.5,
        fy: 0.5,
    };
    let after_resolution_change = vec![
        mon("Built-in", r(0.0, 0.0, 1920.0, 1080.0), r(0.0, 0.0, 1920.0, 1080.0), 1.0, true),
        mon("DELL", r(1920.0, 0.0, 1920.0, 1080.0), r(1920.0, 0.0, 1920.0, 1080.0), 1.0, false),
    ];
    let back = restore_position(&pos, OrbSize::Medium, &after_resolution_change).expect("restored");
    assert_rect(back, (2832.0, 492.0, 96.0, 96.0));
}

#[test]
fn restore_falls_back_to_the_primary_monitor_when_the_saved_one_is_gone() {
    let pos = OrbPosition {
        monitor_name: "DELL".into(),
        monitor_key: "DELL|1920|0|2560|1440".into(),
        fx: 0.5,
        fy: 0.5,
    };
    let only_primary = vec![mon(
        "Built-in",
        r(0.0, 0.0, 1920.0, 1080.0),
        r(0.0, 0.0, 1920.0, 1080.0),
        1.0,
        true,
    )];
    assert_rect(
        restore_position(&pos, OrbSize::Medium, &only_primary).expect("restored"),
        (912.0, 492.0, 96.0, 96.0),
    );
    assert!(restore_position(&pos, OrbSize::Medium, &[]).is_none());
}

#[test]
fn restore_always_clamps() {
    let pos = OrbPosition {
        monitor_name: "Built-in".into(),
        monitor_key: "Built-in|0|0|1920|1080".into(),
        fx: 1.0,
        fy: 1.0,
    };
    let m = vec![mon("Built-in", r(0.0, 0.0, 1920.0, 1080.0), r(0.0, 0.0, 1920.0, 1080.0), 1.0, true)];
    assert_rect(
        restore_position(&pos, OrbSize::Medium, &m).expect("restored"),
        (1804.0, 964.0, 96.0, 96.0),
    );
}

#[test]
fn only_the_declared_position_fields_are_serialised() {
    let pos = OrbPosition {
        monitor_name: "a".into(),
        monitor_key: "b".into(),
        fx: 0.25,
        fy: 0.75,
    };
    let value = serde_json::to_value(&pos).unwrap();
    let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["fx", "fy", "monitor_key", "monitor_name"]);
}
