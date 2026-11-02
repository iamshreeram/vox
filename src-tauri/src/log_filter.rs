//! Log-level caps for third-party HTTP client targets (NET #190, DESIGN.md R3-2).
//!
//! reqwest and hyper log the proxy address and the destination URL at Debug/Info
//! (for example `proxy({proxy:?}) intercepts '{dst:?}'`). Vox must not write those
//! values to any sink (file, stdout or webview), so these targets are capped at Warn.
//! Rule: raw reqwest errors are never logged; only the content-free line from
//! `managers::source_update::describe_failure` is.
//!
//! ONE gate: `apply_log_caps` is the only place that applies the caps. Production
//! (`lib.rs`) and the capture test both call it, so they cannot drift apart.

use log::LevelFilter;

/// Dependency targets capped at Warn. Matched as module-path prefixes (the target
/// itself or a `::` child of it).
pub(crate) const CAPPED_TARGETS: &[&str] = &["reqwest", "hyper_util"];

/// The cap applied to every target in [`CAPPED_TARGETS`].
pub(crate) const CAP_LEVEL: LevelFilter = LevelFilter::Warn;

/// Applies the caps to a logger configuration. `level_for` is the sink's own per-target
/// filter method (`tauri_plugin_log::Builder::level_for` in production,
/// `fern::Dispatch::level_for` in the capture test). The table is walked here and nowhere else.
pub(crate) fn apply_log_caps<T>(
    sink: T,
    level_for: impl Fn(T, &'static str, LevelFilter) -> T,
) -> T {
    CAPPED_TARGETS
        .iter()
        .fold(sink, |sink, target| level_for(sink, target, CAP_LEVEL))
}

/// `Some(Warn)` for a capped dependency target, `None` for everything else. Reads the same
/// table as [`apply_log_caps`]. Used by the unit tests as the executable statement of the rule.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn target_level_cap(target: &str) -> Option<LevelFilter> {
    let capped = CAPPED_TARGETS.iter().any(|prefix| {
        target == *prefix
            || target
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with("::"))
    });
    capped.then_some(CAP_LEVEL)
}
