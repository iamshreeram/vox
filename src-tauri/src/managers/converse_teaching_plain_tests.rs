//! Regression tests: stories, quizzes and lessons are plain answers, never roles.
//!
//! PR-A taught the planner to start a role (`set_role`) for "tell me a story", "quiz me" and
//! "teach me". A role persists until the user ends it and switches the speech rule to "as long
//! as the task needs", so after one story or quiz every later reply was long, slow (extra
//! non-streamed call, chunked playback) and steered back into the role ("Shall we get back to
//! the story?"). Live replay on the real model showed exactly that. Roles are now reserved for
//! explicit personas (act as, pretend, a doctor, a lawyer); stories, quizzes and lessons are
//! plain `answer` turns that use the conversation history.
//!
//! The rule is one constant in the roles-enabled block (production always sets it), so the
//! agent-only prompt stays byte-identical and the agent addendum stays a pure suffix.
//!
//! Every check that uses a constant first asserts it is non-empty, because
//! `str::contains("")` is always true.

use super::*;
use crate::managers::converse_plan::{
    parse_plan_with_job_policy, JobParsePolicy, MAX_ROLE_SPEECH_CHARS, MAX_SPEECH_CHARS,
};

fn input(
    roles_enabled: bool,
    agent_enabled: bool,
    role: Option<&'static str>,
) -> PlanInput<'static> {
    PlanInput {
        utterance: "tell me a story",
        history: &[],
        last_draft: None,
        style: "warm and brief",
        vault_context: None,
        memory_context: None,
        role,
        lesson_paused: false,
        roles_enabled,
        agent_enabled,
        planner_enabled: false,
        role_state: None,
    }
}

fn defined() {
    assert!(!TEACHING_RULE.is_empty(), "TEACHING_RULE must be defined");
}

#[test]
fn t1_the_roles_prompt_carries_the_teaching_rule_once_with_and_without_a_role() {
    defined();
    for role in [None, Some("a patient doctor")] {
        for agent in [false, true] {
            let p = build_plan_prompt(&input(true, agent, role));
            assert_eq!(
                p.system.matches(TEACHING_RULE).count(),
                1,
                "role = {role:?}, agent = {agent}"
            );
        }
    }
}

#[test]
fn t2_the_rule_is_absent_from_prompts_without_roles() {
    defined();
    for agent in [false, true] {
        let p = build_plan_prompt(&input(false, agent, None));
        assert!(!p.system.contains(TEACHING_RULE), "agent = {agent}");
    }
}

#[test]
fn t3_the_prompt_no_longer_starts_a_role_for_stories_quizzes_or_teaching() {
    defined();
    let system = build_plan_prompt(&input(true, false, None)).system;
    for old in [
        "to be quizzed, to be told a story",
        "a teacher, a quiz host, a storyteller, a language tutor",
    ] {
        assert!(
            !system.contains(old),
            "the old role trigger {old:?} must be gone"
        );
    }
    let lower = system.to_lowercase();
    assert!(
        lower.contains("do not start a role to tell a story"),
        "the prompt says stories, quizzes and lessons are not roles"
    );
}

#[test]
fn t4_professional_and_persona_roles_still_work() {
    defined();
    let lower = build_plan_prompt(&input(true, false, None))
        .system
        .to_lowercase();
    for word in [
        "kind set_role",
        "act as",
        "doctor",
        "lawyer",
        "not professional advice",
        "kind clear_role",
    ] {
        assert!(lower.contains(word), "{word:?} must remain in the prompt");
    }
}

#[test]
fn t5_the_rule_says_how_to_tell_a_story_and_run_a_quiz() {
    defined();
    let rule = TEACHING_RULE.to_lowercase();
    for needle in [
        "never roles",
        "start to finish",
        "do not stop to ask",
        "one question at a time",
        "running score",
        "never repeat a question",
    ] {
        assert!(rule.contains(needle), "rule must contain {needle:?}");
    }
}

#[test]
fn t6_the_rule_lets_a_story_override_the_short_speech_rule() {
    defined();
    let rule = TEACHING_RULE.to_lowercase();
    assert!(
        rule.contains("despite the short-speech rule"),
        "an explicit override, because the short rule is in the same prompt"
    );
    let system = build_plan_prompt(&input(true, false, None)).system;
    let short_at = system.find(SPEAK_SHORT_RULE).expect("short rule present");
    let rule_at = system.find(TEACHING_RULE).expect("teaching rule present");
    assert!(
        short_at < rule_at,
        "the override comes after the rule it overrides"
    );
}

#[test]
fn t7_an_active_role_does_not_steer_unrelated_chat_back_into_itself() {
    defined();
    assert!(
        TEACHING_RULE
            .to_lowercase()
            .contains("do not steer back to the role"),
        "the live replay showed in-role replies pulling the user back"
    );
}

#[test]
fn t8_the_rule_is_one_short_paragraph_and_cannot_break_the_data_fences() {
    defined();
    assert!(TEACHING_RULE.chars().count() <= 1000, "rule stays short");
    assert!(!TEACHING_RULE.contains("<<<") && !TEACHING_RULE.contains(">>>"));
    assert!(!TEACHING_RULE.contains('\n'), "one paragraph");
}

#[test]
fn t9_the_rule_sits_before_the_role_addendum_and_after_the_live_facts_rule() {
    defined();
    let system = build_plan_prompt(&input(true, false, None)).system;
    let live_at = system.find(LIVE_FACTS_RULE).expect("live facts rule");
    let rule_at = system.find(TEACHING_RULE).expect("teaching rule");
    let roles_at = system.find("Roles: when the user asks").expect("addendum");
    assert!(live_at < rule_at && rule_at < roles_at);
}

// ---- long-form speech: a requested story or lesson is spoken whole -----------------------------
//
// A plain answer is capped at MAX_SPEECH_CHARS (400) as a safety net against a runaway reply, and
// the cap cut a story mid-word. The model opts in with `"long": true` for a requested story or
// lesson, which allows MAX_ROLE_SPEECH_CHARS and trims back to a sentence end. The default cap
// is untouched.

fn parse_plain(raw: &str) -> TurnPlan {
    parse_plan_with_job_policy(raw, "THE SENT PROMPT", &JobParsePolicy::default())
        .expect("a valid plan")
}

/// `n` whole sentences, each 26 chars incl. the trailing space ("Pip flew to the oak tree. ").
fn sentences(n: usize) -> String {
    "Pip flew to the oak tree. ".repeat(n)
}

fn answer_json(speech: &str, extra: &str) -> String {
    format!(r#"{{"kind":"answer",{extra}"speech":"{speech}"}}"#)
}

#[test]
fn l1_a_long_flagged_plain_answer_of_a_thousand_chars_arrives_whole() {
    let story = sentences(38).trim_end().to_string(); // 987 chars
    assert!(
        story.chars().count() > MAX_SPEECH_CHARS,
        "control: over 400"
    );
    assert!(
        story.chars().count() <= MAX_ROLE_SPEECH_CHARS,
        "control: under 1200"
    );
    let plan = parse_plain(&answer_json(&story, r#""long":true,"#));
    assert_eq!(
        plan.speech, story,
        "the whole story, ending on its last sentence"
    );
    assert!(plan.speech.ends_with('.'));
}

#[test]
fn l2_without_the_long_flag_a_plain_answer_is_still_capped_at_400() {
    let story = sentences(38).trim_end().to_string();
    let plan = parse_plain(&answer_json(&story, ""));
    assert!(plan.speech.chars().count() <= MAX_SPEECH_CHARS);
    assert!(
        plan.speech.chars().count() > 300,
        "control: it was cut, not dropped"
    );
}

#[test]
fn l3_a_long_flagged_answer_over_1200_is_trimmed_back_to_a_sentence_end() {
    let story = sentences(60); // 1560 chars
    let plan = parse_plain(&answer_json(story.trim_end(), r#""long":true,"#));
    let n = plan.speech.chars().count();
    assert!(n <= MAX_ROLE_SPEECH_CHARS, "{n} chars");
    assert!(n > MAX_SPEECH_CHARS, "long-form is not cut at 400");
    assert!(plan.speech.ends_with("tree."), "ends on a whole sentence");
    assert!(
        story.starts_with(&plan.speech),
        "an exact prefix of the reply"
    );
    assert_eq!(
        story.chars().nth(n),
        Some(' '),
        "the cut is at a sentence boundary, never mid-word"
    );
}

#[test]
fn l4_long_must_be_a_json_boolean_true() {
    let story = sentences(38).trim_end().to_string();
    for extra in [
        r#""long":"true","#,
        r#""long":1,"#,
        r#""long":false,"#,
        r#""long":null,"#,
        r#""long":[true],"#,
    ] {
        let plan = parse_plain(&answer_json(&story, extra));
        assert!(
            plan.speech.chars().count() <= MAX_SPEECH_CHARS,
            "{extra} must not lift the cap"
        );
    }
}

#[test]
fn l5_the_long_flag_does_not_lift_the_clarify_cap() {
    let story = sentences(38).trim_end().to_string();
    let raw = format!(r#"{{"kind":"clarify","long":true,"speech":"{story}"}}"#);
    let plan = parse_plain(&raw);
    assert!(plan.speech.chars().count() <= MAX_SPEECH_CHARS);
}

#[test]
fn l6_with_a_role_active_the_long_flag_changes_nothing() {
    let story = sentences(60);
    let policy = JobParsePolicy {
        role_active: true,
        ..JobParsePolicy::default()
    };
    let with = parse_plan_with_job_policy(
        &answer_json(story.trim_end(), r#""long":true,"#),
        "THE SENT PROMPT",
        &policy,
    )
    .unwrap();
    let without = parse_plan_with_job_policy(
        &answer_json(story.trim_end(), ""),
        "THE SENT PROMPT",
        &policy,
    )
    .unwrap();
    assert_eq!(with.speech, without.speech);
}

#[test]
fn l7_the_rule_tells_the_model_to_flag_a_story_and_how_long_it_may_be() {
    defined();
    assert!(
        TEACHING_RULE.contains(r#""long": true"#),
        "the rule must name the flag"
    );
    assert!(
        TEACHING_RULE.contains("1000 characters"),
        "the rule must give the model the length that fits the cap"
    );
    // The advertised length must fit under the cap; checked at compile time.
    const _ADVERTISED_LENGTH_FITS_THE_CAP: () = assert!(1000 < MAX_ROLE_SPEECH_CHARS);
}

#[test]
fn l8_the_default_400_cut_lands_on_a_sentence_end_never_mid_word() {
    // The live replay of a long bedtime story the model forgot to flag was spoken up to
    // "One evening, a cold wind swept" and then stopped.
    let story = sentences(38);
    let plan = parse_plain(&answer_json(story.trim_end(), ""));
    let n = plan.speech.chars().count();
    assert!(n <= MAX_SPEECH_CHARS && n > 300, "{n} chars");
    assert!(plan.speech.ends_with("tree."), "ends on a whole sentence");
    assert!(
        story.starts_with(&plan.speech),
        "an exact prefix of the reply"
    );
    assert_eq!(
        story.chars().nth(n),
        Some(' '),
        "cut at a sentence boundary"
    );
}

#[test]
fn l9_a_reply_with_no_sentence_end_inside_the_cap_is_still_hard_cut() {
    // Nothing to trim back to: the safety cap still holds. (Existing behaviour.)
    let run_on = "word ".repeat(200);
    let plan = parse_plain(&answer_json(run_on.trim_end(), ""));
    assert!(plan.speech.chars().count() <= MAX_SPEECH_CHARS);
    assert!(!plan.speech.is_empty());
}

#[test]
fn l10_the_rule_puts_the_long_flag_before_the_speech() {
    defined();
    assert!(
        TEACHING_RULE.contains("before speech") || TEACHING_RULE.contains("before the speech"),
        "the model decides before it writes the story, not after"
    );
}

#[test]
fn l11_an_ellipsis_counts_as_a_sentence_end_like_the_early_speech_streamer() {
    // The streamer releases a sentence ending in an ellipsis followed by a space. If the trim
    // ignored it, the trimmed speech could be shorter than the text already spoken early, and
    // the reconcile step would abort the turn.
    let reply = format!(
        "{} Then Pip waited for the rain\u{2026} And later he flew all the way home again. {}",
        sentences(13).trim_end(),
        sentences(10).trim_end()
    );
    let ellipsis_end = reply.find('\u{2026}').unwrap() + '\u{2026}'.len_utf8();
    let ellipsis_chars = reply[..ellipsis_end].chars().count();
    assert!(
        ellipsis_chars < MAX_SPEECH_CHARS && reply.chars().count() > MAX_SPEECH_CHARS,
        "premise: the ellipsis sentence is inside the cap and the reply is over it"
    );
    let plan = parse_plain(&answer_json(&reply, ""));
    assert!(
        plan.speech.chars().count() >= ellipsis_chars,
        "never shorter than the early-spoken text, {} < {ellipsis_chars}",
        plan.speech.chars().count()
    );
    assert!(plan.speech.chars().count() <= MAX_SPEECH_CHARS);
}
