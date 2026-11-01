//! Red tests for the transcription integration seams of the auto language route
//! (DESIGN.md Revision 4, A3b). The seams are stubs in `transcription.rs`; these tests pin
//! their contracts and fail until they are implemented.

use super::*;
use crate::managers::auto_lang::Source;
use crate::managers::model::{EngineType, ModelSource};

// ---- helpers ----

fn model(
    id: &str,
    filename: &str,
    engine: EngineType,
    downloaded: bool,
    downloading: bool,
    languages: &[&str],
    detects: bool,
) -> ModelInfo {
    ModelInfo {
        id: id.to_string(),
        name: id.to_string(),
        description: String::new(),
        filename: filename.to_string(),
        source: ModelSource::Local,
        size_mb: 100,
        is_downloaded: downloaded,
        is_downloading: downloading,
        partial_size: 0,
        is_directory: false,
        engine_type: engine,
        accuracy_score: 0.5,
        speed_score: 0.5,
        supports_translation: false,
        is_recommended: false,
        supported_languages: languages.iter().map(|s| s.to_string()).collect(),
        supports_language_selection: true,
        is_custom: false,
        supports_streaming: false,
        supports_language_detection: detects,
    }
}

/// A downloaded, idle, multilingual Whisper GGUF model.
fn multilingual_whisper(id: &str) -> ModelInfo {
    model(
        id,
        &format!("ggml-{id}.bin"),
        EngineType::TranscribeCpp,
        true,
        false,
        &["en", "hi", "ta"],
        true,
    )
}

fn ids(candidates: &[LidCandidate]) -> Vec<&str> {
    candidates.iter().map(|c| c.id.as_str()).collect()
}

fn lid_settings() -> AppSettings {
    let mut settings = crate::settings::get_default_settings();
    settings.chinese_script = ChineseScript::AsTranscribed;
    settings.custom_words = Vec::new();
    settings.custom_filler_words = None;
    settings.filler_word_removal_enabled = true;
    settings
}

fn langs(codes: &[&str]) -> Vec<String> {
    codes.iter().map(|c| c.to_string()).collect()
}

// ---- classify_refined ----

#[test]
fn classify_keep_active_discards_refined_text_whatever_it_is() {
    for text in ["", "active text", "திருத்தப்பட்ட உரை"] {
        let refined = AutoRefined {
            text: text.to_string(),
            source: Source::KeepActive,
        };
        assert_eq!(
            classify_refined(refined),
            AutoApplied::KeepActive,
            "{text:?}"
        );
    }
}

#[test]
fn classify_tamil_pack_returns_the_text_exactly() {
    let text = "வணக்கம் தமிழ்நாடு";
    let refined = AutoRefined {
        text: text.to_string(),
        source: Source::TamilPack,
    };
    assert_eq!(
        classify_refined(refined),
        AutoApplied::Tamil(text.to_string())
    );
}

#[test]
fn classify_lid_text_replaces_with_text_and_detected_language() {
    let text = "मैं आज बाजार गया था";
    let refined = AutoRefined {
        text: text.to_string(),
        source: Source::LidText {
            lang: "hi".to_string(),
        },
    };
    assert_eq!(
        classify_refined(refined),
        AutoApplied::Replaced {
            text: text.to_string(),
            lang: "hi".to_string(),
        }
    );
}

// ---- count_nonblank_tokens ----

#[test]
fn count_nonblank_tokens_skips_blank_rows() {
    let rows = ["a", " ", "", "b\n", "c"];
    assert_eq!(count_nonblank_tokens(rows.iter().copied()), 3);
}

#[test]
fn count_nonblank_tokens_of_empty_iterator_is_zero() {
    assert_eq!(count_nonblank_tokens(std::iter::empty::<&str>()), 0);
}

#[test]
fn count_nonblank_tokens_of_all_blank_rows_is_zero() {
    let rows = [" ", "\n", "\t", ""];
    assert_eq!(count_nonblank_tokens(rows.iter().copied()), 0);
}

#[test]
fn count_nonblank_tokens_counts_unicode_tokens() {
    let rows = ["வணக்கம்", " ", "தமிழ்", "\u{0BBE}"];
    // The last row is a lone vowel sign: it is not whitespace, so it counts.
    assert_eq!(count_nonblank_tokens(rows.iter().copied()), 3);
}

// ---- finalize_lid_text (DESIGN C4) ----

#[test]
fn finalize_lid_text_applies_custom_words_because_they_were_not_prompted() {
    let mut settings = lid_settings();
    settings.custom_words = vec!["hello".to_string(), "world".to_string()];

    // The same input is left uncorrected when the custom words were already prompted,
    // so this case distinguishes "not prompted" from "prompted".
    let prompted = post_process_transcription_text(
        "helo wrold".to_string(),
        &settings,
        true,
        &OutputLanguageEvidence::ModelDetected("en".to_string()),
        &langs(&["en"]),
    );
    assert_eq!(prompted, "helo wrold");

    assert_eq!(
        finalize_lid_text("helo wrold".to_string(), "en", &settings),
        "hello world"
    );
}

#[test]
fn finalize_lid_text_converts_chinese_script_only_for_zh() {
    let mut settings = lid_settings();
    settings.chinese_script = ChineseScript::Simplified;
    settings.filler_word_removal_enabled = false;
    let text = "我們今天下午一起去學校圖書館看書，然後再去吃晚飯。";

    assert_eq!(
        finalize_lid_text(text.to_string(), "zh", &settings),
        "我们今天下午一起去学校图书馆看书，然后再去吃晚饭。"
    );
    assert_eq!(finalize_lid_text(text.to_string(), "hi", &settings), text);
}

#[test]
fn finalize_lid_text_uses_model_detected_evidence_for_filler_removal() {
    let settings = lid_settings();

    // Unknown evidence keeps the gated "um"; ModelDetected("en") removes it.
    let unknown = post_process_transcription_text(
        "um uhm ok".to_string(),
        &settings,
        false,
        &OutputLanguageEvidence::Unknown,
        &langs(&["en", "pt"]),
    );
    assert_eq!(unknown, "um ok");

    assert_eq!(
        finalize_lid_text("um uhm ok".to_string(), "en", &settings),
        "ok"
    );
    assert_eq!(
        finalize_lid_text("um uhm ok".to_string(), "pt", &settings),
        "um ok"
    );
}

#[test]
fn finalize_lid_text_of_empty_text_is_empty() {
    let settings = lid_settings();
    assert_eq!(finalize_lid_text(String::new(), "en", &settings), "");
}

#[test]
fn finalize_lid_text_passes_devanagari_and_tamil_through_unchanged() {
    let settings = lid_settings();
    let hindi = "नमस्ते दुनिया, आप कैसे हैं?";
    let tamil = "வணக்கம் தமிழ்நாடு, நீங்கள் எப்படி இருக்கிறீர்கள்?";

    assert_eq!(finalize_lid_text(hindi.to_string(), "hi", &settings), hindi);
    assert_eq!(finalize_lid_text(tamil.to_string(), "ta", &settings), tamil);
}

// ---- lid_candidates_from ----

#[test]
fn downloaded_multilingual_whisper_is_an_inactive_candidate() {
    let models = [multilingual_whisper("whisper-small")];
    let out = lid_candidates_from(&models, "some-other-model");

    assert_eq!(ids(&out), ["whisper-small"]);
    assert!(!out[0].is_active);
    assert!(out[0].multilingual_whisper);
}

#[test]
fn whisper_equal_to_active_id_is_an_active_candidate() {
    let models = [multilingual_whisper("whisper-small")];
    let out = lid_candidates_from(&models, "whisper-small");

    assert_eq!(ids(&out), ["whisper-small"]);
    assert!(out[0].is_active);
}

#[test]
fn english_only_whisper_is_excluded() {
    // Single supported language (the `.en` family).
    let single_language = model(
        "whisper-base.en",
        "ggml-whisper-base.en.bin",
        EngineType::TranscribeCpp,
        true,
        false,
        &["en"],
        true,
    );
    // Multiple languages listed but no language detection.
    let no_detection = model(
        "whisper-tiny-nodetect",
        "ggml-whisper-tiny-nodetect.bin",
        EngineType::TranscribeCpp,
        true,
        false,
        &["en", "hi"],
        false,
    );
    let out = lid_candidates_from(&[single_language, no_detection], "");
    assert!(out.is_empty(), "{:?}", ids(&out));
}

#[test]
fn whisper_that_is_not_downloaded_is_excluded() {
    let mut not_downloaded = multilingual_whisper("whisper-small");
    not_downloaded.is_downloaded = false;
    assert!(lid_candidates_from(&[not_downloaded], "").is_empty());
}

#[test]
fn whisper_that_is_downloading_is_excluded() {
    let mut downloading = multilingual_whisper("whisper-small");
    downloading.is_downloading = true;
    assert!(lid_candidates_from(&[downloading], "").is_empty());
}

#[test]
fn whisper_on_a_non_transcribe_cpp_engine_is_excluded() {
    let onnx = model(
        "whisper-onnx-test",
        "whisper-onnx-test.onnx",
        EngineType::Moonshine,
        true,
        false,
        &["en", "hi", "ta"],
        true,
    );
    assert!(lid_candidates_from(&[onnx], "").is_empty());
}

#[test]
fn multilingual_parakeet_is_excluded_because_its_id_has_no_whisper() {
    let parakeet = model(
        "parakeet-tdt-0.6b-v3",
        "parakeet-tdt-0.6b-v3.gguf",
        EngineType::TranscribeCpp,
        true,
        false,
        &["en", "de", "fr", "es"],
        true,
    );
    assert!(lid_candidates_from(&[parakeet], "").is_empty());
}

#[test]
fn whisper_is_matched_by_filename_as_well_as_id() {
    let by_filename = model(
        "medium-q5",
        "ggml-whisper-medium-q5.bin",
        EngineType::TranscribeCpp,
        true,
        false,
        &["en", "hi"],
        true,
    );
    assert_eq!(ids(&lid_candidates_from(&[by_filename], "")), ["medium-q5"]);
}

#[test]
fn empty_model_list_has_no_candidates() {
    assert!(lid_candidates_from(&[], "whisper-small").is_empty());
}

#[test]
fn candidates_keep_the_order_of_the_model_list() {
    let models = [
        multilingual_whisper("whisper-small"),
        model(
            "parakeet-tdt-0.6b-v3",
            "parakeet.gguf",
            EngineType::TranscribeCpp,
            true,
            false,
            &["en", "de"],
            true,
        ),
        multilingual_whisper("whisper-medium"),
        model(
            "whisper-base.en",
            "ggml-whisper-base.en.bin",
            EngineType::TranscribeCpp,
            true,
            false,
            &["en"],
            true,
        ),
        multilingual_whisper("whisper-large"),
    ];
    let out = lid_candidates_from(&models, "whisper-medium");

    assert_eq!(
        ids(&out),
        ["whisper-small", "whisper-medium", "whisper-large"]
    );
    assert_eq!(
        out.iter().map(|c| c.is_active).collect::<Vec<_>>(),
        [false, true, false]
    );
}
