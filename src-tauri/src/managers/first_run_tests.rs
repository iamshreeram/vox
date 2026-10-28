//! P4 spec: first-run voice and Brain steps (DESIGN.md Rev 2 'First run (C4)', Rev 3/4 consent).
//! Authoritative: do not weaken an assertion; if a test is wrong, report it instead of editing it.
//!
//! Contract under test (module `crate::managers::first_run`):
//!   enum FirstRunStep { Voice, Brain }                      (Copy, Debug, PartialEq, serde snake_case, specta Type)
//!   enum FirstRunOutcome { Unset, Skipped, Done }           (Copy, Debug, PartialEq, Default = Unset, serde snake_case, specta Type)
//!   fn next_step(&AppSettings) -> Option<FirstRunStep>
//!   fn finish_step(&mut AppSettings, FirstRunStep, FirstRunOutcome) -> Result<(), String>
//! AppSettings fields: first_run_setup_pending: bool (serde default false; true in get_default_settings),
//!   first_run_tts: FirstRunOutcome, first_run_brain: FirstRunOutcome (both serde default Unset).
use crate::managers::brain::{self, BrainFeature};
use crate::managers::first_run::*;
use crate::settings::{get_default_settings, AppSettings, BrainSource, SecretString};

fn fresh() -> AppSettings {
    get_default_settings()
}

/// A store written before this feature existed, loaded the way an upgrade loads it.
fn legacy_store() -> AppSettings {
    let mut json = serde_json::to_value(get_default_settings()).expect("serialise defaults");
    let map = json.as_object_mut().expect("settings object");
    for key in ["first_run_setup_pending", "first_run_tts", "first_run_brain"] {
        map.remove(key);
    }
    serde_json::from_value(json).expect("a store without the new keys must load")
}

fn brain_ready(settings: &mut AppSettings) {
    brain::apply_connection(
        settings,
        "custom",
        "http://localhost:11434/v1",
        "llama3.2",
    )
    .expect("loopback connection is valid");
    settings.brain_token = SecretString::from("");
    settings.brain_test_ack_host = brain::destination("custom", "http://localhost:11434/v1");
    brain::use_brain_checked(settings, BrainFeature::Composer).expect("complete connection");
}

#[test]
fn a_fresh_install_starts_first_run_setup_with_both_steps_unset() {
    let s = fresh();
    assert!(s.first_run_setup_pending);
    assert_eq!(s.first_run_tts, FirstRunOutcome::Unset);
    assert_eq!(s.first_run_brain, FirstRunOutcome::Unset);
    assert_eq!(next_step(&s), Some(FirstRunStep::Voice));
}

#[test]
fn an_upgraded_store_never_sees_the_new_steps() {
    let s = legacy_store();
    assert!(!s.first_run_setup_pending, "a missing key means no first run");
    assert_eq!(next_step(&s), None);
}

#[test]
fn an_upgraded_store_with_nothing_configured_still_sees_no_steps() {
    // Default-valued existing store: the Brain is unset and spoken replies are off, yet no wizard.
    let s = legacy_store();
    assert!(s.brain_provider_id.is_empty());
    assert!(!s.tts_enabled);
    assert_eq!(next_step(&s), None);
}

#[test]
fn salvaged_or_unreadable_stores_are_not_fresh() {
    let broken = serde_json::json!({ "settings": { "selected_model": 42, "onboarding_completed": true } });
    let salvaged = crate::settings::salvage_settings(&broken["settings"]);
    assert!(!salvaged.first_run_setup_pending);
    assert_eq!(next_step(&salvaged), None);
}

#[test]
fn steps_come_in_order_and_resume_at_the_first_unset_one() {
    let mut s = fresh();
    assert_eq!(next_step(&s), Some(FirstRunStep::Voice));
    finish_step(&mut s, FirstRunStep::Voice, FirstRunOutcome::Skipped).unwrap();
    assert_eq!(next_step(&s), Some(FirstRunStep::Brain));
    // A restart is a reload of the same settings: same answer.
    let reloaded: AppSettings = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    assert_eq!(next_step(&reloaded), Some(FirstRunStep::Brain));
}

#[test]
fn finishing_the_voice_step_turns_spoken_replies_on_and_skipping_does_not() {
    let mut done = fresh();
    assert!(!done.tts_enabled);
    finish_step(&mut done, FirstRunStep::Voice, FirstRunOutcome::Done).unwrap();
    assert!(done.tts_enabled);
    assert_eq!(done.first_run_tts, FirstRunOutcome::Done);

    let mut skipped = fresh();
    finish_step(&mut skipped, FirstRunStep::Voice, FirstRunOutcome::Skipped).unwrap();
    assert!(!skipped.tts_enabled, "skipping must not enable anything");
    assert_eq!(skipped.first_run_tts, FirstRunOutcome::Skipped);
}

#[test]
fn skipping_the_brain_leaves_conversation_off_and_changes_nothing_else() {
    let mut s = fresh();
    finish_step(&mut s, FirstRunStep::Voice, FirstRunOutcome::Done).unwrap();
    let before = serde_json::to_value(&s).unwrap();
    finish_step(&mut s, FirstRunStep::Brain, FirstRunOutcome::Skipped).unwrap();
    let mut after = serde_json::to_value(&s).unwrap();
    // Only the outcome and the pending flag may differ.
    let map = after.as_object_mut().unwrap();
    let mut expected = before.clone();
    let expected_map = expected.as_object_mut().unwrap();
    for key in ["first_run_brain", "first_run_setup_pending"] {
        map.remove(key);
        expected_map.remove(key);
    }
    assert_eq!(after, expected);
    assert_eq!(s.first_run_brain, FirstRunOutcome::Skipped);
    assert_eq!(s.brain_source_composer, BrainSource::Inherit, "fresh default, untouched");
    assert!(s.brain_provider_id.is_empty());
}

#[test]
fn done_for_the_brain_needs_a_complete_connection_and_transcript_consent() {
    let mut s = fresh();
    finish_step(&mut s, FirstRunStep::Voice, FirstRunOutcome::Done).unwrap();
    // Nothing configured.
    assert!(finish_step(&mut s, FirstRunStep::Brain, FirstRunOutcome::Done).is_err());
    assert_eq!(s.first_run_brain, FirstRunOutcome::Unset, "a refused step leaves no trace");

    // Connection saved but no consent for spoken replies yet.
    brain::apply_connection(&mut s, "custom", "http://localhost:11434/v1", "llama3.2").unwrap();
    assert!(finish_step(&mut s, FirstRunStep::Brain, FirstRunOutcome::Done).is_err());

    // After 'Use this for spoken replies' (acknowledged test + use_brain_checked) it is accepted.
    let mut ok = fresh();
    finish_step(&mut ok, FirstRunStep::Voice, FirstRunOutcome::Done).unwrap();
    brain_ready(&mut ok);
    finish_step(&mut ok, FirstRunStep::Brain, FirstRunOutcome::Done).unwrap();
    assert_eq!(ok.first_run_brain, FirstRunOutcome::Done);
    assert_eq!(next_step(&ok), None);
    assert!(!ok.first_run_setup_pending, "completion clears the pending flag");
}

#[test]
fn consent_for_another_destination_does_not_count() {
    let mut s = fresh();
    finish_step(&mut s, FirstRunStep::Voice, FirstRunOutcome::Done).unwrap();
    brain_ready(&mut s);
    // The user edits the endpoint afterwards: consent is cleared by apply_connection.
    brain::apply_connection(&mut s, "custom", "http://localhost:1234/v1", "other").unwrap();
    assert!(finish_step(&mut s, FirstRunStep::Brain, FirstRunOutcome::Done).is_err());
}

#[test]
fn completion_is_idempotent_and_never_downgrades() {
    let mut s = fresh();
    finish_step(&mut s, FirstRunStep::Voice, FirstRunOutcome::Done).unwrap();
    let once = serde_json::to_value(&s).unwrap();
    finish_step(&mut s, FirstRunStep::Voice, FirstRunOutcome::Done).unwrap();
    assert_eq!(serde_json::to_value(&s).unwrap(), once);
    // A later 'skipped' for an already finished step does not undo it or the setting it made.
    finish_step(&mut s, FirstRunStep::Voice, FirstRunOutcome::Skipped).unwrap();
    assert_eq!(s.first_run_tts, FirstRunOutcome::Done);
    assert!(s.tts_enabled);
}

#[test]
fn steps_are_a_no_op_once_first_run_is_over_and_for_upgraded_stores() {
    let mut s = legacy_store();
    let before = serde_json::to_value(&s).unwrap();
    assert!(finish_step(&mut s, FirstRunStep::Voice, FirstRunOutcome::Done).is_ok());
    assert_eq!(serde_json::to_value(&s).unwrap(), before, "no settings change for an upgraded store");
    assert!(!s.tts_enabled);
}

#[test]
fn first_run_never_touches_post_processing_or_other_consents() {
    let mut s = fresh();
    s.post_process_enabled = true;
    let enabled = s.post_process_enabled;
    finish_step(&mut s, FirstRunStep::Voice, FirstRunOutcome::Done).unwrap();
    brain_ready(&mut s);
    finish_step(&mut s, FirstRunStep::Brain, FirstRunOutcome::Done).unwrap();
    assert_eq!(s.post_process_enabled, enabled);
    assert_eq!(s.brain_source_postprocess, fresh().brain_source_postprocess);
    assert_eq!(s.brain_source_realtime, fresh().brain_source_realtime);
}

#[test]
fn the_new_keys_survive_the_launch_field_merge_and_ipc_redaction() {
    let mut committed = fresh();
    finish_step(&mut committed, FirstRunStep::Voice, FirstRunOutcome::Done).unwrap();
    let mut stale = fresh(); // an old snapshot that still says Unset / pending
    stale.first_run_tts = FirstRunOutcome::Unset;
    let merged = crate::settings::preserve_launch_fields(stale, &committed);
    assert_eq!(merged.first_run_tts, FirstRunOutcome::Done, "stale writes cannot undo a step");
    assert!(merged.tts_enabled || !committed.tts_enabled);
    let redacted = committed.redacted_for_ipc();
    assert_eq!(redacted.first_run_tts, FirstRunOutcome::Done);
}
