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
    assert!(TEACHING_RULE.chars().count() <= 900, "rule stays short");
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
