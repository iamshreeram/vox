//! TRACK S step S-A: tests for wake word tuning validation (confidence
//! threshold, cooldown, silence timeout). Written test-first; the
//! `validate_wake_word_*` functions and bound constants are defined in
//! `settings.rs` by the production step.

use crate::settings::*;

// ---- confidence threshold ----

#[test]
fn confidence_threshold_accepts_inclusive_bounds_and_midpoint() {
    assert_eq!(validate_wake_word_confidence_threshold(0.10), Ok(0.10));
    assert_eq!(validate_wake_word_confidence_threshold(0.5), Ok(0.5));
    assert_eq!(validate_wake_word_confidence_threshold(0.95), Ok(0.95));
}

#[test]
fn confidence_threshold_bound_constants_match_contract() {
    assert_eq!(WAKE_WORD_CONFIDENCE_MIN, 0.10);
    assert_eq!(WAKE_WORD_CONFIDENCE_MAX, 0.95);
}

#[test]
fn confidence_threshold_returns_input_value_unchanged_on_ok() {
    let input: f32 = 0.7312;
    let out = validate_wake_word_confidence_threshold(input).expect("in-range value must be Ok");
    assert_eq!(out, input);
}

#[test]
fn confidence_threshold_rejects_just_outside_bounds() {
    assert!(validate_wake_word_confidence_threshold(0.0999).is_err());
    assert!(validate_wake_word_confidence_threshold(0.9501).is_err());
}

#[test]
fn confidence_threshold_rejects_zero_negative_and_one() {
    assert!(validate_wake_word_confidence_threshold(0.0).is_err());
    assert!(validate_wake_word_confidence_threshold(-1.0).is_err());
    assert!(validate_wake_word_confidence_threshold(1.0).is_err());
}

#[test]
fn confidence_threshold_rejects_non_finite_values() {
    assert!(validate_wake_word_confidence_threshold(f32::NAN).is_err());
    assert!(validate_wake_word_confidence_threshold(f32::INFINITY).is_err());
    assert!(validate_wake_word_confidence_threshold(f32::NEG_INFINITY).is_err());
}

#[test]
fn confidence_threshold_error_message_is_non_empty_and_not_a_panic_text() {
    for bad in [0.0_f32, 1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let err = validate_wake_word_confidence_threshold(bad)
            .expect_err("out-of-range or non-finite value must be Err");
        assert!(!err.trim().is_empty(), "error message empty for {bad:?}");
        assert!(
            !err.to_lowercase().contains("panic"),
            "error message must not mention a panic for {bad:?}: {err}"
        );
    }
}

// ---- cooldown ----

#[test]
fn cooldown_bound_constant_matches_contract() {
    assert_eq!(WAKE_WORD_COOLDOWN_MAX_MS, 10_000);
}

#[test]
fn cooldown_accepts_zero_default_and_max() {
    assert_eq!(validate_wake_word_cooldown_ms(0), Ok(0));
    assert_eq!(validate_wake_word_cooldown_ms(500), Ok(500));
    assert_eq!(validate_wake_word_cooldown_ms(10_000), Ok(10_000));
}

#[test]
fn cooldown_rejects_above_max() {
    assert!(validate_wake_word_cooldown_ms(10_001).is_err());
    assert!(validate_wake_word_cooldown_ms(u32::MAX).is_err());
}

#[test]
fn cooldown_error_message_is_non_empty() {
    let err = validate_wake_word_cooldown_ms(u32::MAX).expect_err("must be Err");
    assert!(!err.trim().is_empty());
}

// ---- silence timeout ----

#[test]
fn silence_timeout_bound_constant_matches_contract() {
    assert_eq!(WAKE_WORD_SILENCE_TIMEOUT_MAX_MS, 30_000);
}

#[test]
fn silence_timeout_accepts_zero_default_and_max() {
    assert_eq!(validate_wake_word_silence_timeout_ms(0), Ok(0));
    assert_eq!(validate_wake_word_silence_timeout_ms(1500), Ok(1500));
    assert_eq!(validate_wake_word_silence_timeout_ms(30_000), Ok(30_000));
}

#[test]
fn silence_timeout_rejects_above_max() {
    assert!(validate_wake_word_silence_timeout_ms(30_001).is_err());
    assert!(validate_wake_word_silence_timeout_ms(u32::MAX).is_err());
}

#[test]
fn silence_timeout_error_message_is_non_empty() {
    let err = validate_wake_word_silence_timeout_ms(u32::MAX).expect_err("must be Err");
    assert!(!err.trim().is_empty());
}

// ---- defaults must satisfy their own validators ----

#[test]
fn default_wake_word_tuning_values_pass_their_validators() {
    let defaults = get_default_settings();
    assert!(
        validate_wake_word_confidence_threshold(defaults.wake_word_confidence_threshold).is_ok(),
        "default confidence threshold must validate"
    );
    assert!(
        validate_wake_word_cooldown_ms(defaults.wake_word_cooldown_ms).is_ok(),
        "default cooldown must validate"
    );
    assert!(
        validate_wake_word_silence_timeout_ms(defaults.wake_word_silence_timeout_ms).is_ok(),
        "default silence timeout must validate"
    );
}

// ---- serde round trip ----

#[test]
fn wake_word_tuning_fields_survive_json_round_trip() {
    let mut settings = get_default_settings();
    settings.wake_word_confidence_threshold = 0.75;
    settings.wake_word_cooldown_ms = 2_000;
    settings.wake_word_silence_timeout_ms = 0;

    let json = serde_json::to_value(&settings).expect("serialize AppSettings");
    let back: AppSettings = serde_json::from_value(json).expect("deserialize AppSettings");

    assert_eq!(back.wake_word_confidence_threshold, 0.75);
    assert_eq!(back.wake_word_cooldown_ms, 2_000);
    assert_eq!(back.wake_word_silence_timeout_ms, 0);
}

#[test]
fn legacy_json_without_wake_word_tuning_keys_deserializes_to_defaults() {
    // Start from a fully serialized current object so every other field is
    // present, then strip only the three tuning keys to simulate legacy data.
    let defaults = get_default_settings();
    let mut json = serde_json::to_value(&defaults).expect("serialize AppSettings");
    let obj = json
        .as_object_mut()
        .expect("AppSettings serializes to an object");
    assert!(obj.remove("wake_word_confidence_threshold").is_some());
    assert!(obj.remove("wake_word_cooldown_ms").is_some());
    assert!(obj.remove("wake_word_silence_timeout_ms").is_some());

    let legacy: AppSettings = serde_json::from_value(json).expect("legacy JSON must deserialize");

    assert_eq!(legacy.wake_word_confidence_threshold, 0.5);
    assert_eq!(legacy.wake_word_cooldown_ms, 500);
    assert_eq!(legacy.wake_word_silence_timeout_ms, 1500);
}
