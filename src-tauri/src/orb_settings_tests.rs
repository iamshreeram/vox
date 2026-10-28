//! G1 spec: the orb settings (DESIGN.md Rev 4: 'Settings (G1)'). Authoritative: do not weaken
//! an assertion; if a test is wrong, report it.
//!
//! AppSettings gains: `chat_presentation: Option<Presentation>` (None = platform default),
//! `orb_visible: Option<bool>` (None = release default, see ORB_VISIBLE_DEFAULT),
//! `orb_size: OrbSize` (default Medium), `orb_position: Option<OrbPosition>`.
//! All four are `#[serde(default)]`, so older stores load unchanged, and they are copied from the
//! committed settings by `preserve_launch_fields` (a stale whole-object write cannot move or
//! hide the orb; dedicated commands arrive in G3).
use crate::chat_presentation::Presentation;
use crate::orb_geometry::{OrbPosition, OrbSize};
use crate::settings::{get_default_settings, preserve_launch_fields, AppSettings};

fn legacy_store() -> AppSettings {
    let mut json = serde_json::to_value(get_default_settings()).expect("serialise defaults");
    let map = json.as_object_mut().expect("settings object");
    for key in [
        "chat_presentation",
        "orb_visible",
        "orb_size",
        "orb_position",
    ] {
        map.remove(key);
    }
    serde_json::from_value(json).expect("a store without the orb keys must load")
}

fn position() -> OrbPosition {
    OrbPosition {
        monitor_name: "Built-in".into(),
        monitor_key: "Built-in|0|0|1920|1080".into(),
        fx: 0.9,
        fy: 0.8,
    }
}

#[test]
fn fresh_defaults_leave_every_choice_to_the_platform_and_release_defaults() {
    let s = get_default_settings();
    assert_eq!(s.chat_presentation, None);
    assert_eq!(s.orb_visible, None);
    assert_eq!(s.orb_size, OrbSize::Medium);
    assert_eq!(s.orb_position, None);
}

#[test]
fn an_upgraded_store_gets_the_same_defaults() {
    let s = legacy_store();
    assert_eq!(s.chat_presentation, None);
    assert_eq!(s.orb_visible, None);
    assert_eq!(s.orb_size, OrbSize::Medium);
    assert_eq!(s.orb_position, None);
}

#[test]
fn explicit_choices_survive_a_serde_round_trip() {
    let mut s = get_default_settings();
    s.chat_presentation = Some(Presentation::Window);
    s.orb_visible = Some(true);
    s.orb_size = OrbSize::Large;
    s.orb_position = Some(position());
    let back: AppSettings = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    assert_eq!(back.chat_presentation, Some(Presentation::Window));
    assert_eq!(back.orb_visible, Some(true));
    assert_eq!(back.orb_size, OrbSize::Large);
    assert_eq!(back.orb_position, Some(position()));
}

#[test]
fn a_stale_write_cannot_change_the_orb_settings() {
    let mut committed = get_default_settings();
    committed.chat_presentation = Some(Presentation::Floating);
    committed.orb_visible = Some(true);
    committed.orb_size = OrbSize::Small;
    committed.orb_position = Some(position());

    let stale = get_default_settings(); // an old snapshot with every orb field at its default
    let merged = preserve_launch_fields(stale, &committed);
    assert_eq!(merged.chat_presentation, Some(Presentation::Floating));
    assert_eq!(merged.orb_visible, Some(true));
    assert_eq!(merged.orb_size, OrbSize::Small);
    assert_eq!(merged.orb_position, Some(position()));
}

#[test]
fn the_orb_settings_reach_the_frontend_through_ipc_redaction() {
    let mut s = get_default_settings();
    s.orb_size = OrbSize::Large;
    s.orb_position = Some(position());
    let redacted = s.redacted_for_ipc();
    assert_eq!(redacted.orb_size, OrbSize::Large);
    assert_eq!(redacted.orb_position, Some(position()));
}

#[test]
fn salvaging_an_unreadable_store_keeps_the_orb_defaults() {
    let broken = serde_json::json!({ "selected_model": 42, "orb_size": "gigantic" });
    let salvaged = crate::settings::salvage_settings(&broken);
    assert_eq!(salvaged.orb_size, OrbSize::Medium);
    assert_eq!(salvaged.chat_presentation, None);
}
