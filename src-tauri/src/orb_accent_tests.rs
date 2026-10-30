//! Orb accent setting (feat/orb-click-chrome N1c, DESIGN Revision 3, D3).
//!
//! `orb_accent: Option<Accent>` is `None` by default (follow the app accent). It is read leniently
//! (garbage, numbers, null and missing all read as `None`), kept by `preserve_launch_fields`,
//! carried in the appearance snapshot, and written only by `change_orb_accent_setting` through
//! `commit_appearance`. Main window only. Nothing here logs colours or paths.

use crate::appearance::{
    commit_in_memory, plan_deliveries, snapshot_from_settings, AppearanceHolder,
};
use crate::settings::{get_default_settings, preserve_launch_fields, Accent, AppSettings};
use crate::window_commands::window_command_allowed;
use serde::Deserialize;
use serde_json::{json, Value};

const ORB: &str = "orb";
const BUBBLES: &str = "orb-bubbles";

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
fn orb_accent_defaults_to_none_follow_app_accent() {
    assert_eq!(get_default_settings().orb_accent, None);
}

#[test]
fn orb_accent_reads_known_wire_values() {
    assert_eq!(
        stored_with(json!("rose")).orb_accent,
        Some(Accent::Rose),
        "a known wire value is kept"
    );
    assert_eq!(stored_with(json!("slate")).orb_accent, Some(Accent::Slate));
}

#[test]
fn orb_accent_reads_garbage_as_none_never_an_error() {
    for garbage in [
        json!("garbage"),
        json!("Rose"),
        json!(""),
        json!(42),
        json!(-1.5),
        json!(true),
        json!(null),
        json!(["rose"]),
        json!({"accent": "rose"}),
    ] {
        assert_eq!(
            stored_with(garbage.clone()).orb_accent,
            None,
            "{garbage} reads as follow-app"
        );
    }
}

#[test]
fn orb_accent_missing_key_reads_as_none() {
    assert_eq!(stored_without().orb_accent, None);
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
fn settings_round_trip_json_with_and_without_the_field() {
    let defaults = get_default_settings();
    let round: AppSettings =
        serde_json::from_value(serde_json::to_value(&defaults).expect("serialize"))
            .expect("round trip");
    assert_eq!(round.orb_accent, None);

    let mut set = get_default_settings();
    set.orb_accent = Some(Accent::Emerald);
    let json = serde_json::to_value(&set).expect("serialize");
    assert_eq!(json["orb_accent"], json!("emerald"));
    let back: AppSettings = serde_json::from_value(json).expect("round trip");
    assert_eq!(back.orb_accent, Some(Accent::Emerald));
}

#[test]
fn lenient_deserializer_is_usable_on_its_own() {
    #[derive(Deserialize)]
    struct Probe {
        #[serde(default, deserialize_with = "crate::settings::deserialize_orb_accent")]
        orb_accent: Option<Accent>,
    }
    let parsed: Probe = serde_json::from_value(json!({"orb_accent": 7})).expect("lenient");
    assert_eq!(parsed.orb_accent, None);
    let parsed: Probe = serde_json::from_value(json!({})).expect("missing");
    assert_eq!(parsed.orb_accent, None);
}

#[test]
fn preserve_launch_fields_keeps_the_committed_orb_accent() {
    let mut committed = get_default_settings();
    committed.orb_accent = Some(Accent::Blue);
    let incoming = get_default_settings();
    let merged = preserve_launch_fields(incoming, &committed);
    assert_eq!(merged.orb_accent, Some(Accent::Blue));
}

#[test]
fn a_stale_whole_object_write_cannot_change_the_orb_accent() {
    let committed = get_default_settings();
    let mut stale = get_default_settings();
    stale.orb_accent = Some(Accent::Rose);
    let merged = preserve_launch_fields(stale, &committed);
    assert_eq!(merged.orb_accent, None, "only the setter changes it");
}

#[test]
fn snapshot_carries_the_orb_accent_and_null_when_follow_app() {
    let mut s = get_default_settings();
    s.orb_accent = Some(Accent::Violet);
    let snap = snapshot_from_settings(3, &s);
    assert_eq!(snap.orb_accent, Some(Accent::Violet));
    let value = serde_json::to_value(&snap).expect("serialize");
    assert_eq!(value["orb_accent"], json!("violet"));

    let follow = snapshot_from_settings(4, &get_default_settings());
    let value = serde_json::to_value(&follow).expect("serialize");
    assert!(
        value.get("orb_accent").is_some(),
        "the key is always present"
    );
    assert_eq!(value["orb_accent"], Value::Null);
}

#[test]
fn an_unrelated_commit_keeps_the_orb_accent() {
    let holder = AppearanceHolder::new();
    let mut s = get_default_settings();
    s.orb_accent = Some(Accent::Violet);
    let snap = commit_in_memory(&mut s, &holder, |f| f.accent = Accent::Blue);
    assert_eq!(snap.orb_accent, Some(Accent::Violet));
    assert_eq!(s.orb_accent, Some(Accent::Violet));
}

#[test]
fn the_orb_accent_setter_is_one_commit_and_one_strictly_higher_rev() {
    let holder = AppearanceHolder::new();
    let mut s = get_default_settings();
    let before = holder.revision();
    let snap = commit_in_memory(&mut s, &holder, |f| f.orb_accent = Some(Accent::Emerald));
    assert_eq!(snap.rev, before + 1, "exactly one bump");
    assert_eq!(holder.revision(), before + 1);
    assert_eq!(snap.orb_accent, Some(Accent::Emerald));
    assert_eq!(s.orb_accent, Some(Accent::Emerald));

    let back = commit_in_memory(&mut s, &holder, |f| f.orb_accent = None);
    assert_eq!(back.rev, snap.rev + 1, "reset is one more commit");
    assert_eq!(back.orb_accent, None);
}

#[test]
fn the_orb_gets_the_orb_accent_snapshot_without_a_chat_window() {
    let mut s = get_default_settings();
    s.orb_accent = Some(Accent::Rose);
    let snap = snapshot_from_settings(2, &s);
    let exists = |label: &str| label == ORB || label == BUBBLES;
    let plan = plan_deliveries(&snap, exists);
    let orb: Vec<_> = plan.iter().filter(|(label, _)| *label == ORB).collect();
    assert_eq!(orb.len(), 1, "the orb is delivered to");
    assert_eq!(orb[0].1.orb_accent, Some(Accent::Rose));
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
    assert!(commands.contains(
        "pub async fn change_orb_accent_setting(app: AppHandle, accent: Option<Accent>)"
    ));
    assert!(commands.contains("fields.orb_accent = accent"));
    let lib = code_only(include_str!("lib.rs"));
    assert!(lib.contains("commands::appearance::change_orb_accent_setting,"));
    let matrix = code_only(include_str!("window_commands.rs"));
    assert!(matrix.contains("\"change_orb_accent_setting\","));
}
