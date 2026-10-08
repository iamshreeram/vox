//! RollingTranscript ring buffer (Phase 6). See
//! `docs/vox-phases/PHASE-6-ambient-mode.md` section 3.
//!
//! NOTE: the phase doc shows `&mut self` methods, but NFR2 (safe to push
//! from an audio-callback thread while read from the UI/coordinator thread)
//! requires `&self` + internal `Mutex`-guarded state instead, mirroring
//! `MemoryManager`'s `Arc<Self>` + `&self` pattern elsewhere in this crate.

use std::sync::Mutex;

/// Mirrors vox's TranscriptSegment exactly (fields vox's own
/// EngagementJudge.evaluate() and RollingTranscript both rely on).
#[derive(Clone, Debug, PartialEq)]
#[allow(dead_code)]
pub struct TranscriptSegment {
    pub segment_id: String,
    pub text: String,
    pub start_time_ms: u64,
    pub end_time_ms: u64,
    pub confidence: f32,
    pub during_tts: bool,
}

/// RAM-only ring buffer, ~90 seconds of segments by default. `now_ms` is
/// always caller-supplied, never read from the wall clock internally --
/// required for deterministic tests.
#[derive(Default)]
#[allow(dead_code)]
pub struct RollingTranscript {
    window_ms: u64,
    segments: Mutex<Vec<TranscriptSegment>>,
    next_id: Mutex<u64>,
}

#[allow(dead_code)]
impl RollingTranscript {
    pub fn new(window_ms: u64) -> Self {
        Self {
            window_ms,
            segments: Mutex::new(Vec::new()),
            next_id: Mutex::new(0),
        }
    }

    pub fn add_segment(
        &self,
        text: &str,
        start_time_ms: u64,
        end_time_ms: u64,
        confidence: f32,
        during_tts: bool,
    ) -> TranscriptSegment {
        let segment_id = {
            let mut next_id = self.next_id.lock().unwrap();
            let id = *next_id;
            *next_id += 1;
            id.to_string()
        };

        let segment = TranscriptSegment {
            segment_id,
            text: text.to_string(),
            start_time_ms,
            end_time_ms,
            confidence,
            during_tts,
        };

        let mut segments = self.segments.lock().unwrap();
        segments.push(segment.clone());
        Self::expire(&mut segments, end_time_ms, self.window_ms);
        segment
    }

    pub fn recent_segments(&self, now_ms: u64, within_ms: Option<u64>) -> Vec<TranscriptSegment> {
        let mut segments = self.segments.lock().unwrap();
        Self::expire(&mut segments, now_ms, self.window_ms);
        let cutoff = within_ms.unwrap_or(self.window_ms);
        segments
            .iter()
            .filter(|segment| now_ms.saturating_sub(segment.end_time_ms) <= cutoff)
            .cloned()
            .collect()
    }

    /// Explicit privacy action.
    pub fn clear(&self) {
        self.segments.lock().unwrap().clear();
    }

    /// Exposed for tests (and future debug UI) to confirm expired segments
    /// are actually freed from storage, not just filtered out at query time.
    pub fn stored_segment_count(&self) -> usize {
        self.segments.lock().unwrap().len()
    }

    fn expire(segments: &mut Vec<TranscriptSegment>, now_ms: u64, window_ms: u64) {
        segments.retain(|segment| now_ms.saturating_sub(segment.end_time_ms) <= window_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment_at(transcript: &RollingTranscript, text: &str, time_ms: u64) -> TranscriptSegment {
        transcript.add_segment(text, time_ms, time_ms, 0.9, false)
    }

    #[test]
    fn recent_segments_returns_all_three_oldest_first() {
        let transcript = RollingTranscript::new(90_000);
        segment_at(&transcript, "one", 0);
        segment_at(&transcript, "two", 1_000);
        segment_at(&transcript, "three", 2_000);

        let recent = transcript.recent_segments(2_000, None);
        assert_eq!(
            recent.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
            ["one", "two", "three"]
        );
    }

    #[test]
    fn segments_older_than_the_window_are_excluded() {
        let transcript = RollingTranscript::new(90_000);
        segment_at(&transcript, "old", 0);
        let recent = transcript.recent_segments(100_000, None);
        assert!(recent.iter().all(|s| s.text != "old"));
    }

    #[test]
    fn expired_segments_are_actually_freed_from_storage_not_just_filtered() {
        let transcript = RollingTranscript::new(90_000);
        for i in 0..1000u64 {
            transcript.add_segment("seg", i * 1_000, i * 1_000, 0.9, false);
        }
        assert!(transcript.stored_segment_count() < 1000);
    }

    #[test]
    fn clear_empties_the_transcript_immediately() {
        let transcript = RollingTranscript::new(90_000);
        segment_at(&transcript, "one", 0);
        segment_at(&transcript, "two", 1_000);
        transcript.clear();
        assert!(transcript.recent_segments(1_000, None).is_empty());
        assert_eq!(transcript.stored_segment_count(), 0);
    }

    #[test]
    fn recent_segments_on_empty_transcript_is_empty_and_does_not_panic() {
        let transcript = RollingTranscript::new(90_000);
        assert!(transcript.recent_segments(0, None).is_empty());
    }

    #[test]
    fn concurrent_add_and_read_do_not_panic_or_race() {
        use std::sync::Arc;
        use std::thread;

        let transcript = Arc::new(RollingTranscript::new(90_000));
        let writer_transcript = Arc::clone(&transcript);
        let writer = thread::spawn(move || {
            for i in 0..500u64 {
                writer_transcript.add_segment("seg", i * 10, i * 10, 0.9, false);
            }
        });
        let reader_transcript = Arc::clone(&transcript);
        let reader = thread::spawn(move || {
            for _ in 0..500 {
                let _ = reader_transcript.recent_segments(5_000, None);
            }
        });
        writer.join().unwrap();
        reader.join().unwrap();
    }
}
