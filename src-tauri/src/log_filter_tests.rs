//! Spec for the NET (#190) log configuration (DESIGN.md R3-2): the HTTP client targets
//! `reqwest`, `reqwest::connect` and `hyper_util*` must not write proxy or destination
//! detail at Debug/Trace. Pure function `crate::log_filter::target_level_cap`
//! (None = no cap). Authoritative: do not weaken an assertion; report a wrong test.
//!
//! The capture tests below run the REAL gate (`apply_log_caps`, the same call lib.rs makes)
//! on a non-global fern dispatch, so no process-global logger is installed.

use crate::log_filter::{apply_log_caps, target_level_cap};
use crate::managers::source_update::{describe_failure, UpdateCheckError};
use log::{Level, LevelFilter, Log, Record};
use std::sync::{Arc, Mutex};
use tauri_plugin_log::fern::{Dispatch, Output};

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

/// A logger built exactly like production's gate, but writing to a vector.
fn capturing_logger() -> (Arc<Mutex<Vec<String>>>, Box<dyn Log>) {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink = captured.clone();
    let dispatch = apply_log_caps(
        Dispatch::new().level(LevelFilter::Debug),
        |d, target, level| d.level_for(target, level),
    )
    .chain(Output::call(move |record| {
        sink.lock()
            .expect("capture lock")
            .push(format!("{} {}", record.target(), record.args()));
    }));
    let (_, logger) = dispatch.into_log();
    (captured, logger)
}

fn emit(logger: &dyn Log, level: Level, target: &str, msg: &str) {
    logger.log(
        &Record::builder()
            .level(level)
            .target(target)
            .args(format_args!("{}", msg))
            .build(),
    );
}

const LEAKY_REQWEST_CONNECT: &str =
    "proxy(http://user:pw@secret-proxy.invalid:8080) intercepts 'https://api.example/x'";

/// R3-2 capture (replaces the old ignored placeholder): reqwest's Debug proxy/destination
/// line is blocked, its Warn passes, and the sanitized app line passes without leaking.
#[test]
fn capture_sink_never_receives_reqwest_connect_debug() {
    let (captured, logger) = capturing_logger();

    emit(
        &*logger,
        Level::Debug,
        "reqwest::connect",
        LEAKY_REQWEST_CONNECT,
    );
    emit(
        &*logger,
        Level::Debug,
        "hyper_util::client::legacy::connect::http",
        LEAKY_REQWEST_CONNECT,
    );
    emit(&*logger, Level::Warn, "reqwest", "warn-from-reqwest-root");
    emit(
        &*logger,
        Level::Debug,
        "vox_app_lib::managers::source_update",
        "app-debug-line",
    );
    emit(
        &*logger,
        Level::Warn,
        "vox_app_lib::managers::source_update",
        "app-warn-line",
    );
    let line = describe_failure(
        &UpdateCheckError::Network(
            "http://user:pw@secret-proxy.invalid https://api.example/x".into(),
        ),
        true,
        true,
    );
    emit(
        &*logger,
        Level::Warn,
        "vox_app_lib::managers::source_update",
        &line,
    );

    let lines = captured.lock().expect("capture lock").clone();
    let all = lines.join("\n");
    assert!(
        !all.contains("proxy(") && !all.contains("intercepts"),
        "reqwest Debug line leaked: {all}"
    );
    assert!(
        !lines.iter().any(|l| l.starts_with("reqwest::connect ")),
        "reqwest::connect must be blocked at Debug: {all}"
    );
    assert!(
        !lines.iter().any(|l| l.starts_with("hyper_util")),
        "hyper_util must be blocked at Debug: {all}"
    );
    assert!(
        lines.iter().any(|l| l == "reqwest warn-from-reqwest-root"),
        "{all}"
    );
    assert!(
        lines.iter().any(|l| l.ends_with(" app-debug-line")),
        "app Debug must pass: {all}"
    );
    assert!(
        lines.iter().any(|l| l.ends_with(" app-warn-line")),
        "app Warn must pass: {all}"
    );
    assert!(
        lines.iter().any(|l| l.contains(
            "source update check failed: class=network proxy_configured=true retried_direct=true"
        )),
        "sanitized failure line must be captured: {all}"
    );
    for leak in ["secret-proxy", "user:pw", "api.example"] {
        assert!(
            !all.contains(leak),
            "captured output leaked {leak:?}: {all}"
        );
    }
}
