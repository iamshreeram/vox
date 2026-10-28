//! G3d spec: free orb size (DESIGN.md R6-5 'Free orb size (G3d)' and A7-5 'Size: one setter,
//! serialized saves'; A7-5 overrides R6-5). Authoritative: do not weaken an assertion; if a test is
//! wrong, report it. Written before the production code, so these tests are expected to fail
//! until the implementer lands the symbols below.
//!
//! Contract:
//!   settings: AppSettings.orb_size_px: Option<u16> (default None, serde round trip, absent -> None,
//!             kept by preserve_launch_fields / copy_orb_fields).
//!   crate::orb_geometry: ORB_PX_MIN = 48, ORB_PX_MAX = 192,
//!     effective_orb_logical_px(OrbSize, Option<u16>) -> f64, nearest_preset(u16) -> OrbSize,
//!     clamp_orb_px(u16) -> u16, resize_keep_center(Rect, new_side: f64, work: Rect) -> Rect.
//!   crate::orb_runtime: GeometryGuard { new, bump -> u64, accept(rev) -> bool },
//!     PendingResize { new, defer(Option<u16>), take(gesture_active) -> Option<Option<u16>> }.
//!   Single setter (source contracts): set_orb_size in orb_native.rs is the ONLY writer of
//!     orb_size_px and orb_size; change_orb_size_px_setting is async, takes px: Option<u16>, is
//!     registered in lib.rs and is not in window_commands.rs; the legacy change_orb_size_setting
//!     calls set_orb_size; the size menu presets route through set_orb_size(Some(72|96|128)).

use crate::orb_geometry::{
    clamp_orb_px, effective_orb_logical_px, nearest_preset, resize_keep_center, OrbSize, Rect,
    ORB_PX_MAX, ORB_PX_MIN,
};
use crate::orb_runtime::{GeometryGuard, PendingResize};
use crate::settings::{get_default_settings, preserve_launch_fields, AppSettings};
use std::path::{Path, PathBuf};

// ---------- helpers ----------

fn src(path: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Text of the function whose signature line contains `signature`, up to its closing brace at
/// column zero. Panics when the signature is missing, so a renamed function fails the contract.
fn fn_body<'a>(text: &'a str, signature: &str) -> &'a str {
    let start = text
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` not found"));
    let rest = &text[start..];
    let end = rest.find("\n}\n").map(|i| i + 3).unwrap_or(rest.len());
    &rest[..end]
}

/// The text from `signature` up to (not including) the first ')' after it.
fn signature_of<'a>(text: &'a str, signature: &str) -> &'a str {
    let start = text
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` not found"));
    let rest = &text[start..];
    let end = rest
        .find(')')
        .unwrap_or_else(|| panic!("`{signature}` has no closing paren"));
    &rest[..end]
}

/// Byte offsets of every write to a `.orb_size` or `.orb_size_px` field (an assignment, not a
/// comparison and not a read).
fn field_writes(text: &str) -> Vec<usize> {
    text.match_indices(".orb_size")
        .filter_map(|(at, _)| {
            let mut rest = &text[at + ".orb_size".len()..];
            if let Some(after_px) = rest.strip_prefix("_px") {
                rest = after_px;
            }
            let rest = rest.trim_start_matches(' ');
            let tail = rest.strip_prefix('=')?;
            (!tail.starts_with('=')).then_some(at)
        })
        .collect()
}

/// Every non-test Rust source under src/, as (path, text).
fn production_sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read src dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let is_rust = path.extension().and_then(|e| e.to_str()) == Some("rs");
            if !is_rust || name.ends_with("_tests.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read source");
            out.push((path.display().to_string(), text));
        }
    }
    out
}

/// Whether the arguments of some `set_orb_size(` call in `text` start with `Some(<px>)`
/// (whitespace ignored; the first argument may be `app`).
fn calls_setter_with_some(text: &str, px: u16) -> bool {
    let wanted = format!("Some({px})");
    text.match_indices("set_orb_size(").any(|(at, _)| {
        let window: String = text[at..]
            .chars()
            .take(48)
            .filter(|c| !c.is_whitespace())
            .collect();
        window.contains(&wanted)
    })
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { x, y, w, h }
}

fn screen() -> Rect {
    rect(0.0, 0.0, 1920.0, 1080.0)
}

/// Settings store as an upgraded install has it: the `orb_size_px` key is absent.
fn store_without_px_key() -> AppSettings {
    let mut json = serde_json::to_value(get_default_settings()).expect("serialise defaults");
    json.as_object_mut()
        .expect("settings object")
        .remove("orb_size_px");
    serde_json::from_value(json).expect("a store without orb_size_px must load")
}

// ---------- 1. settings ----------

#[test]
fn fresh_defaults_leave_the_px_size_unset() {
    assert_eq!(get_default_settings().orb_size_px, None);
}

#[test]
fn an_old_store_without_the_px_key_loads_as_none() {
    assert_eq!(store_without_px_key().orb_size_px, None);
}

#[test]
fn a_custom_px_size_survives_a_serde_round_trip() {
    for value in [Some(140u16), Some(48), Some(192), None] {
        let mut s = get_default_settings();
        s.orb_size_px = value;
        let back: AppSettings =
            serde_json::from_value(serde_json::to_value(&s).expect("serialise")).expect("load");
        assert_eq!(back.orb_size_px, value, "orb_size_px {value:?} round-trips");
    }
}

#[test]
fn a_stale_whole_object_write_cannot_clobber_the_px_size() {
    let mut committed = get_default_settings();
    committed.orb_size_px = Some(140);
    let stale = get_default_settings(); // an old snapshot: orb_size_px None
    let merged = preserve_launch_fields(stale, &committed);
    assert_eq!(merged.orb_size_px, Some(140), "committed px kept");

    let mut committed_none = get_default_settings();
    committed_none.orb_size_px = None;
    let mut stale_px = get_default_settings();
    stale_px.orb_size_px = Some(72);
    let merged_none = preserve_launch_fields(stale_px, &committed_none);
    assert_eq!(
        merged_none.orb_size_px, None,
        "a stale px cannot resurrect a reset"
    );
}

// ---------- 2. effective size and presets ----------

#[test]
fn no_px_means_the_legacy_preset() {
    assert_eq!(effective_orb_logical_px(OrbSize::Small, None), 72.0);
    assert_eq!(effective_orb_logical_px(OrbSize::Medium, None), 96.0);
    assert_eq!(effective_orb_logical_px(OrbSize::Large, None), 128.0);
}

#[test]
fn a_px_wins_over_the_preset_and_is_clamped_to_the_range() {
    assert_eq!(effective_orb_logical_px(OrbSize::Medium, Some(140)), 140.0);
    assert_eq!(effective_orb_logical_px(OrbSize::Medium, Some(0)), 48.0);
    assert_eq!(effective_orb_logical_px(OrbSize::Medium, Some(10)), 48.0);
    assert_eq!(effective_orb_logical_px(OrbSize::Small, Some(100)), 100.0);
    assert_eq!(effective_orb_logical_px(OrbSize::Large, Some(500)), 192.0);
}

#[test]
fn the_range_constants_are_48_and_192() {
    assert_eq!(ORB_PX_MIN as f64, 48.0);
    assert_eq!(ORB_PX_MAX as f64, 192.0);
}

#[test]
fn clamp_orb_px_keeps_the_range() {
    assert_eq!(clamp_orb_px(0), 48);
    assert_eq!(clamp_orb_px(47), 48);
    assert_eq!(clamp_orb_px(48), 48);
    assert_eq!(clamp_orb_px(100), 100);
    assert_eq!(clamp_orb_px(192), 192);
    assert_eq!(clamp_orb_px(193), 192);
    assert_eq!(clamp_orb_px(500), 192);
    assert_eq!(clamp_orb_px(u16::MAX), 192);
}

#[test]
fn nearest_preset_splits_at_84_and_112() {
    assert_eq!(nearest_preset(48), OrbSize::Small);
    assert_eq!(nearest_preset(72), OrbSize::Small);
    assert_eq!(nearest_preset(84), OrbSize::Small);
    assert_eq!(nearest_preset(85), OrbSize::Medium);
    assert_eq!(nearest_preset(112), OrbSize::Medium);
    assert_eq!(nearest_preset(113), OrbSize::Large);
    assert_eq!(nearest_preset(192), OrbSize::Large);
}

// ---------- 3. resize keeps the centre, then clamps into the work area ----------

#[test]
fn growing_keeps_the_centre_when_there_is_room() {
    let got = resize_keep_center(rect(100.0, 100.0, 96.0, 96.0), 128.0, screen());
    assert_eq!(
        got,
        rect(84.0, 84.0, 128.0, 128.0),
        "centre stays at (148, 148)"
    );
}

#[test]
fn shrinking_keeps_the_centre() {
    let got = resize_keep_center(rect(100.0, 100.0, 128.0, 128.0), 96.0, screen());
    assert_eq!(got, rect(116.0, 116.0, 96.0, 96.0));
}

#[test]
fn a_rect_at_the_top_left_corner_is_shifted_just_enough() {
    let got = resize_keep_center(rect(0.0, 0.0, 96.0, 96.0), 128.0, screen());
    assert_eq!(got, rect(0.0, 0.0, 128.0, 128.0));
}

#[test]
fn a_rect_at_the_bottom_right_corner_is_shifted_just_enough() {
    let got = resize_keep_center(rect(1824.0, 984.0, 96.0, 96.0), 128.0, screen());
    assert_eq!(got, rect(1792.0, 952.0, 128.0, 128.0));
}

#[test]
fn negative_origins_on_a_monitor_left_of_the_primary_are_respected() {
    let work = rect(-1920.0, 0.0, 1920.0, 1080.0);
    let got = resize_keep_center(rect(-1900.0, 10.0, 96.0, 96.0), 128.0, work);
    assert_eq!(got, rect(-1916.0, 0.0, 128.0, 128.0));
}

#[test]
fn a_side_larger_than_the_work_area_clamps_to_the_work_area() {
    let work = rect(0.0, 0.0, 100.0, 80.0);
    let got = resize_keep_center(rect(10.0, 10.0, 20.0, 20.0), 200.0, work);
    assert_eq!(got, work);
}

// ---------- 4. geometry revisions and deferred resizes ----------

#[test]
fn a_save_captured_before_a_newer_resize_is_rejected() {
    let mut guard = GeometryGuard::new();
    let first = guard.bump();
    let second = guard.bump();
    assert!(second > first, "bump returns a newer revision");
    assert!(!guard.accept(first), "the older revision is rejected");
    assert!(guard.accept(second), "the latest revision is accepted");
}

#[test]
fn a_save_captured_before_a_drag_is_rejected_after_it() {
    let mut guard = GeometryGuard::new();
    let resize_save = guard.bump(); // resize, position save captured against it
    let drag_end = guard.bump(); // the user drags the orb afterwards
    assert!(!guard.accept(resize_save), "stale resize save is dropped");
    assert!(guard.accept(drag_end), "the drag-end revision is current");
}

#[test]
fn a_resize_during_a_gesture_is_deferred_and_applied_once_after_it() {
    let mut pending = PendingResize::new();
    pending.defer(Some(128));
    assert_eq!(
        pending.take(true),
        None,
        "nothing applies while a gesture is active"
    );
    assert_eq!(
        pending.take(false),
        Some(Some(128)),
        "applied once the gesture ends"
    );
    assert_eq!(pending.take(false), None, "and not again");
}

#[test]
fn the_latest_deferred_value_wins() {
    let mut pending = PendingResize::new();
    pending.defer(Some(100));
    pending.defer(Some(140));
    pending.defer(Some(96));
    assert_eq!(pending.take(true), None);
    assert_eq!(pending.take(false), Some(Some(96)));
}

#[test]
fn a_deferred_reset_is_applied_as_none() {
    let mut pending = PendingResize::new();
    pending.defer(Some(100));
    pending.defer(None);
    assert_eq!(pending.take(false), Some(None));
}

#[test]
fn nothing_deferred_means_nothing_to_apply() {
    let mut pending = PendingResize::new();
    assert_eq!(pending.take(false), None);
    assert_eq!(pending.take(true), None);
}

// ---------- 5. single setter (source contracts) ----------

#[test]
fn set_orb_size_is_the_one_setter_and_it_clamps_and_writes_both_fields() {
    let native = src("src/orb_native.rs");
    let body = fn_body(&native, "fn set_orb_size(");
    assert!(
        body.contains("clamp_orb_px("),
        "set_orb_size clamps through clamp_orb_px"
    );
    assert!(
        body.contains("nearest_preset("),
        "set_orb_size derives the legacy preset"
    );
    assert!(
        body.contains("orb_size_px"),
        "set_orb_size writes orb_size_px"
    );
    assert!(
        body.contains("write_settings_gated("),
        "set_orb_size saves through write_settings_gated"
    );
}

#[test]
fn the_px_command_is_async_takes_px_and_is_registered() {
    let commands = src("src/commands/orb.rs");
    let sig = signature_of(&commands, "async fn change_orb_size_px_setting(");
    assert!(
        sig.contains("async fn change_orb_size_px_setting("),
        "command is async"
    );
    assert!(
        sig.contains("px: Option<u16>"),
        "command takes px: Option<u16>"
    );

    let lib = src("src/lib.rs");
    assert!(
        lib.contains("commands::orb::change_orb_size_px_setting,"),
        "change_orb_size_px_setting is registered in lib.rs"
    );
}

#[test]
fn the_px_command_is_not_a_window_command() {
    let window = src("src/window_commands.rs");
    assert!(
        !window.contains("change_orb_size_px_setting"),
        "the settings command is not listed in the ORB or CHAT window lists"
    );
}

#[test]
fn the_legacy_size_command_delegates_to_the_setter() {
    let commands = src("src/commands/orb.rs");
    let body = fn_body(&commands, "pub async fn change_orb_size_setting(");
    assert!(
        body.contains("set_orb_size("),
        "the legacy change_orb_size_setting body calls set_orb_size"
    );
    assert!(
        field_writes(body).is_empty(),
        "the legacy command writes no orb size field itself"
    );
}

#[test]
fn the_size_menu_presets_route_through_the_setter() {
    // The S/M/L menu ids are handled in orb_native.rs (handle_menu -> set_size). A tray.rs size
    // menu, if one is added, may route through the setter instead; either file satisfies this.
    let native = src("src/orb_native.rs");
    let tray = src("src/tray.rs");
    for px in [72u16, 96, 128] {
        assert!(
            calls_setter_with_some(&native, px) || calls_setter_with_some(&tray, px),
            "a size menu handler calls set_orb_size(Some({px})) in orb_native.rs or tray.rs"
        );
    }
}

#[test]
fn the_size_menu_does_not_write_the_size_fields_directly() {
    for path in ["src/tray.rs", "src/orb_native.rs"] {
        let text = src(path);
        let scanned = if path.ends_with("orb_native.rs") {
            text.replacen(fn_body(&text, "fn set_orb_size("), "", 1)
        } else {
            text.clone()
        };
        assert!(
            field_writes(&scanned).is_empty(),
            "{path}: menu code must not write orb_size or orb_size_px directly"
        );
    }
}

#[test]
fn set_orb_size_is_the_only_writer_of_the_size_fields() {
    let mut offenders = Vec::new();
    for (path, text) in production_sources() {
        // The one allowed write is inside set_orb_size; the preserve helper copies, it does not
        // set a new value, so its body is exempt in settings.rs.
        let scanned = if path.ends_with("orb_native.rs") {
            text.replacen(fn_body(&text, "fn set_orb_size("), "", 1)
        } else if path.ends_with("settings.rs") {
            text.replacen(fn_body(&text, "fn copy_orb_fields("), "", 1)
        } else {
            text.clone()
        };
        for at in field_writes(&scanned) {
            let line = scanned[..at].matches('\n').count() + 1;
            offenders.push(format!("{path}:{line}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "orb_size / orb_size_px written outside set_orb_size: {offenders:?}"
    );
}
