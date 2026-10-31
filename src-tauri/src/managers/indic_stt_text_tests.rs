//! Text-safety tests for realistic Tamil recognition output.
//!
//! Each sentence goes through the public cleanup functions in the same order
//! and with the same defaults as `transcription::post_process_transcription_text`
//! (custom words, filler removal, normalization). That function is private to
//! `transcription.rs`, so the sequence is mirrored here instead of called. The
//! Chinese script conversion step is skipped on purpose: it only touches output
//! whose language is a Chinese variety, and none of these sentences is Chinese.

use crate::audio_toolkit::{
    apply_custom_words, normalize_transcription_output, remove_filler_words, OutputLanguageEvidence,
};

/// Default `word_correction_threshold` in `settings.rs`.
const DEFAULT_THRESHOLD: f64 = 0.18;

/// Realistic Tamil transcription output. Covers vowel signs, pulli (virama),
/// aytham, grantha letters, and Tamil text with `, . ? ! ;` punctuation.
/// No Tamil digits are used.
const TAMIL_SENTENCES: [&str; 11] = [
    "வணக்கம், நீங்கள் எப்படி இருக்கிறீர்கள்?",
    "இன்று வானிலை மிகவும் அழகாக இருக்கிறது.",
    "நான் பள்ளிக்கூடத்திற்குச் செல்கிறேன்.",
    "எனக்கு ஒரு கப் தேநீர் வேண்டும், சர்க்கரை இல்லாமல்.",
    "அவர் ஜனவரி மாதம் சென்னைக்கு வருவார்!",
    "எஃகு பாலம் மிகவும் உறுதியானது.",
    "ஸ்ரீ ராம் ஹலோ சொன்னார்.",
    "மெதுவாக பேசுங்கள், புரிகிறது.",
    "நாளை மழை பெய்யலாம்; குடை எடுத்துச் செல்லுங்கள்.",
    "தமிழ் மொழி மிகப் பழமையானது.",
    "ஷாப்பிங் செய்ய கடைக்குப் போனேன்.",
];

/// Mirrors the default cleanup path: custom words only when the list is
/// non-empty (as in `post_process_transcription_text`), then filler removal
/// enabled, then normalization.
fn default_cleanup(
    text: &str,
    language: &OutputLanguageEvidence,
    custom_words: &[String],
    custom_filler_words: &Option<Vec<String>>,
) -> String {
    let corrected = if custom_words.is_empty() {
        text.to_string()
    } else {
        apply_custom_words(text, custom_words, DEFAULT_THRESHOLD)
    };
    let without_fillers = remove_filler_words(&corrected, language, custom_filler_words, true);
    normalize_transcription_output(&without_fillers)
}

fn evidence_cases() -> Vec<OutputLanguageEvidence> {
    vec![
        OutputLanguageEvidence::Unknown,
        OutputLanguageEvidence::UserSelected("ta".to_string()),
        OutputLanguageEvidence::ModelDetected("ta".to_string()),
    ]
}

#[test]
fn tamil_corpus_covers_the_scripts_it_is_meant_to_cover() {
    let all: String = TAMIL_SENTENCES.concat();
    for (name, ch) in [
        ("vowel sign i", '\u{0BBF}'),
        ("pulli", '\u{0BCD}'),
        ("aytham", '\u{0B83}'),
        ("grantha ja", '\u{0B9C}'),
        ("grantha sha", '\u{0BB7}'),
        ("grantha sa", '\u{0BB8}'),
        ("grantha ha", '\u{0BB9}'),
        ("comma", ','),
        ("full stop", '.'),
        ("question mark", '?'),
        ("exclamation mark", '!'),
    ] {
        assert!(all.contains(ch), "corpus lacks {name}");
    }
    assert!(
        !all.chars().any(|c| ('\u{0BE6}'..='\u{0BEF}').contains(&c)),
        "corpus must not use Tamil digits"
    );
}

#[test]
fn tamil_sentences_pass_default_cleanup_byte_identical() {
    for language in evidence_cases() {
        for sentence in TAMIL_SENTENCES {
            let cleaned = default_cleanup(sentence, &language, &[], &None);
            assert_eq!(
                cleaned,
                sentence.trim(),
                "language: {language:?}, sentence: {sentence:?}"
            );
        }
    }
}

#[test]
fn tamil_sentences_pass_cleanup_with_english_and_tamil_custom_words() {
    // "Vox" does not occur and no ASCII is in the sentences. "சென்னை" does not
    // occur. "பள்ளிக்கூடம்" does occur. Unicode custom words are skipped by the
    // fuzzy fallback (see `is_supported_fuzzy_key` in audio_toolkit/text.rs),
    // so none of them changes Tamil text.
    let cases: [Vec<String>; 2] = [
        vec!["Vox".to_string(), "சென்னை".to_string()],
        vec!["ChatGPT".to_string(), "பள்ளிக்கூடம்".to_string()],
    ];
    for custom_words in &cases {
        for sentence in TAMIL_SENTENCES {
            let cleaned = default_cleanup(
                sentence,
                &OutputLanguageEvidence::Unknown,
                custom_words,
                &None,
            );
            assert_eq!(cleaned, sentence.trim(), "custom: {custom_words:?}");
        }
    }
}

#[test]
fn english_custom_word_corrects_a_misspelling_inside_tamil_text() {
    // A custom English word is applied in mixed output. The Tamil words around
    // it are left as they are.
    let custom_words = vec!["ChatGPT".to_string()];
    let cleaned = default_cleanup(
        "நான் chatgbt உடன் பேசினேன்.",
        &OutputLanguageEvidence::Unknown,
        &custom_words,
        &None,
    );
    assert_eq!(cleaned, "நான் ChatGPT உடன் பேசினேன்.");
}

#[test]
fn english_custom_word_absent_from_mixed_text_changes_nothing() {
    let custom_words = vec!["ChatGPT".to_string()];
    let cleaned = default_cleanup(
        "நான் Vox பயன்படுத்துகிறேன்.",
        &OutputLanguageEvidence::Unknown,
        &custom_words,
        &None,
    );
    assert_eq!(cleaned, "நான் Vox பயன்படுத்துகிறேன்.");
}

#[test]
fn two_tamil_repetitions_are_kept_by_stutter_collapse() {
    // Stutter collapse only acts on three or more repeats, so this emphatic
    // repetition must survive.
    let sentence = "மெதுவாக மெதுவாக பேசுங்கள்.";
    assert_eq!(
        default_cleanup(sentence, &OutputLanguageEvidence::Unknown, &[], &None),
        sentence
    );
}

#[test]
fn built_in_english_filler_is_removed_from_tamil_output() {
    // "hmm" is a universal filler. Removing it must not disturb the Tamil text.
    assert_eq!(
        default_cleanup(
            "hmm வணக்கம், நீங்கள் எப்படி இருக்கிறீர்கள்?",
            &OutputLanguageEvidence::Unknown,
            &[],
            &None,
        ),
        "வணக்கம், நீங்கள் எப்படி இருக்கிறீர்கள்?"
    );
}

#[test]
fn custom_filler_does_not_match_inside_a_tamil_word() {
    // A custom filler that is only a prefix of a longer Tamil word must not be
    // removed. The word boundary has to treat the pulli and vowel signs as word
    // characters.
    let fillers = Some(vec!["இரு".to_string()]);
    assert_eq!(
        default_cleanup(
            "இன்று வானிலை மிகவும் அழகாக இருக்கிறது.",
            &OutputLanguageEvidence::Unknown,
            &[],
            &fillers,
        ),
        "இன்று வானிலை மிகவும் அழகாக இருக்கிறது."
    );
}

#[test]
fn custom_filler_removes_a_whole_tamil_word() {
    let fillers = Some(vec!["நான்".to_string()]);
    assert_eq!(
        default_cleanup(
            "நான் பள்ளிக்கூடத்திற்குச் செல்கிறேன்.",
            &OutputLanguageEvidence::Unknown,
            &[],
            &fillers,
        ),
        "பள்ளிக்கூடத்திற்குச் செல்கிறேன்."
    );
}

#[test]
fn normalization_trims_and_collapses_spaces_in_tamil_text() {
    assert_eq!(
        default_cleanup(
            "  வணக்கம்,   நீங்கள்  எப்படி  இருக்கிறீர்கள்?  ",
            &OutputLanguageEvidence::Unknown,
            &[],
            &None,
        ),
        "வணக்கம், நீங்கள் எப்படி இருக்கிறீர்கள்?"
    );
}
