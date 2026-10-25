//! M4 S6 RED tests for the production wiring of the planner policy (DESIGN Revision 4: D1,
//! D2, D3 PolicySource part, D6). Red by design: `job_parse_policy` and
//! `planner_prompt_for_policy` are `todo!("M4 S6")`, and the new `PolicySource` methods
//! (`ask_allowed`, `allowed_agents`, `agent_flag`) use the trait defaults until the
//! implementer overrides them. Do not edit these tests to make code pass.
//!
//! Tests that need jobs to be allowed use a real executable fake CLI and are `#[cfg(unix)]`.

use super::converse_platform_jobs_tests::write_fake_cli;
use super::*;
use crate::managers::converse_confirm::PolicySource;
use crate::managers::converse_plan::{JobMode, JobParsePolicy, JobSelection, PlanInput};
use crate::settings::{clean_agent_names, get_default_settings, AppSettings, MAX_AGENT_NAMES};
use std::path::Path;
use std::sync::Mutex;

// ---------------------------------------------------------------- helpers

/// Default settings with jobs fully enabled over `bin` (a valid absolute binary on unix).
#[cfg(unix)]
fn jobs_settings(bin: &Path) -> AppSettings {
    let mut s = get_default_settings();
    s.agent_jobs_enabled = true;
    s.agent_bridge_enabled = true;
    s.agent_bridge_binary_path = Some(bin.to_string_lossy().into_owned());
    s
}

/// A live settings getter over a shared value, so tests can flip switches between calls.
fn live_getter(shared: &Arc<Mutex<AppSettings>>) -> Arc<dyn Fn() -> AppSettings + Send + Sync> {
    let s = Arc::clone(shared);
    Arc::new(move || s.lock().expect("settings lock").clone())
}

/// A `SettingsPolicy` over a fixed settings value.
fn policy_of(settings: AppSettings) -> SettingsPolicy {
    SettingsPolicy::new(Arc::new(move || settings.clone()))
}

/// A planner input with no history, so the prompt depends only on the policy.
fn plain_input() -> PlanInput<'static> {
    PlanInput {
        utterance: "what does the project do",
        history: &[],
        last_draft: None,
        style: "plain",
        vault_context: None,
        lesson_paused: false,
        role: None,
        roles_enabled: false,
        agent_enabled: false,
        planner_enabled: false,
    }
}

// ---------------------------------------------------------------- job_parse_policy

#[test]
fn parse_policy_with_jobs_disabled_exposes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_fake_cli(dir.path(), "exit 0");
    let mut s = jobs_settings(&bin);
    s.agent_jobs_enabled = false;
    s.agent_jobs_ask_enabled = true;
    s.agent_jobs_agents = vec!["alphaagent".to_string()];
    let p = job_parse_policy(&s);
    assert!(!p.jobs_allowed, "jobs disabled => jobs refused");
    assert!(!p.ask_allowed, "jobs disabled => ask refused");
    assert!(
        p.agents.is_empty(),
        "jobs disabled must never leak the allowlist"
    );
}

#[test]
fn parse_policy_with_bridge_disabled_exposes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_fake_cli(dir.path(), "exit 0");
    let mut s = jobs_settings(&bin);
    s.agent_bridge_enabled = false;
    s.agent_jobs_ask_enabled = true;
    s.agent_jobs_agents = vec!["alphaagent".to_string()];
    let p = job_parse_policy(&s);
    assert!(!p.jobs_allowed);
    assert!(!p.ask_allowed);
    assert!(p.agents.is_empty());
}

#[test]
fn parse_policy_without_a_valid_binary_exposes_nothing() {
    let mut s = get_default_settings();
    s.agent_jobs_enabled = true;
    s.agent_bridge_enabled = true;
    s.agent_jobs_ask_enabled = true;
    s.agent_jobs_agents = vec!["alphaagent".to_string()];
    s.agent_bridge_binary_path = Some("/nonexistent/vox-m4-s6/fake-cli".to_string());
    let p = job_parse_policy(&s);
    assert!(!p.jobs_allowed, "a missing binary => jobs refused");
    assert!(!p.ask_allowed);
    assert!(p.agents.is_empty());
}

#[cfg(unix)]
#[test]
fn parse_policy_jobs_allowed_with_ask_off_lists_the_cleaned_allowlist() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_fake_cli(dir.path(), "exit 0");
    let mut s = jobs_settings(&bin);
    s.agent_jobs_ask_enabled = false;
    s.agent_jobs_agents = vec!["alphaagent".to_string(), "betaagent".to_string()];
    let p = job_parse_policy(&s);
    assert!(p.jobs_allowed);
    assert!(!p.ask_allowed, "ask off => ask refused");
    assert_eq!(
        p.agents,
        vec!["alphaagent".to_string(), "betaagent".to_string()]
    );
}

#[cfg(unix)]
#[test]
fn parse_policy_jobs_allowed_with_ask_on_allows_ask() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_fake_cli(dir.path(), "exit 0");
    let mut s = jobs_settings(&bin);
    s.agent_jobs_ask_enabled = true;
    let p = job_parse_policy(&s);
    assert!(p.jobs_allowed);
    assert!(p.ask_allowed, "jobs allowed + ask on => ask allowed");
}

#[cfg(unix)]
#[test]
fn parse_policy_ask_on_without_jobs_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_fake_cli(dir.path(), "exit 0");
    let mut s = jobs_settings(&bin);
    s.agent_jobs_enabled = false;
    s.agent_jobs_ask_enabled = true;
    let p = job_parse_policy(&s);
    assert!(!p.jobs_allowed);
    assert!(!p.ask_allowed, "ask on but jobs not allowed => ask refused");
}

#[cfg(unix)]
#[test]
fn parse_policy_cleans_invalid_and_duplicate_agent_names() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_fake_cli(dir.path(), "exit 0");
    let mut s = jobs_settings(&bin);
    s.agent_jobs_agents = vec![
        "good".to_string(),
        "bad name".to_string(),
        "GOOD".to_string(),
        "ok-1".to_string(),
        "../escape".to_string(),
        String::new(),
    ];
    let p = job_parse_policy(&s);
    assert_eq!(
        p.agents,
        vec!["good".to_string(), "ok-1".to_string()],
        "invalid names dropped, case-insensitive duplicate dropped, first spelling kept"
    );
}

#[cfg(unix)]
#[test]
fn parse_policy_caps_the_allowlist_at_twenty_names() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_fake_cli(dir.path(), "exit 0");
    let mut s = jobs_settings(&bin);
    s.agent_jobs_agents = (0..25).map(|i| format!("agent{i}")).collect();
    let p = job_parse_policy(&s);
    assert_eq!(MAX_AGENT_NAMES, 20, "the cap under test is 20");
    assert_eq!(p.agents.len(), 20, "at most 20 agents reach the prompt");
    assert_eq!(p.agents, s.agent_jobs_agents[..20].to_vec());
}

#[cfg(unix)]
#[test]
fn parse_policy_empty_allowlist_gives_no_agents() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_fake_cli(dir.path(), "exit 0");
    let mut s = jobs_settings(&bin);
    s.agent_jobs_agents = Vec::new();
    s.agent_jobs_ask_enabled = true;
    let p = job_parse_policy(&s);
    assert!(p.jobs_allowed);
    assert!(
        p.agents.is_empty(),
        "an empty allowlist means no agent selection"
    );
    assert!(p.ask_allowed);
}

#[cfg(unix)]
#[test]
fn parse_policy_agents_equal_clean_agent_names_of_the_stored_list() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_fake_cli(dir.path(), "exit 0");
    let mut s = jobs_settings(&bin);
    s.agent_jobs_agents = vec!["x1".to_string(), "bad/one".to_string(), "x1".to_string()];
    let p = job_parse_policy(&s);
    assert_eq!(p.agents, clean_agent_names(s.agent_jobs_agents.clone()));
}

// ---------------------------------------------------------------- planner_prompt_for_policy

#[test]
fn prompt_with_jobs_refused_is_byte_identical_to_the_plain_builder() {
    let policy = JobParsePolicy {
        jobs_allowed: false,
        ask_allowed: false,
        agents: vec!["alphaagent".to_string()],
        role_active: false,
    };
    let input = plain_input();
    assert_eq!(
        planner_prompt_for_policy(&input, &policy),
        build_plan_prompt(&input),
        "jobs off must be byte-identical to build_plan_prompt"
    );
}

#[test]
fn prompt_with_jobs_allowed_no_agents_and_ask_off_matches_today_jobs_prompt() {
    let policy = JobParsePolicy {
        jobs_allowed: true,
        ask_allowed: false,
        agents: Vec::new(),
        role_active: false,
    };
    let input = plain_input();
    assert_eq!(
        planner_prompt_for_policy(&input, &policy),
        planner_prompt_for(&input, true),
        "jobs on with no agents and ask off must equal today's job prompt"
    );
}

#[test]
fn prompt_with_agents_and_ask_lists_each_agent_once_and_explains_ask() {
    let policy = JobParsePolicy {
        jobs_allowed: true,
        ask_allowed: true,
        agents: vec!["alphaagent".to_string(), "betaagent".to_string()],
        role_active: false,
    };
    let prompt = planner_prompt_for_policy(&plain_input(), &policy);
    for name in ["alphaagent", "betaagent"] {
        assert_eq!(
            prompt.system.matches(name).count(),
            1,
            "agent {name} must appear exactly once in the system prompt"
        );
    }
    assert!(
        prompt.system.contains("\"ask\""),
        "the ask mode explanation must be in the system prompt"
    );
}

#[test]
fn prompt_combined_is_system_blank_line_user_for_every_policy_shape() {
    let shapes = [
        JobParsePolicy {
            jobs_allowed: false,
            ask_allowed: false,
            agents: Vec::new(),
            role_active: false,
        },
        JobParsePolicy {
            jobs_allowed: true,
            ask_allowed: false,
            agents: Vec::new(),
            role_active: false,
        },
        JobParsePolicy {
            jobs_allowed: true,
            ask_allowed: true,
            agents: vec!["alphaagent".to_string()],
            role_active: false,
        },
    ];
    for policy in &shapes {
        let p = planner_prompt_for_policy(&plain_input(), policy);
        assert_eq!(
            p.combined,
            format!("{}\n\n{}", p.system, p.user),
            "combined must be system + blank line + user"
        );
    }
}

// ---------------------------------------------------------------- SettingsPolicy as PolicySource

#[cfg(unix)]
#[test]
fn settings_policy_ask_allowed_needs_jobs_and_the_ask_switch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_fake_cli(dir.path(), "exit 0");

    let mut both = jobs_settings(&bin);
    both.agent_jobs_ask_enabled = true;
    assert!(
        policy_of(both).ask_allowed(),
        "jobs allowed AND ask on => true"
    );

    let mut ask_only = jobs_settings(&bin);
    ask_only.agent_jobs_enabled = false;
    ask_only.agent_jobs_ask_enabled = true;
    assert!(
        !policy_of(ask_only).ask_allowed(),
        "ask on without jobs => false"
    );

    let mut jobs_only = jobs_settings(&bin);
    jobs_only.agent_jobs_ask_enabled = false;
    assert!(
        !policy_of(jobs_only).ask_allowed(),
        "jobs on with ask off => false"
    );
}

#[test]
fn settings_policy_defaults_refuse_ask_and_agents() {
    let p = policy_of(get_default_settings());
    assert!(!p.ask_allowed());
    assert!(p.allowed_agents().is_empty());
}

#[cfg(unix)]
#[test]
fn settings_policy_allowed_agents_is_the_cleaned_list_only_when_jobs_are_allowed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_fake_cli(dir.path(), "exit 0");
    let mut on = jobs_settings(&bin);
    on.agent_jobs_agents = vec![
        "good".to_string(),
        "bad name".to_string(),
        "GOOD".to_string(),
    ];
    assert_eq!(
        policy_of(on.clone()).allowed_agents(),
        vec!["good".to_string()]
    );

    on.agent_jobs_enabled = false;
    assert!(
        policy_of(on).allowed_agents().is_empty(),
        "jobs not allowed => no agents reach the prompt"
    );
}

#[test]
fn settings_policy_agent_flag_defaults_to_dash_dash_agent() {
    let p = policy_of(get_default_settings());
    assert_eq!(p.agent_flag(), "--agent");
}

#[test]
fn settings_policy_agent_flag_uses_a_valid_stored_flag() {
    let mut s = get_default_settings();
    s.agent_jobs_agent_flag = "--model".to_string();
    assert_eq!(policy_of(s).agent_flag(), "--model");
}

#[test]
fn settings_policy_agent_flag_falls_back_when_invalid_or_equal_to_prompt_flag() {
    let mut invalid = get_default_settings();
    invalid.agent_jobs_agent_flag = "not valid".to_string();
    assert_eq!(policy_of(invalid).agent_flag(), "--agent");

    let mut same = get_default_settings();
    same.agent_bridge_prompt_flag = "-p".to_string();
    same.agent_jobs_agent_flag = "-p".to_string();
    assert_eq!(
        policy_of(same).agent_flag(),
        "--agent",
        "a flag equal to the prompt flag must fall back"
    );
}

#[test]
fn settings_policy_launch_config_for_is_none_when_jobs_are_not_enabled() {
    let p = policy_of(get_default_settings());
    assert!(p.launch_config_for(None).is_none());
    let sel = JobSelection {
        agent: Some("alphaagent".to_string()),
        mode: JobMode::Task,
    };
    assert!(p.launch_config_for(Some(&sel)).is_none());
}

#[cfg(unix)]
#[test]
fn settings_policy_launch_config_for_sets_the_agent_flag_only_for_a_selection() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_fake_cli(dir.path(), "exit 0");
    let mut s = jobs_settings(&bin);
    s.agent_jobs_agent_flag = "--model".to_string();
    let p = policy_of(s);

    let plain = p
        .launch_config_for(None)
        .expect("launch config without a selection");
    assert_eq!(plain.agent_flag, None, "no selection => no agent flag");

    let sel = JobSelection {
        agent: Some("alphaagent".to_string()),
        mode: JobMode::Task,
    };
    let chosen = p
        .launch_config_for(Some(&sel))
        .expect("launch config for a selection");
    assert_eq!(chosen.agent_flag, Some("--model".to_string()));
}

#[cfg(unix)]
#[test]
fn settings_policy_reads_the_getter_on_every_call() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_fake_cli(dir.path(), "exit 0");
    let shared = Arc::new(Mutex::new(jobs_settings(&bin)));
    let policy = SettingsPolicy::new(live_getter(&shared));

    assert!(!policy.ask_allowed(), "ask starts off");
    shared.lock().expect("lock").agent_jobs_ask_enabled = true;
    assert!(
        policy.ask_allowed(),
        "flip seen without rebuilding the policy"
    );

    shared.lock().expect("lock").agent_jobs_agents = vec!["later".to_string()];
    assert_eq!(policy.allowed_agents(), vec!["later".to_string()]);

    shared.lock().expect("lock").agent_jobs_agent_flag = "--flavour".to_string();
    assert_eq!(policy.agent_flag(), "--flavour");
}
