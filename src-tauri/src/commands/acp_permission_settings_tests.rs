//! Tests for the pure helpers behind `change_acp_permission_mode_setting` and
//! `change_acp_permission_timeout_secs_setting` (commands/acp_permission.rs).
//! Registered in commands/mod.rs as `#[cfg(test)] mod acp_permission_settings_tests;`.

use super::acp_permission::{apply_acp_permission_mode, apply_acp_permission_timeout};
use crate::managers::acp_permission::PermissionMode;
use crate::settings::get_default_settings;
use std::time::Duration;

/// The settings as JSON with the two ACP permission keys removed, so other fields can be
/// compared exactly.
fn json_without_permission_fields(settings: &crate::settings::AppSettings) -> serde_json::Value {
    let mut value = serde_json::to_value(settings).expect("settings serialize");
    let object = value.as_object_mut().expect("settings are a JSON object");
    object.remove("acp_permission_mode");
    object.remove("acp_permission_timeout_secs");
    value
}

#[test]
fn mode_round_trips_for_all_variants() {
    for mode in [
        PermissionMode::AskWrites,
        PermissionMode::AskAll,
        PermissionMode::Deny,
    ] {
        let mut settings = get_default_settings();
        apply_acp_permission_mode(&mut settings, mode);
        assert_eq!(settings.acp_permission_mode, mode);
        let json = serde_json::to_value(&settings).expect("serialize");
        let back: crate::settings::AppSettings = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.acp_permission_mode, mode);
    }
}

#[test]
fn timeout_is_clamped_when_stored() {
    let cases = [(5, 10), (0, 10), (999, 300), (60, 60)];
    for (input, expected) in cases {
        let mut settings = get_default_settings();
        apply_acp_permission_timeout(&mut settings, input);
        assert_eq!(
            settings.acp_permission_timeout_secs, expected,
            "input {input}"
        );
    }
}

#[test]
fn mode_change_leaves_other_fields_untouched() {
    let mut settings = get_default_settings();
    let before = json_without_permission_fields(&settings);
    apply_acp_permission_mode(&mut settings, PermissionMode::Deny);
    assert_eq!(json_without_permission_fields(&settings), before);
}

#[test]
fn timeout_change_leaves_other_fields_untouched() {
    let mut settings = get_default_settings();
    let before = json_without_permission_fields(&settings);
    apply_acp_permission_timeout(&mut settings, 999);
    assert_eq!(json_without_permission_fields(&settings), before);
}

#[test]
fn stored_values_are_reflected_by_acp_permission_settings_immediately() {
    let mut settings = get_default_settings();
    apply_acp_permission_mode(&mut settings, PermissionMode::AskAll);
    apply_acp_permission_timeout(&mut settings, 5);
    let policy = settings.acp_permission_settings();
    assert_eq!(policy.mode, PermissionMode::AskAll);
    assert_eq!(policy.approval_timeout, Duration::from_secs(10));

    apply_acp_permission_timeout(&mut settings, 120);
    assert_eq!(
        settings.acp_permission_settings().approval_timeout,
        Duration::from_secs(120)
    );
}
