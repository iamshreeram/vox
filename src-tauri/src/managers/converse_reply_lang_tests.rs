//! U8 reply-language tests (REPLY_LANG.md rev 2): the directive matrix, the shared wrapper
//! `planner_prompt_localised`, request capture on the four planner routes (plain non-streaming,
//! plain streaming, tool loop, unsupported-tools fallback) and Tamil speech parsing.
//!
//! Requests are captured on the loopback provider through the helpers in `converse_rag_tests`.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use serde_json::Value;

use crate::managers::converse_plan::{
    parse_plan, reply_language_directive, JobParsePolicy, PlanKind, PlanPrompt,
    TAMIL_REPLY_DIRECTIVE,
};
use crate::managers::converse_platform::{
    plan_turn_core, planner_prompt_for_policy, planner_prompt_localised, with_reply_language,
    ToolSupport,
};
use crate::managers::converse_rag_tests::{
    core_for, input_with, plan_json, primary_factory, ready_rig, record_probe_for, request_json,
    serve_sequence, text_reply,
};

/// Tamil script, mostly Tamil letters: `is_mostly_tamil` is true.
const TAMIL_UTTERANCE: &str = "வணக்கம், இன்று நீங்கள் எப்படி இருக்கிறீர்கள்?";
/// Plain English utterance.
const ENGLISH_UTTERANCE: &str = "status of Atlas";
/// Tamil speech for the parse test.
const TAMIL_SPEECH: &str = "வணக்கம், நான் நலமாக இருக்கிறேன்.";

fn plain_policy() -> JobParsePolicy {
    JobParsePolicy::default()
}

fn jobs_policy() -> JobParsePolicy {
    JobParsePolicy {
        jobs_allowed: true,
        ask_allowed: true,
        agents: vec!["researcher".to_string()],
        role_active: false,
    }
}

fn count_directive(text: &str) -> usize {
    text.matches(TAMIL_REPLY_DIRECTIVE).count()
}

// ---- Directive matrix ----------------------------------------------------------------------

#[test]
fn directive_ta_setting_with_english_utterance_is_some() {
    assert_eq!(
        reply_language_directive("ta", ENGLISH_UTTERANCE),
        Some(TAMIL_REPLY_DIRECTIVE)
    );
}

#[test]
fn directive_en_setting_with_tamil_utterance_is_some() {
    assert_eq!(
        reply_language_directive("en", TAMIL_UTTERANCE),
        Some(TAMIL_REPLY_DIRECTIVE)
    );
}

#[test]
fn directive_en_setting_with_english_utterance_is_none() {
    assert_eq!(reply_language_directive("en", ENGLISH_UTTERANCE), None);
}

#[test]
fn directive_ta_setting_with_empty_utterance_is_some() {
    assert_eq!(
        reply_language_directive("ta", ""),
        Some(TAMIL_REPLY_DIRECTIVE)
    );
}

#[test]
fn directive_latin_dominant_utterance_with_one_tamil_word_is_none_under_en() {
    // One Tamil word among many Latin letters: is_mostly_tamil is false.
    let utterance = "please check the notes for அட்லஸ் today and reply briefly";
    assert_eq!(reply_language_directive("en", utterance), None);
}

#[test]
fn directive_auto_setting_follows_the_utterance() {
    assert_eq!(
        reply_language_directive("auto", TAMIL_UTTERANCE),
        Some(TAMIL_REPLY_DIRECTIVE)
    );
    assert_eq!(reply_language_directive("auto", ENGLISH_UTTERANCE), None);
}

// ---- Wrapper: planner_prompt_localised / with_reply_language -------------------------------

#[test]
fn wrapper_none_is_byte_identical_to_planner_prompt_for_policy() {
    for policy in [plain_policy(), jobs_policy()] {
        let input = input_with(ENGLISH_UTTERANCE);
        let base = planner_prompt_for_policy(&input, &policy);
        let got = planner_prompt_localised(&input, &policy, "en");
        assert_eq!(got, base, "en + English must not change the prompt");
        assert_eq!(count_directive(&got.system), 0);
    }
}

#[test]
fn wrapper_appends_directive_once_after_the_original_system() {
    let input = input_with(ENGLISH_UTTERANCE);
    let policy = plain_policy();
    let base = planner_prompt_for_policy(&input, &policy);
    let got = planner_prompt_localised(&input, &policy, "ta");

    assert!(
        got.system.starts_with(&base.system),
        "original system is a prefix"
    );
    assert_eq!(
        got.system,
        format!("{}\n{TAMIL_REPLY_DIRECTIVE}", base.system)
    );
    assert_eq!(count_directive(&got.system), 1, "directive appended once");
}

#[test]
fn wrapper_combined_is_system_then_user_and_user_is_unchanged() {
    let input = input_with(TAMIL_UTTERANCE);
    let policy = jobs_policy();
    let base = planner_prompt_for_policy(&input, &policy);
    let got = planner_prompt_localised(&input, &policy, "en");

    assert_eq!(got.combined, format!("{}\n\n{}", got.system, got.user));
    assert_eq!(got.user, base.user, "user text is untouched");
    assert!(got.system.contains(TAMIL_REPLY_DIRECTIVE));
}

#[test]
fn wrapper_is_idempotent_when_directive_already_present() {
    let input = input_with(ENGLISH_UTTERANCE);
    let base = planner_prompt_for_policy(&input, &plain_policy());
    let once = with_reply_language(base, ENGLISH_UTTERANCE, "ta");
    let twice = with_reply_language(once.clone(), ENGLISH_UTTERANCE, "ta");

    assert_eq!(twice, once, "a second application changes nothing");
    assert_eq!(count_directive(&twice.system), 1);
    assert_eq!(
        twice.combined,
        format!("{}\n\n{}", twice.system, twice.user)
    );
}

#[test]
fn wrapper_with_reply_language_none_returns_the_prompt_unchanged() {
    let input = input_with(ENGLISH_UTTERANCE);
    let base: PlanPrompt = planner_prompt_for_policy(&input, &plain_policy());
    let got = with_reply_language(base.clone(), ENGLISH_UTTERANCE, "en");
    assert_eq!(got, base);
}

// ---- Parse: Tamil speech is accepted --------------------------------------------------------

#[test]
fn parse_plan_accepts_an_answer_whose_speech_is_tamil_script() {
    let raw = plan_json("answer", TAMIL_SPEECH);
    let plan = parse_plan(&raw, "sent prompt").expect("Tamil speech parses");
    assert_eq!(plan.kind, PlanKind::Answer);
    assert_eq!(plan.speech, TAMIL_SPEECH);
}

// ---- Request capture on the four planner routes ---------------------------------------------

/// Which planner route a capture test drives (DESIGN in REPLY_LANG.md step 2).
#[derive(Clone, Copy)]
enum Route {
    /// (a) text route, non-streaming.
    PlainNonStreaming,
    /// (b) text route, streaming.
    PlainStreaming,
    /// (c) tool loop (`plan_with_tools`), no unsupported probe.
    ToolLoop,
    /// (d) tool route whose probe is Unsupported: the plain fallback.
    UnsupportedFallback,
}

/// Plans one turn on `route` with `selected` as the settings language and returns the
/// `system` field of the first captured request.
async fn captured_system(route: Route, selected: &str, utterance: &str) -> String {
    let (base, seen) = serve_sequence(vec![text_reply(&plan_json("answer", "On track."))]);
    let tool_loop = matches!(route, Route::ToolLoop | Route::UnsupportedFallback);
    let mut rig = ready_rig(&base, tool_loop);
    if matches!(route, Route::UnsupportedFallback) {
        record_probe_for(&rig, &base, ToolSupport::Unsupported);
    }
    rig.settings.selected_language = selected.into();
    let core = core_for(Some(rig.runtime.session(&rig.settings)), true);
    let make = primary_factory(Arc::new(AtomicUsize::new(0)));
    let mut sink = |_: &str| {};
    let on_text: &mut (dyn FnMut(&str) + Send) = &mut sink;
    let streaming = if matches!(route, Route::PlainStreaming) {
        Some(on_text)
    } else {
        None
    };

    let plan = plan_turn_core(
        &core,
        &rig.settings,
        input_with(utterance),
        &make,
        streaming,
    )
    .await
    .expect("planner route returns a plan");
    assert_eq!(plan.kind, PlanKind::Answer);

    let reqs = seen.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let first = reqs.first().expect("at least one captured request");
    let sent: Value = request_json(first);
    sent["system"].as_str().unwrap_or("").to_string()
}

#[tokio::test]
async fn reply_directive_reaches_plain_non_streaming_request() {
    let system = captured_system(Route::PlainNonStreaming, "ta", ENGLISH_UTTERANCE).await;
    assert_eq!(count_directive(&system), 1, "directive once in system");
}

#[tokio::test]
async fn reply_directive_reaches_plain_streaming_request() {
    let system = captured_system(Route::PlainStreaming, "ta", ENGLISH_UTTERANCE).await;
    assert_eq!(count_directive(&system), 1, "directive once in system");
}

#[tokio::test]
async fn reply_directive_reaches_tool_loop_system_prompt() {
    let system = captured_system(Route::ToolLoop, "ta", ENGLISH_UTTERANCE).await;
    assert_eq!(count_directive(&system), 1, "directive once in system");
}

#[tokio::test]
async fn reply_directive_reaches_unsupported_tools_fallback_request() {
    let system = captured_system(Route::UnsupportedFallback, "ta", ENGLISH_UTTERANCE).await;
    assert_eq!(count_directive(&system), 1, "directive once in system");
}

#[tokio::test]
async fn en_english_turn_sends_no_directive_on_plain_non_streaming() {
    let system = captured_system(Route::PlainNonStreaming, "en", ENGLISH_UTTERANCE).await;
    assert_eq!(count_directive(&system), 0);
}

#[tokio::test]
async fn en_english_turn_sends_no_directive_on_plain_streaming() {
    let system = captured_system(Route::PlainStreaming, "en", ENGLISH_UTTERANCE).await;
    assert_eq!(count_directive(&system), 0);
}

#[tokio::test]
async fn en_english_turn_sends_no_directive_on_tool_loop() {
    let system = captured_system(Route::ToolLoop, "en", ENGLISH_UTTERANCE).await;
    assert_eq!(count_directive(&system), 0);
}

#[tokio::test]
async fn en_english_turn_sends_no_directive_on_unsupported_fallback() {
    let system = captured_system(Route::UnsupportedFallback, "en", ENGLISH_UTTERANCE).await;
    assert_eq!(count_directive(&system), 0);
}
