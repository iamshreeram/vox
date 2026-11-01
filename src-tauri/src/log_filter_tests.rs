//! Spec for the NET (#190) log configuration (DESIGN.md R3-2): the HTTP client targets
//! `reqwest`, `reqwest::connect` and `hyper_util*` must not write proxy or destination
//! detail at Debug/Trace. Pure function `crate::log_filter::target_level_cap`
//! (None = no cap). Authoritative: do not weaken an assertion; report a wrong test.

use crate::log_filter::target_level_cap;
use log::{Level, LevelFilter};

#[test]
fn reqwest_connect_target_is_capped_at_warn() {
    assert_eq!(
        target_level_cap("reqwest::connect"),
        Some(LevelFilter::Warn)
    );
}

#[test]
fn reqwest_root_target_is_capped_at_warn() {
    assert_eq!(target_level_cap("reqwest"), Some(LevelFilter::Warn));
}

#[test]
fn hyper_util_connect_target_is_capped_at_warn() {
    assert_eq!(
        target_level_cap("hyper_util::client::legacy::connect"),
        Some(LevelFilter::Warn)
    );
}

#[test]
fn app_targets_are_not_capped() {
    assert_eq!(
        target_level_cap("vox_app_lib::managers::source_update"),
        None
    );
    assert_eq!(target_level_cap("vox_app_lib"), None);
}

#[test]
fn capped_target_blocks_debug_and_keeps_warn() {
    let cap = target_level_cap("reqwest::connect").expect("reqwest::connect must be capped");
    assert!(
        !(Level::Debug <= cap),
        "Debug records from reqwest::connect must be blocked"
    );
    assert!(
        !(Level::Info <= cap),
        "Info records from reqwest::connect must be blocked"
    );
    assert!(
        Level::Warn <= cap,
        "Warn records from reqwest::connect must still pass"
    );
}

/// Capture-sink verification (R3-2 "every sink"): installing a process-global logger
/// cannot coexist with other tests in this binary that set a logger, and the sink
/// wiring lives in lib.rs (not yet specified as a callable API). Deferred to manual
/// verification with a Debug-level reqwest::connect event in the built app's log file,
/// stdout and webview console. The filter claim itself is covered by
/// `capped_target_blocks_debug_and_keeps_warn` above.
#[test]
#[ignore = "process-global logger cannot be installed alongside other tests; see doc comment"]
fn capture_sink_never_receives_reqwest_connect_debug() {}
