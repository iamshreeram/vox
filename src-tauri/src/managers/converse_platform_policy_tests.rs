//! RED tests for M3 S3c-1 (DESIGN Rev 5: M3-1, R2-3, R3-2, D3, S1b). Phase A: these
//! compile against the todo!() stubs and fail at runtime. Split from converse_platform_jobs_tests.rs
//! to keep each file under 600 lines. Do not edit to make code pass.
#![allow(dead_code)]

use super::converse_platform_jobs_tests::{file_id_of, write_fake_cli, TempDir};
use super::*;
use crate::managers::agent_jobs::JobError;
use crate::managers::converse_confirm::{ActionPolicy, LaunchConfig, PolicySource};
use crate::managers::converse_plan::{
    JobRequest, PlanInput, PlanKind, PlanPrompt, TurnPlan, JOB_PLAN_ADDENDUM,
};
use crate::managers::emotion::Emotion;
use crate::managers::job_announcer::LowPriorityMouth;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------------------
// SettingsPolicy (M3-1, R2-3, R3-2)
// ---------------------------------------------------------------------------------------

fn policy_over(settings: AppSettings) -> SettingsPolicy {
    SettingsPolicy::new(Arc::new(move || settings.clone()))
}

fn base_settings() -> AppSettings {
    crate::settings::get_default_settings()
}

#[test]
fn settings_policy_defaults_block_everything() {
    let p = policy_over(base_settings());
    assert_eq!(
        p.current(),
        ActionPolicy {
            gate_active: false,
            jobs_allowed: false
        }
    );
    assert!(p.launch_config().is_none());
}

#[test]
fn settings_policy_current_is_effective_policy_for_the_same_settings() {
    let mut s = base_settings();
    s.agent_tools_enabled = true;
    s.agent_confirm_actions = true;
    let p = policy_over(s.clone());
    assert_eq!(
        p.current(),
        crate::managers::converse_confirm::effective_policy(&s)
    );
    assert!(p.current().gate_active);
}

#[test]
fn settings_policy_reads_settings_each_call_not_once() {
    use std::sync::Mutex;
    let state = Arc::new(Mutex::new(base_settings()));
    let shared = state.clone();
    let p = SettingsPolicy::new(Arc::new(move || shared.lock().expect("lock").clone()));
    assert!(!p.current().gate_active);
    state.lock().expect("lock").agent_tools_enabled = true;
    assert!(
        p.current().gate_active,
        "a later settings change must be seen"
    );
}

#[test]
fn launch_config_is_none_unless_jobs_and_bridge_are_both_on() {
    let dir = TempDir::new("policy-flags");
    let bin = write_fake_cli(dir.path(), "exit 0");
    let path = bin.to_string_lossy().into_owned();
    for (jobs, bridge, expect) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (true, true, true),
    ] {
        let mut s = base_settings();
        s.agent_jobs_enabled = jobs;
        s.agent_bridge_enabled = bridge;
        s.agent_bridge_binary_path = Some(path.clone());
        let cfg = policy_over(s).launch_config();
        assert_eq!(cfg.is_some(), expect, "jobs={jobs} bridge={bridge}");
    }
}

#[test]
fn launch_config_is_none_for_missing_relative_or_non_executable_binary() {
    let dir = TempDir::new("policy-badbin");
    let plain = dir.path().join("not-exec");
    std::fs::write(&plain, "data").expect("write");
    let cases: Vec<Option<String>> = vec![
        None,
        Some("relative/cli".to_string()),
        Some(dir.path().join("missing").to_string_lossy().into_owned()),
        Some(plain.to_string_lossy().into_owned()),
    ];
    for path in cases {
        let mut s = base_settings();
        s.agent_jobs_enabled = true;
        s.agent_bridge_enabled = true;
        s.agent_bridge_binary_path = path.clone();
        assert!(
            policy_over(s).launch_config().is_none(),
            "binary {path:?} must not configure a launch"
        );
    }
}

#[test]
fn launch_config_carries_the_binary_identity_prompt_flag_and_clamped_timeout() {
    let dir = TempDir::new("policy-launch");
    let bin = write_fake_cli(dir.path(), "exit 0");
    let mut s = base_settings();
    s.agent_jobs_enabled = true;
    s.agent_bridge_enabled = true;
    s.agent_bridge_binary_path = Some(bin.to_string_lossy().into_owned());
    s.agent_bridge_prompt_flag = "--prompt".to_string();
    s.agent_jobs_timeout_minutes = 30;
    let cfg = policy_over(s.clone()).launch_config().expect("configured");
    assert_eq!(cfg.binary, bin);
    assert_eq!(cfg.binary_id, file_id_of(&bin));
    assert_eq!(cfg.prompt_flag, "--prompt");
    assert_eq!(cfg.timeout, Duration::from_secs(30 * 60));

    s.agent_jobs_timeout_minutes = 0;
    let low = policy_over(s.clone()).launch_config().expect("configured");
    assert_eq!(low.timeout, Duration::from_secs(60), "0 clamps to 1 minute");

    s.agent_jobs_timeout_minutes = 9_999;
    let high = policy_over(s).launch_config().expect("configured");
    assert_eq!(
        high.timeout,
        Duration::from_secs(120 * 60),
        "clamps to 120 minutes"
    );
}

#[test]
fn launch_config_timeout_is_read_at_each_call() {
    let dir = TempDir::new("policy-timeout");
    let bin = write_fake_cli(dir.path(), "exit 0");
    let mut s = base_settings();
    s.agent_jobs_enabled = true;
    s.agent_bridge_enabled = true;
    s.agent_bridge_binary_path = Some(bin.to_string_lossy().into_owned());
    s.agent_jobs_timeout_minutes = 5;
    let first = policy_over(s.clone()).launch_config().expect("cfg");
    s.agent_jobs_timeout_minutes = 7;
    let second = policy_over(s).launch_config().expect("cfg");
    assert_eq!(first.timeout, Duration::from_secs(300));
    assert_eq!(second.timeout, Duration::from_secs(420));
}

// ---------------------------------------------------------------------------------------
// Planner prompt and parse policy (D3, M3-5)
// ---------------------------------------------------------------------------------------

fn plan_input<'a>() -> PlanInput<'a> {
    PlanInput {
        utterance: "what time is it",
        history: &[],
        last_draft: None,
        style: "",
        vault_context: None,
        lesson_paused: false,
        role: None,
        roles_enabled: false,
        agent_enabled: false,
        planner_enabled: false,
    }
}

#[test]
fn planner_prompt_with_jobs_off_is_byte_identical_to_the_legacy_prompt() {
    let input = plan_input();
    let legacy = crate::managers::converse_plan::build_plan_prompt(&input);
    let got: PlanPrompt = planner_prompt_for(&input, false);
    assert_eq!(got, legacy, "default settings must not change the prompt");
    assert!(!got.system.contains(JOB_PLAN_ADDENDUM));
}

#[test]
fn planner_prompt_with_jobs_on_appends_the_job_addendum() {
    let input = plan_input();
    let legacy = crate::managers::converse_plan::build_plan_prompt(&input);
    let got = planner_prompt_for(&input, true);
    assert!(
        got.system.contains(JOB_PLAN_ADDENDUM),
        "addendum must be present"
    );
    assert!(
        got.system.starts_with(&legacy.system),
        "addendum is appended, the rest is unchanged"
    );
    assert_ne!(got, legacy);
}

#[test]
fn planner_prompt_jobs_on_keeps_the_user_part_unchanged() {
    let input = plan_input();
    let legacy = crate::managers::converse_plan::build_plan_prompt(&input);
    let got = planner_prompt_for(&input, true);
    assert_eq!(got.user, legacy.user);
}

fn job_json(project: &str, task: &str) -> String {
    format!(
        "{{\"kind\":\"job\",\"speech\":\"Starting.\",\"project\":\"{project}\",\"task\":\"{task}\"}}"
    )
}

struct FixedPolicy {
    jobs: bool,
}

impl PolicySource for FixedPolicy {
    fn current(&self) -> ActionPolicy {
        ActionPolicy {
            gate_active: true,
            jobs_allowed: self.jobs,
        }
    }
    fn launch_config(&self) -> Option<LaunchConfig> {
        None
    }
}

#[test]
fn parse_with_jobs_off_treats_a_job_plan_as_unparseable() {
    let raw = job_json("alpha", "fix the tests");
    let got = parse_with_policy(&raw, "sent", &FixedPolicy { jobs: false });
    assert!(got.is_err(), "job kind must be rejected when jobs are off");
}

#[test]
fn parse_with_jobs_on_accepts_a_job_plan_with_its_request() {
    let raw = job_json("alpha", "fix the tests");
    let plan: TurnPlan = parse_with_policy(&raw, "sent", &FixedPolicy { jobs: true })
        .expect("job plan must parse when jobs are allowed");
    assert_eq!(plan.kind, PlanKind::Job);
    assert_eq!(
        plan.job,
        Some(JobRequest {
            project: "alpha".to_string(),
            task: "fix the tests".to_string(),
            agent: None,
            mode: crate::managers::converse_plan::JobMode::Task,
        })
    );
}

#[test]
fn parse_with_jobs_on_still_rejects_an_invalid_project_name() {
    let raw = job_json("../escape", "fix");
    assert!(parse_with_policy(&raw, "sent", &FixedPolicy { jobs: true }).is_err());
}

#[test]
fn parse_with_jobs_off_for_a_plain_answer_is_unchanged() {
    let raw = "{\"kind\":\"answer\",\"speech\":\"Noon.\"}";
    let plan = parse_with_policy(raw, "sent", &FixedPolicy { jobs: false }).expect("answer");
    assert_eq!(plan.kind, PlanKind::Answer);
    assert_eq!(plan.speech, "Noon.");
    assert!(plan.job.is_none());
    assert_eq!(plan.emotion, Emotion::Neutral);
}

// ---------------------------------------------------------------------------------------
// guard_plan treats Job like Paste/Command (D3, M3-5)
// ---------------------------------------------------------------------------------------

fn job_plan() -> TurnPlan {
    TurnPlan {
        plan: None,
        kind: PlanKind::Job,
        speech: "Starting.".to_string(),
        emotion: Emotion::Neutral,
        paste_text: None,
        command: None,
        role: None,
        job: Some(JobRequest {
            project: "alpha".to_string(),
            task: "fix the tests".to_string(),
            agent: None,
            mode: crate::managers::converse_plan::JobMode::Task,
        }),
        agent: None,
    }
}

fn tainted() -> crate::managers::tool_loop::TaintState {
    let t = crate::managers::tool_loop::TaintState::new();
    t.mark();
    t
}

#[test]
fn guard_refuses_a_job_when_tainted_and_guard_is_on() {
    let out = guard_plan(job_plan(), &tainted(), true);
    assert_eq!(
        out.kind,
        PlanKind::Answer,
        "tainted job must become the refusal"
    );
    assert!(out.job.is_none());
    assert_eq!(
        out.speech,
        crate::managers::tool_loop::REFUSAL_SPEECH,
        "a tainted Job gets the same fixed refusal as Paste/Command"
    );
}

#[test]
fn guard_passes_a_job_through_when_untainted() {
    let out = guard_plan(
        job_plan(),
        &crate::managers::tool_loop::TaintState::new(),
        true,
    );
    assert_eq!(out.kind, PlanKind::Job);
    assert!(out.job.is_some());
}

#[test]
fn guard_passes_a_job_through_when_the_guard_is_off() {
    let out = guard_plan(job_plan(), &tainted(), false);
    assert_eq!(out.kind, PlanKind::Job);
}

#[test]
fn guard_refusal_for_job_is_idempotent() {
    let once = guard_plan(job_plan(), &tainted(), true);
    let twice = guard_plan(once.clone(), &tainted(), true);
    assert_eq!(once, twice);
}

// ---------------------------------------------------------------------------------------
// TtsLowPriorityMouth (M3-7, S1b)
// ---------------------------------------------------------------------------------------

struct CountingLow {
    calls: std::sync::Mutex<Vec<String>>,
    answer: bool,
}

impl crate::managers::job_announcer::LowPriorityMouth for CountingLow {
    fn try_speak_if_idle(&self, text: &str) -> bool {
        self.calls.lock().expect("lock").push(text.to_string());
        self.answer
    }
}

#[test]
fn tts_low_priority_mouth_is_constructible_as_a_trait_object() {
    let tts = Arc::new(crate::managers::tts::TtsManager::new(Box::new(
        crate::managers::tts::NullBackend,
    )));
    let mouth: Arc<dyn LowPriorityMouth> =
        crate::managers::job_announcer::TtsLowPriorityMouth::shared(tts);
    // Empty text is never spoken, so the answer is false without starting a process.
    assert!(!mouth.try_speak_if_idle(""));
}

#[test]
fn tts_low_priority_mouth_refuses_empty_and_whitespace_text() {
    let tts = Arc::new(crate::managers::tts::TtsManager::new(Box::new(
        crate::managers::tts::NullBackend,
    )));
    let mouth = crate::managers::job_announcer::TtsLowPriorityMouth(tts);
    assert!(!mouth.try_speak_if_idle(""));
    assert!(!mouth.try_speak_if_idle("   \n\t "));
}

#[test]
fn fake_low_priority_mouth_records_the_text_and_returns_its_answer() {
    let fake = CountingLow {
        calls: std::sync::Mutex::new(vec![]),
        answer: false,
    };
    assert!(
        !crate::managers::job_announcer::LowPriorityMouth::try_speak_if_idle(&fake, "Started.")
    );
    assert_eq!(fake.calls.lock().expect("lock").as_slice(), ["Started."]);
}
