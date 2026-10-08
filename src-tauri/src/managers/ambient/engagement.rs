use super::transcript::TranscriptSegment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Engagement {
    Addressed { extracted_request: String },
    NotAddressed,
}

/// Deterministic judge configured with the names that address the assistant.
pub struct EngagementJudge {
    pub wake_names: Vec<String>,
}

impl EngagementJudge {
    pub fn new(wake_names: Vec<String>) -> Self {
        Self { wake_names }
    }

    /// Evaluate a transcript window for an utterance addressed to Vox.
    pub fn judge(&self, segments: &[TranscriptSegment]) -> Engagement {
        let _ = segments;
        unimplemented!("implemented in the EngagementJudge task")
    }
}
