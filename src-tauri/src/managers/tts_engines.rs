//! Data-driven catalog of the text-to-speech engines Vox can use (DESIGN.md Rev 4, P5).
//!
//! Platform-neutral on purpose: the settings UI reads this catalog on every platform, so
//! entries for engines that cannot run here are still listed, marked with
//! `supported_on_platform = false`. The only platform check is a compile-time `cfg!` on
//! the per-engine flag. The `recommended` value is a badge and an ordering hint only. It
//! never changes the default engine, which stays `TtsEngine::System`.

use crate::managers::natural_model::{KOKORO_MODEL_SIZE, KOKORO_VOICES_SIZE};
use crate::settings::TtsEngine;
use serde::{Deserialize, Serialize};
use specta::Type;

/// Total size in bytes of the pinned Phonon 31M English checkpoint (the four asset files).
pub const PHONON_TOTAL_BYTES: u64 = 65_051_089;

/// What runs the engine's speech synthesis.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum TtsRuntime {
    /// Pure Rust, no separate native runtime.
    PureRust,
    /// Runs on ONNX Runtime.
    OnnxRuntime,
    /// Provided by the operating system.
    OperatingSystem,
}

/// One row of the engine picker.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Type)]
pub struct TtsEngineInfo {
    pub id: TtsEngine,
    pub recommended: bool,
    pub runtime: TtsRuntime,
    pub requires_download: bool,
    pub size_bytes: u64,
    pub english_only: bool,
    pub supports_voice_choice: bool,
    pub supports_rate: bool,
    pub supported_on_platform: bool,
    pub license: String,
}

fn phonon_entry() -> TtsEngineInfo {
    TtsEngineInfo {
        id: TtsEngine::Phonon,
        recommended: true,
        runtime: TtsRuntime::PureRust,
        requires_download: true,
        size_bytes: PHONON_TOTAL_BYTES,
        english_only: true,
        supports_voice_choice: false,
        supports_rate: false,
        supported_on_platform: cfg!(target_os = "macos"),
        license: "Apache-2.0".to_string(),
    }
}

fn natural_entry() -> TtsEngineInfo {
    TtsEngineInfo {
        id: TtsEngine::Natural,
        recommended: false,
        runtime: TtsRuntime::OnnxRuntime,
        requires_download: true,
        size_bytes: KOKORO_MODEL_SIZE + KOKORO_VOICES_SIZE,
        english_only: false,
        supports_voice_choice: true,
        supports_rate: true,
        supported_on_platform: cfg!(target_os = "macos"),
        license: "Apache-2.0".to_string(),
    }
}

fn system_entry() -> TtsEngineInfo {
    TtsEngineInfo {
        id: TtsEngine::System,
        recommended: false,
        runtime: TtsRuntime::OperatingSystem,
        requires_download: false,
        size_bytes: 0,
        english_only: false,
        supports_voice_choice: true,
        supports_rate: true,
        supported_on_platform: true,
        license: "Operating system".to_string(),
    }
}

/// Every engine, in picker order: Phonon, Natural, System. Exactly one entry is recommended.
pub fn tts_engine_catalog() -> Vec<TtsEngineInfo> {
    vec![phonon_entry(), natural_entry(), system_entry()]
}

/// The catalog entry for one engine.
// Not yet called from production code: the P6 picker UI reads the catalog via `list_tts_engines`.
#[allow(dead_code)]
pub fn engine_info(id: TtsEngine) -> TtsEngineInfo {
    match id {
        TtsEngine::Phonon => phonon_entry(),
        TtsEngine::Natural => natural_entry(),
        TtsEngine::System => system_entry(),
    }
}

/// Decides whether a settings write may move from `current` to `requested`.
///
/// Rules, in order:
/// - Keeping the current selection is always allowed, so a persisted selection is never
///   rewritten, even when this platform cannot run it.
/// - Selecting System is always allowed.
/// - Anything else must be in the catalog and supported on this platform.
pub fn validate_engine_selection(
    current: TtsEngine,
    requested: TtsEngine,
    catalog: &[TtsEngineInfo],
) -> Result<(), String> {
    if requested == current || requested == TtsEngine::System {
        return Ok(());
    }
    match catalog.iter().find(|entry| entry.id == requested) {
        Some(entry) if entry.supported_on_platform => Ok(()),
        Some(_) => Err(format!(
            "The {requested:?} speech engine is not available on this platform."
        )),
        None => Err(format!("The {requested:?} speech engine is unknown.")),
    }
}
