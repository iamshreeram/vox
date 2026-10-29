//! Table tests for `preflight_text_check`: a text chat skips ONLY the spoken-replies requirement
//! and keeps every other check of `preflight_check`. Child module of `converse_platform`.

use super::*;
use crate::managers::composer_config::TtsComposerMode;
use crate::settings::AppSettings;

const SPOKEN: &str = "Turn on spoken replies in Settings > Models > Text-to-speech first.";
const BACKEND: &str = "Set up a reply backend in the speech settings first.";
const AMBIENT: &str = "Turn off ambient listening before starting a conversation.";

/// Settings in which every preflight check passes (the same base as the voice tests).
fn passing_settings() -> AppSettings {
    let mut s = crate::settings::get_default_settings();
    // Legacy fixture: each feature keeps its own provider settings.
    s.brain_source_composer = crate::settings::BrainSource::Own;
    s.brain_source_realtime = crate::settings::BrainSource::Own;
    s.brain_source_postprocess = crate::settings::BrainSource::Own;
    s.tts_composer_model = "legacy-model".into();
    s.tts_enabled = true;
    s.tts_composer_mode = TtsComposerMode::Model;
    s.wake_word_enabled = false;
    s.ambient_mode_enabled = false;
    s.experimental_enabled = false;
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
            name: "wake word no longer refuses a text chat",
            edit: |s| s.wake_word_enabled = true,
            voice: Ok(()),
            text: Ok(()),
        },
        Case {
            name: "effective ambient listening still refuses a text chat",
            edit: |s| {
                s.experimental_enabled = true;
                s.ambient_mode_enabled = true;
            },
            voice: Err(AMBIENT),
            text: Err(AMBIENT),
        },
        Case {
            name: "stored ambient flag with Experimental off does not refuse",
            edit: |s| s.ambient_mode_enabled = true,
            voice: Ok(()),
            text: Ok(()),
        },
        Case {
            name: "wake word with spoken replies off reports only the spoken-reply refusal",
            edit: |s| {
                s.tts_enabled = false;
                s.wake_word_enabled = true;
            },
            voice: Err(SPOKEN),
            text: Ok(()),
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
