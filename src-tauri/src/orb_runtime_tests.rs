//! G3b-B spec: the orb runtime drivers (generic over small traits) and the wiring contracts of the
//! native orb (DESIGN.md Rev 5: 'Gesture engine owned by Rust', 'Rust modules and commands').
//! Authoritative: do not weaken an assertion; if a test is wrong, report it.
//!
//! Contract, module `crate::orb_runtime` (pure, no Tauri types):
//!   trait ButtonProbe { fn left_down(&self) -> bool }
//!   trait CursorSource { fn cursor(&self) -> Option<(f64, f64)> }        // physical px; None = unavailable
//!   trait OrbHost {
//!       fn orb_rect(&self) -> Option<Rect>;  fn scale(&self) -> f64;  fn move_to(&mut self, x: f64, y: f64);
//!       fn emit_gesture(&mut self, id: u64, result: GestureResult);
//!       fn click(&mut self);                  // activates the conversation (N1b: start or stop, same as Enter/Space)
//!       fn save_position(&mut self, final_orb: Rect);   // called exactly once per finished Drag, never for Click/Cancel
//!       fn emit_gaze(&mut self, gaze: Gaze);
//!   }
//!   struct GestureDriver; new(); is_active(); pointer_down(id, now_ms, &impl CursorSource, &mut impl OrbHost);
//!       step(now_ms, &impl ButtonProbe, &impl CursorSource, &mut impl OrbHost); cancel(&mut impl OrbHost)
//!     Finish handling: Click -> emit_gesture(id, Click) then click(); Drag -> save_position(final) then emit_gesture(id, Drag);
//!     Cancel -> emit_gesture(id, Cancel) only. pointer_down with no cursor or no orb rect emits Cancel for that id and arms nothing.
//!   struct GazeDriver; new(); set_visible(bool); set_reduced_motion(bool); next_interval_ms(now_ms) -> Option<u64>;
//!       step(now_ms, &impl CursorSource, &mut impl OrbHost)   // observes with the orb centre and reach = GAZE_REACH_LOGICAL x scale
//!   const GAZE_REACH_LOGICAL: f64 = 500.0
//!   struct RawMonitor { name: Option<String>, position: (i32, i32), size: (u32, u32), work_position: (i32, i32), work_size: (u32, u32), scale: f64 }
//!   fn monitors_from_raw(raw: &[RawMonitor], primary_index: Option<usize>) -> Vec<MonitorInfo>
//!     (missing name -> "monitor-<index>"; key = monitor_key(name, rect); primary = primary_index, else the first one)
//!   fn orb_menu_items(size: OrbSize, chat_open: bool) -> Vec<MenuEntry>
//!     enum MenuEntry { Item { id: &'static str, checked: Option<bool> }, Separator }
//!     ids in order: "orb_toggle_chat", Separator, "orb_size_small", "orb_size_medium", "orb_size_large" (checked Some(true/false) by size),
//!     Separator, "orb_hide", "orb_settings"
use crate::orb_gaze::Gaze;
use crate::orb_geometry::{monitor_key, OrbSize, Rect};
use crate::orb_gesture::GestureResult;
use crate::orb_runtime::*;
use std::cell::Cell;

fn orb() -> Rect {
    Rect {
        x: 100.0,
        y: 200.0,
        w: 96.0,
        h: 96.0,
    }
}

struct Probe(Cell<bool>);
impl ButtonProbe for Probe {
    fn left_down(&self) -> bool {
        self.0.get()
    }
}

struct Cursor(Cell<Option<(f64, f64)>>);
impl CursorSource for Cursor {
    fn cursor(&self) -> Option<(f64, f64)> {
        self.0.get()
    }
}

#[derive(Default)]
struct Host {
    rect: Option<Rect>,
    scale: f64,
    moves: Vec<(f64, f64)>,
    gestures: Vec<(u64, GestureResult)>,
    clicks: usize,
    saved: Vec<Rect>,
    gazes: Vec<Gaze>,
    log: Vec<&'static str>,
}

impl Host {
    fn new() -> Self {
        Host {
            rect: Some(orb()),
            scale: 1.0,
            ..Default::default()
        }
    }
}

impl OrbHost for Host {
    fn orb_rect(&self) -> Option<Rect> {
        self.rect
    }
    fn scale(&self) -> f64 {
        self.scale
    }
    fn move_to(&mut self, x: f64, y: f64) {
        self.moves.push((x, y));
        if let Some(r) = self.rect.as_mut() {
            r.x = x;
            r.y = y;
        }
    }
    fn emit_gesture(&mut self, id: u64, result: GestureResult) {
        self.gestures.push((id, result));
        self.log.push("gesture");
    }
    fn click(&mut self) {
        self.clicks += 1;
        self.log.push("click");
    }
    fn save_position(&mut self, final_orb: Rect) {
        self.saved.push(final_orb);
        self.log.push("save");
    }
    fn emit_gaze(&mut self, gaze: Gaze) {
        self.gazes.push(gaze);
    }
}

fn cursor(p: (f64, f64)) -> Cursor {
    Cursor(Cell::new(Some(p)))
}

// ---- gesture driver ----

#[test]
fn a_still_click_emits_click_then_activates_the_conversation_and_saves_nothing() {
    let mut d = GestureDriver::new();
    let mut h = Host::new();
    let c = cursor((130.0, 230.0));
    d.pointer_down(1, 0, &c, &mut h);
    assert!(d.is_active());
    let probe = Probe(Cell::new(true));
    d.step(16, &probe, &c, &mut h);
    probe.0.set(false);
    d.step(32, &probe, &c, &mut h);
    assert!(!d.is_active());
    assert_eq!(h.gestures, vec![(1, GestureResult::Click)]);
    assert_eq!(h.clicks, 1);
    assert!(h.saved.is_empty());
    assert!(h.moves.is_empty());
    assert_eq!(h.log, vec!["gesture", "click"]);
}

#[test]
fn a_drag_moves_the_window_saves_once_then_reports_the_drag() {
    let mut d = GestureDriver::new();
    let mut h = Host::new();
    let c = cursor((130.0, 230.0));
    d.pointer_down(4, 0, &c, &mut h);
    let probe = Probe(Cell::new(true));
    c.0.set(Some((330.0, 430.0)));
    d.step(16, &probe, &c, &mut h);
    c.0.set(Some((500.0, 100.0)));
    d.step(32, &probe, &c, &mut h);
    assert_eq!(h.moves, vec![(300.0, 400.0), (470.0, 70.0)]);
    probe.0.set(false);
    d.step(48, &probe, &c, &mut h);
    assert_eq!(h.saved.len(), 1);
    assert_eq!((h.saved[0].x, h.saved[0].y), (470.0, 70.0));
    assert_eq!(h.gestures, vec![(4, GestureResult::Drag)]);
    assert_eq!(h.clicks, 0, "a drag never activates the conversation");
    assert_eq!(h.log, vec!["save", "gesture"]);
}

#[test]
fn a_lost_release_far_from_the_orb_still_ends_the_drag_on_one_tick() {
    let mut d = GestureDriver::new();
    let mut h = Host::new();
    let c = cursor((130.0, 230.0));
    d.pointer_down(1, 0, &c, &mut h);
    let probe = Probe(Cell::new(true));
    c.0.set(Some((900.0, 700.0)));
    d.step(16, &probe, &c, &mut h);
    probe.0.set(false); // the mouse went up while the cursor was over another app
    d.step(32, &probe, &c, &mut h);
    assert!(!d.is_active());
    assert_eq!(h.gestures, vec![(1, GestureResult::Drag)]);
    assert_eq!(h.clicks, 0);
}

#[test]
fn cancel_reports_cancel_and_neither_toggles_nor_saves() {
    let mut d = GestureDriver::new();
    let mut h = Host::new();
    let c = cursor((130.0, 230.0));
    d.pointer_down(1, 0, &c, &mut h);
    let probe = Probe(Cell::new(true));
    c.0.set(Some((400.0, 400.0)));
    d.step(16, &probe, &c, &mut h);
    d.cancel(&mut h);
    assert_eq!(h.gestures, vec![(1, GestureResult::Cancel)]);
    assert_eq!(h.clicks, 0);
    assert!(h.saved.is_empty());
    d.cancel(&mut h);
    assert_eq!(h.gestures.len(), 1, "cancel while idle does nothing");
}

#[test]
fn a_press_without_a_cursor_or_orb_is_cancelled_at_once() {
    let mut d = GestureDriver::new();
    let mut h = Host::new();
    d.pointer_down(1, 0, &Cursor(Cell::new(None)), &mut h);
    assert!(!d.is_active());
    assert_eq!(h.gestures, vec![(1, GestureResult::Cancel)]);

    let mut h = Host::new();
    h.rect = None;
    d.pointer_down(2, 0, &cursor((1.0, 1.0)), &mut h);
    assert!(!d.is_active());
    assert_eq!(h.gestures, vec![(2, GestureResult::Cancel)]);
}

#[test]
fn a_second_press_cancels_the_first_and_both_report() {
    let mut d = GestureDriver::new();
    let mut h = Host::new();
    let c = cursor((130.0, 230.0));
    d.pointer_down(1, 0, &c, &mut h);
    d.pointer_down(2, 10, &c, &mut h);
    assert_eq!(h.gestures, vec![(1, GestureResult::Cancel)]);
    let probe = Probe(Cell::new(false));
    d.step(30, &probe, &c, &mut h);
    assert_eq!(
        h.gestures,
        vec![(1, GestureResult::Cancel), (2, GestureResult::Click)]
    );
    assert_eq!(h.clicks, 1, "only the second gesture clicked");
}

#[test]
fn a_missing_cursor_mid_drag_is_ignored_for_that_tick() {
    let mut d = GestureDriver::new();
    let mut h = Host::new();
    let c = cursor((130.0, 230.0));
    d.pointer_down(1, 0, &c, &mut h);
    let probe = Probe(Cell::new(true));
    c.0.set(None);
    d.step(16, &probe, &c, &mut h);
    assert!(d.is_active());
    assert!(h.moves.is_empty() && h.gestures.is_empty());
}

// ---- gaze driver ----

#[test]
fn the_gaze_driver_emits_from_the_orb_centre_with_a_scaled_reach() {
    let mut g = GazeDriver::new();
    let mut h = Host::new();
    h.scale = 2.0;
    h.rect = Some(Rect {
        x: 0.0,
        y: 0.0,
        w: 192.0,
        h: 192.0,
    }); // centre (96, 96)
    assert_eq!(g.next_interval_ms(0), None, "hidden until shown");
    g.set_visible(true);
    assert!(g.next_interval_ms(0).is_some());
    // reach = 500 x 2 = 1000: a cursor 500 px right of the centre is 0.5.
    g.step(0, &cursor((596.0, 96.0)), &mut h);
    assert_eq!(h.gazes, vec![Gaze { x: 0.5, y: 0.0 }]);
    g.step(33, &cursor((596.0, 96.0)), &mut h);
    assert_eq!(h.gazes.len(), 1, "unchanged value is not re-emitted");
}

#[test]
fn the_gaze_driver_is_silent_when_hidden_in_reduced_motion_or_without_a_cursor() {
    let mut g = GazeDriver::new();
    let mut h = Host::new();
    g.step(0, &cursor((900.0, 900.0)), &mut h);
    assert!(h.gazes.is_empty(), "hidden");
    g.set_visible(true);
    g.set_reduced_motion(true);
    g.step(10, &cursor((900.0, 900.0)), &mut h);
    assert!(h.gazes.is_empty());
    assert_eq!(g.next_interval_ms(10), None);
    g.set_reduced_motion(false);
    g.step(20, &Cursor(Cell::new(None)), &mut h);
    assert!(h.gazes.is_empty());
    let mut h2 = Host::new();
    h2.rect = None;
    g.step(30, &cursor((900.0, 900.0)), &mut h2);
    assert!(h2.gazes.is_empty());
}

// ---- monitors and menu ----

#[test]
fn raw_monitors_become_monitor_infos_with_keys_and_a_primary() {
    let raw = vec![
        RawMonitor {
            name: Some("Built-in".into()),
            position: (0, 0),
            size: (1920, 1080),
            work_position: (0, 25),
            work_size: (1920, 1055),
            scale: 1.0,
        },
        RawMonitor {
            name: None,
            position: (-1280, 0),
            size: (1280, 1024),
            work_position: (-1280, 0),
            work_size: (1280, 1024),
            scale: 2.0,
        },
    ];
    let m = monitors_from_raw(&raw, Some(1));
    assert_eq!(m.len(), 2);
    assert_eq!(m[0].name, "Built-in");
    assert_eq!(m[0].key, monitor_key("Built-in", &m[0].rect));
    assert_eq!((m[0].work.y, m[0].work.h), (25.0, 1055.0));
    assert!(!m[0].primary);
    assert_eq!(m[1].name, "monitor-1");
    assert_eq!((m[1].rect.x, m[1].rect.w), (-1280.0, 1280.0));
    assert_eq!(m[1].scale, 2.0);
    assert!(m[1].primary);
    let first_primary = monitors_from_raw(&raw, None);
    assert!(first_primary[0].primary && !first_primary[1].primary);
    assert!(monitors_from_raw(&[], None).is_empty());
}

#[test]
fn the_orb_menu_lists_the_documented_items_in_order() {
    let items = orb_menu_items(OrbSize::Large, true);
    let ids: Vec<Option<&str>> = items
        .iter()
        .map(|e| match e {
            MenuEntry::Item { id, .. } => Some(*id),
            MenuEntry::Separator => None,
        })
        .collect();
    assert_eq!(
        ids,
        vec![
            Some("orb_toggle_chat"),
            None,
            Some("orb_size_small"),
            Some("orb_size_medium"),
            Some("orb_size_large"),
            None,
            Some("orb_hide"),
            Some("orb_settings"),
        ]
    );
    let checked = |wanted: &str| {
        items.iter().find_map(|e| match e {
            MenuEntry::Item { id, checked } if *id == wanted => Some(*checked),
            _ => None,
        })
    };
    assert_eq!(checked("orb_size_large"), Some(Some(true)));
    assert_eq!(checked("orb_size_small"), Some(Some(false)));
    assert_eq!(checked("orb_size_medium"), Some(Some(false)));
    assert_eq!(checked("orb_hide"), Some(None));
}

// ---- wiring contracts (source level) ----

fn src(path: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn the_new_commands_are_registered_and_gated_by_the_window_matrix() {
    let lib = src("src/lib.rs");
    for command in [
        "orb_pointer_down",
        "orb_activate",
        "orb_menu",
        "orb_dismiss",
        "orb_get_state",
        "orb_set_reduced_motion",
        "change_orb_visible_setting",
        "change_orb_size_setting",
        "change_chat_presentation_setting",
        "reset_orb_position",
    ] {
        assert!(
            lib.contains(command),
            "{command} must be registered in lib.rs"
        );
    }
    assert!(
        lib.contains("window_command_allowed("),
        "the live gate must be the window matrix"
    );
    assert!(
        !lib.contains("chat_window::chat_command_allowed("),
        "the old chat-only gate is replaced"
    );
    assert!(
        lib.contains("orb_native::apply_visibility"),
        "startup shows the orb when enabled"
    );
}

#[test]
fn the_orb_window_has_events_only_and_the_main_capability_does_not_include_it() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities/orb.json");
    assert!(path.exists(), "capabilities/orb.json must exist");
    let default = src("capabilities/default.json");
    assert!(
        !default.contains("\"orb\""),
        "default.json must not list the orb window"
    );
    let chat = src("capabilities/chat.json");
    assert!(!chat.contains("\"orb\""));
}

#[test]
fn the_native_orb_follows_the_proven_panel_pattern_and_never_touches_the_activation_policy() {
    let text = src("src/orb_native.rs");
    for needed in [
        "PanelLevel::Status",
        "can_join_all_spaces",
        "full_screen_auxiliary",
        "nonactivating_panel",
        "orb_window_spec",
        "always_on_top",
        "skip_taskbar",
    ] {
        assert!(text.contains(needed), "orb_native.rs must contain {needed}");
    }
    for banned in [
        "set_activation_policy",
        "println!",
        "eprintln!",
        "dbg!",
        "std::fs",
        "File::",
    ] {
        assert!(
            !text.contains(banned),
            "orb_native.rs must not use {banned}"
        );
    }
    let rt = src("src/orb_runtime.rs");
    for banned in ["tauri::", "println!", "eprintln!", "std::fs"] {
        assert!(!rt.contains(banned), "orb_runtime.rs is pure: no {banned}");
    }
}

#[test]
fn the_tray_offers_a_show_hide_orb_item_and_the_click_goes_through_the_dispatcher() {
    let tray = src("src/tray.rs");
    assert!(
        tray.contains("\"orb\""),
        "tray menu needs an item with id orb"
    );
    let lib = src("src/lib.rs");
    assert!(
        lib.contains("\"orb\" =>") || lib.contains("\"orb\"=>"),
        "lib.rs menu handler needs an orb arm"
    );
    let native = src("src/orb_native.rs");
    assert!(
        native.contains("chat_surface::toggle"),
        "orb click and menu open the chat through the dispatcher"
    );
}

#[test]
fn the_setting_commands_use_the_gated_writer() {
    let text = src("src/commands/orb.rs");
    assert!(text.contains("write_settings_gated"));
    for name in [
        "change_orb_visible_setting",
        "change_orb_size_setting",
        "change_chat_presentation_setting",
        "reset_orb_position",
    ] {
        assert!(text.contains(name), "{name}");
    }
    assert!(!text.contains("println!"));
}
