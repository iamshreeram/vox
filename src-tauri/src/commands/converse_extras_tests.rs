//! Pure tests for the four experimental converse toggles. Each applier takes only
//! `&mut AppSettings`, so it cannot reach a model manager by construction.

use super::*;
use crate::settings::{get_default_settings, SpeakerSensitivity};
use serde_json::Value;

/// Keys whose JSON value differs between `before` and `after` (both objects).
fn changed_keys(before: &Value, after: &Value) -> Vec<String> {
    let (Some(b), Some(a)) = (before.as_object(), after.as_object()) else {
        panic!("settings must serialize to a JSON object");
    };
    let mut keys: Vec<String> = b
        .keys()
        .chain(a.keys())
        .filter(|k| b.get(*k) != a.get(*k))
        .cloned()
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

fn json(s: &AppSettings) -> Value {
    serde_json::to_value(s).expect("settings serialize")
}

#[test]
fn appliers_take_only_settings_and_no_manager() {
    // Compile-time guard: a signature change that adds a manager parameter breaks this.
    let _: fn(&mut AppSettings, bool) = apply_smart_turn;
    let _: fn(&mut AppSettings, bool) = apply_latency_panel;
    let _: fn(&mut AppSettings, bool) = apply_speaker_filter_enabled;
    let _: fn(&mut AppSettings, SpeakerSensitivity) = apply_speaker_filter_sensitivity;
}

#[test]
fn smart_turn_sets_only_its_field_and_is_idempotent() {
    let mut s = get_default_settings();
    assert!(!s.converse_smart_turn);
    let before = json(&s);
    apply_smart_turn(&mut s, true);
    assert!(s.converse_smart_turn);
    assert_eq!(changed_keys(&before, &json(&s)), ["converse_smart_turn"]);
    let once = json(&s);
    apply_smart_turn(&mut s, true);
    assert_eq!(json(&s), once);
    apply_smart_turn(&mut s, false);
    assert!(!s.converse_smart_turn);
    assert_eq!(changed_keys(&once, &json(&s)), ["converse_smart_turn"]);
}

#[test]
fn latency_panel_sets_only_its_field_and_is_idempotent() {
    let mut s = get_default_settings();
    assert!(!s.converse_latency_panel);
    let before = json(&s);
    apply_latency_panel(&mut s, true);
    assert!(s.converse_latency_panel);
    assert_eq!(changed_keys(&before, &json(&s)), ["converse_latency_panel"]);
    let once = json(&s);
    apply_latency_panel(&mut s, true);
    assert_eq!(json(&s), once);
}

#[test]
fn speaker_filter_enabled_sets_only_its_field_and_is_idempotent() {
    let mut s = get_default_settings();
    assert!(!s.speaker_filter_enabled);
    let before = json(&s);
    apply_speaker_filter_enabled(&mut s, true);
    assert!(s.speaker_filter_enabled);
    assert_eq!(changed_keys(&before, &json(&s)), ["speaker_filter_enabled"]);
    let once = json(&s);
    apply_speaker_filter_enabled(&mut s, true);
    assert_eq!(json(&s), once);
    apply_speaker_filter_enabled(&mut s, false);
    assert!(!s.speaker_filter_enabled);
}

#[test]
fn speaker_sensitivity_sets_only_its_field_and_is_idempotent() {
    let mut s = get_default_settings();
    assert_eq!(s.speaker_filter_sensitivity, SpeakerSensitivity::Balanced);
    let before = json(&s);
    apply_speaker_filter_sensitivity(&mut s, SpeakerSensitivity::Strict);
    assert_eq!(s.speaker_filter_sensitivity, SpeakerSensitivity::Strict);
    assert_eq!(
        changed_keys(&before, &json(&s)),
        ["speaker_filter_sensitivity"]
    );
    let once = json(&s);
    apply_speaker_filter_sensitivity(&mut s, SpeakerSensitivity::Strict);
    assert_eq!(json(&s), once);
    apply_speaker_filter_sensitivity(&mut s, SpeakerSensitivity::Lenient);
    assert_eq!(s.speaker_filter_sensitivity, SpeakerSensitivity::Lenient);
}

#[test]
fn sensitivity_serializes_as_snake_case() {
    assert_eq!(
        serde_json::to_value(SpeakerSensitivity::Strict).expect("serialize"),
        Value::String("strict".into())
    );
}
