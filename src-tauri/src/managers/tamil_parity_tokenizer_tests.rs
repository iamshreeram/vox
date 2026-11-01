//! Real-pack parity tests for the tokenizer (fixture: src-tauri/tests/fixtures/tamil/tamil_tokenizer_parity.json,
//! made by scripts/tamil_tts_golden.py). `#[ignore]`d because they need the Tamil pack:
//!   VOX_TAMIL_DIR=<pack folder> cargo test --lib tamil_parity -- --ignored
//! They assert the corpus is non-empty so a skipped test can never pass vacuously.

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
fn tamil_parity_tokenizer_encode_matches_the_reference_for_every_case() {
    let tok = real_tokenizer();
    let fx = fixture("tamil_tokenizer_parity.json");
    let cases = fx["encode"].as_array().unwrap();
    assert!(cases.len() >= 30, "corpus too small: {}", cases.len());
    let mut bad = Vec::new();
    for case in cases {
        let text = case["text"].as_str().unwrap();
        let want = ids(&case["ids"]);
        let got = tok.encode(text);
        if got != want {
            bad.push(format!("{text:?}: want {want:?} got {got:?}"));
        }
    }
    assert!(
        bad.is_empty(),
        "{} mismatches:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
#[ignore = "needs the Tamil pack; set VOX_TAMIL_DIR"]
fn tamil_parity_tokenizer_decode_matches_the_reference() {
    let tok = real_tokenizer();
    let fx = fixture("tamil_tokenizer_parity.json");
    let mut checked = 0;
    for case in fx["decode"].as_array().unwrap() {
        assert_eq!(
            tok.decode(&ids(&case["ids"])),
            case["text"].as_str().unwrap(),
            "decode of {:?}",
            case["ids"]
        );
        checked += 1;
    }
    // Round trip text from the encode corpus too (the reference decode of its own ids).
    for case in fx["encode"].as_array().unwrap() {
        assert_eq!(
            tok.decode(&ids(&case["ids"])),
            case["decoded"].as_str().unwrap()
        );
        checked += 1;
    }
    assert!(checked >= 30);
}
