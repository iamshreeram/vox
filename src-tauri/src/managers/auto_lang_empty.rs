//! Empty-text path of the automatic language route (DESIGN Revision 5 and 6).
//!
//! Parakeet-style English models often return an EMPTY transcript for speech in another
//! language. The ordinary gate (`needs_language_check`) never fires on empty text, so this
//! module adds a guarded second path: ask the language identifier only when the audio clearly
//! contains speech (measured by the app's own Earshot voice detector), and accept its answer
//! only under stricter rules than the ordinary path.
//!
//! Nothing here logs or stores text. Pure functions plus two small traits, so every rule is
//! tested with fakes; the real detector is exercised by the ignored corpus test.

use super::auto_lang::{
    choose_route, letter_count, non_latin_letter_fraction, tamil_letter_fraction, AutoRefined,
    AutoRoute, LidIdentify, LidOutcome, LidResult, Source, TamilPackRoute, LID_MAX_SECS,
    SAMPLE_RATE_HZ, SCRIPT_MIN_FRACTION,
};
use crate::audio_toolkit::vad::{EarshotVad, VoiceActivityDetector};

/// Samples per Earshot prediction (16 ms at 16 kHz). Mirrors `EARSHOT_FRAME_SAMPLES` in
/// `audio_toolkit/vad/earshot.rs`, which is not re-exported from the `vad` module.
const EARSHOT_FRAME: usize = 256;

/// Shortest whole clip that may take the empty path (seconds).
pub const EMPTY_MIN_AUDIO_SECS: f32 = 1.0;
/// Least detector-voiced audio needed before the identifier is asked (seconds).
pub const EMPTY_MIN_SPEECH_SECS: f32 = 1.0;
/// The identifier transcript must have at least this many letters to be accepted.
pub const EMPTY_MIN_LETTERS: usize = 6;
/// The identifier transcript must have at least this many whitespace-separated words.
pub const EMPTY_MIN_WORDS: usize = 2;
/// Detector threshold, the same value the recorder uses for Earshot.
pub const EMPTY_EARSHOT_THRESHOLD: f32 = 0.5;

/// Measures how many seconds of the audio a voice detector calls speech.
pub trait SpeechMeter {
    /// Seconds of speech in 16 kHz mono samples. Never panics; unusable input gives `0.0`.
    fn speech_seconds(&self, audio: &[f32]) -> f32;
}

/// The real meter: Earshot over consecutive 256-sample (16 ms) frames. A trailing partial
/// frame is ignored; a frame holding any non-finite sample counts as unvoiced and is not
/// passed to the detector; a detector error gives `0.0`.
pub struct EarshotMeter;

impl SpeechMeter for EarshotMeter {
    fn speech_seconds(&self, audio: &[f32]) -> f32 {
        let Ok(mut vad) = EarshotVad::new(EMPTY_EARSHOT_THRESHOLD) else {
            return 0.0;
        };
        let mut voiced: usize = 0;
        let (frames, _tail) = audio.as_chunks::<EARSHOT_FRAME>();
        for frame in frames {
            if frame.iter().any(|sample| !sample.is_finite()) {
                continue;
            }
            if matches!(vad.is_voice(frame), Ok(true)) {
                voiced += 1;
            }
        }
        voiced as f32 * (EARSHOT_FRAME as f32 / SAMPLE_RATE_HZ)
    }
}

/// Whether the identifier may be asked for this clip, and the measured speech seconds
/// (`0.0` when the meter was not consulted). The meter is consulted only when the active text
/// is blank (empty or whitespace only) and the clip is at least `EMPTY_MIN_AUDIO_SECS` long.
/// Allowed iff blank AND clip >= `EMPTY_MIN_AUDIO_SECS` AND speech >= `EMPTY_MIN_SPEECH_SECS`.
pub fn empty_text_check_allowed(
    active_text: &str,
    audio: &[f32],
    meter: &dyn SpeechMeter,
) -> (bool, f32) {
    let blank = active_text.trim().is_empty();
    let audio_secs = audio.len() as f32 / SAMPLE_RATE_HZ;
    if !blank || audio_secs < EMPTY_MIN_AUDIO_SECS {
        return (false, 0.0);
    }
    let speech = meter.speech_seconds(audio);
    (speech >= EMPTY_MIN_SPEECH_SECS, speech)
}

/// Stricter acceptance for the empty path: a language other than English is reported, the
/// transcript has at least `EMPTY_MIN_LETTERS` letters and `EMPTY_MIN_WORDS` words, and the
/// script matches the language (Tamil letters >= 0.5 for `ta`, non-Latin letters >= 0.5 for
/// every other language).
pub fn accept_empty_lid(lid: &LidResult) -> bool {
    let lang = lid
        .lang
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if lang.is_empty() || lang == "en" {
        return false;
    }
    if letter_count(&lid.text) < EMPTY_MIN_LETTERS {
        return false;
    }
    // A word is a whitespace-separated token holding at least one letter; punctuation-only
    // tokens do not count.
    let words = lid
        .text
        .split_whitespace()
        .filter(|token| letter_count(token) > 0)
        .count();
    if words < EMPTY_MIN_WORDS {
        return false;
    }
    if lang == "ta" {
        tamil_letter_fraction(&lid.text) >= SCRIPT_MIN_FRACTION
    } else {
        non_latin_letter_fraction(&lid.text) >= SCRIPT_MIN_FRACTION
    }
}

/// Result of `auto_refine_empty`.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
pub struct EmptyRefined {
    /// The text and where it came from (`Source::KeepActive` with empty text when nothing was
    /// accepted).
    pub refined: AutoRefined,
    /// Detector speech seconds (`0.0` when the meter was not consulted).
    pub speech_s: f32,
    /// True when the identifier was asked.
    pub admitted: bool,
}

/// The whole empty-path refinement. Calls `lid` only when `empty_text_check_allowed`; passes at
/// most `LID_MAX_SECS` of audio to `lid`; calls `tamil` (whole audio) only when the route is
/// `UseTamilPack`; on a Tamil pack failure falls through as if the pack were absent. Any
/// rejection returns `KeepActive` with `active_text` unchanged.
#[cfg_attr(not(test), allow(dead_code))]
pub fn auto_refine_empty(
    active_text: &str,
    audio: &[f32],
    tamil_pack_ready: bool,
    lid: &dyn LidIdentify,
    tamil: &dyn TamilPackRoute,
    meter: &dyn SpeechMeter,
) -> EmptyRefined {
    let (allowed, speech_s) = empty_text_check_allowed(active_text, audio, meter);
    if !allowed {
        return EmptyRefined {
            refined: AutoRefined {
                text: active_text.to_string(),
                source: Source::KeepActive,
            },
            speech_s,
            admitted: false,
        };
    }
    EmptyRefined {
        refined: refine_admitted(active_text, audio, tamil_pack_ready, lid, tamil),
        speech_s,
        admitted: true,
    }
}

/// The post-admission part of `auto_refine_empty`: identify (first `LID_MAX_SECS` only), accept
/// under the stricter rules, then route with `choose_route`. Callers must have passed
/// `empty_text_check_allowed` already. Shared with `route_auto_empty` so the meter runs once.
pub(crate) fn refine_admitted(
    active_text: &str,
    audio: &[f32],
    tamil_pack_ready: bool,
    lid: &dyn LidIdentify,
    tamil: &dyn TamilPackRoute,
) -> AutoRefined {
    let keep_active = || AutoRefined {
        text: active_text.to_string(),
        source: Source::KeepActive,
    };
    let audio_secs = audio.len() as f32 / SAMPLE_RATE_HZ;
    let lid_cap = (LID_MAX_SECS * SAMPLE_RATE_HZ) as usize;
    let lid_take = audio.len().min(lid_cap);
    let lid_audio = audio.get(..lid_take).unwrap_or(audio);
    let result = match lid.identify(lid_audio) {
        LidOutcome::Done(result) => result,
        LidOutcome::Busy | LidOutcome::Unavailable(_) => return keep_active(),
    };
    if !accept_empty_lid(&result) {
        return keep_active();
    }

    let mut route = choose_route(&result, tamil_pack_ready, audio_secs);
    if route == AutoRoute::UseTamilPack {
        if let Some(text) = tamil.transcribe(audio) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return AutoRefined {
                    text: trimmed.to_string(),
                    source: Source::TamilPack,
                };
            }
        }
        // Pack failed or returned nothing: fall through as if the pack were not installed.
        route = choose_route(&result, false, audio_secs);
    }

    if route != AutoRoute::UseLidText {
        return keep_active();
    }
    AutoRefined {
        text: result.text.trim().to_string(),
        source: Source::LidText {
            lang: result
                .lang
                .as_deref()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase(),
        },
    }
}

/// Facts logged for one empty-path decision. No text.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoEmptyLogFacts {
    pub speech_s: f32,
    pub admitted: bool,
    /// `keep`, `tamil_pack` or `lid_text:<lang>`.
    pub source: String,
}

/// `auto language: empty_path=true admitted=<bool> speech_s=<f32, 1 decimal> source=<source> ms=<n>`
pub fn format_empty_log(facts: &AutoEmptyLogFacts, ms: u128) -> String {
    format!(
        "auto language: empty_path=true admitted={} speech_s={:.1} source={} ms={}",
        facts.admitted, facts.speech_s, facts.source, ms
    )
}
