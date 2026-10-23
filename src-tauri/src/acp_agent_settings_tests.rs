//! Contract tests for the ACP agent settings (DESIGN.md rev 3.1, section 7).
//! Registered in lib.rs by step S2 (`#[cfg(test)] mod acp_agent_settings_tests;`).
//! Workers must NOT edit this file.

use crate::settings::*;

#[test]
fn defaults_are_off_unset_and_120_seconds() {
    let s = get_default_settings();
    assert!(!s.acp_agent_enabled);
    assert_eq!(s.acp_agent_command, None);
    assert_eq!(s.acp_agent_args, "");
    assert_eq!(
        s.acp_agent_turn_timeout_secs,
        ACP_AGENT_TURN_TIMEOUT_DEFAULT_SECS
    );
    assert_eq!(ACP_AGENT_TURN_TIMEOUT_DEFAULT_SECS, 120);
}

#[test]
fn turn_timeout_bounds() {
    assert_eq!(ACP_AGENT_TURN_TIMEOUT_MIN_SECS, 10);
    assert_eq!(ACP_AGENT_TURN_TIMEOUT_MAX_SECS, 900);
    assert_eq!(validate_acp_agent_turn_timeout_secs(10), Ok(10));
    assert_eq!(validate_acp_agent_turn_timeout_secs(120), Ok(120));
    assert_eq!(validate_acp_agent_turn_timeout_secs(900), Ok(900));
    assert!(validate_acp_agent_turn_timeout_secs(9).is_err());
    assert!(validate_acp_agent_turn_timeout_secs(901).is_err());
    assert!(validate_acp_agent_turn_timeout_secs(0).is_err());
    assert!(validate_acp_agent_turn_timeout_secs(u64::MAX).is_err());
}

#[test]
fn command_blank_means_unset() {
    assert_eq!(normalize_acp_agent_command(""), Ok(None));
    assert_eq!(normalize_acp_agent_command("   \t "), Ok(None));
}

#[test]
fn command_is_trimmed_and_must_be_absolute() {
    assert_eq!(
        normalize_acp_agent_command("  /usr/local/bin/my-agent  "),
        Ok(Some("/usr/local/bin/my-agent".to_string()))
    );
    assert!(normalize_acp_agent_command("my-agent").is_err());
    assert!(normalize_acp_agent_command("relative/path/agent").is_err());
    assert!(normalize_acp_agent_command("./agent").is_err());
    assert!(normalize_acp_agent_command("~/bin/agent").is_err());
}

#[test]
fn command_length_and_character_limits() {
    let max = format!("/{}", "a".repeat(1023));
    assert_eq!(max.chars().count(), 1024);
    assert_eq!(normalize_acp_agent_command(&max), Ok(Some(max.clone())));
    let too_long = format!("/{}", "a".repeat(1024));
    assert!(normalize_acp_agent_command(&too_long).is_err());
    assert!(normalize_acp_agent_command("/bin/agent\0x").is_err());
    assert!(normalize_acp_agent_command("/bin/agent\nrm -rf").is_err());
    assert!(normalize_acp_agent_command("/bin/agent\u{7}").is_err());
}

#[test]
fn args_split_on_whitespace_only_with_no_shell_parsing() {
    assert_eq!(validate_acp_agent_args(""), Ok(vec![]));
    assert_eq!(validate_acp_agent_args("   "), Ok(vec![]));
    assert_eq!(
        validate_acp_agent_args("--acp"),
        Ok(vec!["--acp".to_string()])
    );
    assert_eq!(
        validate_acp_agent_args("  --a   --b\t--c  "),
        Ok(vec![
            "--a".to_string(),
            "--b".to_string(),
            "--c".to_string()
        ])
    );
    // quotes are NOT interpreted: they stay part of the argument
    assert_eq!(
        validate_acp_agent_args("--name \"two words\""),
        Ok(vec![
            "--name".to_string(),
            "\"two".to_string(),
            "words\"".to_string()
        ])
    );
    // shell metacharacters are just characters
    assert_eq!(
        validate_acp_agent_args("$(whoami);ls"),
        Ok(vec!["$(whoami);ls".to_string()])
    );
}

#[test]
fn args_count_and_length_limits() {
    let eight = "a b c d e f g h";
    assert_eq!(validate_acp_agent_args(eight).map(|v| v.len()), Ok(8));
    assert!(validate_acp_agent_args("a b c d e f g h i").is_err());
    let longest = "x".repeat(256);
    assert_eq!(validate_acp_agent_args(&longest), Ok(vec![longest.clone()]));
    assert!(validate_acp_agent_args(&"x".repeat(257)).is_err());
}

#[test]
fn args_reject_control_characters_other_than_space_and_tab() {
    assert!(validate_acp_agent_args("a\nb").is_err());
    assert!(validate_acp_agent_args("a\rb").is_err());
    assert!(validate_acp_agent_args("a\0b").is_err());
    assert!(validate_acp_agent_args("\u{7}").is_err());
    assert!(validate_acp_agent_args("a\u{1b}[31m").is_err());
}

#[test]
fn an_old_settings_file_with_the_removed_bridge_timeout_still_loads() {
    let value = serde_json::json!({
        "agent_bridge_timeout_secs": 99,
        "agent_bridge_enabled": true
    });
    let s: AppSettings = serde_json::from_value(value).expect("old files must keep loading");
    assert!(
        s.agent_bridge_enabled,
        "kept (background jobs) setting still reads"
    );
    assert!(!s.acp_agent_enabled, "new settings take their defaults");
    assert_eq!(s.acp_agent_turn_timeout_secs, 120);
}

#[test]
fn background_job_settings_are_untouched() {
    let s = get_default_settings();
    assert!(!s.agent_bridge_enabled);
    assert_eq!(s.agent_bridge_binary_path, None);
    assert_eq!(s.agent_bridge_prompt_flag, "-p");
}
