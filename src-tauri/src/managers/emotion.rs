//! Emotion labels for spoken replies and their (subtle) prosody mapping.
//! PURE: no I/O. See DESIGN.md section 3.2.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum Emotion {
    Neutral,
    Cheerful,
    Excited,
    Calm,
    Concerned,
    Apologetic,
    Serious,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prosody {
    pub rate_mult: f32,
    pub pitch_delta: i8,
}

impl Emotion {
    /// Trims, case-insensitive; unknown or empty => Neutral.
    pub fn from_label(label: &str) -> Emotion {
        todo!()
    }

    pub fn prosody(self) -> Prosody {
        todo!()
    }
}

/// Neutral returns `base` UNCHANGED (including `None`). Otherwise
/// `Some(clamp_rate(round(base.unwrap_or(180) as f32 * rate_mult) as u32))`:
/// multiply, round (f32::round), THEN clamp to the engine range.
pub fn effective_rate(base: Option<u32>, emotion: Emotion) -> Option<u32> {
    todo!()
}

/// `None` when the pitch delta is 0; otherwise `"[[pbas +N]] "` or
/// `"[[pbas -N]] "` (explicit sign, trailing space).
pub fn pitch_prefix(emotion: Emotion) -> Option<String> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Emotion; 7] = [
        Emotion::Neutral,
        Emotion::Cheerful,
        Emotion::Excited,
        Emotion::Calm,
        Emotion::Concerned,
        Emotion::Apologetic,
        Emotion::Serious,
    ];

    #[test]
    fn from_label_is_case_and_whitespace_insensitive() {
        assert_eq!(Emotion::from_label("cheerful"), Emotion::Cheerful);
        assert_eq!(Emotion::from_label("  CHEERFUL \n"), Emotion::Cheerful);
        assert_eq!(Emotion::from_label("Apologetic"), Emotion::Apologetic);
        assert_eq!(Emotion::from_label("serious"), Emotion::Serious);
        assert_eq!(Emotion::from_label("Excited"), Emotion::Excited);
        assert_eq!(Emotion::from_label("calm"), Emotion::Calm);
        assert_eq!(Emotion::from_label("concerned"), Emotion::Concerned);
        assert_eq!(Emotion::from_label("neutral"), Emotion::Neutral);
    }

    #[test]
    fn from_label_unknown_or_empty_is_neutral() {
        for s in ["", "   ", "angry", "cheerful!", "chee rful", "\u{1f600}", "calm\0"] {
            assert_eq!(Emotion::from_label(s), Emotion::Neutral, "{s:?}");
        }
    }

    #[test]
    fn prosody_table() {
        let t = |e: Emotion| {
            let p = e.prosody();
            (p.rate_mult, p.pitch_delta)
        };
        assert_eq!(t(Emotion::Neutral), (1.00, 0));
        assert_eq!(t(Emotion::Cheerful), (1.08, 6));
        assert_eq!(t(Emotion::Excited), (1.15, 8));
        assert_eq!(t(Emotion::Calm), (0.92, -3));
        assert_eq!(t(Emotion::Concerned), (0.95, -4));
        assert_eq!(t(Emotion::Apologetic), (0.90, -5));
        assert_eq!(t(Emotion::Serious), (0.95, -6));
    }

    #[test]
    fn neutral_returns_base_unchanged_including_none() {
        assert_eq!(effective_rate(None, Emotion::Neutral), None);
        assert_eq!(effective_rate(Some(180), Emotion::Neutral), Some(180));
        // Even an out-of-range base is passed through (legacy behavior).
        assert_eq!(effective_rate(Some(10), Emotion::Neutral), Some(10));
        assert_eq!(effective_rate(Some(900), Emotion::Neutral), Some(900));
    }

    #[test]
    fn rate_rounds_exactly() {
        assert_eq!(effective_rate(Some(180), Emotion::Cheerful), Some(194)); // 194.4
        assert_eq!(effective_rate(Some(180), Emotion::Excited), Some(207)); // 207.0
        assert_eq!(effective_rate(Some(180), Emotion::Apologetic), Some(162)); // 162.0
        assert_eq!(effective_rate(Some(180), Emotion::Serious), Some(171)); // 171.0
        assert_eq!(effective_rate(Some(180), Emotion::Concerned), Some(171));
        assert_eq!(effective_rate(Some(200), Emotion::Calm), Some(184)); // 184.0
    }

    #[test]
    fn none_base_uses_the_default_rate() {
        assert_eq!(effective_rate(None, Emotion::Cheerful), Some(194));
        assert_eq!(effective_rate(None, Emotion::Calm), Some(166)); // 165.6
    }

    #[test]
    fn rate_is_clamped_after_rounding() {
        assert_eq!(effective_rate(Some(400), Emotion::Excited), Some(400)); // 460 -> 400
        assert_eq!(effective_rate(Some(80), Emotion::Calm), Some(80)); // 73.6 -> 74 -> 80
        assert_eq!(effective_rate(Some(80), Emotion::Apologetic), Some(80)); // 72 -> 80
        assert_eq!(effective_rate(Some(1_000_000), Emotion::Cheerful), Some(400));
        assert_eq!(effective_rate(Some(0), Emotion::Cheerful), Some(80));
    }

    #[test]
    fn every_non_neutral_rate_stays_in_engine_range() {
        for e in ALL.into_iter().filter(|e| *e != Emotion::Neutral) {
            for base in [None, Some(0), Some(80), Some(180), Some(400), Some(u32::MAX / 4)] {
                let r = effective_rate(base, e).unwrap();
                assert!((80..=400).contains(&r), "{e:?} {base:?} -> {r}");
            }
        }
    }

    #[test]
    fn pitch_prefix_has_explicit_sign_and_none_for_neutral() {
        assert_eq!(pitch_prefix(Emotion::Neutral), None);
        assert_eq!(pitch_prefix(Emotion::Cheerful).as_deref(), Some("[[pbas +6]] "));
        assert_eq!(pitch_prefix(Emotion::Excited).as_deref(), Some("[[pbas +8]] "));
        assert_eq!(pitch_prefix(Emotion::Calm).as_deref(), Some("[[pbas -3]] "));
        assert_eq!(pitch_prefix(Emotion::Concerned).as_deref(), Some("[[pbas -4]] "));
        assert_eq!(pitch_prefix(Emotion::Apologetic).as_deref(), Some("[[pbas -5]] "));
        assert_eq!(pitch_prefix(Emotion::Serious).as_deref(), Some("[[pbas -6]] "));
    }

    #[test]
    fn serde_uses_snake_case_labels() {
        assert_eq!(serde_json::to_string(&Emotion::Apologetic).unwrap(), "\"apologetic\"");
        let e: Emotion = serde_json::from_str("\"cheerful\"").unwrap();
        assert_eq!(e, Emotion::Cheerful);
    }
}
