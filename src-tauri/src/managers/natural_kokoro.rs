//! Kokoro neural engine (macOS only): `EngineLoader`/`SynthEngine` over the `kokoro-tts` crate.
//!
//! Spec = `natural_kokoro_tests.rs`. STUB: bodies are `todo!()` until step S3b. Signatures and
//! doc contracts are binding.
#![allow(dead_code)]

use crate::managers::natural_engine::{EngineLoader, SynthEngine};
use crate::managers::natural_voices::ModelBytes;
use kokoro_tts::Voice;

/// Supplies verified model bytes (see `natural_voices::ModelBytes`; produced by `natural_model`).
pub type BytesProvider = Box<dyn FnMut() -> Result<ModelBytes, String> + Send + 'static>;

/// Maps a curated voice id (`natural_voices::NATURAL_VOICES`) to the crate's `Voice` with the
/// given speed. Exact, case-sensitive ids only; unknown ids -> `None`. Every curated id MUST
/// resolve (a test enforces it). The crate's `Voice` is `Voice::AfHeart(speed)` etc.
pub fn voice_from_id(id: &str, speed: f32) -> Option<Voice> {
    let _ = (id, speed);
    todo!("S3b")
}

pub struct KokoroLoader {
    provider: BytesProvider,
}

impl KokoroLoader {
    pub fn new(provider: BytesProvider) -> Self {
        Self { provider }
    }
}

impl EngineLoader for KokoroLoader {
    /// Calls the provider (verified bytes), builds the model with
    /// `KokoroTts::new_from_bytes(model, voices)` (NEVER from a path), on a small private
    /// current-thread tokio runtime that the returned engine keeps. Provider errors, garbage
    /// bytes and ort errors are returned as `Err(String)`; nothing panics out of `load`
    /// (wrap in `catch_unwind`). Lower the loading thread's QoS to `QOS_CLASS_UTILITY` BEFORE
    /// creating the session so ONNX inference threads yield to speech recognition (best effort:
    /// via `libc::pthread_set_qos_class_self_np` if `libc` is available, else skip).
    fn load(&mut self) -> Result<Box<dyn SynthEngine>, String> {
        todo!("S3b")
    }
}

/// The loaded model. `synth`:
/// * unknown voice id -> `Err`;
/// * the crate PANICS (index out of bounds) above 510 phoneme tokens: so first count tokens
///   with the crate's public `g2p` / tokenizer helpers and return `Err("too long")` above ~480
///   or `Err("nothing to say")` at 2 or fewer (the phonemizer yields a noise blip for those),
///   and ALSO wrap the whole call in `catch_unwind` (a panic becomes `Err`);
/// * returns the 24 kHz mono samples.
pub struct KokoroEngine {
    _private: (),
}

impl SynthEngine for KokoroEngine {
    fn synth(&mut self, text: &str, voice: &str, speed: f32) -> Result<Vec<f32>, String> {
        let _ = (text, voice, speed);
        todo!("S3b")
    }
}
