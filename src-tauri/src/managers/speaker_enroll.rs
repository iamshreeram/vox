//! Speaker enrollment controller (design A3, A3', A8', A9). Contract stubs written in step T10;
//! every method body is `todo!()` until the implementation step. Pinned by
//! `speaker_enroll_tests.rs`.
//!
//! Lifecycle: `begin` takes the `Owner::Enrollment` lease from the real `OwnershipGate` and keeps
//! it until `finish`, `cancel`, idle expiry or drop. Each `step` records one fixed window through
//! an `EnrollRecorder` (production: the `speaker_enroll` binding of the atomic
//! `AudioRecordingManager::try_start_recording` admission) and keeps only the in-memory embedding.
//! Nothing reaches the `VoiceprintStore` before `finish`.

#![allow(dead_code)]

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::managers::ownership::{Lease, OwnershipGate};
use crate::managers::speaker_id::VoiceprintStore;

/// The recorder already holds a recording (another owner's or this one's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecorderBusy;

/// One fixed-length enrollment recording window. `start` and `stop` are called exactly once per
/// successful step; `stop` is never called after a failed `start`.
pub trait EnrollRecorder: Send + Sync {
    /// Starts a recording. `Err(RecorderBusy)` when the admission slot is already held.
    fn start(&self) -> Result<(), RecorderBusy>;
    /// Stops this enrollment's recording and returns its VAD-kept 16 kHz mono samples.
    fn stop(&self) -> Vec<f32>;
}

/// Turns one VAD-kept 16 kHz buffer into a speaker embedding (256-dim). `None` when the model
/// fails, the output is non-finite, or the input is too short. Must honour `cancel`.
pub trait EnrollEmbedder: Send + Sync {
    fn embed(&self, buffer16k: &[f32], cancel: &AtomicBool) -> Option<Vec<f32>>;
}

/// Monotonic millisecond clock plus a blocking sleep. Tests drive a virtual clock.
pub trait EnrollClock: Send + Sync {
    fn now_ms(&self) -> u64;
    fn sleep_ms(&self, ms: u64);
}

/// Enrollment timing and step parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnrollConfig {
    /// Fixed recording window per step, on the controller clock.
    pub window_ms: u64,
    /// Minimum VAD-kept speech per step (`buffer_ms` of the buffer is the speech estimate).
    pub min_speech_ms: u32,
    /// Idle time after which `expire_if_idle` cancels the session.
    pub idle_expiry_ms: u64,
    /// Number of ok steps `finish` requires.
    pub steps: u8,
}

impl Default for EnrollConfig {
    fn default() -> Self {
        EnrollConfig {
            window_ms: 6000,
            min_speech_ms: 2000,
            idle_expiry_ms: 120_000,
            steps: 3,
        }
    }
}

/// Enrollment failures. Serialized to the UI as a bare variant name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
pub enum EnrollError {
    Busy,
    StaleSession,
    NoActiveSession,
    BadStep,
    TooLittleSpeech,
    EmbedFailed,
    NotEnoughSteps,
    Inconsistent,
    UnsupportedPlatform,
    StoreFailed,
    Cancelled,
}

/// Result of one counted step. Numbers and booleans only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
pub struct StepResult {
    pub speech_ms: u32,
    pub ok: bool,
}

/// Per-session state kept under the controller mutex. The `lease` is the Enrollment lease; it
/// releases the gate when the session state is dropped (finish, cancel, expiry or controller
/// drop, including unwinding).
struct SessionState {
    id: u64,
    lease: Lease,
    next_step: u8,
    embeddings: Vec<Vec<f32>>,
    last_activity_ms: u64,
}

pub struct EnrollmentController {
    gate: Arc<OwnershipGate>,
    recorder: Arc<dyn EnrollRecorder>,
    embedder: Arc<dyn EnrollEmbedder>,
    store: Arc<VoiceprintStore>,
    clock: Arc<dyn EnrollClock>,
    cfg: EnrollConfig,
    state: Mutex<Option<SessionState>>,
    next_id: std::sync::atomic::AtomicU64,
}

impl EnrollmentController {
    pub fn new(
        gate: Arc<OwnershipGate>,
        recorder: Arc<dyn EnrollRecorder>,
        embedder: Arc<dyn EnrollEmbedder>,
        store: Arc<VoiceprintStore>,
        clock: Arc<dyn EnrollClock>,
        cfg: EnrollConfig,
    ) -> Self {
        let _ = (gate, recorder, embedder, store, clock, cfg);
        todo!("T10 stub: EnrollmentController::new")
    }

    /// Acquires the Enrollment lease and opens a session. `Busy` when any owner holds the gate or
    /// a session exists. `UnsupportedPlatform` when owner-private files cannot be guaranteed
    /// (checked before the lease is taken). Returns the new session id.
    pub fn begin(&self) -> Result<u64, EnrollError> {
        todo!("T10 stub: EnrollmentController::begin")
    }

    /// Test seam for `begin`: the platform check is passed in so tests can simulate a host
    /// without owner-private files (`private_files_supported == false`). Production `begin` must
    /// pass `cfg!(unix)`. Same contract as `begin`.
    #[cfg(test)]
    pub(crate) fn begin_with_platform(
        &self,
        private_files_supported: bool,
    ) -> Result<u64, EnrollError> {
        let _ = private_files_supported;
        todo!("T10 stub: EnrollmentController::begin_with_platform")
    }

    /// Records one window for `step` (must equal the next expected step: 0, then 1, then 2).
    /// Starts the recorder, sleeps exactly `window_ms` on the clock, stops, embeds in memory.
    /// Fewer than `min_speech_ms` of speech => `TooLittleSpeech` (not counted, retry allowed).
    /// Recorder busy => `Busy` (session kept). Embedder `None` => `EmbedFailed` (not counted).
    pub fn step(&self, session_id: u64, step: u8) -> Result<StepResult, EnrollError> {
        let _ = (session_id, step);
        todo!("T10 stub: EnrollmentController::step")
    }

    /// Cancels the session: stops only its own recording (if one is running), clears the
    /// in-memory embeddings, drops the lease. Never touches the store.
    pub fn cancel(&self, session_id: u64) -> Result<(), EnrollError> {
        let _ = session_id;
        todo!("T10 stub: EnrollmentController::cancel")
    }

    /// Needs `steps` ok steps (else `NotEnoughSteps`). Calls `speaker_id::enroll`; `Inconsistent`
    /// keeps the session open for a retry of the steps. On success saves via the store and drops
    /// the lease.
    pub fn finish(&self, session_id: u64) -> Result<(), EnrollError> {
        let _ = session_id;
        todo!("T10 stub: EnrollmentController::finish")
    }

    /// Cancels the session when idle for at least `idle_expiry_ms` on the clock.
    pub fn expire_if_idle(&self) {
        todo!("T10 stub: EnrollmentController::expire_if_idle")
    }

    /// True while a session (and its lease) exists.
    pub fn has_session(&self) -> bool {
        todo!("T10 stub: EnrollmentController::has_session")
    }
}

#[cfg(test)]
#[path = "speaker_enroll_tests.rs"]
mod tests;
