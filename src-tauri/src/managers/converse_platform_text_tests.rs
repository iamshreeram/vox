//! Table tests for `preflight_text_check`: a text chat skips ONLY the spoken-replies requirement
//! and keeps every other check of `preflight_check`. Child module of `converse_platform`.

use super::*;
use crate::managers::composer_config::TtsComposerMode;
use crate::settings::AppSettings;

const SPOKEN: &str = "Turn on spoken replies in the speech settings first.";
const BACKEND: &str = "Set up a reply backend in the speech settings first.";
const WAKE_OR_AMBIENT: &str =
    "Turn off wake word and ambient listening before starting a conversation.";

/// Settings in which every preflight check passes (the same base as the voice tests).
fn passing_settings() -> AppSettings {
    let mut s = crate::settings::get_default_settings();
    s.tts_enabled = true;
    s.tts_composer_mode = TtsComposerMode::Model;
    s.wake_word_enabled = false;
    s.ambient_mode_enabled = false;
    s
}

struct Case {
    name: &'static str,
    edit: fn(&mut AppSettings),
    /// What `preflight_check` (voice) must answer.
    voice: Result<(), &'static str>,
    /// What `preflight_text_check` (text chat) must answer.
    text: Result<(), &'static str>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "everything set",
            edit: |_| {},
            voice: Ok(()),
            text: Ok(()),
        },
        Case {
            name: "spoken replies off is skipped by a text chat",
            edit: |s| s.tts_enabled = false,
            voice: Err(SPOKEN),
            text: Ok(()),
        },
        Case {
            name: "reply backend missing still fails a text chat",
            edit: |s| s.tts_composer_mode = TtsComposerMode::Off,
            voice: Err(BACKEND),
            text: Err(BACKEND),
        },
        Case {
            name: "reply backend missing still fails when spoken replies are off",
            edit: |s| {
                s.tts_enabled = false;
                s.tts_composer_mode = TtsComposerMode::Off;
            },
            voice: Err(SPOKEN),
            text: Err(BACKEND),
        },
        Case {
            name: "wake word still refuses a text chat",
            edit: |s| s.wake_word_enabled = true,
            voice: Err(WAKE_OR_AMBIENT),
            text: Err(WAKE_OR_AMBIENT),
        },
        Case {
            name: "ambient listening still refuses a text chat",
            edit: |s| s.ambient_mode_enabled = true,
            voice: Err(WAKE_OR_AMBIENT),
            text: Err(WAKE_OR_AMBIENT),
        },
        Case {
            name: "wake word with spoken replies off still refuses a text chat",
            edit: |s| {
                s.tts_enabled = false;
                s.wake_word_enabled = true;
            },
            voice: Err(SPOKEN),
            text: Err(WAKE_OR_AMBIENT),
        },
    ]
}

#[test]
fn text_preflight_skips_only_the_spoken_reply_requirement() {
    for case in cases() {
        let mut s = passing_settings();
        (case.edit)(&mut s);
        assert_eq!(
            preflight_check(&s),
            case.voice.map_err(str::to_string),
            "voice preflight: {}",
            case.name
        );
        assert_eq!(
            preflight_text_check(&s),
            case.text.map_err(str::to_string),
            "text preflight: {}",
            case.name
        );
    }
}

#[test]
fn text_preflight_equals_voice_preflight_once_spoken_replies_are_on() {
    for case in cases() {
        let mut s = passing_settings();
        (case.edit)(&mut s);
        s.tts_enabled = true;
        assert_eq!(
            preflight_text_check(&s),
            preflight_check(&s),
            "with spoken replies on the two checks must agree: {}",
            case.name
        );
    }
}
