// M3 S3a tests: job plan parsing (D3), default invariance, prompt addendum.
// Included from converse_plan.rs as a child module, so `super::*` is the plan module.

use super::*;
use serde_json::json;

const SENT: &str = "THE SENT PROMPT";

fn job_json(project: serde_json::Value, task: serde_json::Value) -> String {
    json!({"kind": "job", "speech": "On it.", "project": project, "task": task}).to_string()
}

fn job_ok(project: &str, task: &str) -> String {
    job_json(json!(project), json!(task))
}

fn parse_job(raw: &str) -> Result<TurnPlan, PlanError> {
    parse_plan_with_policy(raw, SENT, true)
}

fn assert_job_rejected(raw: &str, why: &str) {
    assert_eq!(
        parse_job(raw),
        Err(PlanError::Unparseable),
        "job must be rejected: {why}"
    );
}

// ---------------- valid job plans ----------------

#[test]
fn valid_job_plan_is_parsed_with_policy_allowed() {
    let plan = parse_job(&job_ok("vox", "fix the typo in the readme")).unwrap();
    assert_eq!(plan.kind, PlanKind::Job);
    assert_eq!(plan.speech, "On it.");
    assert_eq!(plan.paste_text, None);
    assert_eq!(plan.command, None);
    assert_eq!(
        plan.job,
        Some(JobRequest {
            project: "vox".to_string(),
            task: "fix the typo in the readme".to_string(),
            agent: None,
            mode: crate::managers::converse_plan::JobMode::Task,
        })
    );
}

#[test]
fn job_kind_matches_case_insensitively_like_other_kinds() {
    let raw = json!({"kind": "JOB", "project": "vox", "task": "t"}).to_string();
    assert_eq!(parse_job(&raw).unwrap().kind, PlanKind::Job);
}

#[test]
fn job_plan_inside_a_code_fence_is_parsed() {
    let raw = format!("```json\n{}\n```", job_ok("vox", "add a test"));
    assert_eq!(parse_job(&raw).unwrap().kind, PlanKind::Job);
}

#[test]
fn job_speech_is_sanitized_like_other_kinds() {
    let raw = json!({"kind": "job", "speech": "[Starting] now", "project": "vox", "task": "t"})
        .to_string();
    let plan = parse_job(&raw).unwrap();
    assert!(!plan.speech.contains('['));
    assert!(plan.speech.contains("Starting"));
}

#[test]
fn job_extra_fields_are_ignored_and_do_not_leak_into_the_plan() {
    let raw = json!({
        "kind": "job",
        "speech": "ok",
        "project": "vox",
        "task": "t",
        "paste_text": "should not appear",
        "command": "rm -rf /",
        "zzz": 1
    })
    .to_string();
    let plan = parse_job(&raw).unwrap();
    assert_eq!(plan.kind, PlanKind::Job);
    assert_eq!(plan.paste_text, None);
    assert_eq!(plan.command, None);
    assert_eq!(plan.job.unwrap().task, "t");
}

#[test]
fn job_task_unicode_is_accepted() {
    let plan = parse_job(&job_ok(
        "vox",
        "\u{6dfb}\u{52a0}\u{6d4b}\u{8bd5} \u{2713} caf\u{e9}",
    ))
    .unwrap();
    assert_eq!(
        plan.job.unwrap().task,
        "\u{6dfb}\u{52a0}\u{6d4b}\u{8bd5} \u{2713} caf\u{e9}"
    );
}

#[test]
fn job_task_keeps_newlines_and_tabs() {
    let plan = parse_job(&job_ok("vox", "line one\nline two\tend")).unwrap();
    assert_eq!(plan.job.unwrap().task, "line one\nline two\tend");
}

#[test]
fn job_task_is_trimmed_at_both_ends() {
    let plan = parse_job(&job_ok("vox", "  fix it \n")).unwrap();
    assert_eq!(plan.job.unwrap().task, "fix it");
}

#[test]
fn job_task_strips_control_characters_other_than_newline_and_tab() {
    let plan = parse_job(&job_ok("vox", "a\u{7}b\u{1b}c")).unwrap();
    assert_eq!(plan.job.unwrap().task, "abc");
}

// ---------------- project name rules ----------------

#[test]
fn job_project_names_that_match_the_pattern_are_accepted() {
    let longest = "a".repeat(64);
    for project in ["vox", "my-app_1.0", "A", longest.as_str()] {
        assert!(
            parse_job(&job_ok(project, "t")).is_ok(),
            "project {project:?} must be accepted"
        );
    }
}

#[test]
fn job_project_names_outside_the_pattern_are_rejected() {
    let cases: Vec<String> = vec![
        String::new(),
        ".".to_string(),
        "..".to_string(),
        "../x".to_string(),
        "a/b".to_string(),
        "a\\b".to_string(),
        "a b".to_string(),
        "na\u{ef}ve".to_string(),
        "\u{540d}\u{524d}".to_string(),
        "vox\n".to_string(),
        "a\u{0}b".to_string(),
        "a\u{200b}".to_string(),
        "x;rm -rf".to_string(),
        "a".repeat(65),
    ];
    for project in &cases {
        assert_job_rejected(&job_ok(project, "t"), &format!("project {project:?}"));
    }
}

// ---------------- task length and content rules ----------------

#[test]
fn job_task_empty_or_whitespace_only_is_rejected() {
    for task in ["", "   ", "\n\t \n", "\u{7}\u{8}"] {
        assert_job_rejected(&job_ok("vox", task), &format!("task {task:?}"));
    }
}

#[test]
fn job_task_length_boundary_is_300_chars() {
    assert!(parse_job(&job_ok("vox", &"a".repeat(300))).is_ok());
    assert_job_rejected(&job_ok("vox", &"a".repeat(301)), "301-char task");
}

#[test]
fn job_task_length_counts_chars_not_bytes() {
    assert!(parse_job(&job_ok("vox", &"\u{e9}".repeat(300))).is_ok());
    assert_job_rejected(&job_ok("vox", &"\u{e9}".repeat(301)), "301 multibyte chars");
}

#[test]
fn job_task_length_is_counted_after_control_stripping() {
    // 10 stripped control chars plus 300 letters: 300 chars after stripping, accepted.
    let leading = format!("{}{}", "\u{7}".repeat(10), "a".repeat(300));
    assert_eq!(
        parse_job(&job_ok("vox", &leading))
            .unwrap()
            .job
            .unwrap()
            .task,
        "a".repeat(300)
    );
    // 301 letters plus a trailing control char: 301 after stripping, rejected.
    let trailing = format!("{}{}", "a".repeat(301), "\u{7}");
    assert_job_rejected(&job_ok("vox", &trailing), "301 chars after stripping");
}

#[test]
fn job_task_with_a_nul_anywhere_is_rejected_not_stripped() {
    for task in ["\u{0}", "fix\u{0}it", "fix it\u{0}"] {
        assert_job_rejected(&job_ok("vox", task), &format!("NUL in task {task:?}"));
    }
}

#[test]
fn job_task_with_escaped_control_char_in_json_is_stripped() {
    // Written as a JSON escape so the test controls exactly what the parser sees.
    let raw = r#"{"kind":"job","speech":"ok","project":"vox","task":"a\u0007b"}"#;
    assert_eq!(parse_job(raw).unwrap().job.unwrap().task, "ab");
}

// ---------------- field type and presence rules ----------------

#[test]
fn job_non_string_fields_are_rejected() {
    assert_job_rejected(&job_json(json!(5), json!("t")), "numeric project");
    assert_job_rejected(&job_json(json!("vox"), json!(5)), "numeric task");
    assert_job_rejected(&job_json(json!("vox"), json!(null)), "null task");
    assert_job_rejected(&job_json(json!("vox"), json!(["t"])), "array task");
    assert_job_rejected(&job_json(json!({"a": 1}), json!("t")), "object project");
    assert_job_rejected(&job_json(json!("vox"), json!(true)), "bool task");
}

#[test]
fn job_missing_project_or_task_is_rejected() {
    let no_project = json!({"kind": "job", "speech": "ok", "task": "t"}).to_string();
    let no_task = json!({"kind": "job", "speech": "ok", "project": "vox"}).to_string();
    assert_job_rejected(&no_project, "missing project");
    assert_job_rejected(&no_task, "missing task");
}

// ---------------- fail-closed when jobs are not allowed ----------------

#[test]
fn job_kind_with_jobs_not_allowed_behaves_exactly_like_an_unknown_kind() {
    let raw = job_ok("vox", "fix it");
    let with_policy = parse_plan_with_policy(&raw, SENT, false);
    assert_eq!(with_policy, Err(PlanError::Unparseable));
    assert_eq!(with_policy, parse_plan(&raw, SENT));
}

#[test]
fn job_with_jobs_not_allowed_and_a_valid_answer_before_it_returns_the_answer() {
    let answer = r#"{"kind":"answer","speech":"Here you go."}"#;
    let raw = format!("{answer}\n{}", job_ok("vox", "fix it"));
    let plan = parse_plan_with_policy(&raw, SENT, false).unwrap();
    assert_eq!(plan.kind, PlanKind::Answer);
    assert_eq!(plan.speech, "Here you go.");
    assert_eq!(plan.job, None);
}

#[test]
fn echoed_job_json_from_the_sent_prompt_is_ignored_even_when_allowed() {
    let example = job_ok("vox", "example task");
    let sent = format!("Reply like this: {example}");
    assert_eq!(
        parse_plan_with_policy(&example, &sent, true),
        Err(PlanError::Unparseable)
    );
}

#[test]
fn unknown_kinds_stay_unparseable_when_jobs_are_allowed() {
    let raw = r#"{"kind":"launch_missiles","speech":"hi"}"#;
    assert_eq!(
        parse_plan_with_policy(raw, SENT, true),
        Err(PlanError::Unparseable)
    );
}

// ---------------- default invariance (parse_plan unchanged) ----------------

#[test]
fn parse_plan_default_results_are_unchanged_for_existing_inputs() {
    let answer = parse_plan(
        r#"{"kind":"answer","speech":"It is three o'clock.","emotion":"cheerful"}"#,
        SENT,
    )
    .unwrap();
    assert_eq!(answer.kind, PlanKind::Answer);
    assert_eq!(answer.speech, "It is three o'clock.");
    assert_eq!(answer.emotion, crate::managers::emotion::Emotion::Cheerful);
    assert_eq!(answer.job, None);

    let paste = parse_plan(
        r#"{"kind":"paste","paste_text":"Hi Sam","speech":"Done."}"#,
        SENT,
    )
    .unwrap();
    assert_eq!(paste.paste_text.as_deref(), Some("Hi Sam"));
    assert_eq!(paste.job, None);

    let command = parse_plan(r#"{"kind":"command","command":"open Safari"}"#, SENT).unwrap();
    assert_eq!(command.command.as_deref(), Some("open Safari"));

    assert_eq!(
        parse_plan("The capital of France is Paris.", SENT)
            .unwrap()
            .speech,
        "The capital of France is Paris."
    );
    assert_eq!(parse_plan("", SENT), Err(PlanError::Empty));
    assert_eq!(
        parse_plan(r#"{"kind":"answer","speech":"oops""#, SENT),
        Err(PlanError::Unparseable)
    );
    assert_eq!(
        parse_plan(r#"{"speech":"hi"}"#, SENT),
        Err(PlanError::Unparseable)
    );
}

#[test]
fn parse_plan_still_rejects_job_kind_without_a_policy() {
    let raw = job_ok("vox", "fix it");
    assert_eq!(parse_plan(&raw, SENT), Err(PlanError::Unparseable));
}

#[test]
fn parse_plan_equals_parse_plan_with_policy_false_for_a_corpus() {
    let corpus: Vec<String> = vec![
        r#"{"kind":"answer","speech":"It is three o'clock."}"#.to_string(),
        r#"{"kind":"paste","paste_text":"Hi Sam","speech":"Done."}"#.to_string(),
        r#"{"kind":"command","command":"open Safari"}"#.to_string(),
        r#"{"kind":"clarify","speech":"Which Sam?"}"#.to_string(),
        r#"{"kind":"end","speech":"Goodbye."}"#.to_string(),
        job_ok("vox", "fix it"),
        "The capital of France is Paris.".to_string(),
        String::new(),
        "   \n\t ".to_string(),
        r#"{"kind":"answer","speech":"oops""#.to_string(),
        r#"{"speech":"hi"}"#.to_string(),
    ];
    for raw in &corpus {
        assert_eq!(
            parse_plan(raw, SENT),
            parse_plan_with_policy(raw, SENT, false),
            "parse_plan and parse_plan_with_policy(false) differ for {raw:?}"
        );
    }
}

// ---------------- prompt addendum ----------------

#[test]
fn job_plan_addendum_names_the_job_kind_and_its_fields() {
    let text = JOB_PLAN_ADDENDUM;
    assert!(!text.is_empty(), "addendum must not be empty");
    assert!(text.contains("job"), "must name the job kind");
    assert!(text.contains("project"), "must name the project field");
    assert!(text.contains("task"), "must name the task field");
}

#[test]
fn job_plan_addendum_states_the_usage_and_safety_rules() {
    let lower = JOB_PLAN_ADDENDUM.to_lowercase();
    assert!(
        lower.contains("explicit"),
        "must require an explicit request"
    );
    assert!(lower.contains("300"), "must state the 300-character cap");
    assert!(lower.contains("file"), "must forbid text copied from files");
    assert!(
        lower.contains("tool result"),
        "must forbid text from tool results"
    );
    assert!(lower.contains("note"), "must forbid text from notes");
    assert!(lower.contains("secret"), "must forbid secrets");
    assert!(
        lower.contains("one job at a time"),
        "must state one job at a time"
    );
}

#[test]
fn job_plan_addendum_is_ascii_only() {
    assert!(JOB_PLAN_ADDENDUM.is_ascii(), "addendum must be ASCII");
}
