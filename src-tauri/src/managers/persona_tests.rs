//! Authoritative persona contract tests (step T4, red). Deterministic, no network.
//! These define correctness for `managers::persona`; do not weaken them.
use super::*;
use crate::managers::composer_rules::{DEFAULT_STYLE, MAX_STYLE_CHARS};
use crate::managers::converse::SessionConfig;
use crate::managers::natural_voices::NATURAL_VOICES;
use crate::settings::{get_default_settings, AppSettings, SpeakerSensitivity};

fn custom(id: &str, name: &str, style: &str, voice: Option<&str>) -> Persona {
    Persona {
        id: id.to_string(),
        name: name.to_string(),
        style: style.to_string(),
        voice: voice.map(str::to_string),
    }
}

/// A new persona as the UI submits it: no id yet.
fn draft(name: &str, style: &str) -> Persona {
    custom("", name, style, None)
}

fn builtin(id: &str) -> Persona {
    builtins()
        .into_iter()
        .find(|p| p.id == id)
        .expect("builtin id present")
}

/// The numeric part of a `custom-<n>` id.
fn custom_number(id: &str) -> u64 {
    id.strip_prefix("custom-")
        .and_then(|n| n.parse().ok())
        .expect("custom-<n> id")
}

// ---------- name validation ----------

#[test]
fn name_length_boundaries_are_zero_one_forty_and_forty_one() {
    assert_eq!(
        validate(&draft("", "warm")).unwrap_err(),
        PersonaError::NameEmpty
    );
    assert!(validate(&draft("a", "warm")).is_ok());
    assert!(validate(&draft(&"n".repeat(NAME_MAX_CHARS), "warm")).is_ok());
    assert_eq!(NAME_MAX_CHARS, 40);
    assert_eq!(
        validate(&draft(&"n".repeat(41), "warm")).unwrap_err(),
        PersonaError::NameTooLong
    );
}

#[test]
fn name_length_counts_characters_not_bytes() {
    // 40 two-byte chars (80 bytes) is still a valid 40-char name.
    assert!(validate(&draft(&"é".repeat(40), "warm")).is_ok());
    assert_eq!(
        validate(&draft(&"é".repeat(41), "warm")).unwrap_err(),
        PersonaError::NameTooLong
    );
    // Four-byte emoji count as one character each.
    let mic = "\u{1F399}";
    assert_eq!(mic.chars().count(), 1);
    assert!(validate(&draft(&mic.repeat(40), "warm")).is_ok());
    assert_eq!(
        validate(&draft(&mic.repeat(41), "warm")).unwrap_err(),
        PersonaError::NameTooLong
    );
}

#[test]
fn whitespace_only_name_is_empty_and_outer_spaces_are_trimmed() {
    for blank in ["   ", " ", "      "] {
        assert_eq!(
            validate(&draft(blank, "warm")).unwrap_err(),
            PersonaError::NameEmpty,
            "{blank:?}"
        );
    }
    // Outer spaces do not count toward the 40-char limit and are removed.
    let padded = format!("  {}  ", "n".repeat(40));
    assert_eq!(
        validate(&draft(&padded, "warm")).unwrap().name,
        "n".repeat(40)
    );
}

// ---------- style validation ----------

#[test]
fn style_empty_cap_and_cap_plus_one() {
    assert_eq!(MAX_STYLE_CHARS, 300, "style cap is the build_system cap");
    assert_eq!(
        validate(&draft("Name", "")).unwrap_err(),
        PersonaError::StyleEmpty
    );
    assert_eq!(
        validate(&draft("Name", "   ")).unwrap_err(),
        PersonaError::StyleEmpty
    );
    assert!(validate(&draft("Name", &"s".repeat(MAX_STYLE_CHARS))).is_ok());
    assert_eq!(
        validate(&draft("Name", &"s".repeat(MAX_STYLE_CHARS + 1))).unwrap_err(),
        PersonaError::StyleTooLong
    );
}

#[test]
fn style_is_trimmed_and_the_cap_is_measured_after_trimming() {
    let ok = validate(&draft("Name", "  warm and brief  ")).unwrap();
    assert_eq!(ok.style, "warm and brief");
    let padded = format!("  {}  ", "s".repeat(MAX_STYLE_CHARS));
    assert_eq!(
        validate(&draft("Name", &padded)).unwrap().style,
        "s".repeat(MAX_STYLE_CHARS)
    );
    let padded_over = format!("  {}  ", "s".repeat(MAX_STYLE_CHARS + 1));
    assert_eq!(
        validate(&draft("Name", &padded_over)).unwrap_err(),
        PersonaError::StyleTooLong
    );
}

// ---------- control characters ----------

#[test]
fn control_characters_in_name_are_rejected() {
    for ctl in ["\n", "\t", "\u{0}", "\u{7f}"] {
        let name = format!("ab{ctl}cd");
        assert_eq!(
            validate(&draft(&name, "warm")).unwrap_err(),
            PersonaError::ControlChars,
            "{ctl:?}"
        );
    }
    assert_eq!(
        validate(&draft("\u{0}ab", "warm")).unwrap_err(),
        PersonaError::ControlChars
    );
}

#[test]
fn control_characters_in_style_are_rejected() {
    for ctl in ["\n", "\t", "\u{0}", "\u{7f}"] {
        let style = format!("warm{ctl}brief");
        assert_eq!(
            validate(&draft("Name", &style)).unwrap_err(),
            PersonaError::ControlChars,
            "{ctl:?}"
        );
    }
}

// ---------- unicode ----------

#[test]
fn unicode_names_and_styles_within_limits_are_accepted_unchanged() {
    // Accent plus a non-ASCII symbol (U+2615) inside the name, accents in the style.
    let name = "Caf\u{e9} \u{2615} crew";
    let style = "chaleureux, précis — naïve";
    let p = validate(&draft(name, style)).expect("unicode accepted");
    assert_eq!(p.name, name);
    assert_eq!(p.style, style);
}

// ---------- voice validation ----------

#[test]
fn invalid_voice_ids_are_rejected() {
    for bad in [
        "zf_xiaoni",
        "AF_HEART",
        "af_heart ",
        "../../etc/passwd",
        "",
        "af_",
    ] {
        let p = custom("custom-1", "Name", "warm", Some(bad));
        assert_eq!(
            validate(&p).unwrap_err(),
            PersonaError::UnknownVoice,
            "{bad:?}"
        );
    }
}

#[test]
fn every_curated_voice_and_no_voice_are_accepted() {
    assert_eq!(NATURAL_VOICES.len(), 28);
    for v in NATURAL_VOICES {
        let p = custom("custom-1", "Name", "warm", Some(v.id));
        assert!(validate(&p).is_ok(), "{}", v.id);
    }
    assert!(validate(&custom("custom-1", "Name", "warm", None)).is_ok());
}

// ---------- built-ins ----------

#[test]
fn there_are_exactly_five_builtins_with_the_reserved_prefix() {
    let b = builtins();
    assert_eq!(b.len(), 5);
    let mut ids: Vec<&str> = b.iter().map(|p| p.id.as_str()).collect();
    ids.sort_unstable();
    let mut expected = vec![
        "builtin-coach",
        "builtin-concise",
        "builtin-default",
        "builtin-dry",
        "builtin-narrator",
    ];
    expected.sort_unstable();
    assert_eq!(ids, expected);
    assert_eq!(BUILTIN_PREFIX, "builtin-");
    for p in &b {
        assert!(p.id.starts_with(BUILTIN_PREFIX), "{}", p.id);
    }
}

#[test]
fn every_builtin_passes_validation_and_has_a_non_empty_style() {
    for p in builtins() {
        assert!(!p.style.trim().is_empty(), "{}", p.id);
        assert!(validate(&p).is_ok(), "{}", p.id);
    }
}

#[test]
fn builtin_default_matches_the_default_setting_and_has_no_voice() {
    let d = builtin("builtin-default");
    assert_eq!(d.style, get_default_settings().tts_composer_style);
    assert_eq!(d.style, DEFAULT_STYLE);
    assert_eq!(d.voice, None);
}

// ---------- save ----------

#[test]
fn saved_ids_are_unique_and_monotonic_even_after_deletes() {
    let mut c: Vec<Persona> = Vec::new();
    let a = save(&mut c, draft("A", "warm")).unwrap();
    let b = save(&mut c, draft("B", "warm")).unwrap();
    assert!(a.id.starts_with("custom-"), "{}", a.id);
    assert_ne!(a.id, b.id);
    delete(&mut c, &a.id).unwrap();

    let d = save(&mut c, draft("D", "calm")).unwrap();
    assert!(d.id.starts_with("custom-"), "{}", d.id);
    let ids: Vec<&str> = c.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(
        ids.iter().filter(|id| **id == d.id).count(),
        1,
        "new id must be unique in the list"
    );
    assert_eq!(c.last().unwrap(), &d, "new persona is appended");
    assert!(
        custom_number(&d.id) > custom_number(&b.id),
        "ids are monotonic, never reusing a live number"
    );
}

#[test]
fn updating_an_existing_custom_persona_keeps_its_position() {
    let mut c = vec![
        custom("custom-1", "A", "warm", None),
        custom("custom-2", "B", "calm", None),
        custom("custom-3", "C", "dry", None),
    ];
    let updated = save(&mut c, custom("custom-2", "B2", "brief", Some("bf_emma"))).unwrap();
    assert_eq!(updated.id, "custom-2");
    let ids: Vec<&str> = c.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, vec!["custom-1", "custom-2", "custom-3"]);
    assert_eq!(c[1].name, "B2");
    assert_eq!(c[1].style, "brief");
    assert_eq!(c[1].voice.as_deref(), Some("bf_emma"));
}

#[test]
fn saving_with_a_builtin_id_is_rejected_as_immutable() {
    let mut c: Vec<Persona> = Vec::new();
    for id in ["builtin-default", "builtin-dry"] {
        assert_eq!(
            save(&mut c, custom(id, "X", "warm", None)).unwrap_err(),
            PersonaError::BuiltinImmutable,
            "{id}"
        );
    }
    assert!(c.is_empty());
}

#[test]
fn saving_with_a_bogus_non_custom_id_is_reserved() {
    let mut c: Vec<Persona> = Vec::new();
    for id in ["bogus", "custom", "x-1", "Custom-1"] {
        assert_eq!(
            save(&mut c, custom(id, "X", "warm", None)).unwrap_err(),
            PersonaError::ReservedId,
            "{id}"
        );
    }
    assert!(c.is_empty());
}

#[test]
fn the_twelfth_custom_persona_is_ok_and_the_thirteenth_is_refused() {
    assert_eq!(MAX_CUSTOM, 12);
    let mut c: Vec<Persona> = Vec::new();
    for i in 0..MAX_CUSTOM {
        save(&mut c, draft(&format!("P{i}"), "warm")).unwrap();
    }
    assert_eq!(c.len(), 12);
    assert_eq!(
        save(&mut c, draft("Extra", "warm")).unwrap_err(),
        PersonaError::LimitReached
    );
    assert_eq!(c.len(), 12);
}

#[test]
fn updating_an_existing_persona_at_the_limit_is_still_allowed() {
    let mut c: Vec<Persona> = Vec::new();
    for i in 0..MAX_CUSTOM {
        save(&mut c, draft(&format!("P{i}"), "warm")).unwrap();
    }
    let first_id = c[0].id.clone();
    let edited = save(&mut c, custom(&first_id, "Edited", "calm", None)).unwrap();
    assert_eq!(edited.name, "Edited");
    assert_eq!(c.len(), 12);
}

#[test]
fn validation_errors_win_over_the_limit_for_an_invalid_persona() {
    let mut c: Vec<Persona> = Vec::new();
    for i in 0..MAX_CUSTOM {
        save(&mut c, draft(&format!("P{i}"), "warm")).unwrap();
    }
    assert_eq!(
        save(&mut c, draft("", "warm")).unwrap_err(),
        PersonaError::NameEmpty
    );
    assert_eq!(
        save(&mut c, draft("Ok", "")).unwrap_err(),
        PersonaError::StyleEmpty
    );
    assert_eq!(
        save(&mut c, custom("", "Ok", "warm", Some("zz_bogus"))).unwrap_err(),
        PersonaError::UnknownVoice
    );
    assert_eq!(
        save(&mut c, draft("Ok", "warm")).unwrap_err(),
        PersonaError::LimitReached
    );
}

#[test]
fn duplicate_names_are_allowed_with_distinct_ids() {
    let mut c: Vec<Persona> = Vec::new();
    let first = save(&mut c, draft("Same", "warm")).unwrap();
    let second = save(&mut c, draft("Same", "calm")).unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(c.len(), 2);
    assert_eq!(c[0].name, c[1].name);
}

// ---------- delete ----------

#[test]
fn deleting_a_builtin_is_rejected_and_changes_nothing() {
    let mut c = vec![custom("custom-1", "A", "warm", None)];
    assert_eq!(
        delete(&mut c, "builtin-default").unwrap_err(),
        PersonaError::BuiltinImmutable
    );
    assert_eq!(c.len(), 1);
}

#[test]
fn deleting_an_unknown_id_is_not_found() {
    let mut c = vec![custom("custom-1", "A", "warm", None)];
    for id in ["custom-99", "nope", ""] {
        assert_eq!(
            delete(&mut c, id).unwrap_err(),
            PersonaError::NotFound,
            "{id:?}"
        );
    }
    assert_eq!(c.len(), 1);
}

#[test]
fn delete_removes_only_that_persona_and_keeps_order() {
    let mut c = vec![
        custom("custom-1", "A", "warm", None),
        custom("custom-2", "B", "calm", None),
        custom("custom-3", "C", "dry", None),
    ];
    delete(&mut c, "custom-2").unwrap();
    let ids: Vec<&str> = c.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, vec!["custom-1", "custom-3"]);
}

// ---------- all ----------

#[test]
fn all_lists_builtins_first_then_customs_in_stored_order() {
    let c = vec![
        custom("custom-1", "A", "warm", None),
        custom("custom-2", "B", "calm", None),
    ];
    let merged = all(&c);
    assert_eq!(merged.len(), 7);
    assert_eq!(merged[..5].to_vec(), builtins());
    assert_eq!(merged[5..].to_vec(), c);
    assert_eq!(all(&[]), builtins());
}

// ---------- apply ----------

#[test]
fn apply_without_a_voice_leaves_the_natural_voice_untouched() {
    let mut s = get_default_settings();
    s.tts_natural_voice = "bf_emma".to_string();
    let p = custom("custom-1", "Pirate", "talk like a terse pirate", None);
    apply(&mut s, &p);
    assert_eq!(s.tts_composer_style, "talk like a terse pirate");
    assert_eq!(s.tts_natural_voice, "bf_emma");
}

#[test]
fn apply_with_a_curated_voice_sets_only_that_field() {
    let mut s = get_default_settings();
    let p = custom("custom-1", "Calm", "speak slowly", Some("am_adam"));
    apply(&mut s, &p);
    assert_eq!(s.tts_composer_style, "speak slowly");
    assert_eq!(s.tts_natural_voice, "am_adam");
}

#[test]
fn apply_with_an_uncurated_voice_leaves_the_natural_voice_untouched() {
    let mut s = get_default_settings();
    s.tts_natural_voice = "bf_emma".to_string();
    let p = custom("custom-1", "Odd", "speak oddly", Some("zz_bogus"));
    apply(&mut s, &p);
    assert_eq!(s.tts_natural_voice, "bf_emma");
}

#[test]
fn apply_changes_no_setting_other_than_style_and_voice() {
    let mut s = get_default_settings();
    let mut before = serde_json::to_value(&s).unwrap();
    apply(&mut s, &builtin("builtin-coach"));
    let mut after = serde_json::to_value(&s).unwrap();
    for key in ["tts_composer_style", "tts_natural_voice"] {
        before.as_object_mut().unwrap().remove(key);
        after.as_object_mut().unwrap().remove(key);
    }
    assert_eq!(before, after);
}

// ---------- active_id ----------

#[test]
fn default_settings_are_active_as_builtin_default() {
    let s = get_default_settings();
    assert_eq!(active_id(&s), Some("builtin-default".to_string()));
}

#[test]
fn applying_a_builtin_makes_it_active() {
    let mut s = get_default_settings();
    apply(&mut s, &builtin("builtin-coach"));
    assert_eq!(active_id(&s), Some("builtin-coach".to_string()));
}

#[test]
fn a_manual_style_edit_clears_the_active_id() {
    let mut s = get_default_settings();
    apply(&mut s, &builtin("builtin-coach"));
    s.tts_composer_style.push_str(" plus something else");
    assert_eq!(active_id(&s), None);
}

#[test]
fn a_custom_twin_of_a_builtin_loses_to_the_builtin() {
    let coach = builtin("builtin-coach");
    let twin = custom("custom-1", "Twin", &coach.style, coach.voice.as_deref());
    let mut s = get_default_settings();
    s.personas = vec![twin.clone()];
    apply(&mut s, &twin);
    assert_eq!(active_id(&s), Some("builtin-coach".to_string()));
}

#[test]
fn deleting_the_active_custom_persona_keeps_the_style_and_clears_active_id() {
    let p = custom(
        "custom-1",
        "Pirate",
        "talk like a terse pirate",
        Some("bm_lewis"),
    );
    let mut s = get_default_settings();
    s.personas = vec![p.clone()];
    apply(&mut s, &p);
    assert_eq!(active_id(&s), Some("custom-1".to_string()));

    delete(&mut s.personas, "custom-1").unwrap();
    assert_eq!(s.tts_composer_style, p.style);
    assert_eq!(active_id(&s), None);
}

// ---------- settings JSON ----------

#[test]
fn settings_with_two_personas_roundtrip_through_json() {
    let mut s = get_default_settings();
    s.personas = vec![
        custom("custom-1", "Mine", "warm and brief", Some("am_adam")),
        custom("custom-2", "Plain", "dry wit", None),
    ];
    let v = serde_json::to_value(&s).unwrap();
    let loaded: AppSettings = serde_json::from_value(v.clone()).unwrap();
    assert_eq!(loaded.personas, s.personas);
    assert_eq!(serde_json::to_value(&loaded).unwrap(), v);
}

#[test]
fn old_settings_without_the_new_keys_load_with_defaults() {
    let mut v = serde_json::to_value(get_default_settings()).unwrap();
    let object = v.as_object_mut().unwrap();
    for key in [
        "converse_smart_turn",
        "converse_latency_panel",
        "speaker_filter_enabled",
        "speaker_filter_sensitivity",
        "personas",
    ] {
        object.remove(key);
    }
    let loaded: AppSettings = serde_json::from_value(v).expect("old settings must still load");
    assert!(!loaded.converse_smart_turn);
    assert!(!loaded.converse_latency_panel);
    assert!(!loaded.speaker_filter_enabled);
    assert_eq!(
        loaded.speaker_filter_sensitivity,
        SpeakerSensitivity::Balanced
    );
    assert!(loaded.personas.is_empty());
}

#[test]
fn a_persona_with_unknown_fields_and_no_voice_still_loads() {
    let mut v = serde_json::to_value(get_default_settings()).unwrap();
    v["personas"] = serde_json::json!([{
        "id": "custom-1",
        "name": "Mine",
        "style": "warm",
        "future_field": {"x": 1}
    }]);
    let loaded: AppSettings = serde_json::from_value(v).expect("persona with extra fields loads");
    assert_eq!(
        loaded.personas,
        vec![custom("custom-1", "Mine", "warm", None)]
    );
}

// ---------- SessionConfig (A6) ----------

#[test]
fn session_config_after_apply_uses_the_persona_style() {
    let mut s = get_default_settings();
    let p = custom("custom-1", "Pirate", "talk like a terse pirate", None);
    apply(&mut s, &p);
    assert_eq!(
        SessionConfig::from_settings(&s).style,
        "talk like a terse pirate"
    );
}

#[test]
fn an_existing_session_config_snapshot_does_not_change_after_apply() {
    let mut s = get_default_settings();
    let snapshot = SessionConfig::from_settings(&s);
    let old_style = snapshot.style.clone();

    apply(&mut s, &custom("custom-1", "Brief", "a new style", None));

    assert_eq!(snapshot.style, old_style, "snapshot is a copy");
    assert_eq!(SessionConfig::from_settings(&s).style, "a new style");
}
