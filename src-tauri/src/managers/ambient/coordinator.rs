//! Ambient-mode coordinator (Phase 6). Wires `RollingTranscript` +
//! `EngagementJudge` + `EchoSuppressor` together, plus the mutual-exclusion
//! rule against Wake Word (Phase 5). See
//! `docs/vox-phases/PHASE-6-ambient-mode.md` section 3.

use super::echo::EchoSuppressor;
use super::engagement::EngagementJudge;
use super::transcript::RollingTranscript;

use std::sync::Arc;

#[allow(dead_code)]
pub struct AmbientCoordinator {
    pub transcript: Arc<RollingTranscript>,
    pub judge: EngagementJudge,
    pub echo: EchoSuppressor,
}

#[allow(dead_code)]
impl AmbientCoordinator {
    pub fn new(
        transcript: RollingTranscript,
        judge: EngagementJudge,
        echo: EchoSuppressor,
    ) -> Self {
        Self::with_shared_transcript(Arc::new(transcript), judge, echo)
    }

    pub fn with_shared_transcript(
        transcript: Arc<RollingTranscript>,
        judge: EngagementJudge,
        echo: EchoSuppressor,
    ) -> Self {
        Self {
            transcript,
            judge,
            echo,
        }
    }
}

/// Mutual exclusion, direction 1: enabling Ambient Mode auto-disables Wake
/// Word if it's currently on. Must be called at the settings-write layer
/// (wherever `ambient_mode_enabled` gets written), not just enforced in the
/// UI, so a raw settings-file edit can't bypass it.
#[allow(dead_code)]
pub fn enabling_ambient_mode_disables_wake_word(settings: &mut crate::settings::AppSettings) {
    settings.wake_word_enabled = false;
    settings.ambient_mode_enabled = true;
}

/// Mutual exclusion, direction 2 (symmetric case): enabling Wake Word
/// auto-disables Ambient Mode if it's currently on.
#[allow(dead_code)]
pub fn enabling_wake_word_disables_ambient_mode(settings: &mut crate::settings::AppSettings) {
    settings.ambient_mode_enabled = false;
    settings.wake_word_enabled = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::get_default_settings;

    #[test]
    fn t16_enabling_ambient_mode_while_wake_word_is_on_auto_disables_wake_word() {
        let mut settings = get_default_settings();
        settings.wake_word_enabled = true;
        settings.ambient_mode_enabled = false;
        enabling_ambient_mode_disables_wake_word(&mut settings);
        assert!(settings.ambient_mode_enabled);
        assert!(!settings.wake_word_enabled);
    }

    #[test]
    fn t17_enabling_ambient_mode_from_fresh_defaults_succeeds_and_wake_word_stays_off() {
        let mut settings = get_default_settings();
        assert!(!settings.ambient_mode_enabled);
        assert!(!settings.wake_word_enabled);
        enabling_ambient_mode_disables_wake_word(&mut settings);
        assert!(settings.ambient_mode_enabled);
        assert!(!settings.wake_word_enabled);
    }

    #[test]
    fn symmetric_enabling_wake_word_while_ambient_mode_is_on_auto_disables_ambient_mode() {
        let mut settings = get_default_settings();
        settings.ambient_mode_enabled = true;
        settings.wake_word_enabled = false;
        enabling_wake_word_disables_ambient_mode(&mut settings);
        assert!(settings.wake_word_enabled);
        assert!(!settings.ambient_mode_enabled);
    }

    #[test]
    fn enabling_wake_word_from_fresh_defaults_succeeds_and_ambient_mode_stays_off() {
        let mut settings = get_default_settings();
        enabling_wake_word_disables_ambient_mode(&mut settings);
        assert!(settings.wake_word_enabled);
        assert!(!settings.ambient_mode_enabled);
    }

    #[test]
    fn coordinator_can_be_constructed_from_its_three_components() {
        let coordinator = AmbientCoordinator::new(
            RollingTranscript::new(90_000),
            EngagementJudge::new(vec!["vox".to_string()]),
            EchoSuppressor::new(),
        );
        coordinator
            .transcript
            .add_segment("hello", 0, 0, 0.9, false);
        assert_eq!(coordinator.transcript.stored_segment_count(), 1);
    }
}
