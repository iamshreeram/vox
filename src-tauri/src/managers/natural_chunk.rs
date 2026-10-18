//! Text chunking and overflow-safe synthesis helpers for the natural voice.
//!
//! Pure functions: no I/O, no threads, no engine types. Spec = `natural_chunk_tests.rs`.
//! STUB: bodies are `todo!()` until step S1 implements them.
#![allow(dead_code)]

/// Hard cap (in `char`s, not bytes) for one synthesis chunk. Real-model probe: synth takes
/// ~0.04-0.065 s per char, so 80 chars bounds a stale (un-abortable) synth to ~3.5-5 s and
/// stays far below the model's 510-token limit even at 1.78 tokens/char.
pub const NATURAL_MAX_CHARS: usize = 80;
/// The first chunk of a request is kept short so the first audio arrives in ~2 s.
pub const NATURAL_FIRST_CHUNK_CHARS: usize = 48;
/// Audio shorter than `MIN_SECS_PER_ALNUM_CHAR * alphanumeric_chars` seconds is treated as a
/// silently truncated synthesis and retried in smaller pieces (probe: prose min 0.082).
pub const MIN_SECS_PER_ALNUM_CHAR: f32 = 0.03;
/// Maximum number of halvings: 1 call + 2 + 4 = at most 7 engine calls per chunk.
pub const MAX_RETRY_DEPTH: usize = 2;

/// Splits `text` into synthesis chunks.
///
/// Contract:
/// * Sanitises first: removes `[[` / `]]` speech-markup brackets and every control character
///   (newlines and tabs become a single space), and collapses runs of whitespace. Reuse the
///   existing `sanitize_speech_text` in `tts.rs` if its contract fits.
/// * Every chunk has at most `max_chars` chars; the FIRST chunk has at most
///   `min(NATURAL_FIRST_CHUNK_CHARS, max_chars)` chars.
/// * Prefers to split after sentence-ending punctuation (`.`, `!`, `?`, `…`), then after
///   clause punctuation (`,`, `;`, `:`, `-`), then at a space, and only hard-splits a run
///   that has no space (never inside a UTF-8 char).
/// * Chunks are trimmed, never empty, and never lack an alphanumeric char (such pieces,
///   e.g. `"..."` or emoji only, are dropped).
/// * No spoken text is lost: concatenating all chunks with whitespace removed equals the
///   sanitised input with whitespace removed (apart from dropped non-alphanumeric pieces).
pub fn split_for_synthesis(text: &str, max_chars: usize) -> Vec<String> {
    let _ = (text, max_chars);
    todo!("S1")
}

/// Language gate: true when at least 80% of the alphabetic chars are Latin script
/// (including accented Latin). Text with no alphabetic char at all is `false`.
pub fn is_mostly_latin(text: &str) -> bool {
    let _ = text;
    todo!("S1")
}

/// Result of `synth_with_overflow_retry`.
#[derive(Debug, Clone, PartialEq)]
pub enum RetryOutcome {
    /// Audio for the chunk (pieces concatenated in order). `dropped_pieces` counts pieces
    /// that still failed at the maximum depth and were skipped.
    Audio {
        samples: Vec<f32>,
        dropped_pieces: usize,
    },
    /// Every piece failed (or the text had nothing to say).
    Dropped,
    /// `is_current()` turned false: stop immediately, return no audio, this is NOT a failure.
    Cancelled,
}

/// Synthesises `text` with `synth`; on an engine error OR audio shorter than
/// `MIN_SECS_PER_ALNUM_CHAR` seconds per alphanumeric char (`sample_rate` Hz) splits the text
/// in half at the space nearest the middle and retries each half, up to `MAX_RETRY_DEPTH`
/// levels (pieces at the maximum depth that fail are dropped, never split further).
///
/// Cancellation (C11): `is_current` is checked BEFORE every `synth` call and AFTER every
/// result. If it is false the function returns `Cancelled` immediately, discards any audio
/// and makes no further `synth` call. Text with no alphanumeric char returns `Dropped`
/// without calling `synth`. Always terminates (at most 7 `synth` calls).
pub fn synth_with_overflow_retry(
    synth: &mut dyn FnMut(&str) -> Result<Vec<f32>, String>,
    text: &str,
    sample_rate: u32,
    is_current: &dyn Fn() -> bool,
) -> RetryOutcome {
    let _ = (synth, text, sample_rate, is_current);
    todo!("S1")
}
