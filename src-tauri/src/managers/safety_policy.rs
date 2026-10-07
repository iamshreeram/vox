//! Deterministic consequential-action gate -- NOT an LLM call. Mirrors
//! vox's `src/vox/safety/policy.py`: a configurable keyword list checked
//! as whole-word/phrase matches against the text, fail-open toward
//! safety (any ambiguity means "require confirmation", never a silent
//! "allow").

use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafetyDecision {
    /// Not consequential -- proceed immediately.
    Allow,
    /// Consequential -- caller must get a confirm/cancel response before
    /// proceeding. Carries the original text so the caller can re-check it
    /// after confirmation.
    RequireConfirmation { original_text: String },
}

/// Default trigger keywords, matched case-insensitively. "rm " keeps its
/// trailing space deliberately: it's a prefix-boundary check (matches
/// "rm -rf /") rather than a strict whole-word match, since "rm" alone as
/// a bare word is vanishingly rare in real dictation but appearing as a
/// command prefix is exactly the dangerous case (FR6/T21).
const DEFAULT_TRIGGER_KEYWORDS: &[&str] = &[
    "delete",
    "remove",
    "send",
    "push",
    "deploy",
    "rm ",
    "format",
    "uninstall",
];

pub struct SafetyPolicy {
    patterns: Vec<Regex>,
}

impl SafetyPolicy {
    pub fn new(trigger_keywords: Vec<String>) -> Self {
        let patterns = trigger_keywords
            .iter()
            .map(|keyword| {
                let trimmed = keyword.trim_end_matches(' ');
                let escaped = regex::escape(trimmed);
                // "rm " (trailing space preserved) is a prefix-boundary
                // check, not a strict whole-word match -- see the
                // DEFAULT_TRIGGER_KEYWORDS doc comment above.
                let pattern = if keyword.ends_with(' ') {
                    format!(r"(?i)\b{escaped} ")
                } else {
                    format!(r"(?i)\b{escaped}\b")
                };
                Regex::new(&pattern).expect("escaped keyword is always a valid regex")
            })
            .collect();
        Self { patterns }
    }

    pub fn default_keywords() -> Vec<String> {
        DEFAULT_TRIGGER_KEYWORDS.iter().map(|s| s.to_string()).collect()
    }

    /// FR4/FR5: whole-word, case-insensitive keyword matching.
    /// FR6: an empty keyword list is a deliberate user opt-out -> Allow.
    pub fn decision_for_text(&self, text: &str) -> SafetyDecision {
        // T20: empty text is explicitly not consequential. Every pattern
        // below would also naturally fail to match "", so this is for
        // clarity/documentation of intent rather than being load-bearing.
        if self.patterns.is_empty() || text.is_empty() {
            return SafetyDecision::Allow;
        }
        for pattern in &self.patterns {
            if pattern.is_match(text) {
                return SafetyDecision::RequireConfirmation {
                    original_text: text.to_string(),
                };
            }
        }
        SafetyDecision::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_policy() -> SafetyPolicy {
        SafetyPolicy::new(SafetyPolicy::default_keywords())
    }

    fn assert_requires_confirmation(decision: SafetyDecision) {
        assert!(
            matches!(decision, SafetyDecision::RequireConfirmation { .. }),
            "expected RequireConfirmation, got {decision:?}"
        );
    }

    #[test]
    fn t13_delete_requires_confirmation() {
        assert_requires_confirmation(default_policy().decision_for_text("delete the file"));
    }

    #[test]
    fn t14_delete_is_case_insensitive() {
        assert_requires_confirmation(default_policy().decision_for_text("DELETE the file"));
    }

    #[test]
    fn t15_send_does_not_match_inside_sendai() {
        assert_eq!(
            default_policy().decision_for_text("I live in Sendai"),
            SafetyDecision::Allow
        );
    }

    #[test]
    fn t16_send_matches_as_whole_word() {
        assert_requires_confirmation(default_policy().decision_for_text("please send this email"));
    }

    #[test]
    fn t17_non_consequential_text_is_allowed() {
        assert_eq!(
            default_policy().decision_for_text("what's the weather"),
            SafetyDecision::Allow
        );
    }

    #[test]
    fn t18_empty_trigger_list_is_explicit_opt_out() {
        let policy = SafetyPolicy::new(vec![]);
        assert_eq!(
            policy.decision_for_text("delete everything"),
            SafetyDecision::Allow
        );
    }

    #[test]
    fn t19_custom_keyword_is_honored() {
        let policy = SafetyPolicy::new(vec!["banana".to_string()]);
        assert_requires_confirmation(policy.decision_for_text("please banana the repo"));
    }

    #[test]
    fn t20_empty_text_is_allowed() {
        assert_eq!(default_policy().decision_for_text(""), SafetyDecision::Allow);
    }

    #[test]
    fn t21_rm_prefix_requires_confirmation() {
        assert_requires_confirmation(default_policy().decision_for_text("rm -rf /"));
    }
}
