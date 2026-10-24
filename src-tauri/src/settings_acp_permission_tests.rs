//! Contract tests for the ACP permission settings (DESIGN.md Revision 3: S4 "Settings",
//! C10). Written test-first: `acp_permission_mode`, `acp_permission_timeout_secs`,
//! `clamp_permission_timeout_secs` and `AppSettings::acp_permission_settings` are
//! implemented in `settings.rs` by the production step.
//! Registered in lib.rs as `#[cfg(test)] mod settings_acp_permission_tests;`.
//! Workers must NOT edit this file.

use crate::managers::acp_permission::PermissionMode;
use crate::settings::*;
use std::time::Duration;

/// The default-settings JSON with both new keys removed, as an older store would hold it.
fn json_without_new_fields() -> serde_json::Value {
    let mut value = serde_json::to_value(get_default_settings()).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("acp_permission_mode");
    object.remove("acp_permission_timeout_secs");
    value
}

#[test]
fn settings_without_new_fields_load_with_defaults() {
    let settings: AppSettings = serde_json::from_value(json_without_new_fields())
        .expect("old stores without the ACP permission keys must still parse");
    assert_eq!(settings.acp_permission_mode, PermissionMode::AskWrites);
    assert_eq!(settings.acp_permission_timeout_secs, 60);
}

#[test]
fn explicit_deny_mode_is_preserved_through_round_trip() {
    let mut settings = get_default_settings();
    settings.acp_permission_mode = PermissionMode::Deny;
    let value = serde_json::to_value(&settings).unwrap();
    let back: AppSettings = serde_json::from_value(value).unwrap();
    assert_eq!(back.acp_permission_mode, PermissionMode::Deny);
}

#[test]
fn explicit_ask_all_is_preserved() {
    let mut settings = get_default_settings();
    settings.acp_permission_mode = PermissionMode::AskAll;
    let value = serde_json::to_value(&settings).unwrap();
    let back: AppSettings = serde_json::from_value(value).unwrap();
    assert_eq!(back.acp_permission_mode, PermissionMode::AskAll);
}

#[test]
fn timeout_clamps_low_and_high() {
    assert_eq!(clamp_permission_timeout_secs(5), 10);
    assert_eq!(clamp_permission_timeout_secs(0), 10);
    assert_eq!(clamp_permission_timeout_secs(999), 300);
    assert_eq!(clamp_permission_timeout_secs(60), 60);
    assert_eq!(clamp_permission_timeout_secs(10), 10);
    assert_eq!(clamp_permission_timeout_secs(300), 300);
}

#[test]
fn acp_permission_settings_applies_clamp_and_mode() {
    let mut settings = get_default_settings();
    settings.acp_permission_mode = PermissionMode::Deny;
    settings.acp_permission_timeout_secs = 5;
    let low = settings.acp_permission_settings();
    assert_eq!(low.mode, PermissionMode::Deny);
    assert_eq!(low.approval_timeout, Duration::from_secs(10));

    settings.acp_permission_timeout_secs = 999;
    let high = settings.acp_permission_settings();
    assert_eq!(high.approval_timeout, Duration::from_secs(300));
}

#[test]
fn default_settings_use_ask_writes_and_60() {
    let defaults = get_default_settings();
    assert_eq!(defaults.acp_permission_mode, PermissionMode::AskWrites);
    assert_eq!(defaults.acp_permission_timeout_secs, 60);
}
