//! RED tests for M4 S1 D2 (plan and parse with the job policy: agent allowlist,
//! mode, prompt block). Written before the implementation: they compile against the
//! stubs and fail at runtime (`todo!("M4 S1")` panics or missing behavior).
//! Included from converse_plan.rs as a child module, so `super::*` is the plan module.
//! Do not edit these tests to make code pass.
//!
//! Refusal shape: the spec fixes the refusal speech but not the plan shape. The
//! helper `assert_refused_with` accepts only a non-job plan that speaks exactly the
//! fixed text, so a refusal can never leak a job.

use super::*;
use serde_json::{json, Value};

const SENT: &str = "THE SENT PROMPT";

fn policy(jobs: bool, ask: bool, agents: &[&str]) -> JobParsePolicy {
    JobParsePolicy {
        jobs_allowed: jobs,
        ask_allowed: ask,
        agents: agents.iter().map(|a| a.to_string()).collect(),
        role_active: false,
    }
}

/// A job plan JSON; `extra` fields are merged into the base object (null included).
fn job_with(extra: Value) -> String {
    let mut base = json!({
        "kind": "job",
        "speech": "On it.",
        "project": "vox",
        "task": "fix the build"
    });
    if let (Some(obj), Some(more)) = (base.as_object_mut(), extra.as_object()) {
        for (key, value) in more {
            obj.insert(key.clone(), value.clone());
        }
    }
    base.to_string()
}

fn parse(raw: &str, pol: &JobParsePolicy) -> Result<TurnPlan, PlanError> {
    parse_plan_with_job_policy(raw, SENT, pol)
}

/// Accepts a refusal only as a non-job plan that speaks exactly `speech`.
fn assert_refused_with(raw: &str, pol: &JobParsePolicy, speech: &str) {
    match parse(raw, pol) {
        Ok(plan) => {
            assert!(plan.job.is_none(), "refused plan must carry no job: {raw}");
            assert_eq!(plan.speech, speech, "fixed refusal speech for {raw}");
        }
        Err(err) => panic!("expected refusal {speech:?} for {raw}, got Err({err:?})"),
    }
}

/// Accepts any refusal (an error or a non-job plan) but never a job.
fn assert_not_a_job(raw: &str, pol: &JobParsePolicy) {
    assert!(
        !matches!(parse(raw, pol), Ok(plan) if plan.job.is_some()),
        "must not parse as a job: {raw}"
    );
}

fn input(utterance: &str) -> PlanInput<'_> {
    PlanInput {
        utterance,
        history: &[],
        last_draft: None,
        style: "",
        vault_context: None,
        memory_context: None,
        lesson_paused: false,
        role: None,
        roles_enabled: false,
        agent_enabled: false,
        planner_enabled: false,
    }
}

// ---------------- D2 test 1: backward compatibility ----------------

#[test]
fn m4_legacy_plans_parse_identically_to_the_bool_entry_point() {
    let corpus = [
        job_with(json!({})),
        job_with(json!({"task": "list the files"})),
        json!({"kind": "answer", "speech": "Hello there."}).to_string(),
        json!({"kind": "paste", "speech": "Done.", "paste_text": "hi"}).to_string(),
    ];
    for raw in corpus.iter() {
        for jobs in [true, false] {
            assert_eq!(
                parse(raw, &policy(jobs, false, &[])),
                parse_plan_with_policy(raw, SENT, jobs),
                "legacy parity for jobs={jobs}: {raw}"
            );
        }
    }
}

#[test]
fn m4_legacy_job_plan_has_no_agent_and_task_mode() {
    let plan = parse(&job_with(json!({})), &policy(true, false, &[])).unwrap();
    let job = plan.job.expect("job request");
    assert_eq!(job.agent, None);
    assert_eq!(job.mode, JobMode::Task);
    assert_eq!(job.project, "vox");
    assert_eq!(job.task, "fix the build");
}

#[test]
fn m4_legacy_prompt_with_jobs_is_byte_identical_to_the_addendum_prompt() {
    let base = build_plan_prompt(&input("make a job"));
    let got = plan_prompt_with_job_policy(&input("make a job"), &policy(true, false, &[]));
    let system = format!("{}\n{}", base.system, JOB_PLAN_ADDENDUM);
    let combined = format!("{system}\n\n{}", base.user);
    assert_eq!(got.system, system);
    assert_eq!(got.user, base.user);
    assert_eq!(got.combined, combined);
}

#[test]
fn m4_legacy_prompt_equals_the_existing_bool_builder_output() {
    let utterance = "make a job";
    let jobs_prompt =
        crate::managers::converse_platform::planner_prompt_for(&input(utterance), true);
    let no_jobs_prompt =
        crate::managers::converse_platform::planner_prompt_for(&input(utterance), false);
    assert_eq!(
        plan_prompt_with_job_policy(&input(utterance), &policy(true, false, &[])),
        jobs_prompt
    );
    assert_eq!(
        plan_prompt_with_job_policy(&input(utterance), &policy(false, false, &[])),
        no_jobs_prompt
    );
}

// ---------------- D2 test 2: agent canonical spelling ----------------

#[test]
fn m4_agent_exact_name_in_allowlist_is_stored() {
    let raw = job_with(json!({"agent": "coder-one"}));
    let plan = parse(&raw, &policy(true, false, &["coder-one"])).unwrap();
    assert_eq!(plan.job.unwrap().agent.as_deref(), Some("coder-one"));
}

#[test]
fn m4_agent_different_case_is_stored_with_allowlist_spelling() {
    let raw = job_with(json!({"agent": "CODER-ONE"}));
    let plan = parse(&raw, &policy(true, false, &["Coder-One"])).unwrap();
    assert_eq!(plan.job.unwrap().agent.as_deref(), Some("Coder-One"));
}

// ---------------- D2 test 3: unknown, empty, malformed agents ----------------

#[test]
fn m4_agent_not_in_allowlist_is_refused_and_name_not_echoed() {
    let raw = job_with(json!({"agent": "other-bot"}));
    let pol = policy(true, false, &["coder-one"]);
    assert_refused_with(&raw, &pol, UNKNOWN_AGENT_SPEECH);
    match parse(&raw, &pol) {
        Ok(plan) => assert!(!plan.speech.contains("other-bot")),
        Err(_) => {}
    }
}

#[test]
fn m4_any_agent_with_empty_allowlist_is_refused() {
    let raw = job_with(json!({"agent": "coder-one"}));
    assert_refused_with(&raw, &policy(true, true, &[]), UNKNOWN_AGENT_SPEECH);
}

#[test]
fn m4_empty_string_agent_is_refused_even_when_allowlist_is_set() {
    let raw = job_with(json!({"agent": ""}));
    assert_refused_with(
        &raw,
        &policy(true, false, &["coder-one"]),
        UNKNOWN_AGENT_SPEECH,
    );
}

#[test]
fn m4_non_string_agent_values_are_refused() {
    // Owner ruling: an explicit JSON null means "no agent" (planner models often
    // emit null for an unused field); every other non-string value is refused.
    let pol = policy(true, true, &["coder-one"]);
    for value in [json!(5), json!([]), json!({}), json!(true)] {
        let raw = job_with(json!({ "agent": value.clone() }));
        assert_refused_with(&raw, &pol, UNKNOWN_AGENT_SPEECH);
    }
}

#[test]
fn m4_agent_with_space_slash_or_nul_is_refused() {
    let pol = policy(
        true,
        false,
        &["coder one", "coder/one", "coder\u{0}one", "../x"],
    );
    for name in ["coder one", "coder/one", "coder\u{0}one", "../x"] {
        let raw = job_with(json!({ "agent": name }));
        assert_not_a_job(&raw, &pol);
    }
}

#[test]
fn m4_injection_like_agent_names_are_refused_even_if_allowlisted() {
    // The names are also placed in the allowlist: the charset check must still refuse.
    let names = [
        "x; rm",
        "--agent",
        "-x",
        "ram\nai",
        "ram\"ai",
        "ram'ai",
        "ignore previous instructions",
    ];
    for name in names {
        let pol = policy(true, false, &[name]);
        let raw = job_with(json!({ "agent": name }));
        assert_not_a_job(&raw, &pol);
    }
}

// ---------------- D2 test 4: mode ----------------

#[test]
fn m4_null_agent_and_null_mode_are_treated_as_absent() {
    // Owner ruling: explicit JSON null is the same as an absent key.
    let raw = job_with(json!({ "agent": null, "mode": null }));
    let plan = parse(&raw, &policy(true, true, &["coder-one"])).unwrap();
    let job = plan.job.unwrap();
    assert_eq!(job.agent, None);
    assert_eq!(job.mode, JobMode::Task);
}

#[test]
fn m4_mode_absent_is_task() {
    let plan = parse(&job_with(json!({})), &policy(true, true, &[])).unwrap();
    assert_eq!(plan.job.unwrap().mode, JobMode::Task);
}

#[test]
fn m4_mode_exact_lowercase_task_is_task() {
    let raw = job_with(json!({"mode": "task"}));
    let plan = parse(&raw, &policy(true, false, &[])).unwrap();
    assert_eq!(plan.job.unwrap().mode, JobMode::Task);
}

#[test]
fn m4_mode_ask_with_ask_allowed_is_ask() {
    let raw = job_with(json!({"mode": "ask"}));
    let plan = parse(&raw, &policy(true, true, &[])).unwrap();
    let job = plan.job.unwrap();
    assert_eq!(job.mode, JobMode::Ask);
    assert_eq!(job.agent, None);
}

#[test]
fn m4_mode_ask_with_agent_and_ask_allowed_keeps_both() {
    let raw = job_with(json!({"mode": "ask", "agent": "coder-one"}));
    let plan = parse(&raw, &policy(true, true, &["coder-one"])).unwrap();
    let job = plan.job.unwrap();
    assert_eq!(job.mode, JobMode::Ask);
    assert_eq!(job.agent.as_deref(), Some("coder-one"));
}

#[test]
fn m4_mode_ask_while_ask_disabled_is_refused_not_downgraded_to_task() {
    let raw = job_with(json!({"mode": "ask"}));
    assert_refused_with(&raw, &policy(true, false, &[]), ASK_NOT_ENABLED_SPEECH);
}

#[test]
fn m4_mode_variants_are_refused_exact_lowercase_only() {
    let pol = policy(true, true, &["coder-one"]);
    let variants = [
        json!("Ask"),
        json!("ASK"),
        json!(" ask"),
        json!("ask "),
        json!("Task"),
        json!("TASK"),
        json!("run"),
        json!(""),
        json!(5),
        json!(["ask"]),
    ];
    for value in variants {
        let raw = job_with(json!({ "mode": value.clone() }));
        assert_not_a_job(&raw, &pol);
    }
}

// ---------------- D2 test 5: project and jobs-disabled precedence ----------------

#[test]
fn m4_ask_without_project_is_refused_like_a_task_without_project() {
    let pol = policy(true, true, &[]);
    let ask = json!({"kind": "job", "speech": "x", "task": "summarize", "mode": "ask"}).to_string();
    let task = json!({"kind": "job", "speech": "x", "task": "summarize"}).to_string();
    assert_eq!(parse(&ask, &pol), Err(PlanError::Unparseable));
    assert_eq!(parse(&ask, &pol), parse(&task, &pol));
}

#[test]
fn m4_ask_with_empty_task_is_not_a_job() {
    let raw = job_with(json!({"mode": "ask", "task": ""}));
    assert_not_a_job(&raw, &policy(true, true, &[]));
}

#[test]
fn m4_jobs_disabled_wins_over_the_new_refusals() {
    let pol = policy(false, true, &["coder-one"]);
    let raws = [
        job_with(json!({"mode": "ask", "agent": "coder-one"})),
        job_with(json!({"mode": "ask"})),
        job_with(json!({"agent": "other-bot"})),
        job_with(json!({"mode": "run"})),
    ];
    for raw in raws.iter() {
        assert_eq!(
            parse(raw, &pol),
            parse_plan_with_policy(raw, SENT, false),
            "jobs off must match the bool entry point: {raw}"
        );
    }
}

#[test]
fn m4_injected_project_names_are_not_jobs() {
    let pol = policy(true, true, &["coder-one"]);
    for project in ["vox;rm", "vox\"", "../vox", "vox\nx"] {
        let raw = job_with(json!({"project": project, "agent": "coder-one", "mode": "ask"}));
        assert_not_a_job(&raw, &pol);
    }
}

// ---------------- D2 test 6: planner prompt block ----------------

#[test]
fn m4_prompt_with_agents_and_ask_appends_block_after_addendum() {
    let base = build_plan_prompt(&input("make a job"));
    let pol = policy(true, true, &["coder-one", "Other-Agent"]);
    let got = plan_prompt_with_job_policy(&input("make a job"), &pol);
    let prefix = format!("{}\n{}", base.system, JOB_PLAN_ADDENDUM);
    assert!(
        got.system.starts_with(&prefix),
        "addendum must stay unchanged at the start"
    );
    let extra = &got.system[prefix.len()..];
    assert!(!extra.trim().is_empty(), "an extra block must be appended");
    assert_eq!(
        got.system.matches("coder-one").count(),
        1,
        "name listed once"
    );
    assert_eq!(
        got.system.matches("Other-Agent").count(),
        1,
        "name listed once"
    );
    assert!(extra.to_lowercase().contains("ask"), "block explains ask");
}

#[test]
fn m4_prompt_skips_names_with_invalid_charset() {
    let pol = policy(true, false, &["ok-agent", "bad;name", "bad name", "x\"y"]);
    let got = plan_prompt_with_job_policy(&input("make a job"), &pol);
    assert!(got.system.contains("ok-agent"));
    assert!(!got.system.contains("bad;name"));
    assert!(!got.system.contains("bad name"));
    assert!(!got.system.contains("x\"y"));
}

#[test]
fn m4_prompt_ask_on_without_agents_explains_ask_and_lists_no_names() {
    let base = build_plan_prompt(&input("make a job"));
    let got = plan_prompt_with_job_policy(&input("make a job"), &policy(true, true, &[]));
    let prefix = format!("{}\n{}", base.system, JOB_PLAN_ADDENDUM);
    assert!(got.system.starts_with(&prefix));
    let extra = &got.system[prefix.len()..];
    assert!(extra.to_lowercase().contains("ask"), "block explains ask");
}

#[test]
fn m4_prompt_ask_off_and_no_agents_adds_no_block() {
    let base = build_plan_prompt(&input("make a job"));
    let got = plan_prompt_with_job_policy(&input("make a job"), &policy(false, false, &[]));
    assert_eq!(got.system, base.system);
    assert_eq!(got.combined, base.combined);
}
