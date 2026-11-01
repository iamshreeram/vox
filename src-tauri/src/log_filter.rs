//! Log-level caps for third-party HTTP client targets (NET #190, DESIGN.md R3-2).
//!
//! reqwest and hyper log the proxy address and the destination URL at Debug/Info
//! (for example `proxy({proxy:?}) intercepts '{dst:?}'`). Vox must not write those
//! values to any sink (file, stdout or webview), so these targets are capped at Warn.
//! The cap is applied globally in `lib.rs` (`with_suppressed_log_targets`), so it covers
//! every sink. Rule: raw reqwest errors are never logged; only the content-free line
//! from `managers::source_update::describe_failure` is.

use log::LevelFilter;

/// Dependency targets capped at Warn. Matched as module-path prefixes.
pub(crate) const CAPPED_TARGETS: &[&str] = &["reqwest", "hyper_util"];

/// `Some(Warn)` for a capped dependency target (the target itself or a `::` child of it),
/// `None` for everything else.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn target_level_cap(target: &str) -> Option<LevelFilter> {
    let capped = CAPPED_TARGETS.iter().any(|prefix| {
        target == *prefix
            || target
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with("::"))
    });
    capped.then_some(LevelFilter::Warn)
}
