//! Production adapters for the speaker enrollment controller (step I10): the `speaker_enroll`
//! binding of the shared recorder, the inference-service embedder, and the system clock.
//! Audio and embeddings are kept only for the call that uses them; nothing here logs them.
//!
//! The recorder cannot be unit-tested without a microphone: its pure helpers are tested, the
//! `AudioRecordingManager` admission itself is covered by manual hardware checks only.

// Constructors are wired by the commands step; until then nothing outside tests builds them.
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::audio_toolkit::VadPolicy;
use crate::managers::audio::{AudioRecordingManager, RecordingReadiness};
use crate::managers::speaker_enroll::{EnrollClock, EnrollEmbedder, EnrollRecorder, RecorderBusy};
use crate::managers::speaker_id::SPEAKER_SPEC;
use crate::managers::voice_models::{ModelKind, Reply, VoiceModelService};

/// Admission binding id of enrollment recordings (distinct from dictation and conversation).
pub const ENROLL_BINDING: &str = "speaker_enroll";
/// Speech policy of enrollment windows: the policy the conversation capture path uses
/// (`converse_platform::conversation_vad_policy`, which is private there and always `Offline`).
const ENROLL_VAD_POLICY: VadPolicy = VadPolicy::Offline;
/// Longest wait for one speaker embedding before the request is abandoned.
const EMBED_DEADLINE: Duration = Duration::from_secs(3);
/// Poll interval while waiting for an embedding (keeps cancel latency under 20 ms).
const EMBED_POLL_SLICE: Duration = Duration::from_millis(20);

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

/// Recorder over the shared `AudioRecordingManager`, always under binding `speaker_enroll`.
pub struct ManagerEnrollRecorder {
    manager: Arc<AudioRecordingManager>,
    /// Cancel generation captured by the last successful start; consumed by `stop`.
    captured: Mutex<Option<u64>>,
}

impl ManagerEnrollRecorder {
    pub fn new(manager: Arc<AudioRecordingManager>) -> Self {
        Self {
            manager,
            captured: Mutex::new(None),
        }
    }
}

impl EnrollRecorder for ManagerEnrollRecorder {
    fn start(&self) -> Result<(), RecorderBusy> {
        let generation = self.manager.cancel_generation();
        let readiness: RecordingReadiness = self
            .manager
            .try_start_recording(ENROLL_BINDING, ENROLL_VAD_POLICY)
            .map_err(map_admission_error)?;
        // The admission succeeded: a later `stop` must match this generation.
        *lock_generation(&self.captured) = Some(generation);
        if let Err(busy) = first_sample_or_busy(readiness.wait()) {
            // Admitted but the capture died before any sample. Discard the recording so the
            // admission slot is released; this is not a stop of a failed start.
            let _ = take_generation(&self.captured);
            let _ = self.manager.stop_recording(ENROLL_BINDING, generation);
            return Err(busy);
        }
        Ok(())
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
