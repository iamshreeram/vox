//! Text chunking and overflow-safe synthesis helpers for the natural voice.
//!
//! Pure functions: no I/O, no threads, no engine types. Spec = `natural_chunk_tests.rs`.
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

/// Sentence-ending punctuation (preferred split points).
const SENTENCE_END: [char; 4] = ['.', '!', '?', '\u{2026}'];
/// Clause punctuation (second-choice split points).
const CLAUSE_END: [char; 4] = [',', ';', ':', '-'];

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
    // `sanitize_speech_text` truncates at 2000 chars, so a private sanitiser is used.
    let clean = sanitize(text);
    let max_chars = max_chars.max(1);
    let mut chunks = Vec::new();
    let mut rest = clean.as_str();
    while !rest.is_empty() {
        let cap = if chunks.is_empty() {
            max_chars.min(NATURAL_FIRST_CHUNK_CHARS)
        } else {
            max_chars
        };
        let (head, tail) = rest.split_at(split_index(rest, cap));
        let piece = head.trim();
        if piece.chars().any(char::is_alphanumeric) {
            chunks.push(piece.to_string());
        }
        rest = tail.trim_start();
    }
    chunks
}

/// Removes `[[`/`]]` markup and control chars (whitespace controls become spaces), then
/// collapses whitespace runs to single spaces and trims.
fn sanitize(text: &str) -> String {
    let unmarked = text.replace("[[", " ").replace("]]", " ");
    let mapped: String = unmarked
        .chars()
        .filter_map(|c| match c {
            c if c.is_whitespace() => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect();
    mapped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Byte index at which to cut `text` (already trimmed at the start) so the head holds at
/// most `cap` chars. Prefers sentence, then clause punctuation, then a space, else a hard
/// cut at `cap` chars. Always a char boundary and always > 0 for non-empty text.
fn split_index(text: &str, cap: usize) -> usize {
    // (byte index, char) for the first `cap + 1` chars: index `cap` is the lookahead.
    let window: Vec<(usize, char)> = text.char_indices().take(cap + 1).collect();
    if window.len() <= cap {
        return text.len();
    }
    let after_punct = |set: &[char]| last_punct_cut(text, &window, cap, set);
    after_punct(&SENTENCE_END)
        .or_else(|| after_punct(&CLAUSE_END))
        .or_else(|| last_space_cut(&window))
        .or_else(|| window.get(cap).map(|(i, _)| *i))
        .unwrap_or(text.len())
}

/// Byte index just after the last punctuation (from `set`) among the first `cap` chars
/// that is followed by whitespace (or the end of text).
fn last_punct_cut(text: &str, window: &[(usize, char)], cap: usize, set: &[char]) -> Option<usize> {
    window
        .iter()
        .enumerate()
        .take(cap)
        .rev()
        .find(|(pos, (_, c))| {
            set.contains(c)
                && !matches!(window.get(pos + 1), Some((_, next)) if !next.is_whitespace())
        })
        .map(|(_, (i, c))| (i + c.len_utf8()).min(text.len()))
}

/// Byte index of the last space in the window (the space may sit at the lookahead slot,
/// since it is trimmed away). Never 0, so progress is guaranteed.
fn last_space_cut(window: &[(usize, char)]) -> Option<usize> {
    window
        .iter()
        .rev()
        .find(|(i, c)| *i > 0 && c.is_whitespace())
        .map(|(i, _)| *i)
}

/// Language gate: true when at least 80% of the alphabetic chars are Latin script
/// (including accented Latin). Text with no alphabetic char at all is `false`.
pub fn is_mostly_latin(text: &str) -> bool {
    let (alpha, latin) = text
        .chars()
        .filter(|c| c.is_alphabetic())
        .fold((0usize, 0usize), |(a, l), c| {
            (a + 1, l + usize::from(is_latin_char(c)))
        });
    alpha > 0 && latin * 5 >= alpha * 4
}

/// True for Latin-script letters: ASCII, Latin-1 supplement, Latin Extended-A/B, IPA,
/// Latin Extended Additional/C/D/E and fullwidth Latin.
fn is_latin_char(c: char) -> bool {
    c.is_ascii_alphabetic()
        || matches!(
            c,
            '\u{00AA}'
                | '\u{00BA}'
                | '\u{00C0}'..='\u{02AF}'
                | '\u{1D00}'..='\u{1DBF}'
                | '\u{1E00}'..='\u{1EFF}'
                | '\u{2C60}'..='\u{2C7F}'
                | '\u{A720}'..='\u{A7FF}'
                | '\u{AB30}'..='\u{AB6F}'
                | '\u{FF21}'..='\u{FF3A}'
                | '\u{FF41}'..='\u{FF5A}'
        )
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
    let mut job = RetryJob {
        synth,
        sample_rate,
        is_current,
        samples: Vec::new(),
        dropped: 0,
    };
    if job.attempt(text.trim(), 0).is_err() {
        return RetryOutcome::Cancelled;
    }
    if job.samples.is_empty() {
        RetryOutcome::Dropped
    } else {
        RetryOutcome::Audio {
            samples: job.samples,
            dropped_pieces: job.dropped,
        }
    }
}

/// Marker: `is_current()` returned false.
struct Cancelled;

/// State of one `synth_with_overflow_retry` run.
struct RetryJob<'a> {
    synth: &'a mut dyn FnMut(&str) -> Result<Vec<f32>, String>,
    sample_rate: u32,
    is_current: &'a dyn Fn() -> bool,
    samples: Vec<f32>,
    dropped: usize,
}

impl RetryJob<'_> {
    /// Synthesises `text` at recursion `depth`, appending audio or counting a dropped piece.
    fn attempt(&mut self, text: &str, depth: usize) -> Result<(), Cancelled> {
        let alnum = text.chars().filter(|c| c.is_alphanumeric()).count();
        if alnum == 0 {
            return Ok(());
        }
        if !(self.is_current)() {
            return Err(Cancelled);
        }
        let result = (self.synth)(text);
        if !(self.is_current)() {
            return Err(Cancelled);
        }
        match result {
            Ok(audio) if !is_truncated(audio.len(), alnum, self.sample_rate) => {
                self.samples.extend_from_slice(&audio);
                Ok(())
            }
            _ => self.retry_halves(text, depth),
        }
    }

    /// Splits a failed piece in two and retries both, or drops it at the maximum depth.
    fn retry_halves(&mut self, text: &str, depth: usize) -> Result<(), Cancelled> {
        match halve(text) {
            Some((left, right)) if depth < MAX_RETRY_DEPTH => {
                self.attempt(left, depth + 1)?;
                self.attempt(right, depth + 1)
            }
            _ => {
                self.dropped += 1;
                Ok(())
            }
        }
    }
}

/// True when `len` samples are too short for `alnum` alphanumeric chars.
fn is_truncated(len: usize, alnum: usize, sample_rate: u32) -> bool {
    let min_len = alnum as f64 * f64::from(MIN_SECS_PER_ALNUM_CHAR) * f64::from(sample_rate);
    (len as f64) < min_len
}

/// Splits `text` at the whitespace nearest its middle char; a text without whitespace is
/// cut at the middle char boundary. Both halves are trimmed; `None` if one would be empty.
fn halve(text: &str) -> Option<(&str, &str)> {
    let total = text.chars().count();
    let mid = total / 2;
    let space_cut = text
        .char_indices()
        .enumerate()
        .filter(|(_, (_, c))| c.is_whitespace())
        .min_by_key(|(pos, _)| pos.abs_diff(mid))
        .map(|(_, (i, _))| i);
    let cut = space_cut.or_else(|| text.char_indices().nth(mid).map(|(i, _)| i))?;
    let (left, right) = text.split_at(cut);
    let (left, right) = (left.trim(), right.trim());
    (!left.is_empty() && !right.is_empty()).then_some((left, right))
}
