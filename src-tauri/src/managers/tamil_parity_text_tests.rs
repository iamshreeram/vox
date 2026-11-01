//! Real-pack parity tests for text preparation and chunking (fixtures: src-tauri/tests/fixtures/tamil/, made by
//! scripts/tamil_tts_golden.py). `#[ignore]`d because they need the Tamil pack:
//!   VOX_TAMIL_DIR=<pack folder> cargo test --lib tamil_parity -- --ignored
//! They assert the corpus is non-empty so a skipped test can never pass vacuously.

use crate::managers::tamil_text::{
    estimate_max_frames, prepare_text_prompt, split_into_chunks, MAX_TOKENS_PER_CHUNK,
};
use crate::managers::tamil_tokenizer::TamilTokenizer;
use serde_json::Value;
use std::path::PathBuf;

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tamil")
        .join(name);
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&raw).expect("fixture is valid json")
}

fn real_tokenizer() -> TamilTokenizer {
    let dir = std::env::var("VOX_TAMIL_DIR").expect("set VOX_TAMIL_DIR to the Tamil pack folder");
    TamilTokenizer::load(&PathBuf::from(dir).join("tokenizer.model"))
        .expect("tokenizer.model loads")
}

fn ids(v: &Value) -> Vec<u32> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_u64().unwrap() as u32)
        .collect()
}

#[test]
#[ignore = "needs the Tamil pack; set VOX_TAMIL_DIR"]
fn tamil_parity_text_prep_and_chunks_match_the_reference() {
    let tok = real_tokenizer();
    let fx = fixture("tamil_text_parity.json");
    assert_eq!(
        fx["max_token_per_chunk"].as_u64().unwrap() as usize,
        MAX_TOKENS_PER_CHUNK
    );
    let cases = fx["cases"].as_array().unwrap();
    assert!(cases.len() >= 25, "corpus too small: {}", cases.len());
    let mut errors_seen = 0;
    let mut multi_chunk_seen = 0;
    let mut overflow_seen = 0;
    for case in cases {
        let text = case["text"].as_str().unwrap();
        let label: String = text.chars().take(24).collect();
        if case["error"].as_bool().unwrap() {
            errors_seen += 1;
            assert!(
                prepare_text_prompt(text).is_err(),
                "{label:?} must be an error"
            );
            continue;
        }
        let (prepared, guess) =
            prepare_text_prompt(text).unwrap_or_else(|e| panic!("{label:?}: {e}"));
        assert_eq!(
            prepared,
            case["prepared"].as_str().unwrap(),
            "prepared {label:?}"
        );
        assert_eq!(
            guess as u64,
            case["frames_after_eos_guess"].as_u64().unwrap(),
            "guess {label:?}"
        );
        let chunks = split_into_chunks(&tok, text, MAX_TOKENS_PER_CHUNK).unwrap();
        let want_chunks: Vec<&str> = case["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        if case["reference_overflow"].as_bool().unwrap() {
            // Documented deviation: the reference would overflow the 50-token budget, Rust splits further.
            overflow_seen += 1;
            assert!(
                chunks.len() > want_chunks.len(),
                "{label:?} must be split further"
            );
            for chunk in &chunks {
                let (p, _) = prepare_text_prompt(chunk).unwrap();
                assert!(
                    tok.encode(&p).len() <= MAX_TOKENS_PER_CHUNK,
                    "chunk over budget in {label:?}"
                );
            }
            continue;
        }
        assert_eq!(chunks, want_chunks, "chunks {label:?}");
        if chunks.len() > 1 {
            multi_chunk_seen += 1;
        }
        for (i, chunk) in chunks.iter().enumerate() {
            let (p, _) = prepare_text_prompt(chunk).unwrap();
            let got_ids = tok.encode(&p);
            assert_eq!(
                got_ids,
                ids(&case["chunk_ids"][i]),
                "chunk ids {label:?}#{i}"
            );
            assert_eq!(
                estimate_max_frames(got_ids.len()) as u64,
                case["max_frames"][i].as_u64().unwrap(),
                "max frames {label:?}#{i}"
            );
        }
    }
    assert!(
        errors_seen >= 2,
        "the corpus must cover empty and whitespace-only input"
    );
    assert!(
        multi_chunk_seen >= 2,
        "the corpus must cover multi-chunk texts"
    );
    assert!(
        overflow_seen >= 1,
        "the corpus must cover an over-budget text"
    );
}
