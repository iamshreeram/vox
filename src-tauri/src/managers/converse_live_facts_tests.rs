//! PR-B acceptance tests: the planner must not invent current facts.
//!
//! Provider built-in web search is not available through the configured gateway (the capability
//! check was refused by its network policy), so this change is prompt-only: one rule, carried by
//! the roles-enabled prompt (production always sets it). It routes lookups to the agent handoff
//! when the prompt describes it, and otherwise makes the planner say it cannot look things up.
//!
//! The rule is a single constant on purpose. The existing agent suites pin the agent addendum as
//! a pure suffix of the roles-only prompt, and the agent-only prompt byte for byte, so the rule
//! cannot vary with the agent flag and cannot appear without roles.
//!
//! Every check that uses the constant first asserts it is non-empty, because
//! `str::contains("")` is always true.

use super::*;

fn input(
    roles_enabled: bool,
    agent_enabled: bool,
    role: Option<&'static str>,
) -> PlanInput<'static> {
    PlanInput {
        utterance: "what is the weather in Chennai today",
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
    assert!(
        !LIVE_FACTS_RULE.is_empty(),
        "LIVE_FACTS_RULE must be defined"
    );
}

#[test]
fn l1_the_roles_prompt_carries_the_rule_with_and_without_the_agent() {
    defined();
    for agent in [false, true] {
        let p = build_plan_prompt(&input(true, agent, None));
        assert!(p.system.contains(LIVE_FACTS_RULE), "agent = {agent}");
    }
}

#[test]
fn l2_the_rule_precedes_the_agent_addendum_so_it_can_refer_to_it() {
    defined();
    let p = build_plan_prompt(&input(true, true, None));
    let rule_at = p.system.find(LIVE_FACTS_RULE).unwrap_or(usize::MAX);
    let agent_at = p.system.find("Use kind agent only when").unwrap_or(0);
    assert!(rule_at < agent_at, "rule first, then the agent addendum");
}

#[test]
fn l3_the_agent_only_prompt_is_unchanged() {
    defined();
    let p = build_plan_prompt(&input(false, true, None));
    assert!(!p.system.contains(LIVE_FACTS_RULE));
}

#[test]
fn l4_a_prompt_with_neither_roles_nor_agent_is_unchanged() {
    defined();
    let p = build_plan_prompt(&input(false, false, None));
    assert!(!p.system.contains(LIVE_FACTS_RULE));
}

#[test]
fn l5_the_rule_appears_exactly_once_also_inside_a_role() {
    defined();
    for role in [None, Some("a patient maths tutor")] {
        for agent in [false, true] {
            let p = build_plan_prompt(&input(true, agent, role));
            assert_eq!(
                p.system.matches(LIVE_FACTS_RULE).count(),
                1,
                "role = {role:?}, agent = {agent}"
            );
        }
    }
}

#[test]
fn l6_the_wording_routes_to_the_agent_or_admits_it_cannot_look_up() {
    defined();
    let rule = LIVE_FACTS_RULE.to_lowercase();
    assert!(
        rule.contains("kind agent"),
        "names the plan kind it routes to"
    );
    assert!(
        rule.contains("if kind agent is described below"),
        "the agent route is conditional on the agent being offered"
    );
    assert!(
        rule.contains("can't look that up"),
        "says it cannot look up"
    );
    assert!(rule.contains("out of date"), "hedges what it already knows");
    assert!(rule.contains("never invent"), "forbids inventing");
    assert!(rule.contains("pretend"), "never pretends it searched");
}

#[test]
fn l7_the_rule_is_short_and_cannot_break_the_data_fences() {
    defined();
    assert!(LIVE_FACTS_RULE.chars().count() <= 700, "rule stays short");
    assert!(!LIVE_FACTS_RULE.contains("<<<") && !LIVE_FACTS_RULE.contains(">>>"));
    assert!(!LIVE_FACTS_RULE.contains('\n'), "one paragraph");
}
