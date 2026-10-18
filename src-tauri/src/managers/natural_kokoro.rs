//! Kokoro neural engine (macOS only): `EngineLoader`/`SynthEngine` over the `kokoro-tts` crate.
//!
//! Spec = `natural_kokoro_tests.rs`. Signatures and doc contracts are binding.
#![allow(dead_code)]

use crate::managers::natural_engine::{EngineLoader, SynthEngine};
use crate::managers::natural_voices::ModelBytes;
use kokoro_tts::{g2p, get_token_ids, KokoroTts, Voice};
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Above this many phoneme tokens synthesis is refused (the crate panics above 510).
const MAX_TOKENS: usize = 480;
/// At or below this many tokens (just the two padding tokens) there is nothing to say.
const MIN_TOKENS: usize = 2;

/// Counts phoneme tokens exactly as the crate's v1.0 synthesiser builds them (all curated
/// voices are v1.0 voices): `get_token_ids(g2p(text, false), false)`, padding included.
/// Returns `(total, speakable)`: `total` includes the two padding tokens; `speakable` counts
/// only tokens that are not punctuation or whitespace (e.g. `"..."` has none).
fn count_tokens(text: &str) -> Result<(usize, usize), String> {
    let phonemes = g2p(text, false).map_err(|e| format!("phonemizer error: {e}"))?;
    let total = get_token_ids(&phonemes, false).len();
    let speakable: String = phonemes
        .chars()
        .filter(|c| !c.is_whitespace() && !is_punctuation(*c))
        .collect();
    let speakable = get_token_ids(&speakable, false).len().saturating_sub(2);
    Ok((total, speakable))
}

fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(
            c,
            '…' | '“' | '”' | '‘' | '’' | '—' | '–' | '¡' | '¿' | '«' | '»'
        )
}

/// Best effort: lower the current thread's QoS so ONNX work yields to speech recognition.
#[cfg(target_os = "macos")]
fn lower_thread_qos() {
    // SAFETY: plain FFI call affecting only the calling thread; the return code is ignored
    // because this is best effort.
    unsafe {
        let _ = libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
}

#[cfg(not(target_os = "macos"))]
fn lower_thread_qos() {}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Supplies verified model bytes (see `natural_voices::ModelBytes`; produced by `natural_model`).
pub type BytesProvider = Box<dyn FnMut() -> Result<ModelBytes, String> + Send + 'static>;

/// Maps a curated voice id (`natural_voices::NATURAL_VOICES`) to the crate's `Voice` with the
/// given speed. Exact, case-sensitive ids only; unknown ids -> `None`. Every curated id MUST
/// resolve (a test enforces it). The crate's `Voice` is `Voice::AfHeart(speed)` etc.
pub fn voice_from_id(id: &str, speed: f32) -> Option<Voice> {
    let voice = match id {
        "af_alloy" => Voice::AfAlloy(speed),
        "af_aoede" => Voice::AfAoede(speed),
        "af_bella" => Voice::AfBella(speed),
        "af_heart" => Voice::AfHeart(speed),
        "af_jessica" => Voice::AfJessica(speed),
        "af_kore" => Voice::AfKore(speed),
        "af_nicole" => Voice::AfNicole(speed),
        "af_nova" => Voice::AfNova(speed),
        "af_river" => Voice::AfRiver(speed),
        "af_sarah" => Voice::AfSarah(speed),
        "af_sky" => Voice::AfSky(speed),
        "am_adam" => Voice::AmAdam(speed),
        "am_echo" => Voice::AmEcho(speed),
        "am_eric" => Voice::AmEric(speed),
        "am_fenrir" => Voice::AmFenrir(speed),
        "am_liam" => Voice::AmLiam(speed),
        "am_michael" => Voice::AmMichael(speed),
        "am_onyx" => Voice::AmOnyx(speed),
        "am_puck" => Voice::AmPuck(speed),
        "am_santa" => Voice::AmSanta(speed),
        "bf_alice" => Voice::BfAlice(speed),
        "bf_emma" => Voice::BfEmma(speed),
        "bf_isabella" => Voice::BfIsabella(speed),
        "bf_lily" => Voice::BfLily(speed),
        "bm_daniel" => Voice::BmDaniel(speed),
        "bm_fable" => Voice::BmFable(speed),
        "bm_george" => Voice::BmGeorge(speed),
        "bm_lewis" => Voice::BmLewis(speed),
        _ => return None,
    };
    Some(voice)
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
        let provider = &mut self.provider;
        let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<KokoroEngine, String> {
            let bytes = provider()?;
            lower_thread_qos();
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("failed to start the engine runtime: {e}"))?;
            let tts = rt
                .block_on(KokoroTts::new_from_bytes(&bytes.model, &bytes.voices))
                .map_err(|e| format!("failed to load the voice model: {e}"))?;
            Ok(KokoroEngine { rt, tts })
        }));
        match outcome {
            Ok(Ok(engine)) => Ok(Box::new(engine)),
            Ok(Err(e)) => Err(e),
            Err(payload) => Err(format!(
                "engine panic while loading: {}",
                panic_message(payload.as_ref())
            )),
        }
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
    rt: tokio::runtime::Runtime,
    tts: KokoroTts,
}

impl SynthEngine for KokoroEngine {
    fn synth(&mut self, text: &str, voice: &str, speed: f32) -> Result<Vec<f32>, String> {
        let voice = voice_from_id(voice, speed).ok_or_else(|| "unknown voice".to_string())?;
        let rt = &self.rt;
        let tts = &self.tts;
        let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<Vec<f32>, String> {
            let (tokens, speakable) = count_tokens(text)?;
            if tokens > MAX_TOKENS {
                return Err("too long".to_string());
            }
            if tokens <= MIN_TOKENS || speakable == 0 {
                return Err("nothing to say".to_string());
            }
            let (samples, _elapsed) = rt
                .block_on(tts.synth(text, voice))
                .map_err(|e| format!("synthesis failed: {e}"))?;
            Ok(samples)
        }));
        match outcome {
            Ok(result) => result,
            Err(_) => Err("engine panic".to_string()),
        }
    }
}
