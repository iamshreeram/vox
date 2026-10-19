//! Pure tests for the persona commands (`list`, `save`, `delete`, `apply`).
//! Every refusal must leave the settings byte-identical.

use super::*;
use crate::managers::composer_rules::MAX_STYLE_CHARS;
use crate::managers::persona::{builtins, MAX_CUSTOM, NAME_MAX_CHARS};
use crate::settings::get_default_settings;
use serde_json::Value;

fn draft(id: &str, name: &str, style: &str, voice: Option<&str>) -> Persona {
    Persona {
        id: id.to_string(),
        name: name.to_string(),
        style: style.to_string(),
        voice: voice.map(str::to_string),
    }
}

fn json(s: &AppSettings) -> Value {
    serde_json::to_value(s).expect("settings serialize")
}

/// Keys whose JSON value differs between `before` and `after`.
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

fn active_ids(s: &AppSettings) -> Vec<String> {
    persona_views(s)
        .into_iter()
        .filter(|v| v.active)
        .map(|v| v.persona.id)
        .collect()
}

const BUILTIN_IDS: [&str; 5] = [
    "builtin-default",
    "builtin-concise",
    "builtin-coach",
    "builtin-narrator",
    "builtin-dry",
];

#[test]
fn list_has_five_builtins_first_then_customs() {
    let mut s = get_default_settings();
    save_in(&mut s, draft("", "Mine", "Be terse.", None)).expect("save");
    let views = persona_views(&s);
    assert_eq!(views.len(), 6);
    let ids: Vec<&str> = views.iter().map(|v| v.persona.id.as_str()).collect();
    assert_eq!(&ids[..5], BUILTIN_IDS);
    assert!(views[..5].iter().all(|v| v.builtin));
    assert!(!views[5].builtin);
    assert_eq!(views[5].persona.id, "custom-1");
}

#[test]
fn exactly_one_persona_is_active_at_default_settings() {
    let s = get_default_settings();
    assert_eq!(active_ids(&s), ["builtin-default"]);
}

#[test]
fn active_flag_follows_apply() {
    let mut s = get_default_settings();
    apply_in(&mut s, "builtin-concise").expect("apply");
    assert_eq!(active_ids(&s), ["builtin-concise"]);
    apply_in(&mut s, "builtin-dry").expect("apply");
    assert_eq!(active_ids(&s), ["builtin-dry"]);
}

#[test]
fn a_hand_edited_style_leaves_no_persona_active() {
    let mut s = get_default_settings();
    s.tts_composer_style = "Something the user typed by hand.".to_string();
    assert!(active_ids(&s).is_empty());
}

#[test]
fn an_applied_custom_persona_is_the_active_one() {
    let mut s = get_default_settings();
    let saved = save_in(&mut s, draft("", "Pirate", "Talk like a pirate.", None)).expect("save");
    apply_in(&mut s, &saved.id).expect("apply");
    assert_eq!(active_ids(&s), [saved.id]);
}

#[test]
fn save_appends_a_custom_persona_with_trimmed_fields() {
    let mut s = get_default_settings();
    let saved = save_in(&mut s, draft("", "  Helper  ", " Be kind. ", None)).expect("save");
    assert_eq!(saved.id, "custom-1");
    assert_eq!(saved.name, "Helper");
    assert_eq!(saved.style, "Be kind.");
    assert_eq!(s.personas, vec![saved]);
}

#[test]
fn save_with_an_existing_custom_id_replaces_it_in_place() {
    let mut s = get_default_settings();
    let first = save_in(&mut s, draft("", "First", "One.", None)).expect("save");
    save_in(&mut s, draft("", "Second", "Two.", None)).expect("save");
    let edited = save_in(&mut s, draft(&first.id, "Renamed", "One again.", None)).expect("edit");
    assert_eq!(s.personas.len(), 2);
    assert_eq!(s.personas[0], edited);
    assert_eq!(s.personas[0].name, "Renamed");
}

#[test]
fn name_length_boundaries_are_inclusive_at_forty() {
    let mut s = get_default_settings();
    let forty = "n".repeat(NAME_MAX_CHARS);
    assert!(save_in(&mut s, draft("", &forty, "x", None)).is_ok());
}

#[test]
fn style_at_the_cap_is_accepted() {
    let mut s = get_default_settings();
    let cap = "s".repeat(MAX_STYLE_CHARS);
    assert!(save_in(&mut s, draft("", "Cap", &cap, None)).is_ok());
}

#[test]
fn every_refused_save_leaves_settings_unchanged() {
    let mut s = get_default_settings();
    save_in(&mut s, draft("", "Keep", "Keep me.", None)).expect("seed");
    let cases = [
        (draft("", "   ", "x", None), PersonaError::NameEmpty),
        (
            draft("", &"n".repeat(NAME_MAX_CHARS + 1), "x", None),
            PersonaError::NameTooLong,
        ),
        (draft("", "ok", "   ", None), PersonaError::StyleEmpty),
        (
            draft("", "ok", &"s".repeat(MAX_STYLE_CHARS + 1), None),
            PersonaError::StyleTooLong,
        ),
        (
            draft("", "bad\u{7}name", "x", None),
            PersonaError::ControlChars,
        ),
        (
            draft("", "ok", "bad\nstyle", None),
            PersonaError::ControlChars,
        ),
        (
            draft("", "ok", "x", Some("not_a_voice")),
            PersonaError::UnknownVoice,
        ),
        (
            draft("builtin-default", "ok", "x", None),
            PersonaError::BuiltinImmutable,
        ),
        (
            draft("custom-99", "ok", "x", None),
            PersonaError::ReservedId,
        ),
    ];
    for (persona, expected) in cases {
        let before = json(&s);
        assert_eq!(
            save_in(&mut s, persona),
            Err(expected.clone()),
            "{expected:?}"
        );
        assert_eq!(json(&s), before, "settings changed on {expected:?}");
    }
}

#[test]
fn the_thirteenth_custom_persona_hits_the_limit_and_changes_nothing() {
    assert_eq!(MAX_CUSTOM, 12);
    let mut s = get_default_settings();
    for i in 0..MAX_CUSTOM {
        save_in(&mut s, draft("", "Same name", &format!("Style {i}."), None)).expect("fill");
    }
    let before = json(&s);
    assert_eq!(
        save_in(&mut s, draft("", "Too many", "One more.", None)),
        Err(PersonaError::LimitReached)
    );
    assert_eq!(json(&s), before);
    assert_eq!(s.personas.len(), MAX_CUSTOM);
}

#[test]
fn delete_removes_only_the_named_custom_persona() {
    let mut s = get_default_settings();
    save_in(&mut s, draft("", "One", "One.", None)).expect("save");
    save_in(&mut s, draft("", "Two", "Two.", None)).expect("save");
    delete_in(&mut s, "custom-1").expect("delete");
    let ids: Vec<&str> = s.personas.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["custom-2"]);
}

#[test]
fn every_refused_delete_leaves_settings_unchanged() {
    let mut s = get_default_settings();
    save_in(&mut s, draft("", "Keep", "Keep.", None)).expect("seed");
    let before = json(&s);
    assert_eq!(
        delete_in(&mut s, "builtin-default"),
        Err(PersonaError::BuiltinImmutable)
    );
    assert_eq!(json(&s), before);
    assert_eq!(delete_in(&mut s, "custom-7"), Err(PersonaError::NotFound));
    assert_eq!(json(&s), before);
}

#[test]
fn apply_writes_exactly_style_and_voice_when_the_persona_has_a_voice() {
    let mut s = get_default_settings();
    let before = json(&s);
    apply_in(&mut s, "builtin-concise").expect("apply");
    let concise = builtin_style("builtin-concise");
    assert_eq!(s.tts_composer_style, concise);
    assert_eq!(s.tts_natural_voice, "af_sky");
    assert_eq!(
        changed_keys(&before, &json(&s)),
        ["tts_composer_style", "tts_natural_voice"]
    );
}

#[test]
fn apply_a_persona_without_voice_keeps_the_current_natural_voice() {
    let mut s = get_default_settings();
    s.tts_composer_style = "A different style.".to_string();
    s.tts_natural_voice = "bm_lewis".to_string();
    let before = json(&s);
    apply_in(&mut s, "builtin-default").expect("apply");
    assert_eq!(s.tts_natural_voice, "bm_lewis");
    assert_eq!(changed_keys(&before, &json(&s)), ["tts_composer_style"]);
}

#[test]
fn apply_an_unknown_id_is_not_found_and_changes_nothing() {
    let mut s = get_default_settings();
    let before = json(&s);
    assert_eq!(apply_in(&mut s, "custom-42"), Err(PersonaError::NotFound));
    assert_eq!(json(&s), before);
}

#[test]
fn apply_a_custom_persona_with_a_voice_writes_that_voice() {
    let mut s = get_default_settings();
    save_in(
        &mut s,
        draft("", "Narrator two", "Tell it slowly.", Some("bf_emma")),
    )
    .expect("save");
    apply_in(&mut s, "custom-1").expect("apply");
    assert_eq!(s.tts_composer_style, "Tell it slowly.");
    assert_eq!(s.tts_natural_voice, "bf_emma");
}

fn builtin_style(id: &str) -> String {
    builtins()
        .into_iter()
        .find(|p| p.id == id)
        .map(|p| p.style)
        .expect("builtin exists")
}
