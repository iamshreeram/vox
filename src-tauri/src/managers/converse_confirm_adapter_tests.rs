//! RED adapter tests for the M3 S1 confirmation gate (DESIGN.md R3-1, R4-1):
//! `confirm_action` driven through the REAL `CaptureHandle` (fed fake levels and
//! speech events) instead of a scripted Ears. Written before the implementation.

use super::*;
use crate::managers::converse::{BoxFuture, Ears, ListenOutcome, Mouth, QuietOutcome, SayOutcome};
use crate::managers::converse_audio::EndpointConfig;
use crate::managers::converse_capture::{
    CaptureHandle, CaptureTimings, CaptureWorker, RecorderPort, StartError, TranscribeFailure,
    Transcriber,
};
use crate::managers::emotion::Emotion;
use crate::managers::job_announcer::{Announcer, LowPriorityMouth};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const LOUD: &[f32] = &[0.0, 0.6, 0.2];
const QUIET: &[f32] = &[0.0, 0.0, 0.0];

/// Recorder port that only tracks whether a recording is active.
#[derive(Default)]
struct Port {
    active: AtomicBool,
    starts: AtomicUsize,
    stops: AtomicUsize,
}

impl RecorderPort for Port {
    fn start(&self) -> Result<(), StartError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.active.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn stop(&self) -> Option<Vec<f32>> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        let was = self.active.swap(false, Ordering::SeqCst);
        if was {
            Some(vec![0.1; 160])
        } else {
            None
        }
    }
    fn cancel(&self) {
        self.active.store(false, Ordering::SeqCst);
    }
}

/// Transcriber that returns a fixed text.
struct Text(&'static str);

impl Transcriber for Text {
    fn transcribe(&self, _samples: Vec<f32>) -> Result<String, TranscribeFailure> {
        Ok(self.0.to_string())
    }
}

fn timings() -> CaptureTimings {
    CaptureTimings {
        busy_retry: Duration::from_millis(10),
        busy_deadline: Duration::from_millis(500),
        readiness_deadline: Duration::from_millis(500),
        abs_slack: Duration::from_millis(500),
        tick: Duration::from_millis(5),
    }
}

fn spawn_capture(text: &'static str) -> (CaptureHandle, Arc<Port>) {
    let port = Arc::new(Port::default());
    let handle = CaptureWorker::spawn(port.clone(), Arc::new(Text(text)), timings());
    (handle, port)
}

/// Ears backed by the real capture handle; records the wall-clock instant of each listen call.
struct CaptureEars {
    handle: CaptureHandle,
    calls: Mutex<Vec<Instant>>,
    /// Silence window (ms) used by the endpointer.
    silence_ms: u64,
}

impl Ears for CaptureEars {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        self.calls.lock().unwrap().push(Instant::now());
        let handle = self.handle.clone();
        let cfg = EndpointConfig {
            silence_ms: self.silence_ms,
            initial_grace_ms: 600,
            max_utterance_ms: 5000,
            min_speech_ms: 100,
            floor: 0.02,
        };
        Box::pin(async move { handle.listen(cfg).await })
    }
}

/// Mouth that speaks instantly and reports Quiet immediately.
struct QuietMouth {
    say_done: Mutex<Option<Instant>>,
}

impl Mouth for QuietMouth {
    fn say<'a>(&'a self, _text: &'a str, _e: Emotion) -> BoxFuture<'a, SayOutcome> {
        *self.say_done.lock().unwrap() = Some(Instant::now());
        Box::pin(async { SayOutcome::Done })
    }
    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async { QuietOutcome::Quiet })
    }
    fn stop(&self) {}
}

#[derive(Default)]
struct LowPriority {
    spoken: Mutex<Vec<String>>,
}

impl LowPriorityMouth for LowPriority {
    fn try_speak_if_idle(&self, text: &str) -> bool {
        self.spoken.lock().unwrap().push(text.to_string());
        true
    }
}

fn cfg(post_speech_guard: Duration) -> ConfirmConfig {
    ConfirmConfig {
        quiet_wait: Duration::from_millis(50),
        listen_timeout: Duration::from_millis(1500),
        post_speech_guard,
    }
}

const QUESTION: &str = "I'll open Notes. Say yes or no.";

/// Feeds speech events and levels for about 400 ms, then silence to end the utterance.
fn feed_speech_then_silence(h: &CaptureHandle, start_ms: u64) {
    let mut t = start_ms;
    while t < start_ms + 400 {
        h.push_speech(t);
        h.push_levels(t, LOUD);
        t += 50;
    }
    while t < start_ms + 1200 {
        h.push_levels(t, QUIET);
        t += 50;
    }
}

/// Waits until the recorder has been started, so events are only fed to an armed listen.
async fn wait_recording(port: &Port) {
    let started = Instant::now();
    while !port.active.load(Ordering::SeqCst) {
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "recording never started"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

// ------------------------------------------------------------------ R3-1 (a)-(d)

#[tokio::test]
async fn speech_pushed_before_listen_is_armed_is_ignored_and_yields_no_speech() {
    let (h, port) = spawn_capture("yes");
    // Speech and levels pushed while nothing is armed must be dropped.
    feed_speech_then_silence(&h, 0);
    let ears = CaptureEars {
        handle: h.clone(),
        calls: Mutex::new(Vec::new()),
        silence_ms: 300,
    };
    let mouth = QuietMouth {
        say_done: Mutex::new(None),
    };
    let announcer = Announcer::new(Arc::new(LowPriority::default()));
    let d = confirm_action(
        &ears,
        &mouth,
        &announcer,
        QUESTION,
        &cfg(Duration::from_millis(10)),
    )
    .await;
    assert_eq!(
        d,
        Decision::Declined(DeclineReason::Silence),
        "pre-arm speech must not produce an utterance"
    );
    assert_eq!(
        port.starts.load(Ordering::SeqCst),
        0,
        "no recording may start from stale speech"
    );
    h.shutdown();
}

#[tokio::test]
async fn levels_only_without_speech_events_yield_no_speech_and_silence() {
    let (h, port) = spawn_capture("yes");
    let ears = CaptureEars {
        handle: h.clone(),
        calls: Mutex::new(Vec::new()),
        silence_ms: 300,
    };
    let mouth = QuietMouth {
        say_done: Mutex::new(None),
    };
    let announcer = Announcer::new(Arc::new(LowPriority::default()));
    let h2 = h.clone();
    let feeder = tokio::spawn(async move {
        wait_recording(&port).await;
        // Noise above the level floor but with no VAD speech events.
        let mut t = 0u64;
        while t < 2000 {
            h2.push_levels(t, LOUD);
            t += 50;
        }
    });
    let d = confirm_action(
        &ears,
        &mouth,
        &announcer,
        QUESTION,
        &cfg(Duration::from_millis(10)),
    )
    .await;
    feeder.abort();
    assert_eq!(d, Decision::Declined(DeclineReason::Silence));
    h.shutdown();
}

#[tokio::test]
async fn fresh_speech_after_arming_with_transcript_yes_is_approved() {
    let (h, port) = spawn_capture("yes");
    let ears = CaptureEars {
        handle: h.clone(),
        calls: Mutex::new(Vec::new()),
        silence_ms: 300,
    };
    let mouth = QuietMouth {
        say_done: Mutex::new(None),
    };
    let announcer = Announcer::new(Arc::new(LowPriority::default()));
    let h2 = h.clone();
    let feeder = tokio::spawn(async move {
        wait_recording(&port).await;
        feed_speech_then_silence(&h2, 0);
    });
    let d = confirm_action(
        &ears,
        &mouth,
        &announcer,
        QUESTION,
        &cfg(Duration::from_millis(10)),
    )
    .await;
    feeder.await.unwrap();
    assert_eq!(d, Decision::Approved);
    h.shutdown();
}

#[tokio::test]
async fn yes_whose_speech_was_pushed_only_before_arming_never_approves() {
    let (h, port) = spawn_capture("yes");
    // Pre-arm speech with a "yes" transcript available: must still never approve.
    feed_speech_then_silence(&h, 0);
    let ears = CaptureEars {
        handle: h.clone(),
        calls: Mutex::new(Vec::new()),
        silence_ms: 300,
    };
    let mouth = QuietMouth {
        say_done: Mutex::new(None),
    };
    let announcer = Announcer::new(Arc::new(LowPriority::default()));
    let d = confirm_action(
        &ears,
        &mouth,
        &announcer,
        QUESTION,
        &cfg(Duration::from_millis(10)),
    )
    .await;
    assert_ne!(
        d,
        Decision::Approved,
        "stale pre-arm speech must never approve"
    );
    assert_eq!(port.starts.load(Ordering::SeqCst), 0);
    h.shutdown();
}

// ------------------------------------------------------------------ R4-1 (e)

#[tokio::test]
async fn no_capture_is_armed_before_post_speech_guard_elapses() {
    let guard = Duration::from_millis(200);
    let (h, port) = spawn_capture("yes");
    let ears = CaptureEars {
        handle: h.clone(),
        calls: Mutex::new(Vec::new()),
        silence_ms: 300,
    };
    let mouth = QuietMouth {
        say_done: Mutex::new(None),
    };
    let announcer = Announcer::new(Arc::new(LowPriority::default()));
    let h2 = h.clone();
    let port2 = Arc::clone(&port);
    // Residual playback during the guard must not count as the user's answer.
    let residual = tokio::spawn(async move {
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(120) {
            h2.push_speech(0);
            h2.push_levels(0, LOUD);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            !port2.active.load(Ordering::SeqCst),
            "no recording may be active during the post-speech guard"
        );
    });
    let d = confirm_action(&ears, &mouth, &announcer, QUESTION, &cfg(guard)).await;
    residual.await.unwrap();
    let said_at = mouth.say_done.lock().unwrap().expect("question spoken");
    let armed_at = ears.calls.lock().unwrap()[0];
    let gap = armed_at.duration_since(said_at);
    assert!(
        gap + Duration::from_millis(20) >= guard,
        "listen armed {gap:?} after speech, guard {guard:?}"
    );
    assert_ne!(
        d,
        Decision::Approved,
        "residual playback during the guard must never approve"
    );
    h.shutdown();
}

#[tokio::test]
async fn residual_speech_pushed_during_guard_never_approves_even_with_fresh_speech_later() {
    let guard = Duration::from_millis(200);
    let (h, port) = spawn_capture("yes");
    let ears = CaptureEars {
        handle: h.clone(),
        calls: Mutex::new(Vec::new()),
        silence_ms: 300,
    };
    let mouth = QuietMouth {
        say_done: Mutex::new(None),
    };
    let announcer = Announcer::new(Arc::new(LowPriority::default()));
    let h2 = h.clone();
    let residual = tokio::spawn(async move {
        // Speech events during the guard window, before any arm.
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(120) {
            h2.push_speech(0);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    let feeder = tokio::spawn(async move {
        wait_recording(&port).await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        // Fresh user speech after arming, then silence.
        feed_speech_then_silence(&h, 1000);
    });
    let d = confirm_action(&ears, &mouth, &announcer, QUESTION, &cfg(guard)).await;
    residual.await.unwrap();
    feeder.await.unwrap();
    assert_eq!(
        d,
        Decision::Approved,
        "fresh speech after the guard should be approved"
    );
}

#[tokio::test]
async fn wait_quiet_immediately_quiet_still_waits_full_post_speech_guard_before_listen() {
    let guard = Duration::from_millis(150);
    let (h, _port) = spawn_capture("no");
    let ears = CaptureEars {
        handle: h.clone(),
        calls: Mutex::new(Vec::new()),
        silence_ms: 300,
    };
    let mouth = QuietMouth {
        say_done: Mutex::new(None),
    };
    let announcer = Announcer::new(Arc::new(LowPriority::default()));
    let started = Instant::now();
    let _ = confirm_action(&ears, &mouth, &announcer, QUESTION, &cfg(guard)).await;
    let armed_at = ears.calls.lock().unwrap()[0];
    assert!(
        armed_at.duration_since(started) >= guard,
        "listen armed {:?} after start, guard {guard:?}",
        armed_at.duration_since(started)
    );
    h.shutdown();
}

#[tokio::test]
async fn announce_during_real_capture_confirmation_speaks_nothing_until_released() {
    let (h, _port) = spawn_capture("yes");
    let ears = CaptureEars {
        handle: h.clone(),
        calls: Mutex::new(Vec::new()),
        silence_ms: 300,
    };
    let mouth = QuietMouth {
        say_done: Mutex::new(None),
    };
    let lp = Arc::new(LowPriority::default());
    let announcer = Announcer::new(lp.clone());
    let announce = async {
        tokio::time::sleep(Duration::from_millis(40)).await;
        announcer.announce(
            crate::managers::job_announcer::AnnounceKind::Progress,
            "tick".into(),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            lp.spoken.lock().unwrap().is_empty(),
            "held while confirming"
        );
    };
    let config = cfg(Duration::from_millis(60));
    let (_d, ()) = tokio::join!(
        confirm_action(&ears, &mouth, &announcer, QUESTION, &config),
        announce
    );
    h.shutdown();
}

#[allow(dead_code)]
fn _keep_imports_used(_: CaptureHandle, _: SayOutcome) {
    let _ = Arc::new(AtomicUsize::new(0));
}
