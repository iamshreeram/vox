//! Phonon TTS engine glue (macOS only). This is the ONLY module that names the `ptts` crate.
//!
//! Built exactly like the working probe (`ptts_explicit_voice_probe.rs.txt`): explicit
//! `add_voice`, no `Checkpoint::resolve`, no directory scan, no hub or network. The weights are
//! loaded only from a verified private snapshot (see `natural_phonon_core::SnapshotLoader`).
//! DESIGN.md Rev 4, section 3 P3. Spec = `natural_phonon_tests.rs`.

use crate::managers::natural_engine::{EngineLoader, SynthEngine};
use crate::managers::natural_model::ModelCoordinator;
use crate::managers::natural_phonon_core::{synthesize_with, SnapshotLoader, PHONON_SAMPLE_RATE};
use crate::managers::natural_snapshot::Snapshot;
use ptts::preprocess::Lang;
use ptts::synth::{Quant, Synth, SynthBuilder};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::Arc;

/// Name under which the bundled default voice is registered.
const VOICE_NAME: &str = "default";

/// Loader for the Phonon voice: a verified private snapshot of the installed files, built into
/// a ptts engine on each load.
// Nothing calls this until the P4 router wiring lands.
#[allow(dead_code)]
pub fn phonon_loader(
    coordinator: Arc<ModelCoordinator>,
    scratch_root: PathBuf,
) -> Box<dyn EngineLoader> {
    Box::new(SnapshotLoader::new(
        coordinator,
        scratch_root,
        Box::new(build_engine),
    ))
}

/// Builds the ptts engine from the snapshot files only. Fails (never silently unvoiced) when the
/// sample rate is wrong or no default voice is registered.
fn build_engine(snapshot: &Snapshot) -> Result<Box<dyn SynthEngine>, String> {
    let config = ptts::checkpoint::read_config(snapshot.path("config.json"))
        .map_err(|e| format!("cannot read the Phonon config: {e}"))?;
    let synth = SynthBuilder::new(config, snapshot.path("model.q8.gguf"), Lang::En)
        .quant(Quant::Q80)
        .tokenizer_file(snapshot.path("tokenizer.json"))
        .add_voice(VOICE_NAME, snapshot.path("default-voice.safetensors"))
        .build()
        .map_err(|e| format!("cannot build the Phonon voice: {e}"))?;
    if synth.sample_rate() != PHONON_SAMPLE_RATE {
        return Err(format!(
            "the Phonon voice runs at {} Hz, expected {PHONON_SAMPLE_RATE} Hz",
            synth.sample_rate()
        ));
    }
    if !synth.voices().iter().any(|name| name == VOICE_NAME) {
        return Err("the Phonon voice has no default voice".to_string());
    }
    Ok(Box::new(PhononEngine { synth }))
}

/// A built Phonon voice.
///
/// `voice` and `speed` are IGNORED by design: the model ships one English voice and no speed
/// control, so the settings UI hides both choices for this engine.
struct PhononEngine {
    synth: Synth,
}

impl SynthEngine for PhononEngine {
    fn synth(&mut self, text: &str, _voice: &str, _speed: f32) -> Result<Vec<f32>, String> {
        let synth = &self.synth;
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            synthesize_with(text, &mut |chunk: &str| {
                synth.say(chunk).map_err(|e| e.to_string())
            })
        }));
        match outcome {
            Ok(result) => result,
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
