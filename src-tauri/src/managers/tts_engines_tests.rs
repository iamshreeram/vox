//! Contract tests for the data-driven TTS engine catalog (DESIGN.md Rev 4, P5).
//! Authoritative spec: do not weaken or delete assertions.

use super::natural_model::{KOKORO_MODEL_SIZE, KOKORO_VOICES_SIZE};
use super::tts_engines::*;
use crate::settings::TtsEngine;

fn info(id: TtsEngine) -> TtsEngineInfo {
    engine_info(id)
}

fn catalog_with_phonon_unsupported() -> Vec<TtsEngineInfo> {
    let mut catalog = tts_engine_catalog();
    for entry in catalog.iter_mut() {
        if entry.id == TtsEngine::Phonon {
            entry.supported_on_platform = false;
        }
    }
    catalog
}

#[test]
fn catalog_lists_phonon_then_natural_then_system() {
    let ids: Vec<TtsEngine> = tts_engine_catalog().iter().map(|e| e.id).collect();
    assert_eq!(
        ids,
        vec![TtsEngine::Phonon, TtsEngine::Natural, TtsEngine::System]
    );
}

#[test]
fn exactly_one_engine_is_recommended_and_it_is_phonon() {
    let recommended: Vec<TtsEngine> = tts_engine_catalog()
        .iter()
        .filter(|e| e.recommended)
        .map(|e| e.id)
        .collect();
    assert_eq!(recommended, vec![TtsEngine::Phonon]);
}

#[test]
fn recommended_does_not_change_the_default_engine() {
    assert_eq!(TtsEngine::default(), TtsEngine::System);
    assert_eq!(
        crate::settings::get_default_settings().tts_engine,
        TtsEngine::System
    );
}

#[test]
fn phonon_entry_describes_the_pinned_31m_english_model() {
    let phonon = info(TtsEngine::Phonon);
    assert_eq!(phonon.runtime, TtsRuntime::PureRust);
    assert!(phonon.requires_download);
    assert_eq!(PHONON_TOTAL_BYTES, 65_051_089);
    assert_eq!(phonon.size_bytes, PHONON_TOTAL_BYTES);
    assert!(phonon.english_only);
    assert!(
        !phonon.supports_voice_choice,
        "Phonon has one built-in voice"
    );
    assert!(!phonon.supports_rate, "Phonon has no speech-rate control");
    assert_eq!(phonon.license, "Apache-2.0");
    assert_eq!(phonon.supported_on_platform, cfg!(target_os = "macos"));
}

#[test]
fn natural_entry_describes_kokoro_on_onnx_runtime() {
    let natural = info(TtsEngine::Natural);
    assert_eq!(natural.runtime, TtsRuntime::OnnxRuntime);
    assert!(natural.requires_download);
    assert_eq!(natural.size_bytes, KOKORO_MODEL_SIZE + KOKORO_VOICES_SIZE);
    assert!(!natural.english_only);
    assert!(natural.supports_voice_choice);
    assert!(natural.supports_rate);
    assert!(!natural.license.is_empty());
    assert_eq!(natural.supported_on_platform, cfg!(target_os = "macos"));
}

#[test]
fn system_entry_is_built_in_and_always_supported() {
    let system = info(TtsEngine::System);
    assert_eq!(system.runtime, TtsRuntime::OperatingSystem);
    assert!(!system.requires_download);
    assert_eq!(system.size_bytes, 0);
    assert!(!system.english_only);
    assert!(system.supports_voice_choice);
    assert!(system.supports_rate);
    assert!(system.supported_on_platform);
    assert!(!system.recommended);
}

#[test]
fn engine_info_matches_the_catalog_entry_for_every_engine() {
    for entry in tts_engine_catalog() {
        assert_eq!(engine_info(entry.id), entry);
    }
}

#[test]
fn engine_ids_serialise_as_snake_case_and_unknown_means_system() {
    assert_eq!(
        serde_json::to_string(&TtsEngine::Phonon).unwrap(),
        "\"phonon\""
    );
    assert_eq!(
        serde_json::to_string(&TtsEngine::Natural).unwrap(),
        "\"natural\""
    );
    assert_eq!(
        serde_json::to_string(&TtsEngine::System).unwrap(),
        "\"system\""
    );
    for (stored, want) in [
        ("phonon", TtsEngine::Phonon),
        ("natural", TtsEngine::Natural),
        ("system", TtsEngine::System),
        ("", TtsEngine::System),
        ("PHONON", TtsEngine::System),
        ("phonon-40m", TtsEngine::System),
        ("something-new", TtsEngine::System),
    ] {
        let got: TtsEngine = serde_json::from_str(&format!("\"{stored}\"")).unwrap();
        assert_eq!(got, want, "{stored:?}");
    }
}

#[test]
fn catalog_entries_serialise_for_the_frontend() {
    let json = serde_json::to_value(tts_engine_catalog()).unwrap();
    let first = &json[0];
    assert_eq!(first["id"], "phonon");
    assert_eq!(first["recommended"], true);
    assert_eq!(first["runtime"], "pure_rust");
    assert!(first["size_bytes"].is_number());
    assert_eq!(json[1]["runtime"], "onnx_runtime");
    assert_eq!(json[2]["runtime"], "operating_system");
}

#[test]
fn selecting_a_supported_engine_is_allowed() {
    let catalog = tts_engine_catalog();
    assert!(validate_engine_selection(TtsEngine::System, TtsEngine::System, &catalog).is_ok());
}

#[test]
fn newly_selecting_an_unsupported_engine_is_rejected() {
    let catalog = catalog_with_phonon_unsupported();
    let err = validate_engine_selection(TtsEngine::System, TtsEngine::Phonon, &catalog)
        .expect_err("Phonon is unsupported in this catalog");
    assert!(!err.is_empty());
}

#[test]
fn an_already_persisted_unsupported_selection_is_kept_not_rewritten() {
    let catalog = catalog_with_phonon_unsupported();
    assert!(validate_engine_selection(TtsEngine::Phonon, TtsEngine::Phonon, &catalog).is_ok());
}

#[test]
fn switching_away_from_an_unsupported_engine_is_always_allowed() {
    let catalog = catalog_with_phonon_unsupported();
    assert!(validate_engine_selection(TtsEngine::Phonon, TtsEngine::System, &catalog).is_ok());
}

#[test]
fn system_is_selectable_even_if_the_catalog_were_empty() {
    assert!(validate_engine_selection(TtsEngine::Phonon, TtsEngine::System, &[]).is_ok());
}

#[test]
fn an_engine_missing_from_the_catalog_is_rejected_unless_already_selected() {
    let catalog: Vec<TtsEngineInfo> = tts_engine_catalog()
        .into_iter()
        .filter(|e| e.id != TtsEngine::Phonon)
        .collect();
    assert!(validate_engine_selection(TtsEngine::System, TtsEngine::Phonon, &catalog).is_err());
    assert!(validate_engine_selection(TtsEngine::Phonon, TtsEngine::Phonon, &catalog).is_ok());
}
