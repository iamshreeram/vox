//! CI-safe tests for the pure-Rust SentencePiece BPE tokenizer. They build a tiny
//! ModelProto in code with a protobuf writer, so they need no Tamil pack and no env var.

use crate::managers::tamil_tokenizer::{PieceKind, TamilTokenizer};

const KIND_NORMAL: u64 = 1;
const KIND_UNKNOWN: u64 = 2;
const KIND_CONTROL: u64 = 3;
const MODEL_TYPE_BPE: u64 = 2;
const MODEL_TYPE_UNIGRAM: u64 = 1;

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

fn normalizer_msg(add_dummy: bool, remove_ws: bool, escape: bool) -> Vec<u8> {
    let mut msg = Vec::new();
    put_bytes(&mut msg, 1, b"nmt_nfkc");
    put_uint(&mut msg, 3, u64::from(add_dummy));
    put_uint(&mut msg, 4, u64::from(remove_ws));
    put_uint(&mut msg, 5, u64::from(escape));
    msg
}

/// Builds a serialized ModelProto. `normalizer` = None omits the normalizer message entirely.
fn model(
    pieces: &[(&str, f32, u64)],
    model_type: u64,
    unk_id: u64,
    normalizer: Option<(bool, bool, bool)>,
) -> Vec<u8> {
    let mut out = Vec::new();
    for (text, score, kind) in pieces {
        put_bytes(&mut out, 1, &piece_msg(text, *score, *kind));
    }
    let mut trainer = Vec::new();
    put_uint(&mut trainer, 3, model_type);
    put_uint(&mut trainer, 40, unk_id);
    put_bytes(&mut out, 2, &trainer);
    if let Some((dummy, remove, escape)) = normalizer {
        put_bytes(&mut out, 3, &normalizer_msg(dummy, remove, escape));
    }
    out
}

/// Toy vocabulary. Ids: 0 unk, 1-2 control, 3 space mark, 4 a, 5 b, 6 c,
/// 7 "ab" (-2), 8 "bc" (-1), 9 "\u{2581}a" (-3).
fn toy_pieces() -> Vec<(&'static str, f32, u64)> {
    vec![
        ("<unk>", 0.0, KIND_UNKNOWN),
        ("<s>", 0.0, KIND_CONTROL),
        ("</s>", 0.0, KIND_CONTROL),
        ("\u{2581}", 0.0, KIND_NORMAL),
        ("a", 0.0, KIND_NORMAL),
        ("b", 0.0, KIND_NORMAL),
        ("c", 0.0, KIND_NORMAL),
        ("ab", -2.0, KIND_NORMAL),
        ("bc", -1.0, KIND_NORMAL),
        ("\u{2581}a", -3.0, KIND_NORMAL),
    ]
}

fn toy_bytes() -> Vec<u8> {
    model(&toy_pieces(), MODEL_TYPE_BPE, 0, Some((true, true, true)))
}

fn toy() -> TamilTokenizer {
    TamilTokenizer::from_bytes(&toy_bytes()).expect("toy model parses")
}

#[test]
fn parser_reads_pieces_scores_and_types() {
    let tok = toy();
    assert_eq!(tok.vocab_size(), 10);
    let unk = tok.piece(0).expect("piece 0");
    assert_eq!(unk.text, "<unk>");
    assert_eq!(unk.kind, PieceKind::Unknown);
    let control = tok.piece(1).expect("piece 1");
    assert_eq!(control.kind, PieceKind::Control);
    let ab = tok.piece(7).expect("piece 7");
    assert_eq!(ab.text, "ab");
    assert_eq!(ab.score, -2.0);
    assert_eq!(ab.kind, PieceKind::Normal);
    assert!(tok.piece(10).is_none());
}

#[test]
fn merges_follow_the_highest_score_first() {
    // "\u{2581}abc": bc (-1) merges before ab (-2) and before "\u{2581}a" (-3) is applied.
    assert_eq!(toy().encode("abc"), vec![9, 8]);
}

#[test]
fn equal_scores_merge_the_leftmost_pair_first() {
    let pieces = [
        ("<unk>", 0.0, KIND_UNKNOWN),
        ("\u{2581}", 0.0, KIND_NORMAL),
        ("a", 0.0, KIND_NORMAL),
        ("b", 0.0, KIND_NORMAL),
        ("c", 0.0, KIND_NORMAL),
        ("ab", -5.0, KIND_NORMAL),
        ("bc", -5.0, KIND_NORMAL),
    ];
    let tok = TamilTokenizer::from_bytes(&model(&pieces, MODEL_TYPE_BPE, 0, None))
        .expect("tie model parses");
    // Leftmost (a,b) wins, so "bc" is never formed. A rightmost rule would give [1, 2, 6].
    assert_eq!(tok.encode("abc"), vec![1, 5, 4]);
}

#[test]
fn dummy_prefix_and_whitespace_are_normalized() {
    let tok = toy();
    assert_eq!(tok.encode("a"), vec![9]);
    assert_eq!(tok.encode("  a   b  "), vec![9, 3, 5]);
    assert_eq!(tok.encode("a b"), tok.encode("  a   b  "));
}

#[test]
fn empty_and_space_only_input_encodes_to_nothing() {
    let tok = toy();
    assert_eq!(tok.encode(""), Vec::<u32>::new());
    assert_eq!(tok.encode("   "), Vec::<u32>::new());
}

#[test]
fn unknown_characters_fuse_into_one_unk_per_run() {
    let tok = toy();
    // "x" and "?" are not in the vocabulary: one fused unk after the space mark.
    assert_eq!(tok.encode("x?y"), vec![3, 0]);
    // Words are separate, so each one gets its own unk.
    assert_eq!(tok.encode("x y"), vec![3, 0, 3, 0]);
    // A known symbol between unknown runs keeps them apart.
    assert_eq!(tok.encode("a?b"), vec![9, 0, 5]);
}

#[test]
fn default_normalizer_applies_when_the_message_is_absent() {
    let no_normalizer =
        TamilTokenizer::from_bytes(&model(&toy_pieces(), MODEL_TYPE_BPE, 0, None)).expect("parses");
    // Default add_dummy_prefix is true.
    assert_eq!(no_normalizer.encode("a"), vec![9]);

    let no_dummy = TamilTokenizer::from_bytes(&model(
        &toy_pieces(),
        MODEL_TYPE_BPE,
        0,
        Some((false, true, true)),
    ))
    .expect("parses");
    assert_eq!(no_dummy.encode("a"), vec![4]);
}

#[test]
fn decode_round_trips_plain_text() {
    let tok = toy();
    assert_eq!(tok.decode(&tok.encode("abc")), "abc");
    assert_eq!(tok.decode(&tok.encode("a b")), "a b");
    assert_eq!(tok.decode(&tok.encode("a?b")), "a \u{2047} b");
}

#[test]
fn decode_renders_unk_as_the_replacement_marker() {
    let tok = toy();
    assert_eq!(tok.decode(&[0]), " \u{2047} ");
    assert_eq!(tok.decode(&[3, 0]), " \u{2047} ");
    assert_eq!(tok.decode(&[0, 3, 4]), " \u{2047}  a");
}

#[test]
fn decode_skips_control_and_survives_out_of_range_ids() {
    let tok = toy();
    assert_eq!(tok.decode(&[1, 9]), "a");
    assert_eq!(tok.decode(&[]), "");
    // Must not panic. Out-of-range ids render like unk.
    assert_eq!(tok.decode(&[9999]), " \u{2047} ");
    assert_eq!(tok.decode(&[u32::MAX, 4]), " \u{2047} a");
}

#[test]
fn garbage_and_empty_input_are_errors() {
    assert!(TamilTokenizer::from_bytes(&[]).is_err());
    assert!(TamilTokenizer::from_bytes(&[0xff; 16]).is_err());
    assert!(TamilTokenizer::from_bytes(&[0x0a, 0xff]).is_err());
}

#[test]
fn non_bpe_model_type_is_an_error() {
    let unigram = model(
        &toy_pieces(),
        MODEL_TYPE_UNIGRAM,
        0,
        Some((true, true, true)),
    );
    let err = TamilTokenizer::from_bytes(&unigram).expect_err("unigram rejected");
    assert!(err.contains("BPE"), "message was: {err}");
}

#[test]
fn model_without_pieces_is_an_error() {
    let empty = model(&[], MODEL_TYPE_BPE, 0, Some((true, true, true)));
    assert!(TamilTokenizer::from_bytes(&empty).is_err());
}

#[test]
fn unk_id_outside_the_vocabulary_is_an_error() {
    let bad = model(&toy_pieces(), MODEL_TYPE_BPE, 99, Some((true, true, true)));
    assert!(TamilTokenizer::from_bytes(&bad).is_err());
}

#[test]
fn unknown_fields_are_skipped_by_wire_type() {
    let mut bytes = Vec::new();
    put_uint(&mut bytes, 99, 12345);
    put_key(&mut bytes, 98, 1);
    bytes.extend_from_slice(&[0u8; 8]);
    put_bytes(&mut bytes, 97, b"ignored");
    bytes.extend_from_slice(&toy_bytes());
    let tok = TamilTokenizer::from_bytes(&bytes).expect("extra fields skipped");
    assert_eq!(tok.encode("a"), vec![9]);
}

#[test]
fn truncated_models_never_panic() {
    let full = toy_bytes();
    assert!(TamilTokenizer::from_bytes(&full).is_ok());
    for cut in 0..full.len() {
        // Either Ok or Err is acceptable; a panic would fail the test.
        let _ = TamilTokenizer::from_bytes(&full[..cut]);
    }
}

#[test]
fn load_of_a_missing_file_is_an_error() {
    let path = std::path::Path::new("/nonexistent/vox-tamil/tokenizer.model");
    assert!(TamilTokenizer::load(path).is_err());
}
