//! EchoSuppressor (Phase 6) -- suppresses the ambient listener's own TTS
//! output from being re-transcribed as a human saying it. Tests against a
//! fake "currently speaking" bool signal, NOT a real TTS dependency (Phase 4
//! doesn't exist yet). See `docs/vox-phases/PHASE-6-ambient-mode.md`
//! section 3.
//!
//! NOTE: excluding `during_tts: true` segments from `EngagementJudge`'s
//! evaluation (so Vox's own speech never triggers a false "addressed"
//! detection) is integration work that belongs to task A5's Coordinator,
//! which wires this type together with `RollingTranscript` and
//! `EngagementJudge` -- not implemented here.

#[derive(Default)]
#[allow(dead_code)]
pub struct EchoSuppressor;

#[allow(dead_code)]
impl EchoSuppressor {
    pub fn new() -> Self {
        Self
    }

    /// Given whether TTS is currently speaking, returns whether a segment
    /// captured right now should be tagged `during_tts: true`.
    pub fn is_during_tts(&self, is_speaking_now: bool) -> bool {
        is_speaking_now
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_captured_while_tts_speaking_is_tagged_during_tts_true() {
        let suppressor = EchoSuppressor::new();
        assert!(suppressor.is_during_tts(true));
    }

    #[test]
    fn segment_captured_while_tts_silent_is_tagged_during_tts_false() {
        let suppressor = EchoSuppressor::new();
        assert!(!suppressor.is_during_tts(false));
    }
}
