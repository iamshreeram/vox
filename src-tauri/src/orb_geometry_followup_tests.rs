//! G3a follow-up spec for three G1 review minors in `crate::orb_geometry`: (a) Above-side
//! hysteresis, (b) restore clamps against the chosen monitor only, (c) unusable monitors are
//! ignored. Authoritative: do not weaken an assertion; if a test is wrong, report it.
use crate::orb_geometry::*;

fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { x, y, w, h }
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

const PANEL: Size = Size { w: 340.0, h: 480.0 };

// ---- (a) place_panel: leaving Above needs 24 logical px of spare space on a horizontal side ----

#[test]
fn above_switches_to_a_horizontal_side_that_fits_with_slack() {
    // Below does not fit. Left has 252 px spare and Right has 396 px spare (room minus the 8 px
    // gap minus the 340 px panel), so the panel leaves Above for Right.
    let work = r(0.0, 0.0, 1440.0, 900.0);
    let orb = r(600.0, 700.0, 96.0, 96.0);
    let p = place_panel(orb, PANEL, work, 1.0, Some(Side::Above));
    assert_eq!(p.side, Side::Right);
}

#[test]
fn above_is_kept_when_the_horizontal_sides_have_less_than_the_slack() {
    // Left has 12 px spare and Right has 7 px spare: both fit, neither has 24 px to spare.
    let work = r(0.0, 0.0, 811.0, 900.0);
    let orb = r(360.0, 700.0, 96.0, 96.0);
    let p = place_panel(orb, PANEL, work, 1.0, Some(Side::Above));
    assert_eq!(p.side, Side::Above);
    // Without a current side the horizontal side is still preferred.
    assert_eq!(place_panel(orb, PANEL, work, 1.0, None).side, Side::Left);
}

// ---- (b) restore_position clamps against the chosen monitor only ----

#[test]
fn restore_with_an_edge_fraction_stays_on_the_chosen_monitor() {
    // Two adjacent monitors: A is 1920x1080 at scale 1, B starts at x = 1920 and has scale 2.
    let a_rect = r(0.0, 0.0, 1920.0, 1080.0);
    let b_rect = r(1920.0, 0.0, 2560.0, 1440.0);
    let monitors = vec![
        mon("Built-in", a_rect, a_rect, 1.0, true),
        mon("DELL", b_rect, b_rect, 2.0, false),
    ];
    let saved = OrbPosition {
        monitor_name: "Built-in".into(),
        monitor_key: monitor_key("Built-in", &a_rect),
        fx: 1.0,
        fy: 0.3,
    };
    let orb = restore_position(&saved, OrbSize::Medium, &monitors).expect("restored");
    // A 96 px orb (A's scale) pushed fully inside A's 1920 px width.
    assert_eq!((orb.x, orb.y, orb.w, orb.h), (1824.0, 276.0, 96.0, 96.0));
    assert!(orb.right() <= 1920.0, "{orb:?}");
}

// ---- (c) monitors with non-finite or empty geometry are ignored ----

#[test]
fn a_nan_or_empty_monitor_never_wins_a_lookup() {
    let good_rect = r(0.0, 0.0, 1000.0, 800.0);
    let good = mon("Good", good_rect, good_rect, 1.0, true);
    let nan_rect = r(f64::NAN, 0.0, 1000.0, 800.0);
    let broken = MonitorInfo {
        name: "Broken".into(),
        key: "Broken|nan".into(),
        rect: nan_rect,
        work: nan_rect,
        scale: 1.0,
        primary: false,
    };
    let empty_rect = r(0.0, 0.0, 0.0, 800.0);
    let empty = mon("Empty", empty_rect, empty_rect, 1.0, false);
    let monitors = vec![broken.clone(), empty, good.clone()];

    // A point far off every monitor goes to the good one, not to the NaN monitor listed first.
    let point = (5000.0, 5000.0);
    assert_eq!(
        monitor_for(point, &monitors).map(|m| m.name.as_str()),
        Some("Good")
    );
    assert_eq!(monitor_for(point, &[broken.clone()]), None);

    let stray = r(5000.0, 5000.0, 96.0, 96.0);
    assert_rect_eq(clamp_orb(stray, &monitors), (884.0, 684.0, 96.0, 96.0));
    assert_eq!(
        position_from_rect(stray, &monitors).map(|p| p.monitor_name),
        Some("Good".to_string())
    );

    // A saved position on the broken monitor falls back to the usable primary.
    let saved = OrbPosition {
        monitor_name: "Broken".into(),
        monitor_key: "Broken|nan".into(),
        fx: 0.5,
        fy: 0.5,
    };
    assert_rect_eq(
        restore_position(&saved, OrbSize::Medium, &monitors).expect("restored"),
        (452.0, 352.0, 96.0, 96.0),
    );
}

fn assert_rect_eq(actual: Rect, expected: (f64, f64, f64, f64)) {
    let (x, y, w, h) = expected;
    assert!(
        (actual.x - x).abs() < 1e-6
            && (actual.y - y).abs() < 1e-6
            && (actual.w - w).abs() < 1e-6
            && (actual.h - h).abs() < 1e-6,
        "expected {expected:?}, got {actual:?}"
    );
}
