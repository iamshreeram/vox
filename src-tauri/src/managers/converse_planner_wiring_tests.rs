//! Stage 4 production wiring (DESIGN-PR4-PLANNER.md Revision 4, A3-R3, A2): a launch prepared
//! under the settings gate records the even generation it was read under (`read_stable`), and
//! admission refuses a run whose epoch no longer matches the gate. The same guard covers the
//! single-agent hand-off and every plan step. A SettingsChanged refusal reaches the executor as
//! the fixed plan line. Tests are the spec; workers must not edit this file.
//! Wired from managers/mod.rs.

use crate::managers::acp_agent::{AcpAgentConfig, AcpAgentError, AcpAgentManager, PromptOptions};
use crate::managers::acp_escalation::user_message_for;
use crate::managers::agent_settings_gate::{read_stable, AgentSettingsGate};
use crate::managers::converse_agent::PreparedAgentLaunch;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Reads like a launch preparation does (`read_stable`), with a fixed payload. Returns the epoch.
fn prepared_epoch(gate: &AgentSettingsGate) -> u64 {
    match read_stable(gate, || Ok::<_, ()>(())) {
        Ok((epoch, ())) => epoch,
        Err(_) => panic!("a stable gate must prepare"),
    }
}

/// A config whose program does not exist: a run that passes admission fails at spawn, and one that
/// is refused never reaches it.
fn unreachable_cfg() -> AcpAgentConfig {
    AcpAgentConfig {
        command: PathBuf::from("/nonexistent/vox-wiring-test-agent"),
        args: Vec::new(),
        cwd: std::env::temp_dir(),
        turn_timeout: Duration::from_secs(5),
        handshake_timeout: Duration::from_secs(5),
        cancel_grace: Duration::from_secs(1),
    }
}

fn options(epoch: u64) -> PromptOptions {
    PromptOptions {
        reuse_session: false,
        expected_epoch: Some(epoch),
    }
}

#[test]
fn prepared_epoch_equals_the_gate_generation() {
    let gate = AgentSettingsGate::new();
    assert_eq!(prepared_epoch(&gate), gate.generation());
    gate.with_update(|| {});
    assert_eq!(prepared_epoch(&gate), gate.generation());
    assert_eq!(gate.generation() % 2, 0, "a prepared epoch is even");
}

#[test]
fn a_launch_prepared_during_an_update_is_not_prepared() {
    let gate = AgentSettingsGate::new();
    let result = gate.with_update(|| read_stable(&gate, || Ok::<_, ()>(())).is_err());
    assert!(result, "an update in progress refuses the preparation");
}

#[test]
fn a_setter_between_prepare_and_start_refuses_the_run() {
    let gate = Arc::new(AgentSettingsGate::new());
    let epoch = prepared_epoch(&gate);
    gate.with_update(|| {});
    let manager = AcpAgentManager::new_with_gate(gate.clone());
    let handle = manager.new_run_handle();
    let refused = manager.prompt_with_handle(&unreachable_cfg(), "hi", options(epoch), &handle);
    assert!(
        matches!(refused, Err(AcpAgentError::SettingsChanged)),
        "a stale epoch is refused before any spawn"
    );
}

#[test]
fn no_change_between_prepare_and_start_lets_the_run_through_admission() {
    let gate = Arc::new(AgentSettingsGate::new());
    let epoch = prepared_epoch(&gate);
    let manager = AcpAgentManager::new_with_gate(gate.clone());
    let handle = manager.new_run_handle();
    let outcome = manager.prompt_with_handle(&unreachable_cfg(), "hi", options(epoch), &handle);
    assert!(
        !matches!(outcome, Err(AcpAgentError::SettingsChanged)),
        "admission passed for an unchanged epoch, got {outcome:?}"
    );
}

#[test]
fn a_single_agent_hand_off_is_protected_the_same_way() {
    // The single agent launch is prepared through the same guarded read as a plan step, so its
    // epoch is refused after a settings change exactly like a step's.
    let gate = Arc::new(AgentSettingsGate::new());
    let launch = PreparedAgentLaunch::new(
        "list files".to_string(),
        None,
        "/nonexistent/vox-wiring-test-agent".to_string(),
        Vec::new(),
        "/tmp".to_string(),
        Duration::from_secs(5),
    )
    .with_epoch(prepared_epoch(&gate));
    gate.with_update(|| {});
    let manager = AcpAgentManager::new_with_gate(gate.clone());
    let handle = manager.new_run_handle();
    let outcome = manager.prompt_with_handle(
        &unreachable_cfg(),
        &launch.prompt_text(),
        options(launch.epoch),
        &handle,
    );
    assert!(matches!(outcome, Err(AcpAgentError::SettingsChanged)));
}

#[test]
fn the_worker_reports_a_settings_refusal_with_the_fixed_failure_text() {
    // The worker sends `Failed` with this text; the executor maps it to the fixed plan line
    // (`converse_planner_loop_tests`, PLAN_SETTINGS_CHANGED).
    let worker_text = user_message_for(&AcpAgentError::SettingsChanged);
    assert_eq!(worker_text.as_deref(), Some("The agent settings changed."));
}

#[test]
fn process_gate_is_one_shared_gate() {
    let first = crate::managers::agent_settings_gate::process_gate();
    let second = crate::managers::agent_settings_gate::process_gate();
    assert!(Arc::ptr_eq(&first, &second), "one process-wide gate");
}

// ---------------- step role text (DESIGN 2.3) ----------------

#[test]
fn step_role_constants_are_short_and_plain() {
    use crate::managers::converse_agent::{
        STEP_ROLE_BROWSER, STEP_ROLE_CODE, STEP_ROLE_FILES, STEP_ROLE_RESEARCH,
    };
    for text in [
        STEP_ROLE_RESEARCH,
        STEP_ROLE_BROWSER,
        STEP_ROLE_CODE,
        STEP_ROLE_FILES,
    ] {
        assert!(!text.trim().is_empty(), "a step role is never empty");
        assert!(
            text.chars().count() <= 300,
            "a step role is at most 300 chars"
        );
        assert!(
            !text.contains(['{', '}', '<', '>', '[', ']']),
            "no placeholders or markup in a step role"
        );
    }
}

#[test]
fn only_the_typed_steps_have_a_role_text() {
    use crate::managers::converse_agent::step_role_text;
    use crate::managers::converse_plan::StepKind;
    assert!(step_role_text(StepKind::General).is_none());
    assert!(step_role_text(StepKind::Research).is_some());
    assert!(step_role_text(StepKind::Browser).is_some());
    assert!(step_role_text(StepKind::Code).is_some());
    assert!(step_role_text(StepKind::Files).is_some());
}

#[test]
fn a_typed_step_prompt_carries_its_role_before_the_request() {
    use crate::managers::converse_agent::{AgentSource, STEP_ROLE_RESEARCH};
    use crate::managers::converse_plan::StepKind;
    let step = PreparedAgentLaunch::new(
        "find the newest release".to_string(),
        None,
        "agent".to_string(),
        Vec::new(),
        "/ws".to_string(),
        Duration::from_secs(60),
    )
    .with_step(StepKind::Research, AgentSource::Main, true);
    let prompt = step.wire_prompt();
    assert_eq!(
        prompt,
        format!("Step role: {STEP_ROLE_RESEARCH}\n\nfind the newest release")
    );
}

#[test]
fn a_single_agent_prompt_has_no_step_role() {
    let single = PreparedAgentLaunch::new(
        "list files".to_string(),
        None,
        "agent".to_string(),
        Vec::new(),
        "/ws".to_string(),
        Duration::from_secs(60),
    );
    assert_eq!(single.prompt_text(), "list files");
}

// ---------------- typed single agent requests (validator round 1, F2) ----------------

/// The stored defaults with the given top-level keys replaced (serde, like the settings tests).
fn settings_with(patch: serde_json::Value) -> crate::settings::AppSettings {
    let mut value =
        serde_json::to_value(crate::settings::get_default_settings()).expect("settings serialize");
    let map = value.as_object_mut().expect("object");
    for (key, val) in patch.as_object().expect("patch object") {
        map.insert(key.clone(), val.clone());
    }
    serde_json::from_value(value).expect("settings deserialize")
}

/// A configured main agent (absolute command, one plain arg) and one research profile.
fn typed_settings(research: serde_json::Value) -> crate::settings::AppSettings {
    settings_with(serde_json::json!({
        "acp_agent_enabled": true,
        "acp_agent_command": "/usr/bin/main-agent",
        "acp_agent_args": "--main",
        "acp_agent_turn_timeout_secs": 120,
        "acp_step_agents": {"research": research},
    }))
}

fn resolve_or_panic(
    settings: &crate::settings::AppSettings,
    kind: crate::managers::converse_plan::StepKind,
) -> crate::managers::converse_agent::ResolvedAgent {
    match crate::managers::converse_agent::resolve_step_agent(settings, kind) {
        Ok(agent) => agent,
        Err(e) => panic!("{kind:?} should resolve, got {e:?}"),
    }
}

#[test]
fn a_typed_single_request_resolves_the_specialist_and_carries_its_step_role() {
    use crate::managers::converse_agent::{AgentSource, PreparedAgentLaunch, STEP_ROLE_RESEARCH};
    use crate::managers::converse_plan::StepKind;
    let settings = typed_settings(serde_json::json!({
        "command": "/opt/research-agent", "args": "--fast"
    }));
    let resolved = resolve_or_panic(&settings, StepKind::Research);
    let launch = PreparedAgentLaunch::for_step(
        resolved,
        "/ws".to_string(),
        StepKind::Research,
        "find the newest release",
        None,
        false,
    );
    assert_eq!(launch.command, "/opt/research-agent");
    assert_eq!(launch.args, vec!["--fast".to_string()]);
    assert_eq!(launch.step_kind, StepKind::Research);
    assert!(matches!(launch.source, AgentSource::Specialist));
    assert!(!launch.in_plan, "a single request is not a plan step");
    assert_eq!(
        launch.wire_prompt(),
        format!("Step role: {STEP_ROLE_RESEARCH}\n\nfind the newest release")
    );
}

#[test]
fn an_absent_or_general_type_uses_the_main_agent_with_no_step_role() {
    use crate::managers::converse_agent::{AgentSource, PreparedAgentLaunch};
    use crate::managers::converse_plan::StepKind;
    let settings =
        typed_settings(serde_json::json!({"command": "/opt/research-agent", "args": ""}));
    let resolved = resolve_or_panic(&settings, StepKind::General);
    let timeout = resolved.turn_timeout;
    let args = resolved.args.clone();
    let launch = PreparedAgentLaunch::for_step(
        resolved,
        "/ws".to_string(),
        StepKind::General,
        "list files",
        None,
        false,
    );
    assert!(matches!(launch.source, AgentSource::Main));
    assert_eq!(launch.command, "/usr/bin/main-agent");
    assert_eq!(launch.wire_prompt(), "list files", "no step role");
    let plain = PreparedAgentLaunch::new(
        "list files".to_string(),
        None,
        "/usr/bin/main-agent".to_string(),
        args,
        "/ws".to_string(),
        timeout,
    );
    assert_eq!(
        launch.fingerprint, plain.fingerprint,
        "a general single request keeps today's launch fingerprint"
    );
}

#[test]
fn a_blank_profile_falls_back_to_the_main_agent_for_a_typed_single_request() {
    use crate::managers::converse_agent::{AgentSource, PreparedAgentLaunch};
    use crate::managers::converse_plan::StepKind;
    let settings = typed_settings(serde_json::json!({"command": "   ", "args": "--x"}));
    let resolved = resolve_or_panic(&settings, StepKind::Research);
    let launch = PreparedAgentLaunch::for_step(
        resolved,
        "/ws".to_string(),
        StepKind::Research,
        "find it",
        None,
        false,
    );
    assert!(matches!(launch.source, AgentSource::Main));
    assert_eq!(launch.command, "/usr/bin/main-agent");
    assert_eq!(launch.args, vec!["--main".to_string()]);
}

#[test]
fn a_single_request_and_a_plan_step_of_the_same_kind_have_different_fingerprints() {
    use crate::managers::converse_agent::PreparedAgentLaunch;
    use crate::managers::converse_plan::StepKind;
    let settings =
        typed_settings(serde_json::json!({"command": "/opt/research-agent", "args": ""}));
    let as_single = PreparedAgentLaunch::for_step(
        resolve_or_panic(&settings, StepKind::Research),
        "/ws".to_string(),
        StepKind::Research,
        "find it",
        None,
        false,
    );
    let as_step = PreparedAgentLaunch::for_step(
        resolve_or_panic(&settings, StepKind::Research),
        "/ws".to_string(),
        StepKind::Research,
        "find it",
        None,
        true,
    );
    assert!(!as_single.in_plan && as_step.in_plan);
    assert_ne!(
        as_single.fingerprint, as_step.fingerprint,
        "plan membership is part of the fingerprint, so neither kind is valid as the other"
    );
}
