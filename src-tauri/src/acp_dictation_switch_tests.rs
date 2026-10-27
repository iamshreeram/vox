//! DICTATION SWITCH SPEC (DESIGN-A Rev 1, written by ram-ai; workers must not edit this file).
//! Option+Space dictation must always paste unless the user turns on "Send dictation to the agent".
//! Turning the conversation agent on must never change what dictation does.
use crate::settings::{dictation_agent_enabled, get_default_settings, AppSettings};

fn src(rel: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// Body of `fn name` (brace matched), so tests never depend on order or length.
fn fn_body(source: &str, name: &str) -> String {
    let start = source
        .find(&format!("fn {name}"))
        .unwrap_or_else(|| panic!("fn {name} must exist"));
    let open = start + source[start..].find('{').expect("body brace");
    let mut depth = 0usize;
    for (i, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return source[open..open + i + 1].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced braces in fn {name}");
}

#[test]
fn a_fresh_install_pastes_dictation_even_when_the_agent_is_on() {
    let s = get_default_settings();
    assert!(
        !s.acp_dictation_enabled,
        "dictation hand-off defaults to off"
    );
    let mut with_agent = s.clone();
    with_agent.acp_agent_enabled = true;
    assert!(
        !dictation_agent_enabled(&with_agent),
        "enabling the agent alone must not capture dictation"
    );
}

#[test]
fn an_old_store_without_the_key_keeps_dictation_pasting() {
    let mut value = serde_json::to_value(get_default_settings()).expect("serialise");
    let object = value.as_object_mut().expect("object");
    object.remove("acp_dictation_enabled");
    object.insert("acp_agent_enabled".into(), serde_json::Value::Bool(true));
    let loaded: AppSettings = serde_json::from_value(value).expect("old store still loads");
    assert!(loaded.acp_agent_enabled);
    assert!(!loaded.acp_dictation_enabled);
    assert!(!dictation_agent_enabled(&loaded));
}

#[test]
fn dictation_goes_to_the_agent_only_when_both_switches_are_on() {
    for agent in [false, true] {
        for dictation in [false, true] {
            let mut s = get_default_settings();
            s.acp_agent_enabled = agent;
            s.acp_dictation_enabled = dictation;
            assert_eq!(
                dictation_agent_enabled(&s),
                agent && dictation,
                "agent={agent} dictation={dictation}"
            );
        }
    }
}

#[test]
fn the_dictation_path_uses_the_combined_switch_not_the_raw_agent_flag() {
    // The whole file: actions.rs has test-only imports near the top, and the tests reference no
    // `hook_settings`, so scanning everything is safe and not order dependent.
    let code = src("src/actions.rs");
    assert!(
        code.contains("dictation_agent_enabled("),
        "actions.rs must decide dictation hand-off with settings::dictation_agent_enabled"
    );
    assert!(
        !code.contains("hook_settings.acp_agent_enabled"),
        "actions.rs must not read the raw acp_agent_enabled flag for dictation"
    );
}

#[test]
fn the_switch_has_its_own_command_registered_for_the_main_window() {
    let lib = src("src/lib.rs");
    assert!(
        lib.contains("change_acp_dictation_enabled_setting"),
        "the command must be registered in collect_commands!"
    );
    let shortcut = src("src/shortcut/mod.rs");
    assert!(shortcut.contains("fn change_acp_dictation_enabled_setting"));
}

#[test]
fn the_dictation_switch_never_changes_the_agent_lifecycle() {
    // Flipping it must not start or retire the agent process; only the agent switch does.
    let shortcut = src("src/shortcut/mod.rs");
    let body = fn_body(&shortcut, "change_acp_dictation_enabled_setting");
    assert!(
        !body.contains("shutdown_managed_acp_agent") && !body.contains("spawn_acp"),
        "the dictation switch must not touch the agent process"
    );
    assert!(
        body.contains("write_settings_gated"),
        "the switch is written through the same gate as the agent switch so writes serialise"
    );
}

#[test]
fn a_stale_whole_object_write_cannot_flip_the_dictation_switch() {
    let mut committed = get_default_settings();
    committed.acp_agent_enabled = true;
    committed.acp_dictation_enabled = false;
    let mut stale = committed.clone();
    stale.acp_dictation_enabled = true; // an old snapshot that still said "on"
    let merged = crate::settings::preserve_launch_fields(stale, &committed);
    assert!(
        !merged.acp_dictation_enabled,
        "the committed dictation switch must win over a stale incoming snapshot"
    );

    let mut committed_on = committed.clone();
    committed_on.acp_dictation_enabled = true;
    let mut stale_off = committed_on.clone();
    stale_off.acp_dictation_enabled = false;
    let merged = crate::settings::preserve_launch_fields(stale_off, &committed_on);
    assert!(merged.acp_dictation_enabled, "and the reverse");
}
