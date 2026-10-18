//! Speech text normalisation for the natural voice.
//!
//! The Kokoro crate's grapheme-to-phoneme step is built for Chinese-plus-English text: it turns
//! the FIRST number of a chunk into Chinese numerals, passes later digits and most symbols
//! through unchanged, and then logs every out-of-vocabulary character. Replies contain numbers,
//! so the engine normalises text BEFORE it reaches the crate: numbers, percentages, currency,
//! ordinals, clock times and symbols become plain English words, markup noise is dropped.
//!
//! STUB: implemented in step S8 (spec = `natural_text_tests.rs`, authoritative).

/// Returns `text` rewritten so it contains only letters (any script), whitespace and the
/// punctuation `. , ! ? ; : ' " - ( ) …`; see `natural_text_tests.rs` for the exact rules.
pub fn normalize_for_speech(text: &str) -> String {
    text.to_string()
}
