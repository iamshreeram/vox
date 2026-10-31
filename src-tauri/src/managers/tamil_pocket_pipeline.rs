//! Text to audio pipeline for the Tamil Pocket TTS engine, independent of the ONNX backend.
//!
//! Sentence chunking, text preparation and token ids come from `tamil_text`, and the latent
//! generation and decoding loops come from `tamil_pocket_core`. This module only joins them:
//! generate the latents of every chunk from the cached voice state, concatenate them in order,
//! decode once, then validate and clamp the audio.
//!
//! Mirrors the reference runtime `pocket_tts_onnx.py` (KevinAHM, MIT License): `generate_latents`
//! concatenates the latents of all chunks, and `decode_latents` decodes that concatenation with
//! one mimi state carried across the whole pass. The reference `generate()` (used for sentence
//! playback) instead resets the mimi state per sentence. This pipeline follows the
//! `generate_latents` + `decode_latents` form, so the audio is one continuous decode.

use crate::managers::tamil_pocket_core::{
    decode_latents, generate_chunk_latents, Bundle, GenParams, PocketBackend, Rng, VoiceState,
};
use crate::managers::tamil_text::{
    estimate_max_frames, prepare_text_prompt, py_isspace, split_into_chunks, MAX_TOKENS_PER_CHUNK,
};
use crate::managers::tamil_tokenizer::TamilTokenizer;

/// Latent frames per decoder call, as in the reference worker (`decode_chunk_size`).
const DECODE_GROUP_FRAMES: usize = 12;

/// Synthesize `text` into mono f32 audio at the bundle's sample rate.
///
/// * Empty or whitespace-only text is a no-op: `Ok(vec![])`, and no backend call is made.
/// * Any other chunking error propagates.
/// * Before each chunk, `cancelled()` is checked. A `true` result returns `Err("cancelled")`
///   immediately, with no decode, so a caller can tell a cancel from an empty result.
/// * Chunks whose prepared text has no token ids are skipped.
/// * Latents of all chunks are concatenated in order and decoded once (see the module docs).
/// * If no frame was generated at all, the result is `Ok(vec![])`.
/// * Non-finite decoder output is an error. Remaining samples are clamped to `[-1.0, 1.0]`.
#[allow(clippy::too_many_arguments)]
pub fn synthesize_text(
    backend: &mut dyn PocketBackend,
    bundle: &Bundle,
    tok: &TamilTokenizer,
    voice: &VoiceState,
    text: &str,
    params: &GenParams,
    rng: &mut Rng,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<f32>, String> {
    let chunks = match split_into_chunks(tok, text, MAX_TOKENS_PER_CHUNK) {
        Ok(chunks) => chunks,
        Err(_) if text.chars().all(py_isspace) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };

    let total = chunks.len();
    let mut latents: Vec<f32> = Vec::new();
    for (index, chunk) in chunks.iter().enumerate() {
        if cancelled() {
            return Err("cancelled".to_string());
        }
        let position = index + 1;
        let (prepared, guess) =
            prepare_text_prompt(chunk).map_err(|e| format!("chunk {position} of {total}: {e}"))?;
        // Reference: `effective_frames = model_recommended_frames_after_eos or (guess + 2)`.
        // A pack that recommends a value wins; otherwise the default is guess + 2.
        let frames_after_eos = bundle
            .recommended_frames_after_eos
            .unwrap_or_else(|| guess.saturating_add(2));
        let ids: Vec<i64> = tok.encode(&prepared).into_iter().map(i64::from).collect();
        if ids.is_empty() {
            continue;
        }
        let max_frames = estimate_max_frames(ids.len());
        let chunk_latents = generate_chunk_latents(
            backend,
            bundle,
            voice,
            &ids,
            frames_after_eos,
            max_frames,
            params,
            rng,
        )
        .map_err(|e| format!("chunk {position} of {total}: {e}"))?;
        latents.extend(chunk_latents);
    }

    if latents.is_empty() {
        return Ok(Vec::new());
    }

    let mut audio = decode_latents(backend, bundle, &latents, DECODE_GROUP_FRAMES)
        .map_err(|e| format!("decoding failed: {e}"))?;
    if audio.iter().any(|sample| !sample.is_finite()) {
        return Err("non-finite audio".to_string());
    }
    for sample in audio.iter_mut() {
        *sample = sample.clamp(-1.0, 1.0);
    }
    Ok(audio)
}

/// Peak absolute sample and root-mean-square of `samples`. Both are `0.0` for empty input.
pub fn peak_and_rms(samples: &[f32]) -> (f32, f32) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    let mut peak = 0.0f32;
    let mut sum_of_squares = 0.0f64;
    for &sample in samples {
        peak = peak.max(sample.abs());
        sum_of_squares += f64::from(sample) * f64::from(sample);
    }
    let rms = (sum_of_squares / samples.len() as f64).sqrt();
    (peak, rms as f32)
}
