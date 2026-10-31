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

/// English function words used by `is_probably_english`. No entry may also appear in
/// `FOREIGN_FUNCTION_WORDS`.
const ENGLISH_FUNCTION_WORDS: &[&str] = &[
    "the", "and", "of", "to", "is", "are", "was", "were", "be", "been", "have", "has", "had",
    "will", "would", "can", "could", "do", "does", "am", "i", "you", "your", "he", "she", "it",
    "we", "they", "me", "my", "his", "her", "its", "our", "their", "them", "this", "that", "these",
    "what", "which", "when", "where", "how", "who", "there", "here", "with", "for", "from", "by",
    "at", "on", "in", "or", "but", "if", "not", "no", "so", "as", "an", "all", "about", "out",
    "one", "let", "also", "then", "than", "any",
];

/// Function words and greetings of Spanish, French, German, Portuguese and Italian used by
/// `is_probably_english`. Words that are also common English words (`die`, `per`, `ate`,
/// `do`, `ha`, `no`, `on`, `son`, `tag`, `bin`) are deliberately omitted.
const FOREIGN_FUNCTION_WORDS: &[&str] = &[
    // Spanish
    "hola", "amigo", "amiga", "gracias", "muchas", "muchos", "por", "todo", "todos", "con", "eso",
    "el", "la", "los", "las", "de", "que", "y", "en", "es", "un", "una", "para", "pero", "como",
    "del", "al", "lo", "se", "su", "sus", "mi", "tu", "yo", "ya", "hay", "esta", "este", "muy",
    "ser", "tiene", "puedo", "nos", "buenos", "buenas", // French
    "bonjour", "merci", "le", "les", "des", "avec", "je", "vous", "est", "et", "du", "une", "pour",
    "dans", "sur", "qui", "pas", "mais", "ou", "nous", "sont", "il", "elle", "ce", "cette", "mon",
    "ma", "votre", "oui", "bien", "au", "aux", "ont", "salut", "bonsoir", // German
    "hallo", "danke", "ich", "das", "ist", "ein", "eine", "einen", "und", "der", "mit", "dem",
    "den", "nicht", "auch", "auf", "bei", "zu", "sie", "wir", "ihr", "ihre", "ihrer", "ihrem",
    "ihnen", "ihren", "wird", "wurde", "sind", "habe", "hat", "oder", "aus", "wie", "dass", "sich",
    "im", "zum", "zur", "von", "ja", "nein", "bitte", "es", "doch", "noch", "sehr", "gut", "guten",
    // Portuguese
    "obrigado", "obrigada", "voce", "nao", "uma", "um", "com", "para", "que", "da", "dos", "das",
    "ao", "aos", "muito", "sim", "ola", "tudo", "bem", "ele", "ela", "mas", "seu", "sua", "pra",
    "esta", "sao", "isso", "aqui", "em", "os", "na", "eu", // Italian
    "ciao", "grazie", "il", "di", "che", "non", "sono", "con", "della", "delle", "degli", "gli",
    "questo", "questa", "anche", "molto", "bene", "ho", "hai", "ma", "come", "sei", "siamo", "nel",
    "nella", "alla", "uno", "lo", "dei", "tuo", "tua", "al", "del", "si",
];

/// Letters and marks that are common in the foreign lists and rare in English. Each word
/// carrying one of them scores one foreign point. Inverted `?` and `!` count as well.
const NON_ENGLISH_LATIN_MARKS: [char; 25] = [
    '\u{00F1}', '\u{00E7}', '\u{00DF}', '\u{00E4}', '\u{00F6}', '\u{00FC}', '\u{00E9}', '\u{00E8}',
    '\u{00EA}', '\u{00E0}', '\u{00E2}', '\u{00EC}', '\u{00EE}', '\u{00F2}', '\u{00F4}', '\u{00F9}',
    '\u{00FB}', '\u{00E3}', '\u{00F5}', '\u{00E1}', '\u{00ED}', '\u{00F3}', '\u{00FA}', '\u{00BF}',
    '\u{00A1}',
];

/// BEST-EFFORT English detector for routing the Phonon engine. It is NOT a language
/// identifier and must not be relied on as one.
///
/// Rule: returns `false` for text with no alphabetic char, and for text that is not
/// `is_mostly_latin`. Otherwise it scores the alphabetic words (Unicode letters, lowercased;
/// apostrophes and hyphens separate words). `E` counts words in a short English
/// function-word list. `F` counts words in a short function-word/greeting list for
/// Spanish, French, German, Portuguese and Italian, plus one point per word carrying a
/// non-English Latin letter. The result is `F == 0 || E > F`.
///
/// Known false positives: short or rare-vocabulary foreign text that matches no list (for
/// example "Zdravo prijatelju") counts as English and is spoken with English pronunciation.
/// Known false negatives: English text with several accented loanwords (for example
/// "Let's meet at the café near the bar") can score as foreign and falls back to the
/// system voice, which is always safe.
pub fn is_probably_english(text: &str) -> bool {
    if !text.chars().any(char::is_alphabetic) {
        return false;
    }
    if !is_mostly_latin(text) {
        return false;
    }
    let (english, foreign) = score_function_words(text);
    foreign == 0 || english > foreign
}

/// Returns `(english_hits, foreign_hits)` for `text`, see `is_probably_english`.
fn score_function_words(text: &str) -> (usize, usize) {
    let mut english = 0usize;
    let mut foreign = 0usize;
    // Inverted ? and ! are kept inside tokens so they can mark the word they start.
    let is_token_char = |c: char| c.is_alphabetic() || c == '\u{00BF}' || c == '\u{00A1}';
    for token in text.split(|c: char| !is_token_char(c)) {
        let lowered = token.to_lowercase();
        let word: String = lowered.chars().filter(|c| c.is_alphabetic()).collect();
        if word.is_empty() {
            continue;
        }
        if ENGLISH_FUNCTION_WORDS.contains(&word.as_str()) {
            english += 1;
        }
        if FOREIGN_FUNCTION_WORDS.contains(&word.as_str()) {
            foreign += 1;
        }
        if lowered
            .chars()
            .any(|c| NON_ENGLISH_LATIN_MARKS.contains(&c))
        {
            foreign += 1;
        }
    }
    (english, foreign)
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

/// Smallest `max_chars` the Tamil splitter uses; smaller requests are raised to this value
/// so that every cut makes progress.
const TAMIL_MIN_CHUNK_CHARS: usize = 2;
/// Tamil pulli (virama, U+0BCD): a cut directly after it would separate a consonant from the
/// consonant it joins.
const TAMIL_PULLI: char = '\u{0BCD}';

/// True for any char of the Tamil Unicode block U+0B80..=U+0BFF (letters, marks, digits and
/// signs).
pub fn is_tamil_char(c: char) -> bool {
    ('\u{0B80}'..='\u{0BFF}').contains(&c)
}

/// True for Tamil letters: independent vowels and consonants (U+0B85..=U+0BB9) and OM
/// (U+0BD0). Marks, digits and signs are not letters.
pub fn is_tamil_letter(c: char) -> bool {
    matches!(c, '\u{0B85}'..='\u{0BB9}' | '\u{0BD0}')
}

/// True for Tamil combining marks: anusvara, vowel signs, pulli and the AU length mark
/// (U+0B82 U+0BBE..=U+0BCD U+0BD7). Aytham (U+0B83) is a letter, not a mark, so it is excluded.
fn is_tamil_mark(c: char) -> bool {
    matches!(c, '\u{0B82}' | '\u{0BBE}'..='\u{0BCD}' | '\u{0BD7}')
}

/// Language gate for Tamil: at least 60% of the alphabetic chars (Tamil combining marks
/// count too) are in the Tamil block. Text with no such char is `false`.
pub fn is_mostly_tamil(text: &str) -> bool {
    let (counted, tamil) = text
        .chars()
        .filter(|c| c.is_alphabetic() || is_tamil_mark(*c))
        .fold((0usize, 0usize), |(n, t), c| {
            (n + 1, t + usize::from(is_tamil_char(c)))
        });
    counted > 0 && tamil * 5 >= counted * 3
}

/// Splits Tamil text into synthesis chunks of at most `max_chars` chars (counted in chars).
///
/// Contract (DESIGN R2-C):
/// * Whitespace is collapsed first; pieces are trimmed and empty pieces are dropped.
/// * Sentences end after `.`, `!`, `?` or `\u{2026}` followed by whitespace or the end of the
///   text, so `3.5` and an unspaced `\u{0BAE}\u{0BCD}.` do not split. There is no 48-char
///   first-chunk cap.
/// * A piece longer than `max_chars` is cut at the LAST whitespace within `max_chars`. Without
///   whitespace the cut moves to the last grapheme-safe boundary: never directly before a
///   Tamil combining mark and never directly after a pulli. If no such boundary is within
///   `max_chars`, the cut moves to the next safe boundary (a cluster is never split), so a
///   piece may exceed `max_chars` only in that case.
/// * `max_chars` below 2 is treated as 2, so every cut makes progress.
/// * Pieces never start with a Tamil combining mark; leading orphan marks are dropped.
pub fn split_tamil_for_synthesis(text: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(TAMIL_MIN_CHUNK_CHARS);
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut pieces = Vec::new();
    for sentence in tamil_sentences(&collapsed) {
        let chars: Vec<char> = sentence.chars().collect();
        split_long_tamil_piece(&chars, max_chars, &mut pieces);
    }
    pieces
}

/// Groups the single-spaced words of `collapsed` into sentences ending on `. ! ? ...`.
fn tamil_sentences(collapsed: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for word in collapsed.split(' ').filter(|w| !w.is_empty()) {
        current.push(word);
        if word.ends_with(['.', '!', '?', '\u{2026}']) {
            sentences.push(current.join(" "));
            current.clear();
        }
    }
    if !current.is_empty() {
        sentences.push(current.join(" "));
    }
    sentences
}

/// Appends the pieces of one sentence (already single-spaced) to `out`, cutting as documented
/// on `split_tamil_for_synthesis`. Each loop iteration removes at least one char.
fn split_long_tamil_piece(piece: &[char], max_chars: usize, out: &mut Vec<String>) {
    let mut rest = piece;
    loop {
        rest = skip_leading_tamil_noise(rest);
        if rest.is_empty() {
            return;
        }
        if rest.len() <= max_chars {
            out.push(rest.iter().collect());
            return;
        }
        let cut = tamil_space_cut(rest, max_chars)
            .or_else(|| tamil_grapheme_cut(rest, max_chars))
            .unwrap_or(rest.len());
        let head: String = rest[..cut].iter().collect();
        let head = head.trim_end();
        if !head.is_empty() {
            out.push(head.to_string());
        }
        rest = &rest[cut..];
    }
}

/// Drops leading whitespace and combining marks (they cannot start a chunk).
fn skip_leading_tamil_noise(mut rest: &[char]) -> &[char] {
    while let Some((first, tail)) = rest.split_first() {
        if first.is_whitespace() || is_tamil_mark(*first) {
            rest = tail;
        } else {
            break;
        }
    }
    rest
}

/// Index of the last whitespace in `rest[1..=max_chars]`, the cut point before it.
fn tamil_space_cut(rest: &[char], max_chars: usize) -> Option<usize> {
    (1..=max_chars.min(rest.len() - 1))
        .rev()
        .find(|&k| rest[k].is_whitespace())
}

/// Grapheme-safe cut index: the largest `p` in `1..=max_chars` that is safe, otherwise the
/// smallest safe `p` above `max_chars`. A cut at `p` is safe when `rest[p]` is not a Tamil
/// combining mark and `rest[p - 1]` is not a pulli.
fn tamil_grapheme_cut(rest: &[char], max_chars: usize) -> Option<usize> {
    let safe = |p: usize| !is_tamil_mark(rest[p]) && rest[p - 1] != TAMIL_PULLI;
    (1..=max_chars)
        .rev()
        .find(|&p| safe(p))
        .or_else(|| (max_chars + 1..rest.len()).find(|&p| safe(p)))
}
