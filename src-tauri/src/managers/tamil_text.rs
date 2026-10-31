//! Text preparation and sentence/token chunking for the Tamil Pocket TTS engine.
//!
//! Adapted from the reference runtime `pocket_tts_onnx.py` by KevinAHM (Apache License 2.0):
//! `_prepare_text_prompt`, `_find_boundary_indices`, `_segments_from_boundaries`,
//! `_split_into_best_sentences` and `_estimate_max_gen_len`. The Python `str` semantics that
//! matter here are reproduced exactly: `strip`/`split` use `str.isspace`, `isalnum` is true
//! only for Unicode categories L* and N* (so Tamil vowel signs and the pulli are not alnum),
//! `upper()` uses the full case mapping.
//!
//! Documented deviation from the reference: the reference leaves a chunk longer than the token
//! budget when one sentence is longer than the budget (its KV cache then overflows). Here such a
//! chunk is split further by `hard_split`, at the last whitespace that still fits, otherwise at
//! the last grapheme-safe boundary that fits, and a single over-budget cluster is emitted alone.

use crate::managers::tamil_tokenizer::TamilTokenizer;
use std::collections::HashSet;
use unicode_categories::UnicodeCategories;

/// Maximum prepared token count of one chunk (`max_token_per_chunk` in the pack's bundle.json).
pub const MAX_TOKENS_PER_CHUNK: usize = 50;

/// Reference `TOKENS_PER_SECOND_ESTIMATE`.
const TOKENS_PER_SECOND_ESTIMATE: f64 = 3.0;
/// Reference `GEN_SECONDS_PADDING`.
const GEN_SECONDS_PADDING: f64 = 2.0;
/// Pack `frame_rate` (bundle.json).
const FRAME_RATE: f64 = 12.5;
/// Reference padding width for short inputs (only used when the bundle enables it).
const SHORT_INPUT_PAD: usize = 8;
/// Reference word count below which short inputs are padded (only when enabled).
const SHORT_INPUT_WORDS: usize = 5;
/// Sentence-ending text whose token ids are the boundary set (reference `".!...?"`).
const BOUNDARY_TEXT: &str = ".!...?";
/// Clause-ending text whose token ids form the fallback split set (reference `",;:"`).
const FALLBACK_TEXT: &str = ",;:";
/// Tamil pulli (virama), U+0BCD.
const PULLI: char = '\u{0BCD}';

/// Text-preparation switches of the reference runtime, read from the pack's bundle.json.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PromptOptions {
    /// Reference `pad_with_spaces_for_short_inputs`.
    pub(crate) pad_short_inputs: bool,
    /// Reference `remove_semicolons`.
    pub(crate) remove_semicolons: bool,
}

/// Options of the shipped pack (`pad_with_spaces_for_short_inputs` and `remove_semicolons` are
/// both false in its bundle.json).
const PACK_OPTIONS: PromptOptions = PromptOptions {
    pad_short_inputs: false,
    remove_semicolons: false,
};

/// Python `str.isspace` for one character: Unicode White_Space plus U+001C..=U+001F.
pub(crate) fn py_isspace(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1C}'..='\u{1F}')
}

/// Python `str.isalnum` for one character: Unicode categories L* (letters) and N* (numbers).
///
/// Rust's `char::is_alphanumeric` is not used because it also accepts combining marks such as
/// the Tamil vowel signs and the pulli, which Python rejects.
pub fn py_isalnum(c: char) -> bool {
    c.is_letter() || c.is_number()
}

/// Number of whitespace-separated words, as `len(text.split())` in Python.
fn count_words(text: &str) -> usize {
    text.split(py_isspace)
        .filter(|word| !word.is_empty())
        .count()
}

/// Port of `_prepare_text_prompt` with explicit options.
pub(crate) fn prepare_with(text: &str, options: PromptOptions) -> Result<(String, usize), String> {
    let mut text = text.trim_matches(py_isspace).to_string();
    if text.is_empty() {
        return Err("Text cannot be empty".to_string());
    }
    text = text.replace(['\n', '\r'], " ").replace("  ", " ");
    if options.remove_semicolons {
        text = text.replace(';', ",");
    }

    let frames_after_eos_guess = if count_words(&text) <= 4 { 3 } else { 1 };

    let first = text.chars().next().ok_or("Text cannot be empty")?;
    // Python: `if not text[0].isupper(): text = text[0].upper() + text[1:]`.
    if !first.is_uppercase() {
        let upper: String = first.to_uppercase().collect();
        text = format!("{upper}{}", &text[first.len_utf8()..]);
    }
    if text.chars().next_back().is_some_and(py_isalnum) {
        text.push('.');
    }
    if options.pad_short_inputs && count_words(&text) < SHORT_INPUT_WORDS {
        text = format!("{}{text}", " ".repeat(SHORT_INPUT_PAD));
    }
    Ok((text, frames_after_eos_guess))
}

/// Port of `_prepare_text_prompt` for the shipped pack.
///
/// Returns the prepared text and the `frames_after_eos_guess` (3 for up to four words, else 1).
/// Errors for empty or whitespace-only input, like the reference `ValueError`.
pub fn prepare_text_prompt(text: &str) -> Result<(String, usize), String> {
    prepare_with(text, PACK_OPTIONS)
}

/// Port of `_estimate_max_gen_len`: the frame limit for a chunk with `n_tokens` text tokens.
pub fn estimate_max_frames(n_tokens: usize) -> usize {
    let seconds = n_tokens as f64 / TOKENS_PER_SECOND_ESTIMATE + GEN_SECONDS_PADDING;
    (seconds * FRAME_RATE).ceil() as usize
}

/// Port of `_find_boundary_indices`: segment start indices, with 0 and `tokens.len()` at the ends.
fn find_boundary_indices(tokens: &[u32], boundary: &HashSet<u32>) -> Vec<usize> {
    let mut indices = vec![0];
    let mut previous_was_boundary = false;
    for (index, token) in tokens.iter().enumerate() {
        if boundary.contains(token) {
            previous_was_boundary = true;
        } else {
            if previous_was_boundary {
                indices.push(index);
            }
            previous_was_boundary = false;
        }
    }
    indices.push(tokens.len());
    indices
}

/// Port of `_segments_from_boundaries`: (token count, decoded text) per segment.
fn segments_from_boundaries(
    tok: &TamilTokenizer,
    tokens: &[u32],
    boundaries: &[usize],
) -> Vec<(usize, String)> {
    boundaries
        .windows(2)
        .map(|pair| {
            let slice = tokens.get(pair[0]..pair[1]).unwrap_or(&[]);
            (slice.len(), tok.decode(slice))
        })
        .collect()
}

/// Token count of the prepared form of `text`, or None when the text cannot be prepared.
fn prepared_token_count(tok: &TamilTokenizer, text: &str) -> Option<usize> {
    prepare_text_prompt(text)
        .ok()
        .map(|(prepared, _)| tok.encode(&prepared).len())
}

/// Port of `_split_into_best_sentences`, plus the hard split of over-budget chunks.
///
/// Returns chunks whose prepared token count is at most `max_tokens`, except for a single
/// grapheme cluster that alone exceeds the budget. Errors for empty input or a zero budget.
pub fn split_into_chunks(
    tok: &TamilTokenizer,
    text: &str,
    max_tokens: usize,
) -> Result<Vec<String>, String> {
    if max_tokens == 0 {
        return Err("max_tokens must be at least 1".to_string());
    }
    let (prepared, _) = prepare_text_prompt(text)?;
    let prepared = prepared.trim_matches(py_isspace);
    let tokens = tok.encode(prepared);

    let boundary: HashSet<u32> = tok.encode(BOUNDARY_TEXT).into_iter().skip(1).collect();
    let boundaries = find_boundary_indices(&tokens, &boundary);
    let segments = segments_from_boundaries(tok, &tokens, &boundaries);

    let fallback: HashSet<u32> = tok.encode(FALLBACK_TEXT).into_iter().skip(1).collect();
    let mut refined: Vec<(usize, String)> = Vec::new();
    for (count, segment) in segments {
        if count <= max_tokens {
            refined.push((count, segment));
            continue;
        }
        let sub_tokens = tok.encode(segment.trim_matches(py_isspace));
        let sub_boundaries = find_boundary_indices(&sub_tokens, &fallback);
        let sub_segments = segments_from_boundaries(tok, &sub_tokens, &sub_boundaries);
        if sub_segments.len() > 1 {
            refined.extend(sub_segments);
        } else {
            refined.push((count, segment));
        }
    }

    let mut packed: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_count = 0usize;
    for (count, segment) in refined {
        if current.is_empty() {
            current = segment;
            current_count = count;
        } else if current_count + count > max_tokens {
            packed.push(current.trim_matches(py_isspace).to_string());
            current = segment;
            current_count = count;
        } else {
            current.push(' ');
            current.push_str(&segment);
            current_count += count;
        }
    }
    if !current.is_empty() {
        packed.push(current.trim_matches(py_isspace).to_string());
    }

    let mut chunks = Vec::with_capacity(packed.len());
    for chunk in packed.into_iter().filter(|chunk| !chunk.is_empty()) {
        if prepared_token_count(tok, &chunk).is_some_and(|count| count <= max_tokens) {
            chunks.push(chunk);
        } else {
            chunks.extend(hard_split(tok, &chunk, max_tokens));
        }
    }
    Ok(chunks)
}

/// True for code points that must not start a chunk: Tamil marks U+0B82, U+0B83, U+0BBE..=U+0BCD,
/// U+0BD7, and any other Mn/Mc/Me mark.
fn is_combining_mark(c: char) -> bool {
    c.is_mark() || matches!(c, '\u{0B82}'..='\u{0B83}' | '\u{0BBE}'..='\u{0BCD}' | '\u{0BD7}')
}

/// True when a chunk may end just before `chars[end]`: whitespace always, otherwise the next
/// character is not a combining mark and the previous one is not a pulli.
fn is_safe_split(chars: &[char], end: usize) -> bool {
    let next = chars.get(end).copied();
    let prev = end
        .checked_sub(1)
        .and_then(|index| chars.get(index).copied());
    match (next, prev) {
        (Some(next), Some(prev)) => py_isspace(next) || (!is_combining_mark(next) && prev != PULLI),
        _ => false,
    }
}

/// Splits one over-budget chunk into pieces that each fit `max_tokens` (prepared), or are a
/// single grapheme cluster when even that does not fit. Always makes progress.
fn hard_split(tok: &TamilTokenizer, text: &str, max_tokens: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut pieces = Vec::new();
    let mut start = skip_spaces(&chars, 0);
    while start < chars.len() {
        let end = best_split_end(tok, &chars, start, max_tokens);
        let piece: String = chars.get(start..end).unwrap_or(&[]).iter().collect();
        let piece = piece.trim_matches(py_isspace);
        if !piece.is_empty() {
            pieces.push(piece.to_string());
        }
        start = skip_spaces(&chars, end);
    }
    pieces
}

/// Index of the first character at or after `from` that is not whitespace.
fn skip_spaces(chars: &[char], from: usize) -> usize {
    let mut index = from;
    while chars.get(index).is_some_and(|c| py_isspace(*c)) {
        index += 1;
    }
    index
}

/// Exclusive end of the piece starting at `start`: the last whitespace position whose left part
/// fits, else the last safe character boundary that fits, else the end of the first cluster.
fn best_split_end(tok: &TamilTokenizer, chars: &[char], start: usize, max_tokens: usize) -> usize {
    let total = chars.len();
    let mut best_space = None;
    let mut best_safe = None;
    for end in start + 1..=total {
        let piece: String = chars.get(start..end).unwrap_or(&[]).iter().collect();
        if !prepared_token_count(tok, &piece).is_some_and(|count| count <= max_tokens) {
            break;
        }
        if end == total {
            return total;
        }
        match chars.get(end) {
            Some(next) if py_isspace(*next) => best_space = Some(end),
            _ if is_safe_split(chars, end) => best_safe = Some(end),
            _ => {}
        }
    }
    if let Some(end) = best_space.or(best_safe) {
        return end;
    }
    let mut end = start + 1;
    while end < total && !is_safe_split(chars, end) {
        end += 1;
    }
    end
}
