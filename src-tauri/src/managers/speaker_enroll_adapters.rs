//! Production adapters for the speaker enrollment controller (step I10): the `speaker_enroll`
//! binding of the shared recorder, the inference-service embedder, and the system clock.
//! Audio and embeddings are kept only for the call that uses them; nothing here logs them.
//!
//! The recorder cannot be unit-tested without a microphone: its pure helpers are tested, the
//! `AudioRecordingManager` admission itself is covered by manual hardware checks only.

// Constructors are wired by the commands step; until then nothing outside tests builds them.
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::audio_toolkit::VadPolicy;
use crate::managers::audio::{AudioRecordingManager, RecordingReadiness};
use crate::managers::speaker_enroll::{EnrollClock, EnrollEmbedder, EnrollRecorder, RecorderBusy};
use crate::managers::speaker_id::SPEAKER_SPEC;
use crate::managers::voice_models::{ModelKind, Reply, VoiceModelService};

/// Admission binding id of enrollment recordings (distinct from dictation and conversation).
pub const ENROLL_BINDING: &str = "speaker_enroll";
/// Speech policy of enrollment windows. MUST stay equal to the policy the conversation capture
/// path uses: `converse_platform::conversation_vad_policy` (private there, always `Offline`).
/// Not pinned by a test: that function is private and converse_platform.rs is out of scope.
const ENROLL_VAD_POLICY: VadPolicy = VadPolicy::Offline;
/// Longest wait for one speaker embedding before the request is abandoned.
const EMBED_DEADLINE: Duration = Duration::from_secs(3);
/// Poll interval while waiting for an embedding (keeps cancel latency under 20 ms).
const EMBED_POLL_SLICE: Duration = Duration::from_millis(20);
/// Longest wait for the recorder to report first samples. A capture that delivers nothing in
/// this time is released, so `step` never blocks on a dead microphone.
pub(crate) const READINESS_TIMEOUT: Duration = Duration::from_secs(3);
/// Poll interval while waiting for readiness; an abort is honoured within one slice.
pub(crate) const READINESS_POLL_SLICE: Duration = Duration::from_millis(20);

fn lock_generation(slot: &Mutex<Option<u64>>) -> MutexGuard<'_, Option<u64>> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Every admission failure (busy slot, microphone open failure, missing recorder) is reported
/// as `RecorderBusy`: the controller cannot tell them apart and needs no more detail.
fn map_admission_error<E>(_error: E) -> RecorderBusy {
    RecorderBusy
}

/// `true` when the first samples arrived. A readiness channel that closed without samples means
/// the recording died before it produced audio.
fn first_sample_or_busy(ready: bool) -> Result<(), RecorderBusy> {
    if ready {
        Ok(())
    } else {
        Err(RecorderBusy)
    }
}

/// Takes the cancel generation captured by the last successful start. `None` when no start
/// succeeded (or it was already stopped), so `stop` never acts without a matching start.
fn take_generation(slot: &Mutex<Option<u64>>) -> Option<u64> {
    lock_generation(slot).take()
}

/// Recording samples, or an empty buffer when there was nothing to stop.
fn samples_or_empty(kept: Option<Vec<f32>>) -> Vec<f32> {
    kept.unwrap_or_default()
}

/// Accepts only a reply with the spec dimension and only finite values.
fn accept_embedding(reply: Option<Reply>, dim: usize) -> Option<Vec<f32>> {
    match reply {
        Some(Reply::Output(values))
            if values.len() == dim && values.iter().all(|x| x.is_finite()) =>
        {
            Some(values)
        }
        _ => None,
    }
}

/// Polls the service for `id` until a reply arrives, `deadline` passes or `cancel` is set. On
/// the last two the request is abandoned (never joined) and `None` is returned.
fn await_reply(
    service: &VoiceModelService,
    id: u64,
    deadline: Duration,
    cancel: &AtomicBool,
) -> Option<Reply> {
    let started = Instant::now();
    loop {
        if let Some(reply) = service.poll(id) {
            return Some(reply);
        }
        let elapsed = started.elapsed();
        if cancel.load(Ordering::Relaxed) || elapsed >= deadline {
            service.abandon(id);
            return None;
        }
        std::thread::sleep(EMBED_POLL_SLICE.min(deadline - elapsed));
    }
}

/// Outcome of waiting for the first samples of an admitted recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadinessOutcome {
    /// First samples arrived in time.
    Ready,
    /// The capture ended without delivering samples.
    Failed,
    /// No samples before the deadline.
    TimedOut,
    /// The controller aborted the pending start.
    Aborted,
}

/// Decision logic of the bounded readiness wait, free of threads and real time. `poll` returns
/// `None` while the helper has not reported; a report wins over the deadline. `aborted` is
/// checked every slice, so an abort is seen within one `slice`.
pub(crate) fn await_readiness(
    mut poll: impl FnMut() -> Option<bool>,
    aborted: impl Fn() -> bool,
    elapsed: impl Fn() -> Duration,
    mut pause: impl FnMut(Duration),
    timeout: Duration,
    slice: Duration,
) -> ReadinessOutcome {
    loop {
        if let Some(ready) = poll() {
            return if ready {
                ReadinessOutcome::Ready
            } else {
                ReadinessOutcome::Failed
            };
        }
        if aborted() {
            return ReadinessOutcome::Aborted;
        }
        let waited = elapsed();
        if waited >= timeout {
            return ReadinessOutcome::TimedOut;
        }
        pause(slice.min(timeout - waited));
    }
}

/// Admitted-start orchestration. `wait` (the manager's blocking readiness wait) runs on a
/// short-lived helper thread; this thread polls its report with a bounded deadline and the
/// abort flag. Any outcome other than `Ready` calls `release` exactly once, which must end the
/// admission so that a blocked `wait` returns. A late report from the helper is discarded.
pub(crate) fn start_with_readiness(
    wait: impl FnOnce() -> bool + Send + 'static,
    aborted: impl Fn() -> bool,
    release: impl FnOnce(),
    timeout: Duration,
    slice: Duration,
) -> Result<(), RecorderBusy> {
    let (report_tx, report_rx) = mpsc::channel::<bool>();
    let spawned = thread::Builder::new()
        .name("speaker-enroll-readiness".to_owned())
        .spawn(move || {
            let ready = wait();
            let _ = report_tx.send(ready);
        });
    if spawned.is_err() {
        release();
        return Err(RecorderBusy);
    }
    let started = Instant::now();
    let outcome = await_readiness(
        || match report_rx.try_recv() {
            Ok(ready) => Some(ready),
            Err(mpsc::TryRecvError::Empty) => None,
            // The helper ended without reporting: it panicked, so the capture is unusable.
            Err(mpsc::TryRecvError::Disconnected) => Some(false),
        },
        aborted,
        || started.elapsed(),
        thread::sleep,
        timeout,
        slice,
    );
    match outcome {
        ReadinessOutcome::Ready => Ok(()),
        ReadinessOutcome::Failed | ReadinessOutcome::TimedOut | ReadinessOutcome::Aborted => {
            release();
            Err(RecorderBusy)
        }
    }
}

/// Releases this enrollment's admission after a start that never became ready. Cancels only
/// the `speaker_enroll` binding (never another owner's recording); the cancel drops the
/// capture's readiness sender, so a blocked readiness wait returns.
fn release_admission(manager: &AudioRecordingManager, captured: &Mutex<Option<u64>>) {
    let _ = take_generation(captured);
    let _ = manager.cancel_recording_if_binding(ENROLL_BINDING);
}

/// Recorder over the shared `AudioRecordingManager`, always under binding `speaker_enroll`.
pub struct ManagerEnrollRecorder {
    manager: Arc<AudioRecordingManager>,
    /// Cancel generation captured by the last successful start; consumed by `stop`.
    captured: Mutex<Option<u64>>,
    /// Set by `abort_start` while a start waits for readiness.
    aborted: AtomicBool,
}

impl ManagerEnrollRecorder {
    pub fn new(manager: Arc<AudioRecordingManager>) -> Self {
        Self {
            manager,
            captured: Mutex::new(None),
            aborted: AtomicBool::new(false),
        }
    }
}

impl EnrollRecorder for ManagerEnrollRecorder {
    fn start(&self) -> Result<(), RecorderBusy> {
        // A stale abort from an earlier step must not cancel this start.
        self.aborted.store(false, Ordering::SeqCst);
        let generation = self.manager.cancel_generation();
        let readiness: RecordingReadiness = self
            .manager
            .try_start_recording(ENROLL_BINDING, ENROLL_VAD_POLICY)
            .map_err(map_admission_error)?;
        // The admission succeeded: a later `stop` must match this generation.
        *lock_generation(&self.captured) = Some(generation);
        start_with_readiness(
            move || readiness.wait(),
            || self.aborted.load(Ordering::SeqCst),
            || release_admission(&self.manager, &self.captured),
            READINESS_TIMEOUT,
            READINESS_POLL_SLICE,
        )
    }

    fn abort_start(&self) {
        self.aborted.store(true, Ordering::SeqCst);
    }

    fn stop(&self) -> Vec<f32> {
        match take_generation(&self.captured) {
            Some(generation) => {
                samples_or_empty(self.manager.stop_recording(ENROLL_BINDING, generation))
            }
            None => Vec::new(),
        }
    }
}

/// Embedder over the speaker session of the inference service.
pub struct ServiceEnrollEmbedder {
    service: Arc<VoiceModelService>,
}

impl ServiceEnrollEmbedder {
    pub fn new(service: Arc<VoiceModelService>) -> Self {
        Self { service }
    }
}

impl EnrollEmbedder for ServiceEnrollEmbedder {
    fn embed(&self, buffer16k: &[f32], cancel: &AtomicBool) -> Option<Vec<f32>> {
        if buffer16k.is_empty() || cancel.load(Ordering::Relaxed) {
            return None;
        }
        let id = self
            .service
            .try_submit(ModelKind::Speaker, buffer16k.to_vec())
            .ok()?;
        let reply = await_reply(&self.service, id, EMBED_DEADLINE, cancel);
        accept_embedding(reply, SPEAKER_SPEC.dim)
    }
}

/// Monotonic clock on `std::time::Instant` with a blocking `std::thread::sleep`.
pub struct SystemEnrollClock {
    origin: Instant,
}

impl SystemEnrollClock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemEnrollClock {
    fn default() -> Self {
        Self::new()
    }
}

impl EnrollClock for SystemEnrollClock {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

#[cfg(test)]
#[path = "speaker_enroll_adapters_tests.rs"]
mod tests;
