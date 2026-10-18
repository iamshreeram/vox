//! Curated natural voices (English Kokoro v1.0 voices) and the verified model bytes handed to
//! the engine loader. Pure data: compiles and is tested on every platform.
#![allow(dead_code)]

use crate::managers::natural_engine::DEFAULT_NATURAL_VOICE;

/// One selectable natural voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NaturalVoiceInfo {
    /// Kokoro voice id, e.g. `af_heart` (`a`/`b` = American/British, `f`/`m` = female/male).
    pub id: &'static str,
    /// Display name, e.g. `Heart`.
    pub name: &'static str,
}

impl NaturalVoiceInfo {
    pub fn accent(&self) -> &'static str {
        if self.id.starts_with('b') {
            "British"
        } else {
            "American"
        }
    }

    pub fn gender(&self) -> &'static str {
        if self.id.as_bytes().get(1) == Some(&b'm') {
            "Male"
        } else {
            "Female"
        }
    }
}

macro_rules! voices {
    ($(($id:literal, $name:literal)),* $(,)?) => {
        pub const NATURAL_VOICES: &[NaturalVoiceInfo] = &[
            $(NaturalVoiceInfo { id: $id, name: $name }),*
        ];
    };
}

voices![
    ("af_alloy", "Alloy"),
    ("af_aoede", "Aoede"),
    ("af_bella", "Bella"),
    ("af_heart", "Heart"),
    ("af_jessica", "Jessica"),
    ("af_kore", "Kore"),
    ("af_nicole", "Nicole"),
    ("af_nova", "Nova"),
    ("af_river", "River"),
    ("af_sarah", "Sarah"),
    ("af_sky", "Sky"),
    ("am_adam", "Adam"),
    ("am_echo", "Echo"),
    ("am_eric", "Eric"),
    ("am_fenrir", "Fenrir"),
    ("am_liam", "Liam"),
    ("am_michael", "Michael"),
    ("am_onyx", "Onyx"),
    ("am_puck", "Puck"),
    ("am_santa", "Santa"),
    ("bf_alice", "Alice"),
    ("bf_emma", "Emma"),
    ("bf_isabella", "Isabella"),
    ("bf_lily", "Lily"),
    ("bm_daniel", "Daniel"),
    ("bm_fable", "Fable"),
    ("bm_george", "George"),
    ("bm_lewis", "Lewis"),
];

/// True when `id` is one of the curated voices (exact, case-sensitive match).
pub fn is_known_voice(id: &str) -> bool {
    NATURAL_VOICES.iter().any(|v| v.id == id)
}

/// The id to use for a stored setting: a known id is kept, anything else (unknown, empty,
/// attacker-controlled) becomes `DEFAULT_NATURAL_VOICE`. Surrounding whitespace is ignored.
pub fn normalize_natural_voice(id: &str) -> String {
    let trimmed = id.trim();
    if is_known_voice(trimmed) {
        trimmed.to_string()
    } else {
        DEFAULT_NATURAL_VOICE.to_string()
    }
}

/// Model and voice-pack bytes that were hashed and verified IN MEMORY: the loader consumes
/// exactly these buffers, so nothing can be swapped between verification and load.
pub struct ModelBytes {
    pub model: Vec<u8>,
    pub voices: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_are_28_unique_english_voices_with_well_formed_ids() {
        assert_eq!(NATURAL_VOICES.len(), 28);
        let mut ids: Vec<&str> = NATURAL_VOICES.iter().map(|v| v.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 28, "duplicate ids");
        for v in NATURAL_VOICES {
            let (prefix, name) = v.id.split_once('_').expect("id has an underscore");
            assert!(["af", "am", "bf", "bm"].contains(&prefix), "{}", v.id);
            assert!(name.chars().all(|c| c.is_ascii_lowercase()), "{}", v.id);
            assert_eq!(v.name.to_lowercase(), name, "display name matches the id");
        }
    }

    #[test]
    fn the_default_voice_is_curated() {
        assert!(is_known_voice(DEFAULT_NATURAL_VOICE));
    }

    #[test]
    fn accent_and_gender_come_from_the_id_prefix() {
        let find = |id: &str| NATURAL_VOICES.iter().find(|v| v.id == id).unwrap();
        assert_eq!(find("af_heart").accent(), "American");
        assert_eq!(find("af_heart").gender(), "Female");
        assert_eq!(find("bm_george").accent(), "British");
        assert_eq!(find("bm_george").gender(), "Male");
        assert_eq!(find("am_puck").gender(), "Male");
        assert_eq!(find("bf_emma").accent(), "British");
    }

    #[test]
    fn normalize_keeps_known_ids_and_defaults_everything_else() {
        assert_eq!(normalize_natural_voice("bf_emma"), "bf_emma");
        assert_eq!(normalize_natural_voice("  am_adam \n"), "am_adam");
        for bad in ["", " ", "AF_HEART", "af_heart; rm -rf /", "../../etc/passwd", "zf_xiaoni", "af_"] {
            assert_eq!(normalize_natural_voice(bad), DEFAULT_NATURAL_VOICE, "{bad:?}");
        }
    }
}
