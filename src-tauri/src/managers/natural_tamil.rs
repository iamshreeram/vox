//! Tamil neural voice glue: the `SynthEngine` over the Pocket TTS Tamil pack, the pack status
//! check and the Tamil peer stack. Modelled on `natural_phonon.rs`.
//!
//! The pack is installed by `scripts/download_tamil_tts.sh` (no in-app download in v1), so there
//! is no coordinator here. `tamil_pack_status` only stats the nine pinned files (UI polls must
//! stay cheap). The weights load lazily on the worker thread, on the first Tamil burst.

use crate::managers::natural_engine::{
    EngineLoader, FailureReason, NaturalConfig, NaturalHealth, NaturalSynth, SynthEngine, TextMode,
    DEFAULT_AUTO_RETRY_COOLDOWN,
};
use crate::managers::natural_router::TamilRoute;
use crate::managers::natural_sink::{ManagedSink, StreamFactory};
use crate::managers::tamil_pocket_core::{Bundle, GenParams, PocketBackend, Rng, VoiceState};
use crate::managers::tamil_pocket_ort::PocketModel;
use crate::managers::tamil_pocket_pipeline::synthesize_text;
use crate::managers::tamil_tokenizer::TamilTokenizer;
use crate::managers::tts::SpeechBackend;
use crate::managers::tts_queue::{QueueBackend, SharedCore, SpeakerCore};
use serde::Serialize;
use specta::Type;
use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Folder name of the Tamil pack under the `tts_models` root.
pub const TAMIL_MODEL_DIRNAME: &str = "pocket-tts-tamil-int4";

/// Sample rate of the Tamil engine (the pack's bundle runs at 24 kHz).
pub const TAMIL_SAMPLE_RATE: u32 = 24_000;

/// The nine files of the pack with their exact byte sizes, as published by
/// `scripts/download_tamil_tts.sh`. Files are checked by size only (no hashing on the hot path).
pub const TAMIL_PACK_FILES: [(&str, u64); 9] = [
    ("bundle.json", 24_361),
    ("tokenizer.model", 345_594),
    ("text_conditioner.onnx", 16_388_344),
    ("flow_lm_main_int4.onnx", 40_802_614),
    ("flow_lm_flow_int8.onnx", 9_962_530),
    ("mimi_decoder_int8.onnx", 22_684_077),
    ("mimi_encoder_int8.onnx", 20_802_362),
    ("bos_before_voice.npy", 4_224),
    ("sample_geetha.wav", 276_524),
];

/// Directory of the Tamil pack under the `tts_models` root.
pub fn tamil_model_dir(root: &Path) -> PathBuf {
    root.join(TAMIL_MODEL_DIRNAME)
}

/// Result of the pack check. The names are file names from `TAMIL_PACK_FILES`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TamilPackStatus {
    /// All nine files exist with their exact sizes.
    Ready,
    /// These files are absent (a directory where a file belongs counts as absent).
    Missing(Vec<String>),
    /// These files exist but have the wrong size. Reported only when none is missing.
    WrongSize(Vec<String>),
}

/// Checks the pack in `dir` by stat only. `Missing` wins over `WrongSize`; extra files in the
/// folder are ignored; a folder that does not exist is `Missing` with all nine names.
pub fn tamil_pack_status(dir: &Path) -> TamilPackStatus {
    let mut missing = Vec::new();
    let mut wrong_size = Vec::new();
    for (name, expected) in TAMIL_PACK_FILES {
        match fs::metadata(dir.join(name)) {
            Ok(meta) if meta.is_file() && meta.len() == expected => {}
            Ok(meta) if meta.is_file() => wrong_size.push(name.to_string()),
            _ => missing.push(name.to_string()),
        }
    }
    if !missing.is_empty() {
        TamilPackStatus::Missing(missing)
    } else if !wrong_size.is_empty() {
        TamilPackStatus::WrongSize(wrong_size)
    } else {
        TamilPackStatus::Ready
    }
}

/// A built Tamil voice.
///
/// `voice` and `speed` are IGNORED by design: the pack ships one voice (Geetha, encoded from
/// `sample_geetha.wav` at load time) and the engine has no speed control. Output is mono f32 at
/// `TAMIL_SAMPLE_RATE`.
pub struct TamilEngine {
    backend: Box<dyn PocketBackend + Send>,
    bundle: Bundle,
    tokenizer: TamilTokenizer,
    voice: VoiceState,
    params: GenParams,
    rng: Rng,
}

impl TamilEngine {
    /// Builds the engine from parts. Tests use this with a fake backend, so they need no pack.
    pub fn from_parts(
        backend: Box<dyn PocketBackend + Send>,
        bundle: Bundle,
        tokenizer: TamilTokenizer,
        voice: VoiceState,
        params: GenParams,
        seed: u64,
    ) -> TamilEngine {
        TamilEngine {
            backend,
            bundle,
            tokenizer,
            voice,
            params,
            rng: Rng::new(seed),
        }
    }

    /// Builds the production engine from a loaded pack. Parameters are the reference defaults
    /// (temperature 0.7, one LSD step, EOS threshold -4.0). The RNG is seeded from the system
    /// clock so each load gives different noise.
    pub fn from_model(model: PocketModel) -> TamilEngine {
        let PocketModel {
            bundle,
            tokenizer,
            backend,
            voice,
        } = model;
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(1);
        TamilEngine::from_parts(
            Box::new(backend),
            bundle,
            tokenizer,
            voice,
            GenParams::default(),
            seed,
        )
    }

    /// The cached voice state (tests check that synthesis never changes it).
    #[cfg(test)]
    pub(crate) fn voice(&self) -> &VoiceState {
        &self.voice
    }
}

impl SynthEngine for TamilEngine {
    fn synth(&mut self, text: &str, _voice: &str, _speed: f32) -> Result<Vec<f32>, String> {
        let TamilEngine {
            backend,
            bundle,
            tokenizer,
            voice,
            params,
            rng,
        } = self;
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            synthesize_text(
                &mut **backend,
                bundle,
                tokenizer,
                voice,
                text,
                params,
                rng,
                &|| false,
            )
        }));
        match outcome {
            Ok(Ok(samples)) => Ok(samples),
            Ok(Err(e)) => Err(format!("Tamil voice failed: {e}")),
            Err(payload) => Err(format!(
                "engine panic while speaking: {}",
                panic_message(payload.as_ref())
            )),
        }
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Loader for the Tamil voice: checks the pack, then loads the model on the worker thread.
struct TamilLoader {
    dir: PathBuf,
}

impl EngineLoader for TamilLoader {
    fn load(&mut self) -> Result<Box<dyn SynthEngine>, String> {
        let dir = self.dir.as_path();
        match tamil_pack_status(dir) {
            TamilPackStatus::Ready => {}
            TamilPackStatus::Missing(names) => {
                return Err(format!(
                    "the Tamil voice pack in {} is not installed (missing {}); run scripts/download_tamil_tts.sh",
                    dir.display(),
                    names.join(", ")
                ));
            }
            TamilPackStatus::WrongSize(names) => {
                return Err(format!(
                    "the Tamil voice pack in {} has files of the wrong size ({}); run scripts/download_tamil_tts.sh",
                    dir.display(),
                    names.join(", ")
                ));
            }
        }
        let model = PocketModel::load(dir)
            .map_err(|e| format!("cannot load the Tamil voice pack in {}: {e}", dir.display()))?;
        if model.bundle.sample_rate != TAMIL_SAMPLE_RATE {
            return Err(format!(
                "the Tamil voice runs at {} Hz, expected {TAMIL_SAMPLE_RATE} Hz",
                model.bundle.sample_rate
            ));
        }
        Ok(Box::new(TamilEngine::from_model(model)))
    }
}

/// Loader for the Tamil pack in `dir` (the folder itself, see `tamil_model_dir`). Nothing is
/// read until `load` runs on the worker thread.
pub fn tamil_loader(dir: PathBuf) -> Box<dyn EngineLoader> {
    Box::new(TamilLoader { dir })
}

/// The parts of the Tamil peer engine that lib.rs wires next to Phonon's.
pub struct TamilStack {
    /// `QueueBackend` over the shared Tamil core (the router's Tamil route).
    pub backend: Box<dyn SpeechBackend>,
    /// The Tamil engine health: independent of Phonon's and Kokoro's.
    pub health: Arc<NaturalHealth>,
}

/// Synthesis config of the Tamil stack: Tamil text policy and Tamil chunking (see `TextMode`).
pub(crate) fn tamil_natural_config() -> NaturalConfig {
    NaturalConfig {
        text_mode: TextMode::Tamil,
        ..NaturalConfig::default()
    }
}

/// Builds the Tamil peer stack for the pack under `root` (the `tts_models` folder). Nothing is
/// read or opened here: the model loads lazily on the first Tamil burst, and a missing pack
/// shows up as a load failure (see `tamil_pack_status` for the UI).
pub fn build_tamil_stack(root: &Path, factory: Arc<dyn StreamFactory>) -> TamilStack {
    let health = NaturalHealth::new(DEFAULT_AUTO_RETRY_COOLDOWN);
    let sink = Arc::new(ManagedSink::new(factory, Duration::from_secs(3)));
    let synth = NaturalSynth::new(
        tamil_loader(tamil_model_dir(root)),
        sink,
        health.clone(),
        tamil_natural_config(),
    );
    let core: SharedCore<_> = Arc::new(Mutex::new(SpeakerCore::new(synth)));
    TamilStack {
        backend: Box::new(QueueBackend::new(core)),
        health,
    }
}

/// Builds the Tamil route for `EngineBackend::with_tamil_route`: the stack from
/// `build_tamil_stack`, the live `enabled` check (the `tts_tamil_enabled` setting, read by the
/// caller on every call) and `ready`, which stats the pack (`tamil_pack_status == Ready`).
pub fn build_tamil_route(
    root: &Path,
    factory: Arc<dyn StreamFactory>,
    enabled: Arc<dyn Fn() -> bool + Send + Sync>,
) -> TamilRoute {
    let stack = build_tamil_stack(root, factory);
    let dir = tamil_model_dir(root);
    TamilRoute {
        backend: stack.backend,
        health: stack.health,
        enabled,
        ready: Arc::new(move || tamil_pack_status(&dir) == TamilPackStatus::Ready),
    }
}

/// Status of the Tamil voice for the settings row (the `tamil_voice_status` command).
#[derive(Serialize, Debug, Clone, PartialEq, Eq, Type)]
pub struct TamilVoiceStatus {
    /// The nine pack files are present with their exact sizes.
    pub installed: bool,
    /// Installed and the Tamil engine has not failed.
    pub ready: bool,
    /// The `tts_tamil_enabled` setting.
    pub enabled: bool,
    /// The pack folder.
    pub dir: String,
    /// File names absent from the pack (empty when installed).
    pub missing: Vec<String>,
    /// File names present with the wrong size (empty when installed).
    pub wrong_size: Vec<String>,
    /// User-safe message of the last engine failure, or none.
    pub engine_error: Option<String>,
}

/// Pure status of the Tamil voice: the pack in `dir` (stat only), the live `enabled` setting and
/// the engine failure message, if any. `ready` is `installed` and no engine failure; `enabled`
/// does not change `ready`.
pub fn tamil_voice_status_for(
    dir: &Path,
    enabled: bool,
    health_error: Option<String>,
) -> TamilVoiceStatus {
    let (installed, missing, wrong_size) = match tamil_pack_status(dir) {
        TamilPackStatus::Ready => (true, Vec::new(), Vec::new()),
        TamilPackStatus::Missing(names) => (false, names, Vec::new()),
        TamilPackStatus::WrongSize(names) => (false, Vec::new(), names),
    };
    TamilVoiceStatus {
        installed,
        ready: installed && health_error.is_none(),
        enabled,
        dir: dir.display().to_string(),
        missing,
        wrong_size,
        engine_error: health_error,
    }
}

/// User-safe message for an engine failure. The health keeps only the reason, so the status
/// shows this text, never the raw error.
pub fn tamil_failure_message(reason: FailureReason) -> String {
    match reason {
        FailureReason::Load => "The Tamil voice could not be loaded. Replies use the system voice for now.",
        FailureReason::Synth => "The Tamil voice could not speak this text. Replies use the system voice for now.",
        FailureReason::Sink => "The audio output failed while the Tamil voice was speaking. Replies use the system voice for now.",
        FailureReason::WorkerExit => "The Tamil voice stopped unexpectedly. Replies use the system voice for now.",
    }
    .to_string()
}
