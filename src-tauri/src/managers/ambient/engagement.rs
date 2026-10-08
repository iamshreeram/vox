//! EngagementJudge heuristic (Phase 6). Ports vox's exact regex from
//! `src/vox/ambient/engagement.py`'s `EngagementJudge.evaluate()`:
//! `^\s*(?:hey|okay|ok)?[\s,]*(\w+)\b[,:]?\s*(.*)$` (case-insensitive).
//!
//! This is a BINARY addressed/not-addressed decision, purely positional:
//! does the wake name appear as literally the first captured word? There is
//! no "mentioned vs addressed" distinction -- "Vox is a good name for this"
//! IS addressed, by design (vox's own module docstring: conservative means
//! "check everything that starts with the name," not the opposite). See
//! `docs/vox-phases/PHASE-6-ambient-mode.md` section 3 for the full
//! rationale and worked examples (T7-T15, ported verbatim as tests below).

use super::transcript::TranscriptSegment;
use once_cell::sync::Lazy;
use regex::Regex;

static ADDRESS_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^\s*(?:hey|okay|ok)?[\s,]*(\w+)\b[,:]?\s*(.*)$").unwrap());

#[derive(Clone, Debug, PartialEq)]
#[allow(dead_code)]
pub enum Engagement {
    Addressed { extracted_request: String },
    NotAddressed,
}

/// `wake_names` defaults to `["vox"]`; a Vec allows future settings-driven
/// aliases without an API change.
#[allow(dead_code)]
pub struct EngagementJudge {
    pub wake_names: Vec<String>,
}

#[allow(dead_code)]
impl EngagementJudge {
    pub fn new(wake_names: Vec<String>) -> Self {
        Self { wake_names }
    }

    /// Scans `window` (oldest-first) from NEWEST to OLDEST. For the first
    /// segment whose text matches the address pattern AND whose captured
    /// name matches a known wake name, takes its remainder as the query. If
    /// that remainder is "effectively empty" (e.g. the lone "." left over
    /// from a bare "Hey Vox."), joins every segment AFTER it in the window
    /// as the query instead. No match anywhere -> NotAddressed.
    pub fn evaluate(&self, window: &[TranscriptSegment]) -> Engagement {
        for (index, segment) in window.iter().enumerate().rev() {
            let Some(captures) = ADDRESS_PATTERN.captures(&segment.text) else {
                continue;
            };
            let candidate_name = captures.get(1).map(|m| m.as_str()).unwrap_or("");
            let is_known_wake_name = self
                .wake_names
                .iter()
                .any(|name| name.eq_ignore_ascii_case(candidate_name));
            if !is_known_wake_name {
                continue;
            }
            let remainder = captures.get(2).map(|m| m.as_str()).unwrap_or("").trim();
            if !is_effectively_empty(remainder) {
                return Engagement::Addressed {
                    extracted_request: remainder.to_string(),
                };
            }
            let joined = window[index + 1..]
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            return Engagement::Addressed {
                extracted_request: joined,
            };
        }
        Engagement::NotAddressed
    }
}

/// Treats a remainder made up only of whitespace/punctuation (e.g. the lone
/// "." left over from "Hey Vox.") as empty, so a bare address-only segment
/// correctly falls through to the next segment's text as the real query.
fn is_effectively_empty(remainder: &str) -> bool {
    remainder
        .trim_matches(|c: char| c.is_whitespace() || c.is_ascii_punctuation())
        .is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str) -> TranscriptSegment {
        TranscriptSegment {
            segment_id: "0".to_string(),
            text: text.to_string(),
            start_time_ms: 0,
            end_time_ms: 0,
            confidence: 1.0,
            during_tts: false,
        }
    }

    fn window(texts: &[&str]) -> Vec<TranscriptSegment> {
        texts.iter().map(|t| seg(t)).collect()
    }

    #[test]
    fn t7_comma_and_capitalized_wake_word_is_addressed() {
        let judge = EngagementJudge::new(vec!["vox".to_string()]);
        let result = judge.evaluate(&window(&["Vox, what time is it"]));
        assert_eq!(
            result,
            Engagement::Addressed {
                extracted_request: "what time is it".to_string()
            }
        );
    }

    #[test]
    fn t8_no_comma_lowercase_wake_word_is_addressed() {
        let judge = EngagementJudge::new(vec!["vox".to_string()]);
        let result = judge.evaluate(&window(&["vox what time is it"]));
        assert_eq!(
            result,
            Engagement::Addressed {
                extracted_request: "what time is it".to_string()
            }
        );
    }

    #[test]
    fn t9_wake_word_not_in_first_position_is_not_addressed() {
        let judge = EngagementJudge::new(vec!["vox".to_string()]);
        let result = judge.evaluate(&window(&["I was telling Vox about the project"]));
        assert_eq!(result, Engagement::NotAddressed);
    }

    #[test]
    fn t10_statement_starting_with_wake_word_is_still_addressed() {
        let judge = EngagementJudge::new(vec!["vox".to_string()]);
        let result = judge.evaluate(&window(&["Vox is a good name for this"]));
        assert_eq!(
            result,
            Engagement::Addressed {
                extracted_request: "is a good name for this".to_string()
            }
        );
    }

    #[test]
    fn t11_unrelated_speech_is_not_addressed() {
        let judge = EngagementJudge::new(vec!["vox".to_string()]);
        let result = judge.evaluate(&window(&["what's the weather like today"]));
        assert_eq!(result, Engagement::NotAddressed);
    }

    #[test]
    fn t12_empty_window_is_not_addressed_and_does_not_panic() {
        let judge = EngagementJudge::new(vec!["vox".to_string()]);
        let result = judge.evaluate(&window(&[]));
        assert_eq!(result, Engagement::NotAddressed);
    }

    #[test]
    fn t13_custom_wake_name_computer_is_addressed_proving_no_hardcoded_vox() {
        let judge = EngagementJudge::new(vec!["computer".to_string()]);
        let result = judge.evaluate(&window(&["Computer, lights off"]));
        assert_eq!(
            result,
            Engagement::Addressed {
                extracted_request: "lights off".to_string()
            }
        );
    }

    #[test]
    fn t14_bare_address_segment_pulls_in_the_following_segment_as_the_query() {
        let judge = EngagementJudge::new(vec!["vox".to_string()]);
        let result = judge.evaluate(&window(&["Hey Vox.", "what time is it"]));
        assert_eq!(
            result,
            Engagement::Addressed {
                extracted_request: "what time is it".to_string()
            }
        );
    }

    #[test]
    fn t15_only_the_first_word_is_captured_as_the_wake_name() {
        let judge = EngagementJudge::new(vec!["vox".to_string()]);
        let result = judge.evaluate(&window(&["VOX VOX VOX what time is it"]));
        assert_eq!(
            result,
            Engagement::Addressed {
                extracted_request: "VOX VOX what time is it".to_string()
            }
        );
    }
}
