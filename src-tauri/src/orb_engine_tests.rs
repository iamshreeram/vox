//! G3a spec: the pure orb engines (DESIGN.md Rev 5: 'Gesture engine owned by Rust', gaze
//! scheduler, window spec). Authoritative: do not weaken an assertion; if a test is wrong, report it.
//!
//! Contract:
//! crate::orb_gesture:
//!   const WATCHDOG_MS: u64 = 60_000
//!   enum GestureResult { Click, Drag, Cancel }                                     (Debug, Clone, Copy, PartialEq, Eq)
//!   enum GestureEvent { PointerDown { id: u64, cursor: (f64, f64), orb: Rect, scale: f64, now_ms: u64 },
//!                       Tick { cursor: (f64, f64), button_down: bool, now_ms: u64 }, Cancel }
//!   enum GestureAction { MoveTo { x: f64, y: f64 }, Finish { id: u64, result: GestureResult, final_orb: Rect } }  (Debug, Clone, PartialEq)
//!   struct GestureEngine; GestureEngine::new(); .on_event(GestureEvent) -> Vec<GestureAction>; .is_active() -> bool; .active_id() -> Option<u64>
//!   Rules: PointerDown arms (grab offset = cursor - orb origin). Tick with the button up ends the gesture:
//!   Click while the cursor never left the slop radius (4 logical px x scale, measured from the press point), else Drag.
//!   Once the slop is exceeded the gesture is a Drag for good and every Tick with the button down emits MoveTo(cursor - grab).
//!   A PointerDown while active first Finishes the old gesture as Cancel (final_orb = last orb position), then arms the new id;
//!   the same id again is ignored. Cancel and the watchdog (now_ms - press time > WATCHDOG_MS) Finish as Cancel. Idle Ticks do nothing.
//! crate::orb_gaze:
//!   struct Gaze { x: f64, y: f64 } (Debug, Clone, Copy, PartialEq)
//!   struct GazeScheduler; ::new(); .set_visible(bool); .set_reduced_motion(bool);
//!   .poll_interval_ms(now_ms: u64) -> Option<u64>   // None when hidden or reduced motion; 33 while the cursor moved within the last 500 ms, else 500
//!   .observe(now_ms: u64, cursor: (f64, f64), orb_center: (f64, f64), reach_px: f64) -> Option<Gaze>
//!       // vector = clamp((cursor - centre) / reach, -1, 1), quantised to 1/16; Some only when the quantised value changed
//!       // since the last Some (or since the orb was last hidden); None when hidden or reduced motion
//! crate::orb_window (pure part): struct OrbWindowSpec { label: &'static str, logical_px: f64, transparent: bool, decorations: bool,
//!   always_on_top: bool, skip_taskbar: bool, resizable: bool, shadow: bool, focusable: bool, visible_on_all_workspaces: bool }
//!   fn orb_window_spec(size: OrbSize) -> OrbWindowSpec
use crate::orb_gaze::*;
use crate::orb_geometry::{OrbSize, Rect};
use crate::orb_gesture::*;
use crate::orb_window::*;

fn orb() -> Rect {
    Rect {
        x: 100.0,
        y: 200.0,
        w: 96.0,
        h: 96.0,
    }
}

fn down(id: u64, cursor: (f64, f64), scale: f64, now_ms: u64) -> GestureEvent {
    GestureEvent::PointerDown {
        id,
        cursor,
        orb: orb(),
        scale,
        now_ms,
    }
}

fn tick(cursor: (f64, f64), button_down: bool, now_ms: u64) -> GestureEvent {
    GestureEvent::Tick {
        cursor,
        button_down,
        now_ms,
    }
}

fn finish_of(actions: &[GestureAction]) -> Option<(u64, GestureResult, Rect)> {
    actions.iter().find_map(|a| match a {
        GestureAction::Finish {
            id,
            result,
            final_orb,
        } => Some((*id, *result, *final_orb)),
        _ => None,
    })
}

fn moves_of(actions: &[GestureAction]) -> Vec<(f64, f64)> {
    actions
        .iter()
        .filter_map(|a| match a {
            GestureAction::MoveTo { x, y } => Some((*x, *y)),
            _ => None,
        })
        .collect()
}

// ---- gesture engine ----

#[test]
fn a_still_press_and_release_is_a_click_and_never_moves_the_orb() {
    let mut e = GestureEngine::new();
    assert!(!e.is_active());
    assert!(e.on_event(down(1, (130.0, 230.0), 1.0, 0)).is_empty());
    assert!(e.is_active());
    assert_eq!(e.active_id(), Some(1));
    let a = e.on_event(tick((130.0, 230.0), true, 16));
    assert!(a.is_empty());
    let a = e.on_event(tick((130.0, 230.0), false, 32));
    let (id, result, final_orb) = finish_of(&a).expect("finished");
    assert_eq!((id, result), (1, GestureResult::Click));
    assert_eq!(final_orb, orb());
    assert!(!e.is_active());
}

#[test]
fn jitter_inside_the_slop_stays_a_click() {
    let mut e = GestureEngine::new();
    e.on_event(down(1, (130.0, 230.0), 1.0, 0));
    for (i, c) in [(131.0, 230.0), (133.0, 231.0), (134.0, 230.0), (130.0, 230.0)]
        .into_iter()
        .enumerate()
    {
        assert!(moves_of(&e.on_event(tick(c, true, 16 * (i as u64 + 1)))).is_empty());
    }
    let a = e.on_event(tick((130.0, 230.0), false, 100));
    assert_eq!(finish_of(&a).map(|f| f.1), Some(GestureResult::Click));
}

#[test]
fn slop_scales_with_the_monitor() {
    let mut e = GestureEngine::new();
    e.on_event(down(1, (130.0, 230.0), 2.0, 0));
    assert!(moves_of(&e.on_event(tick((138.0, 230.0), true, 16))).is_empty(), "8 px at scale 2 is inside");
    let a = e.on_event(tick((139.0, 230.0), true, 32));
    assert_eq!(moves_of(&a).len(), 1, "9 px at scale 2 starts the drag");

    let mut e = GestureEngine::new();
    e.on_event(down(1, (130.0, 230.0), 1.0, 0));
    assert!(moves_of(&e.on_event(tick((134.0, 230.0), true, 16))).is_empty());
    assert_eq!(moves_of(&e.on_event(tick((135.0, 230.0), true, 32))).len(), 1);
}

#[test]
fn a_drag_keeps_the_grab_offset_and_ends_as_a_drag_at_the_last_position() {
    let mut e = GestureEngine::new();
    // Grab 30 px right and 30 px below the orb origin (100, 200).
    e.on_event(down(7, (130.0, 230.0), 1.0, 0));
    let a = e.on_event(tick((330.0, 430.0), true, 16));
    assert_eq!(moves_of(&a), vec![(300.0, 400.0)]);
    let a = e.on_event(tick((500.0, 100.0), true, 32));
    assert_eq!(moves_of(&a), vec![(470.0, 70.0)]);
    // Released far outside where the window was when the press started.
    let a = e.on_event(tick((510.0, 90.0), false, 48));
    let (id, result, final_orb) = finish_of(&a).expect("finished");
    assert_eq!((id, result), (7, GestureResult::Drag));
    assert_eq!((final_orb.x, final_orb.y, final_orb.w, final_orb.h), (470.0, 70.0, 96.0, 96.0));
}

#[test]
fn once_dragging_a_return_to_the_start_is_still_a_drag() {
    let mut e = GestureEngine::new();
    e.on_event(down(1, (130.0, 230.0), 1.0, 0));
    e.on_event(tick((300.0, 230.0), true, 16));
    e.on_event(tick((131.0, 230.0), true, 32));
    let a = e.on_event(tick((131.0, 230.0), false, 48));
    assert_eq!(finish_of(&a).map(|f| f.1), Some(GestureResult::Drag));
}

#[test]
fn a_lost_release_ends_the_gesture_on_the_first_tick_that_sees_it() {
    let mut e = GestureEngine::new();
    e.on_event(down(1, (130.0, 230.0), 1.0, 0));
    e.on_event(tick((400.0, 400.0), true, 16));
    // The button is already up and the cursor is far away: one tick is enough.
    let a = e.on_event(tick((900.0, 700.0), false, 32));
    assert_eq!(finish_of(&a).map(|f| f.1), Some(GestureResult::Drag));
    assert!(!e.is_active());
    assert!(e.on_event(tick((901.0, 700.0), true, 48)).is_empty(), "later ticks do nothing");
}

#[test]
fn cancel_finishes_as_cancel_and_never_as_a_click() {
    let mut e = GestureEngine::new();
    e.on_event(down(1, (130.0, 230.0), 1.0, 0));
    let a = e.on_event(GestureEvent::Cancel);
    assert_eq!(finish_of(&a).map(|f| (f.0, f.1)), Some((1, GestureResult::Cancel)));
    // A cancel while dragging keeps the last position.
    let mut g = GestureEngine::new();
    g.on_event(down(2, (130.0, 230.0), 1.0, 0));
    g.on_event(tick((330.0, 430.0), true, 16));
    let a = g.on_event(GestureEvent::Cancel);
    let (_, result, final_orb) = finish_of(&a).expect("finished");
    assert_eq!(result, GestureResult::Cancel);
    assert_eq!((final_orb.x, final_orb.y), (300.0, 400.0));
    assert!(g.on_event(GestureEvent::Cancel).is_empty(), "cancel while idle does nothing");
}

#[test]
fn a_new_press_cancels_the_old_gesture_first_and_ids_stay_independent() {
    let mut e = GestureEngine::new();
    e.on_event(down(1, (130.0, 230.0), 1.0, 0));
    e.on_event(tick((300.0, 300.0), true, 16));
    let a = e.on_event(down(2, (310.0, 310.0), 1.0, 20));
    let (id, result, _) = finish_of(&a).expect("old gesture finished");
    assert_eq!((id, result), (1, GestureResult::Cancel));
    assert_eq!(e.active_id(), Some(2));
    let a = e.on_event(tick((310.0, 310.0), false, 36));
    assert_eq!(finish_of(&a).map(|f| (f.0, f.1)), Some((2, GestureResult::Click)));
}

#[test]
fn a_duplicate_press_with_the_same_id_is_ignored() {
    let mut e = GestureEngine::new();
    e.on_event(down(5, (130.0, 230.0), 1.0, 0));
    assert!(e.on_event(down(5, (140.0, 240.0), 1.0, 5)).is_empty());
    assert_eq!(e.active_id(), Some(5));
    let a = e.on_event(tick((130.0, 230.0), false, 20));
    assert_eq!(finish_of(&a).map(|f| f.1), Some(GestureResult::Click));
}

#[test]
fn the_watchdog_ends_a_stuck_gesture_as_a_cancel() {
    let mut e = GestureEngine::new();
    e.on_event(down(1, (130.0, 230.0), 1.0, 1_000));
    assert!(e.on_event(tick((130.0, 230.0), true, 1_000 + WATCHDOG_MS)).is_empty(), "exactly at the limit is still alive");
    let a = e.on_event(tick((130.0, 230.0), true, 1_000 + WATCHDOG_MS + 1));
    assert_eq!(finish_of(&a).map(|f| f.1), Some(GestureResult::Cancel));
    assert!(!e.is_active());
}

#[test]
fn idle_ticks_do_nothing() {
    let mut e = GestureEngine::new();
    assert!(e.on_event(tick((1.0, 1.0), true, 0)).is_empty());
    assert!(e.on_event(tick((1.0, 1.0), false, 16)).is_empty());
}

#[test]
fn a_press_whose_button_is_already_up_resolves_as_a_click_on_the_first_tick() {
    let mut e = GestureEngine::new();
    e.on_event(down(3, (130.0, 230.0), 1.0, 0));
    let a = e.on_event(tick((130.0, 230.0), false, 10));
    assert_eq!(finish_of(&a).map(|f| f.1), Some(GestureResult::Click));
}

// ---- gaze scheduler ----

fn gaze() -> GazeScheduler {
    let mut g = GazeScheduler::new();
    g.set_visible(true);
    g
}

const C: (f64, f64) = (500.0, 500.0);

#[test]
fn nothing_is_polled_or_emitted_while_hidden_or_in_reduced_motion() {
    let mut g = GazeScheduler::new();
    assert_eq!(g.poll_interval_ms(0), None, "hidden by default");
    assert_eq!(g.observe(0, (900.0, 500.0), C, 400.0), None);
    g.set_visible(true);
    assert!(g.poll_interval_ms(0).is_some());
    g.set_reduced_motion(true);
    assert_eq!(g.poll_interval_ms(0), None);
    assert_eq!(g.observe(10, (900.0, 500.0), C, 400.0), None);
    g.set_reduced_motion(false);
    g.set_visible(false);
    assert_eq!(g.poll_interval_ms(20), None);
}

#[test]
fn the_cursor_vector_is_clamped_and_quantised() {
    let mut g = gaze();
    let v = g.observe(0, (900.0, 500.0), C, 400.0).expect("first value");
    assert_eq!((v.x, v.y), (1.0, 0.0));
    let v = g.observe(10, (5000.0, -5000.0), C, 400.0);
    assert_eq!(v, Some(Gaze { x: 1.0, y: -1.0 }), "y changed, x clamped");
    let mut g = gaze();
    let v = g.observe(0, (500.0 + 100.0, 500.0), C, 400.0).expect("value"); // 0.25
    assert_eq!(v.x, 0.25);
    let mut g = gaze();
    // 0.51 quantises to 8/16 = 0.5
    let v = g.observe(0, (500.0 + 204.0, 500.0), C, 400.0).expect("value");
    assert_eq!(v.x, 0.5);
    let mut g = gaze();
    assert_eq!(g.observe(0, C, C, 400.0), Some(Gaze { x: 0.0, y: 0.0 }));
}

#[test]
fn only_a_changed_quantised_value_is_emitted() {
    let mut g = gaze();
    assert!(g.observe(0, (700.0, 500.0), C, 400.0).is_some());
    assert_eq!(g.observe(33, (700.0, 500.0), C, 400.0), None, "unchanged");
    assert_eq!(g.observe(66, (700.5, 500.0), C, 400.0), None, "inside the same quantum");
    assert!(g.observe(99, (760.0, 500.0), C, 400.0).is_some());
}

#[test]
fn polling_is_fast_after_movement_and_slow_when_idle() {
    let mut g = gaze();
    g.observe(1_000, (700.0, 500.0), C, 400.0);
    assert_eq!(g.poll_interval_ms(1_100), Some(33));
    assert_eq!(g.poll_interval_ms(1_500), Some(33), "500 ms after the move is still active");
    assert_eq!(g.poll_interval_ms(1_501), Some(500));
    assert_eq!(g.poll_interval_ms(60_000), Some(500));
    // Movement promotes again.
    g.observe(61_000, (710.0, 500.0), C, 400.0);
    assert_eq!(g.poll_interval_ms(61_010), Some(33));
}

#[test]
fn hiding_forgets_the_last_value_so_the_next_show_emits_again() {
    let mut g = gaze();
    assert!(g.observe(0, (700.0, 500.0), C, 400.0).is_some());
    g.set_visible(false);
    g.set_visible(true);
    assert!(g.observe(10, (700.0, 500.0), C, 400.0).is_some());
}

#[test]
fn a_zero_or_non_finite_reach_never_produces_nan() {
    let mut g = gaze();
    for reach in [0.0, -5.0, f64::NAN, f64::INFINITY] {
        if let Some(v) = g.observe(0, (900.0, 500.0), C, reach) {
            assert!(v.x.is_finite() && v.y.is_finite() && v.x.abs() <= 1.0 && v.y.abs() <= 1.0);
        }
    }
    let mut g = gaze();
    if let Some(v) = g.observe(0, (f64::NAN, 500.0), C, 400.0) {
        assert!(v.x.is_finite() && v.y.is_finite());
    }
}

// ---- window spec ----

#[test]
fn the_orb_window_spec_is_a_transparent_frameless_always_on_top_square() {
    for (size, px) in [(OrbSize::Small, 72.0), (OrbSize::Medium, 96.0), (OrbSize::Large, 128.0)] {
        let s = orb_window_spec(size);
        assert_eq!(s.label, "orb");
        assert_eq!(s.logical_px, px);
        assert!(s.transparent && s.always_on_top && s.skip_taskbar && s.visible_on_all_workspaces);
        assert!(!s.decorations && !s.resizable && !s.shadow);
        assert!(!s.focusable, "the orb never takes keyboard focus");
    }
}
