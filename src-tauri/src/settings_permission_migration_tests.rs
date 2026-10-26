//! Contract tests for the "Allow everything" default and the settings migration
//! (DESIGN-PR2-STAGE2 Revision 3, section 2.7, C11). Red until the production step
//! changes the fresh-store default and seeds `salvage_settings` for existing stores.
//!
//! This file is a child module of `settings` (wired by a `#[path]` line at the end of
//! `settings.rs`), because `salvage_settings` is private and is the existing-store
//! recovery entry point. Store creation and `get_settings` need a Tauri AppHandle and
//! are not reachable from a unit test, so the fresh-store branch is covered through
//! `get_default_settings()`, which that branch calls. Workers must not edit this file.

use super::*;
use crate::managers::acp_permission::PermissionMode;
use serde_json::{json, Value};

/// The full default settings as an existing store would hold them, minus the permission key.
fn stored_without_permission_key() -> Value {
    let mut value = serde_json::to_value(get_default_settings()).unwrap();
    value.as_object_mut().unwrap().remove("acp_permission_mode");
    value
}

/// The full default settings with the permission key set to `raw`.
fn stored_with_permission(raw: Value) -> Value {
    let mut value = serde_json::to_value(get_default_settings()).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("acp_permission_mode".to_string(), raw);
    value
}

/// A stored value where an unrelated enum field (`theme`) is invalid.
fn with_invalid_unrelated_enum(mut value: Value) -> Value {
    value
        .as_object_mut()
        .unwrap()
        .insert("theme".to_string(), json!("neon"));
    value
}

// ---- fresh store and serde defaults ----

#[test]
fn app_settings_default_is_allow_all() {
    assert_eq!(
        AppSettings::default().acp_permission_mode,
        PermissionMode::AllowAll
    );
}

#[test]
fn fresh_store_defaults_are_allow_all() {
    assert_eq!(
        get_default_settings().acp_permission_mode,
        PermissionMode::AllowAll
    );
}

#[test]
fn enum_default_stays_ask_writes_for_serde_missing_keys() {
    assert_eq!(PermissionMode::default(), PermissionMode::AskWrites);
}

#[test]
fn existing_settings_json_with_the_key_missing_deserialises_to_ask_writes() {
    let settings: AppSettings =
        serde_json::from_value(stored_without_permission_key()).expect("old store must parse");
    assert_eq!(settings.acp_permission_mode, PermissionMode::AskWrites);
}

// ---- existing store recovery (salvage_settings) ----

#[test]
fn existing_store_missing_permission_key_salvages_to_ask_writes() {
    let salvaged = salvage_settings(&stored_without_permission_key());
    assert_eq!(salvaged.acp_permission_mode, PermissionMode::AskWrites);
}

#[test]
fn existing_store_with_invalid_permission_value_salvages_to_ask_writes() {
    for raw in [
        json!("allow_everything"),
        json!("AllowAll"),
        json!(42),
        json!(true),
        json!(null),
        json!({}),
    ] {
        let salvaged = salvage_settings(&stored_with_permission(raw.clone()));
        assert_eq!(
            salvaged.acp_permission_mode,
            PermissionMode::AskWrites,
            "invalid stored value {raw}"
        );
    }
}

#[test]
fn existing_store_with_unrelated_invalid_enum_salvages_to_ask_writes() {
    // The permission key is missing, and an unrelated enum field is invalid.
    let stored = with_invalid_unrelated_enum(stored_without_permission_key());
    let salvaged = salvage_settings(&stored);
    assert_eq!(salvaged.acp_permission_mode, PermissionMode::AskWrites);
    assert_eq!(
        salvaged.theme,
        Theme::System,
        "the broken field falls back to its default"
    );
}

#[test]
fn unrelated_invalid_enum_does_not_reset_other_valid_fields() {
    let mut stored = stored_without_permission_key();
    stored
        .as_object_mut()
        .unwrap()
        .insert("selected_language".to_string(), json!("fr"));
    let salvaged = salvage_settings(&with_invalid_unrelated_enum(stored));
    assert_eq!(salvaged.selected_language, "fr");
    assert_eq!(salvaged.acp_permission_mode, PermissionMode::AskWrites);
}

#[test]
fn existing_store_that_is_not_an_object_salvages_to_ask_writes() {
    for raw in [json!("garbage"), json!(null), json!([1, 2]), json!(7)] {
        let salvaged = salvage_settings(&raw);
        assert_eq!(
            salvaged.acp_permission_mode,
            PermissionMode::AskWrites,
            "non-object stored value {raw}"
        );
    }
}

#[test]
fn valid_saved_permission_modes_are_always_preserved_by_salvage() {
    let cases = [
        ("ask_writes", PermissionMode::AskWrites),
        ("ask_all", PermissionMode::AskAll),
        ("deny", PermissionMode::Deny),
        ("allow_all", PermissionMode::AllowAll),
    ];
    for (wire, mode) in cases {
        let salvaged = salvage_settings(&stored_with_permission(json!(wire)));
        assert_eq!(salvaged.acp_permission_mode, mode, "saved {wire}");
    }
}

#[test]
fn valid_saved_permission_mode_survives_an_unrelated_invalid_enum() {
    let cases = [
        ("ask_writes", PermissionMode::AskWrites),
        ("ask_all", PermissionMode::AskAll),
        ("deny", PermissionMode::Deny),
        ("allow_all", PermissionMode::AllowAll),
    ];
    for (wire, mode) in cases {
        let stored = with_invalid_unrelated_enum(stored_with_permission(json!(wire)));
        let salvaged = salvage_settings(&stored);
        assert_eq!(
            salvaged.acp_permission_mode, mode,
            "saved {wire} with a broken theme"
        );
    }
}

#[test]
fn saved_ask_writes_is_not_upgraded_to_allow_all_by_salvage() {
    let salvaged = salvage_settings(&stored_with_permission(json!("ask_writes")));
    assert_eq!(salvaged.acp_permission_mode, PermissionMode::AskWrites);
}

#[test]
fn missing_conversation_agent_handoff_key_salvages_to_on() {
    // An existing store written before the setting existed keeps the default (on).
    let mut stored = serde_json::to_value(get_default_settings()).unwrap();
    stored
        .as_object_mut()
        .unwrap()
        .remove("conversation_agent_handoff");
    assert!(salvage_settings(&stored).conversation_agent_handoff);
}

#[test]
fn saved_conversation_agent_handoff_false_is_preserved_by_salvage() {
    let mut stored = serde_json::to_value(get_default_settings()).unwrap();
    stored
        .as_object_mut()
        .unwrap()
        .insert("conversation_agent_handoff".to_string(), json!(false));
    assert!(!salvage_settings(&stored).conversation_agent_handoff);
}
