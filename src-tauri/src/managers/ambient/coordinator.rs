use super::echo::EchoSuppressor;
use super::engagement::EngagementJudge;
use super::transcript::RollingTranscript;

/// Coordinates the in-memory transcript and the ambient-mode decision helpers.
pub struct AmbientCoordinator {
    pub transcript: RollingTranscript,
    pub engagement_judge: EngagementJudge,
    pub echo_suppressor: EchoSuppressor,
}
