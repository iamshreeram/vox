//! Tiny process-wide hooks that let the dictation code paths ask "is a
//! conversation session using the microphone?" and let the audio callback
//! forward levels to the active capture worker, without those modules
//! depending on the session runtime. Contract: DESIGN.md section 2 and 3.6.
//!
//! Everything here is cheap and non-blocking: it is called from the audio
//! callback thread and from shortcut handlers.

use crate::managers::converse_capture::CaptureHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static SESSION_ACTIVE: AtomicBool = AtomicBool::new(false);
static CAPTURE: Mutex<Option<CaptureHandle>> = Mutex::new(None);
static EPOCH: OnceLock<Instant> = OnceLock::new();

/// Test-only switch so other modules can exercise code that branches on
/// `capture_active()` without spawning a worker. Serialize with `TEST_LOCK`.
#[cfg(test)]
pub(crate) static FORCE_CAPTURE_ACTIVE: AtomicBool = AtomicBool::new(false);

fn now_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Mark a session as running/stopped (set by the session manager; true from
/// start until the finalizer has joined the capture worker).
pub fn set_session_active(active: bool) {
    SESSION_ACTIVE.store(active, Ordering::SeqCst);
}

/// True while a conversation session exists (Running or Draining).
pub fn session_active() -> bool {
    SESSION_ACTIVE.load(Ordering::SeqCst)
}

/// Install (Some) or remove (None) the capture handle that receives levels.
pub fn install_capture(handle: Option<CaptureHandle>) {
    let mut guard = CAPTURE.lock().unwrap_or_else(|e| e.into_inner());
    *guard = handle;
}

/// True only while the installed capture worker owns the recorder.
/// False when nothing is installed.
pub fn capture_active() -> bool {
    #[cfg(test)]
    if FORCE_CAPTURE_ACTIVE.load(Ordering::SeqCst) {
        return true;
    }
    let handle = {
        let guard = CAPTURE.lock().unwrap_or_else(|e| e.into_inner());
        guard.clone()
    };
    match handle {
        Some(h) => h.capture_active(),
        None => false,
    }
}

/// Audio-callback entry: forward one level frame to the installed capture
/// worker. NON-BLOCKING; a no-op when nothing is installed. The lock is held
/// only to clone the handle, never across `push_levels`.
pub fn on_levels(levels: &[f32]) {
    let handle = {
        let guard = CAPTURE.lock().unwrap_or_else(|e| e.into_inner());
        guard.clone()
    };
    if let Some(h) = handle {
        h.push_levels(now_ms(), levels);
    }
}

#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managers::converse::ListenOutcome;
    use crate::managers::converse_audio::EndpointConfig;
    use crate::managers::converse_capture::{
        CaptureTimings, CaptureWorker, RecorderPort, StartError, TranscribeFailure, Transcriber,
    };
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use std::time::Duration;

    struct Port {
        active: AtomicBool,
    }
    impl RecorderPort for Port {
        fn start(&self) -> Result<(), StartError> {
            self.active.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn stop(&self) -> Option<Vec<f32>> {
            self.active
                .swap(false, Ordering::SeqCst)
                .then(|| vec![0.1; 160])
        }
        fn cancel(&self) {
            self.active.store(false, Ordering::SeqCst);
        }
    }
    struct Tr;
    impl Transcriber for Tr {
        fn transcribe(&self, _s: Vec<f32>) -> Result<String, TranscribeFailure> {
            Ok("hi".into())
        }
    }

    fn worker() -> (Arc<Port>, CaptureHandle) {
        let port = Arc::new(Port {
            active: AtomicBool::new(false),
        });
        let h = CaptureWorker::spawn(
            port.clone(),
            Arc::new(Tr),
            CaptureTimings {
                busy_retry: Duration::from_millis(10),
                busy_deadline: Duration::from_millis(100),
                readiness_deadline: Duration::from_secs(5),
                abs_slack: Duration::from_secs(5),
                tick: Duration::from_millis(5),
            },
        );
        (port, h)
    }

    #[test]
    fn defaults_are_inactive_and_levels_are_a_no_op() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        install_capture(None);
        set_session_active(false);
        assert!(!session_active());
        assert!(!capture_active());
        on_levels(&[0.5, 0.5]); // must not panic or block
    }

    #[test]
    fn session_flag_round_trips() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_session_active(true);
        assert!(session_active());
        set_session_active(false);
        assert!(!session_active());
    }

    #[tokio::test]
    async fn installed_handle_reports_capture_and_receives_levels() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (port, h) = worker();
        install_capture(Some(h.clone()));
        assert!(!capture_active());
        let h2 = h.clone();
        let task = tokio::spawn(async move {
            h2.listen(EndpointConfig {
                silence_ms: 200,
                initial_grace_ms: 5000,
                max_utterance_ms: 5000,
                min_speech_ms: 50,
                floor: 0.02,
            })
            .await
        });
        let started = Instant::now();
        while !capture_active() {
            assert!(started.elapsed() < Duration::from_secs(3));
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert!(port.active.load(Ordering::SeqCst));
        // levels flow through the hook (loud then quiet) and end the utterance
        for _ in 0..30 {
            on_levels(&[0.6, 0.3]);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        for _ in 0..60 {
            on_levels(&[0.0]);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let out = tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out, ListenOutcome::Utterance("hi".into()));
        install_capture(None);
        assert!(!capture_active());
        let h3 = h.clone();
        tokio::task::spawn_blocking(move || h3.shutdown())
            .await
            .unwrap();
    }

    #[test]
    fn on_levels_never_blocks_with_a_handle_installed_and_idle() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_port, h) = worker();
        install_capture(Some(h.clone()));
        let calls = AtomicUsize::new(0);
        let started = Instant::now();
        for _ in 0..50_000 {
            on_levels(&[0.4]);
            calls.fetch_add(1, Ordering::Relaxed);
        }
        assert!(started.elapsed() < Duration::from_secs(2));
        install_capture(None);
        h.shutdown();
    }
}
