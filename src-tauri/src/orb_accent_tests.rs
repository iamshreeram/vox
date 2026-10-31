//! Orb colour setting (feat/orb-globe-colours, DESIGN Revision 3, D2).
//!
//! `orb_accent: OrbColor` defaults to `Auto` (state-aware colours). It is read leniently
//! (missing, null, unknown strings, wrong case, numbers, objects and booleans all read as `Auto`),
//! kept by `preserve_launch_fields`, carried in the appearance snapshot as a string that is never
//! null, and written only by `change_orb_accent_setting` through `commit_appearance`. Main window
//! only. Nothing here logs colours or paths.

use crate::appearance::{
    commit_in_memory, plan_deliveries, snapshot_from_settings, AppearanceHolder,
};
use crate::settings::{get_default_settings, preserve_launch_fields, AppSettings, OrbColor};
use crate::window_commands::window_command_allowed;
use serde::Deserialize;
use serde_json::{json, Value};

const ORB: &str = "orb";
const BUBBLES: &str = "orb-bubbles";

/// Every `OrbColor` with its wire string.
const ALL_COLORS: [(OrbColor, &str); 8] = [
    (OrbColor::Auto, "auto"),
    (OrbColor::App, "app"),
    (OrbColor::Amber, "amber"),
    (OrbColor::Blue, "blue"),
    (OrbColor::Violet, "violet"),
    (OrbColor::Emerald, "emerald"),
    (OrbColor::Rose, "rose"),
    (OrbColor::Slate, "slate"),
];

/// Default settings as a raw JSON object with `orb_accent` set to `value`.
fn stored_with(value: Value) -> AppSettings {
    let mut json = serde_json::to_value(get_default_settings()).expect("defaults serialize");
    json.as_object_mut()
        .expect("settings is an object")
        .insert("orb_accent".to_string(), value);
    serde_json::from_value(json).expect("settings parse")
}

/// Default settings as a raw JSON object with no `orb_accent` key at all (an older file).
fn stored_without() -> AppSettings {
    let mut json = serde_json::to_value(get_default_settings()).expect("defaults serialize");
    json.as_object_mut()
        .expect("settings is an object")
        .remove("orb_accent");
    serde_json::from_value(json).expect("older settings still parse")
}

#[test]
fn orb_accent_defaults_to_auto() {
    assert_eq!(get_default_settings().orb_accent, OrbColor::Auto);
}

#[test]
fn orb_accent_reads_each_of_the_eight_wire_values() {
    for (color, wire) in ALL_COLORS {
        assert_eq!(
            stored_with(json!(wire)).orb_accent,
            color,
            "{wire} reads as {color:?}"
        );
    }
}

#[test]
fn orb_accent_reads_every_non_wire_value_as_auto_never_an_error() {
    for garbage in [
        json!("bogus"),
        json!("AMBER"),
        json!("Rose"),
        json!(""),
        json!(42),
        json!(-1.5),
        json!(true),
        json!(null),
        json!(["rose"]),
        json!({}),
        json!({"accent": "rose"}),
    ] {
        assert_eq!(
            stored_with(garbage.clone()).orb_accent,
            OrbColor::Auto,
            "{garbage} reads as auto"
        );
    }
}

#[test]
fn orb_accent_missing_key_reads_as_auto() {
    assert_eq!(stored_without().orb_accent, OrbColor::Auto);
}

#[test]
fn previous_release_null_migrates_to_auto_and_blue_stays_blue() {
    // PR #128 stored `null` to mean "follow the app accent"; it migrates to Auto by design.
    assert_eq!(stored_with(json!(null)).orb_accent, OrbColor::Auto);
    assert_eq!(stored_with(json!("blue")).orb_accent, OrbColor::Blue);
}

#[test]
fn older_settings_without_the_field_keep_every_other_value() {
    let old = stored_without();
    let fresh = get_default_settings();
    assert_eq!(old.accent, fresh.accent);
    assert_eq!(old.chat_theme, fresh.chat_theme);
    assert_eq!(old.visual_style, fresh.visual_style);
    assert_eq!(old.chat_opacity_pct, fresh.chat_opacity_pct);
}

#[test]
fn settings_round_trip_json_as_lowercase_strings() {
    let defaults = get_default_settings();
    let round: AppSettings =
        serde_json::from_value(serde_json::to_value(&defaults).expect("serialize"))
            .expect("round trip");
    assert_eq!(round.orb_accent, OrbColor::Auto);

    for (color, wire) in ALL_COLORS {
        let mut set = get_default_settings();
        set.orb_accent = color;
        let json = serde_json::to_value(&set).expect("serialize");
        assert_eq!(json["orb_accent"], json!(wire));
        let back: AppSettings = serde_json::from_value(json).expect("round trip");
        assert_eq!(back.orb_accent, color);
    }
}

#[test]
fn lenient_deserializer_is_usable_on_its_own() {
    #[derive(Deserialize)]
    struct Probe {
        #[serde(default, deserialize_with = "crate::settings::deserialize_orb_accent")]
        orb_accent: OrbColor,
    }
    let parsed: Probe = serde_json::from_value(json!({"orb_accent": 7})).expect("lenient");
    assert_eq!(parsed.orb_accent, OrbColor::Auto);
    let parsed: Probe = serde_json::from_value(json!({"orb_accent": "slate"})).expect("slate");
    assert_eq!(parsed.orb_accent, OrbColor::Slate);
    let parsed: Probe = serde_json::from_value(json!({})).expect("missing");
    assert_eq!(parsed.orb_accent, OrbColor::Auto);
}

#[test]
fn preserve_launch_fields_keeps_the_committed_orb_accent() {
    let mut committed = get_default_settings();
    committed.orb_accent = OrbColor::Blue;
    let incoming = get_default_settings();
    let merged = preserve_launch_fields(incoming, &committed);
    assert_eq!(merged.orb_accent, OrbColor::Blue);
}

#[test]
fn a_stale_whole_object_write_cannot_change_the_orb_accent() {
    let committed = get_default_settings();
    let mut stale = get_default_settings();
    stale.orb_accent = OrbColor::Rose;
    let merged = preserve_launch_fields(stale, &committed);
    assert_eq!(
        merged.orb_accent,
        OrbColor::Auto,
        "only the setter changes it"
    );
}

#[test]
fn snapshot_always_carries_a_wire_string_never_null() {
    for (color, wire) in ALL_COLORS {
        let mut s = get_default_settings();
        s.orb_accent = color;
        let snap = snapshot_from_settings(3, &s);
        assert_eq!(snap.orb_accent, color);
        let value = serde_json::to_value(&snap).expect("serialize");
        assert_eq!(value["orb_accent"], json!(wire), "{wire} on the wire");
    }

    let default_snap = snapshot_from_settings(4, &get_default_settings());
    let value = serde_json::to_value(&default_snap).expect("serialize");
    assert_eq!(
        value["orb_accent"],
        json!("auto"),
        "default is auto, not null"
    );
}

#[test]
fn an_unrelated_commit_keeps_the_orb_accent() {
    let holder = AppearanceHolder::new();
    let mut s = get_default_settings();
    s.orb_accent = OrbColor::Violet;
    let snap = commit_in_memory(&mut s, &holder, |f| {
        f.accent = crate::settings::Accent::Blue
    });
    assert_eq!(snap.orb_accent, OrbColor::Violet);
    assert_eq!(s.orb_accent, OrbColor::Violet);
}

#[test]
fn the_orb_accent_setter_is_one_commit_and_one_strictly_higher_rev() {
    let holder = AppearanceHolder::new();
    let mut s = get_default_settings();
    let before = holder.revision();
    let snap = commit_in_memory(&mut s, &holder, |f| f.orb_accent = OrbColor::Emerald);
    assert_eq!(snap.rev, before + 1, "exactly one bump");
    assert_eq!(holder.revision(), before + 1);
    assert_eq!(snap.orb_accent, OrbColor::Emerald);
    assert_eq!(s.orb_accent, OrbColor::Emerald);

    let back = commit_in_memory(&mut s, &holder, |f| f.orb_accent = OrbColor::Auto);
    assert_eq!(back.rev, snap.rev + 1, "reset is one more commit");
    assert_eq!(back.orb_accent, OrbColor::Auto);
}

#[test]
fn the_orb_gets_the_orb_accent_snapshot_without_a_chat_window() {
    let mut s = get_default_settings();
    s.orb_accent = OrbColor::Rose;
    let snap = snapshot_from_settings(2, &s);
    let exists = |label: &str| label == ORB || label == BUBBLES;
    let plan = plan_deliveries(&snap, exists);
    let orb: Vec<_> = plan.iter().filter(|(label, _)| *label == ORB).collect();
    assert_eq!(orb.len(), 1, "the orb is delivered to");
    assert_eq!(orb[0].1.orb_accent, OrbColor::Rose);
    assert!(
        plan.iter().all(|(label, _)| *label != "chat"),
        "no chat window was created"
    );
}

#[test]
fn change_orb_accent_setting_is_main_window_only() {
    assert!(window_command_allowed("main", "change_orb_accent_setting"));
    for label in [
        "chat",
        "orb",
        "orb-bubbles",
        "overlay",
        "recording_overlay",
        "settings",
    ] {
        assert!(
            !window_command_allowed(label, "change_orb_accent_setting"),
            "refused from {label}"
        );
    }
}

fn code_only(src: &str) -> String {
    src.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_setter_is_one_commit_appearance_path_and_registered() {
    let commands = code_only(include_str!("commands/appearance.rs"));
    assert!(commands
        .contains("pub async fn change_orb_accent_setting(app: AppHandle, color: OrbColor)"));
    assert!(commands.contains("fields.orb_accent = color"));
    let lib = code_only(include_str!("lib.rs"));
    assert!(lib.contains("commands::appearance::change_orb_accent_setting,"));
    let matrix = code_only(include_str!("window_commands.rs"));
    assert!(matrix.contains("\"change_orb_accent_setting\","));
}

#[test]
fn bindings_ts_exports_the_orb_color_type_and_setter() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/bindings.ts");
    let bindings = std::fs::read_to_string(path).expect("bindings.ts is readable");
    assert!(
        bindings.contains(
            "export type OrbColor = \"auto\" | \"app\" | \"amber\" | \"blue\" | \"violet\" | \"emerald\" | \"rose\" | \"slate\""
        ),
        "OrbColor union is exported"
    );
    assert!(
        bindings.contains("async changeOrbAccentSetting(color: OrbColor) : Promise<void> {"),
        "setter takes OrbColor"
    );
    assert!(bindings.contains("TAURI_INVOKE(\"change_orb_accent_setting\", { color });"));
}
