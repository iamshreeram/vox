//! TYPING SPEC (DESIGN-TYPE Rev 2, written by ram-ai; workers must not edit this file).
//! Child module of `converse_confirm`. Contract: `SessionConfig.confirm_listen_timeout`
//! (default 8 s, the existing behaviour) and `pub fn confirm_config_for(&SessionConfig) -> ConfirmConfig`
//! used by BOTH ordinary and forced confirmation paths.
use super::*;
use crate::managers::converse::SessionConfig;
use std::time::Duration;

#[test]
fn voice_sessions_keep_the_eight_second_confirmation_window() {
    let cfg = SessionConfig::default();
    assert_eq!(cfg.confirm_listen_timeout, Duration::from_secs(8));
    let cc = confirm_config_for(&cfg);
    assert_eq!(cc.listen_timeout, Duration::from_secs(8));
    assert_eq!(cc.listen_timeout, ConfirmConfig::default().listen_timeout);
    assert_eq!(cc.quiet_wait, ConfirmConfig::default().quiet_wait);
}

#[test]
fn a_text_session_config_gets_sixty_seconds_and_no_post_speech_guard() {
    let cfg = SessionConfig {
        confirm_listen_timeout: Duration::from_secs(60),
        post_speech_guard: Duration::ZERO,
        ..SessionConfig::default()
    };
    let cc = confirm_config_for(&cfg);
    assert_eq!(cc.listen_timeout, Duration::from_secs(60));
    assert_eq!(cc.post_speech_guard, Duration::ZERO);
}

#[test]
fn production_code_no_longer_builds_a_default_confirm_config_directly() {
    for rel in [
        "src/managers/converse.rs",
        "src/managers/converse_agent_loop.rs",
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        let src = std::fs::read_to_string(&path).expect("read source");
        // production part only: everything before the first #[cfg(test)] module
        let prod = src.split("#[cfg(test)]").next().unwrap_or(&src);
        assert!(
            !prod.contains("ConfirmConfig::default()"),
            "{rel}: use confirm_config_for(cfg) so text sessions get their window"
        );
    }
}
