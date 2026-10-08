/// Mirrors the segment shape used by Vox's ambient transcript and engagement logic.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptSegment {
    pub segment_id: String,
    pub text: String,
    pub start_time_ms: u64,
    pub end_time_ms: u64,
    pub confidence: f32,
    pub during_tts: bool,
}

/// RAM-only ring buffer, retaining approximately the configured window.
pub struct RollingTranscript {}

impl RollingTranscript {
    /// `now_ms` is caller-supplied (monotonic), never read from the wall clock.
    pub fn add_segment(
        &mut self,
        text: &str,
        start_time_ms: u64,
        end_time_ms: u64,
        confidence: f32,
        during_tts: bool,
    ) -> TranscriptSegment {
        let _ = (text, start_time_ms, end_time_ms, confidence, during_tts);
        unimplemented!("implemented in the RollingTranscript task")
    }

    pub fn recent_segments(
        &mut self,
        now_ms: u64,
        within_ms: Option<u64>,
    ) -> Vec<TranscriptSegment> {
        let _ = (now_ms, within_ms);
        unimplemented!("implemented in the RollingTranscript task")
    }

    pub fn clear(&mut self) {
        unimplemented!("implemented in the RollingTranscript task")
    }
}
