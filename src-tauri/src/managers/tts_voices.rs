//! Lists the speech voices installed on this Mac for the Spoken replies dropdown.
//! Design: DESIGN.md Revision 3, section "Backend (Rust)".

use crate::managers::agent_tools::bounded_cmd::run_bounded;
use crate::managers::tts::sanitize_voice;
use serde::Serialize;
use specta::Type;
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

/// Upper bound on the number of voices returned to the UI.
const MAX_VOICES: usize = 300;

/// One installed speech voice, as reported by `say -v '?'`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct TtsVoice {
    /// Voice name exactly as `say -v` accepts it (passes `sanitize_voice`).
    pub name: String,
    /// Locale code such as `en_US` or `es_419`.
    pub locale: String,
}

/// True for locale codes shaped like `^[a-z]{2,3}_[A-Za-z0-9]{2,4}$`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn is_locale(code: &str) -> bool {
    let Some((language, region)) = code.split_once('_') else {
        return false;
    };
    let language_ok =
        (2..=3).contains(&language.len()) && language.bytes().all(|b| b.is_ascii_lowercase());
    let region_ok =
        (2..=4).contains(&region.len()) && region.bytes().all(|b| b.is_ascii_alphanumeric());
    language_ok && region_ok
}

/// Parses one `say -v '?'` line; `None` when the line does not describe a valid voice.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_line(line: &str) -> Option<TtsVoice> {
    let (left, _sample) = line.split_once('#')?;
    let (name_part, locale) = left.trim().rsplit_once(char::is_whitespace)?;
    if !is_locale(locale) {
        return None;
    }
    let name = name_part.trim_end();
    if sanitize_voice(name).as_deref() != Some(name) {
        return None;
    }
    Some(TtsVoice {
        name: name.to_string(),
        locale: locale.to_string(),
    })
}

/// Parses the output of `say -v '?'` into voices.
///
/// Each line is `<name><spaces><locale><spaces># <sample>`. Lines that do not
/// parse, and names rejected by `sanitize_voice`, are skipped. Duplicates keep
/// the first entry, at most 300 voices are returned, and the result is sorted
/// by locale and then by case-insensitive name.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn parse_say_voice_list(output: &str) -> Vec<TtsVoice> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut voices: Vec<TtsVoice> = Vec::new();
    for line in output.lines() {
        if voices.len() >= MAX_VOICES {
            break;
        }
        let Some(voice) = parse_line(line.trim_end_matches('\r')) else {
            continue;
        };
        if seen.insert(voice.name.clone()) {
            voices.push(voice);
        }
    }
    voices.sort_by(|a, b| {
        a.locale
            .cmp(&b.locale)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    voices
}

/// Runs `program` with `args` through the bounded runner and parses its stdout.
///
/// Any runner error, a truncated stdout, or a non-listing output yields an
/// empty Vec. The exit code is not used: the shared runner can kill a leader
/// that already closed stdout before its exit status was collected, and a
/// failing program yields no valid lines anyway.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) async fn list_voices_via(
    program: &Path,
    args: &[String],
    stdout_cap: usize,
    timeout: Duration,
) -> Vec<TtsVoice> {
    match run_bounded(program, args, Path::new("/"), &[], stdout_cap, timeout).await {
        Ok(output) if !output.stdout_truncated => {
            parse_say_voice_list(&String::from_utf8_lossy(&output.stdout))
        }
        _ => Vec::new(),
    }
}

/// Installed voices on macOS, via `/usr/bin/say -v '?'` with a 5 s timeout.
#[cfg(target_os = "macos")]
pub async fn list_installed_voices() -> Vec<TtsVoice> {
    let args = ["-v".to_string(), "?".to_string()];
    list_voices_via(
        Path::new("/usr/bin/say"),
        &args,
        256 * 1024,
        Duration::from_secs(5),
    )
    .await
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
