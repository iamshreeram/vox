//! CI-safe tests for the Tamil text preparation and chunking port. They use a tiny synthetic
//! SentencePiece model built in code (one piece per character, no merges), so they need no pack
//! and no environment variable. The real-pack parity checks live in tamil_parity_text_tests.rs.

use crate::managers::tamil_text::{
    estimate_max_frames, prepare_text_prompt, prepare_with, py_isalnum, split_into_chunks,
    PromptOptions, MAX_TOKENS_PER_CHUNK,
};
use crate::managers::tamil_tokenizer::TamilTokenizer;

const KIND_NORMAL: u64 = 1;
const KIND_UNKNOWN: u64 = 2;
const KIND_CONTROL: u64 = 3;
const MODEL_TYPE_BPE: u64 = 2;

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let low = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(low);
            return;
        }
        out.push(low | 0x80);
    }
}

fn put_key(out: &mut Vec<u8>, field: u64, wire: u64) {
    put_varint(out, (field << 3) | wire);
}

fn put_bytes(out: &mut Vec<u8>, field: u64, data: &[u8]) {
    put_key(out, field, 2);
    put_varint(out, data.len() as u64);
    out.extend_from_slice(data);
}

fn put_uint(out: &mut Vec<u8>, field: u64, value: u64) {
    put_key(out, field, 0);
    put_varint(out, value);
}

fn put_float(out: &mut Vec<u8>, field: u64, value: f32) {
    put_key(out, field, 5);
    out.extend_from_slice(&value.to_le_bytes());
}

fn piece_msg(text: &str, score: f32, kind: u64) -> Vec<u8> {
    let mut msg = Vec::new();
    put_bytes(&mut msg, 1, text.as_bytes());
    put_float(&mut msg, 2, score);
    put_uint(&mut msg, 3, kind);
    msg
}

fn normalizer_msg() -> Vec<u8> {
    let mut msg = Vec::new();
    put_bytes(&mut msg, 1, b"nmt_nfkc");
    put_uint(&mut msg, 3, 1);
    put_uint(&mut msg, 4, 1);
    put_uint(&mut msg, 5, 1);
    msg
}

/// Serialized ModelProto: pieces, a BPE trainer spec with unk id 0, and a normalizer that adds
/// the dummy prefix, collapses whitespace and escapes spaces (no charsmap).
fn model_bytes(pieces: &[(String, u64)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (text, kind) in pieces {
        put_bytes(&mut out, 1, &piece_msg(text, 0.0, *kind));
    }
    let mut trainer = Vec::new();
    put_uint(&mut trainer, 3, MODEL_TYPE_BPE);
    put_uint(&mut trainer, 40, 0);
    put_bytes(&mut out, 2, &trainer);
    put_bytes(&mut out, 3, &normalizer_msg());
    out
}

const LATIN_SYMBOLS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.,;:!?";

/// Tamil code points used by the tests: consonants, vowel signs, pulli and the anusvara.
const TAMIL_SYMBOLS: [char; 15] = [
    '\u{0B95}', // க
    '\u{0BAE}', // ம
    '\u{0BA9}', // ன
    '\u{0BA8}', // ந
    '\u{0BB0}', // ர
    '\u{0BB5}', // வ
    '\u{0BA3}', // ண
    '\u{0B9F}', // ட
    '\u{0BBE}', // ா (vowel sign aa, Mc)
    '\u{0BBF}', // ி (vowel sign i, Mn)
    '\u{0BC1}', // ு (vowel sign u, Mn)
    '\u{0BCB}', // ோ (vowel sign oo, Mc)
    '\u{0BCD}', // ் (pulli, Mn)
    '\u{0B82}', // ஂ (anusvara, Mn)
    '\u{0BA4}', // த
];

/// One piece per character, no merges, so every character is exactly one token and the
/// prepared text of N characters has N tokens plus the dummy prefix token.
fn toy() -> TamilTokenizer {
    let mut pieces: Vec<(String, u64)> = vec![
        ("<unk>".to_string(), KIND_UNKNOWN),
        ("<s>".to_string(), KIND_CONTROL),
        ("</s>".to_string(), KIND_CONTROL),
        ("\u{2581}".to_string(), KIND_NORMAL),
    ];
    for ch in LATIN_SYMBOLS.chars().chain(TAMIL_SYMBOLS) {
        pieces.push((ch.to_string(), KIND_NORMAL));
    }
    TamilTokenizer::from_bytes(&model_bytes(&pieces)).expect("synthetic model parses")
}

fn tokens_of(tok: &TamilTokenizer, chunk: &str) -> usize {
    let (prepared, _) = prepare_text_prompt(chunk).expect("chunk prepares");
    tok.encode(&prepared).len()
}

// ---------- py_isalnum ----------

#[test]
fn py_isalnum_matches_python_for_each_class() {
    assert!(py_isalnum('a'));
    assert!(py_isalnum('Z'));
    assert!(py_isalnum('5'));
    assert!(py_isalnum('\u{0B95}'), "Tamil letter ka is alnum");
    assert!(py_isalnum('\u{0BE7}'), "Tamil digit one (Nd) is alnum");
    assert!(py_isalnum('\u{00E9}'), "e with acute is alnum");
    assert!(
        !py_isalnum('\u{0BBE}'),
        "Tamil vowel sign aa (Mc) is not alnum"
    );
    assert!(!py_isalnum('\u{0BCD}'), "pulli (Mn) is not alnum");
    assert!(!py_isalnum(' '));
    assert!(!py_isalnum('.'));
    assert!(!py_isalnum('?'));
    assert!(
        !py_isalnum('\u{2047}'),
        "double question mark (Po) is not alnum"
    );
    assert!(!py_isalnum('\u{24B6}'), "circled A (So) is not alnum");
}

// ---------- prepare_text_prompt ----------

#[test]
fn prepare_rejects_empty_and_whitespace_only() {
    assert!(prepare_text_prompt("").is_err());
    assert!(prepare_text_prompt("   \n\t \r\n").is_err());
    assert!(
        prepare_text_prompt("\u{00A0}\u{3000}").is_err(),
        "Unicode spaces are stripped"
    );
    assert!(
        prepare_text_prompt("\u{001C}").is_err(),
        "Python isspace includes U+001C"
    );
}

#[test]
fn prepare_adds_period_only_when_last_char_is_alnum() {
    assert_eq!(
        prepare_text_prompt("hello world").unwrap(),
        ("Hello world.".to_string(), 3)
    );
    assert_eq!(
        prepare_text_prompt("Hello world!").unwrap(),
        ("Hello world!".to_string(), 3)
    );
    assert_eq!(
        prepare_text_prompt("Is it?").unwrap(),
        ("Is it?".to_string(), 3)
    );
    assert_eq!(
        prepare_text_prompt("ab. cd.").unwrap(),
        ("Ab. cd.".to_string(), 3)
    );
    assert_eq!(
        prepare_text_prompt("a; b").unwrap(),
        ("A; b.".to_string(), 3)
    );
}

#[test]
fn prepare_tamil_ending_with_pulli_or_vowel_sign_gets_no_period() {
    assert_eq!(
        prepare_text_prompt("வணக்கம்").unwrap(),
        ("வணக்கம்".to_string(), 3)
    );
    assert_eq!(prepare_text_prompt("நான்கா").unwrap(), ("நான்கா".to_string(), 3));
    assert_eq!(
        prepare_text_prompt("இன்று?").unwrap(),
        ("இன்று?".to_string(), 3)
    );
}

#[test]
fn prepare_tamil_ending_with_a_letter_or_digit_gets_a_period() {
    assert_eq!(
        prepare_text_prompt("அ").unwrap(),
        ("அ.".to_string(), 3),
        "Lo letter"
    );
    assert_eq!(
        prepare_text_prompt("மூன்று 3").unwrap(),
        ("மூன்று 3.".to_string(), 3)
    );
    assert_eq!(prepare_text_prompt("௧").unwrap(), ("௧.".to_string(), 3));
}

#[test]
fn prepare_capitalises_the_first_char_like_python() {
    assert_eq!(prepare_text_prompt("hi").unwrap().0, "Hi.");
    assert_eq!(prepare_text_prompt("Hi").unwrap().0, "Hi.");
    assert_eq!(prepare_text_prompt("élan").unwrap().0, "Élan.");
    // Python upper() of a titlecase digraph and of sharp s uses full case mapping.
    assert_eq!(prepare_text_prompt("\u{01C6}a").unwrap().0, "\u{01C4}a.");
    assert_eq!(prepare_text_prompt("ß a").unwrap().0, "SS a.");
    // Tamil has no case: the first char is left alone.
    assert_eq!(prepare_text_prompt("க").unwrap().0, "க.");
}

#[test]
fn prepare_frames_guess_depends_on_word_count() {
    assert_eq!(prepare_text_prompt("one").unwrap().1, 3);
    assert_eq!(
        prepare_text_prompt("one two three four").unwrap().1,
        3,
        "4 words"
    );
    assert_eq!(
        prepare_text_prompt("one two three four five").unwrap().1,
        1,
        "5 words"
    );
    assert_eq!(
        prepare_text_prompt("a b c d e f g h i j k l").unwrap().1,
        1,
        "long input"
    );
}

#[test]
fn prepare_normalises_newlines_and_double_spaces_once() {
    assert_eq!(
        prepare_text_prompt("one\ntwo\r\nthree").unwrap(),
        ("One two three.".to_string(), 3)
    );
    // Python replaces "  " once and left to right: three spaces become two.
    assert_eq!(prepare_text_prompt("a   b").unwrap().0, "A  b.");
    assert_eq!(prepare_text_prompt("  hello  ").unwrap().0, "Hello.");
}

#[test]
fn prepare_word_count_uses_the_text_after_replacements() {
    // Five words split by a newline and a double space: after replacement it has five words.
    assert_eq!(prepare_text_prompt("a b c d\ne").unwrap().1, 1);
}

#[test]
fn prepare_padding_branch_is_off_for_the_pack_and_correct_when_enabled() {
    let padded = PromptOptions {
        pad_short_inputs: true,
        remove_semicolons: false,
    };
    assert_eq!(
        prepare_with("hi", padded).unwrap(),
        ("        Hi.".to_string(), 3)
    );
    assert_eq!(
        prepare_with("a b c d e", padded).unwrap(),
        ("A b c d e.".to_string(), 1)
    );
    let plain = PromptOptions {
        pad_short_inputs: false,
        remove_semicolons: false,
    };
    assert_eq!(prepare_with("hi", plain).unwrap(), ("Hi.".to_string(), 3));
    assert_eq!(
        prepare_text_prompt("hi").unwrap().0,
        "Hi.",
        "pack default is no padding"
    );
}

#[test]
fn prepare_semicolon_branch_is_off_for_the_pack_and_correct_when_enabled() {
    let replaced = PromptOptions {
        pad_short_inputs: false,
        remove_semicolons: true,
    };
    assert_eq!(prepare_with("a; b", replaced).unwrap().0, "A, b.");
    assert_eq!(
        prepare_text_prompt("a; b").unwrap().0,
        "A; b.",
        "pack default keeps semicolons"
    );
}

// ---------- estimate_max_frames ----------

#[test]
fn estimate_max_frames_matches_the_reference_formula() {
    assert_eq!(estimate_max_frames(0), 25);
    assert_eq!(estimate_max_frames(1), 30);
    assert_eq!(estimate_max_frames(2), 34);
    assert_eq!(estimate_max_frames(6), 50);
    assert_eq!(estimate_max_frames(7), 55);
    assert_eq!(estimate_max_frames(50), 234);
}

#[test]
fn max_tokens_per_chunk_is_fifty() {
    assert_eq!(MAX_TOKENS_PER_CHUNK, 50);
}

// ---------- split_into_chunks ----------

#[test]
fn empty_and_whitespace_input_is_an_error() {
    let tok = toy();
    assert!(split_into_chunks(&tok, "", MAX_TOKENS_PER_CHUNK).is_err());
    assert!(split_into_chunks(&tok, " \n\t ", MAX_TOKENS_PER_CHUNK).is_err());
}

#[test]
fn zero_budget_is_an_error() {
    let tok = toy();
    assert!(split_into_chunks(&tok, "abc", 0).is_err());
}

#[test]
fn single_short_sentence_is_one_chunk() {
    let tok = toy();
    assert_eq!(
        split_into_chunks(&tok, "abc", 50).unwrap(),
        vec!["Abc.".to_string()]
    );
}

#[test]
fn sentences_pack_greedily_up_to_the_budget() {
    let tok = toy();
    let text = "ab. cd. ef. gh.";
    assert_eq!(
        split_into_chunks(&tok, text, 50).unwrap(),
        vec!["Ab. cd. ef. gh.".to_string()]
    );
    assert_eq!(
        split_into_chunks(&tok, text, 12).unwrap(),
        vec!["Ab. cd. ef.".to_string(), "gh.".to_string()]
    );
    assert_eq!(
        split_into_chunks(&tok, text, 4).unwrap(),
        vec!["Ab.", "cd.", "ef.", "gh."]
    );
}

#[test]
fn exact_budget_boundary_fits_at_n_and_splits_at_n_minus_one() {
    let tok = toy();
    // "Ab. cd." is 8 tokens: one chunk at budget 8, two chunks at budget 7.
    assert_eq!(tokens_of(&tok, "Ab. cd."), 8);
    assert_eq!(
        split_into_chunks(&tok, "ab. cd.", 8).unwrap(),
        vec!["Ab. cd.".to_string()]
    );
    assert_eq!(
        split_into_chunks(&tok, "ab. cd.", 7).unwrap(),
        vec!["Ab.".to_string(), "cd.".to_string()]
    );
}

#[test]
fn single_sentence_exactly_at_budget_is_not_split() {
    let tok = toy();
    // "Abcd." is 6 tokens: it fits at 6 and must be hard-split at 5.
    assert_eq!(tokens_of(&tok, "abcd"), 6);
    assert_eq!(
        split_into_chunks(&tok, "abcd", 6).unwrap(),
        vec!["Abcd.".to_string()]
    );
}

#[test]
fn over_long_sentence_without_spaces_is_hard_split_within_budget() {
    let tok = toy();
    let text = "abcdefghijklmnopqrstuvwxyz".repeat(3);
    let chunks = split_into_chunks(&tok, &text, 9).unwrap();
    assert!(chunks.len() > 1);
    for chunk in &chunks {
        assert!(tokens_of(&tok, chunk) <= 9, "chunk over budget: {chunk:?}");
    }
    // The first letter is capitalised and a period is added, as in prepare_text_prompt.
    let expected = format!("A{}.", &text[1..]);
    assert_eq!(chunks.concat(), expected, "no characters lost or reordered");
}

#[test]
fn hard_split_prefers_whitespace_so_words_are_not_cut() {
    let tok = toy();
    let text = "ab cd ef gh ij kl mn op qr st uv wx";
    let chunks = split_into_chunks(&tok, text, 20).unwrap();
    assert!(chunks.len() > 1);
    assert_eq!(chunks.join(" "), "Ab cd ef gh ij kl mn op qr st uv wx.");
    for chunk in &chunks {
        assert!(tokens_of(&tok, chunk) <= 20, "chunk over budget: {chunk:?}");
        for word in chunk.split_whitespace() {
            assert!(
                word.chars().count() == 2 || word == "wx.",
                "word was cut: {word:?} in {chunk:?}"
            );
        }
    }
}

#[test]
fn over_long_unspaced_tamil_never_splits_before_a_combining_mark_or_after_a_pulli() {
    let tok = toy();
    // Each unit is consonant, vowel sign, consonant, pulli, ... and ends with a vowel sign.
    let unit = "க\u{0BBE}ம\u{0BCD}ந\u{0BBF}ர\u{0BC1}வ\u{0BCB}ண\u{0BCD}ட\u{0BBE}";
    let text = unit.repeat(20);
    let chunks = split_into_chunks(&tok, &text, MAX_TOKENS_PER_CHUNK).unwrap();
    assert!(chunks.len() >= 4, "must split: {} chunks", chunks.len());
    assert_eq!(chunks.concat(), text, "concatenation equals the input");
    for chunk in &chunks {
        assert!(
            tokens_of(&tok, chunk) <= MAX_TOKENS_PER_CHUNK,
            "chunk over budget"
        );
        let first = chunk.chars().next().expect("chunks are non-empty");
        assert!(!is_tamil_mark(first), "chunk starts with a mark: {chunk:?}");
        assert!(
            !chunk.ends_with('\u{0BCD}'),
            "chunk ends after a pulli: {chunk:?}"
        );
    }
}

#[test]
fn single_cluster_over_budget_is_emitted_alone_and_terminates() {
    let tok = toy();
    // With budget 1 every character exceeds the budget, so each one is emitted alone.
    let chunks = split_into_chunks(&tok, "ab", 1).unwrap();
    assert_eq!(
        chunks,
        vec!["A".to_string(), "b".to_string(), ".".to_string()]
    );
}

fn is_tamil_mark(c: char) -> bool {
    matches!(c, '\u{0B82}'..='\u{0B83}' | '\u{0BBE}'..='\u{0BCD}' | '\u{0BD7}')
}
