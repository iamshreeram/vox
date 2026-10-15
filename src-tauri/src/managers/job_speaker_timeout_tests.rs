//! JobSpeaker timed-out message with a configured limit (M3 S3c-2, DESIGN D5). The unset case
//! is covered in `job_speaker_tests.rs`; this file covers the set case added by the owner ruling.

use super::JobSpeaker;
use crate::managers::agent_jobs::{JobObserver, JobSnapshot, JobState};
use crate::managers::job_announcer::{Announcer, LowPriorityMouth};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Records every text the mouth was offered, and always reports it spoke.
#[derive(Default)]
struct RecordingMouth {
    spoken: Mutex<Vec<String>>,
}

impl LowPriorityMouth for RecordingMouth {
    fn try_speak_if_idle(&self, text: &str) -> bool {
        if let Ok(mut g) = self.spoken.lock() {
            g.push(text.to_string());
        }
        true
    }
}

/// Event sink that discards everything.
struct NullSink;

impl super::JobEventSink for NullSink {
    fn emit(&self, _dto: &crate::commands::agent_jobs::AgentJobStatusDto) {}
    fn refresh_tray(&self) {}
}

fn timed_out_snapshot() -> JobSnapshot {
    JobSnapshot {
        id: 1,
        project: "proj".to_string(),
        state: JobState::TimedOut,
        elapsed_secs: 600,
        tail: Vec::new(),
    }
}

#[test]
fn timed_out_message_names_the_configured_minutes_when_set() {
    let mouth = Arc::new(RecordingMouth::default());
    let announcer = Arc::new(Announcer::with_timing(
        mouth.clone(),
        Duration::from_millis(10),
        Duration::from_secs(60),
    ));
    let speaker = JobSpeaker::new(
        "proj".to_string(),
        announcer,
        Arc::new(|| true),
        Arc::new(NullSink),
    )
    .with_timeout_minutes(10);
    speaker.on_finish(&timed_out_snapshot());
    let said = mouth.spoken.lock().map(|g| g.clone()).unwrap_or_default();
    assert_eq!(
        said,
        vec!["The coding job in proj timed out after 10 minutes.".to_string()]
    );
}
