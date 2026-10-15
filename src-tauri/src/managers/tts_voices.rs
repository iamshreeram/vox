//! Lists the speech voices installed on this Mac for the Spoken replies dropdown.
//! Design: DESIGN.md Revision 3, section "Backend (Rust)".
//!
//! STUB (voice picker S1-A): the bodies below are `todo!()` until S1-B lands.

use serde::Serialize;
use specta::Type;
use std::path::Path;
use std::time::Duration;

/// One installed speech voice, as reported by `say -v '?'`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct TtsVoice {
    /// Voice name exactly as `say -v` accepts it (passes `sanitize_voice`).
    pub name: String,
    /// Locale code such as `en_US` or `es_419`.
    pub locale: String,
}

/// Parses the output of `say -v '?'` into voices.
///
/// Each line is `<name><spaces><locale><spaces># <sample>`. Lines that do not
/// parse, and names rejected by `sanitize_voice`, are skipped. Duplicates keep
/// the first entry, at most 300 voices are returned, and the result is sorted
/// by locale and then by case-insensitive name.
#[allow(dead_code)] // stub: unused until S1-B wires it into list_installed_voices
pub fn parse_say_voice_list(output: &str) -> Vec<TtsVoice> {
    let _ = output;
    todo!()
}

/// Runs `program` with `args` through the bounded runner and parses its stdout.
///
/// Any runner error, a truncated stdout, or a non-listing output yields an
/// empty Vec. The exit code is not used.
#[allow(dead_code)] // stub: unused until S1-B wires it into list_installed_voices
pub(crate) async fn list_voices_via(
    program: &Path,
    args: &[String],
    stdout_cap: usize,
    timeout: Duration,
) -> Vec<TtsVoice> {
    let _ = (program, args, stdout_cap, timeout);
    todo!()
}

/// Installed voices on macOS, via `/usr/bin/say -v '?'` with a 5 s timeout.
#[cfg(target_os = "macos")]
#[allow(dead_code)] // stub: unused until S1-B wires it into the command
pub async fn list_installed_voices() -> Vec<TtsVoice> {
    todo!()
}

/// Installed voices on non-macOS targets: always empty, no process is spawned.
#[cfg(not(target_os = "macos"))]
pub async fn list_installed_voices() -> Vec<TtsVoice> {
    Vec::new()
}

#[cfg(test)]
#[path = "tts_voices_tests.rs"]
mod tts_voices_tests;

#[cfg(all(test, unix))]
#[path = "tts_voices_proc_tests.rs"]
mod tts_voices_proc_tests;
