//! Speaker enrollment controller (design A3, A3', A8', A9). Pinned by `speaker_enroll_tests.rs`.
//!
//! Lifecycle: `begin` takes the `Owner::Enrollment` lease from the real `OwnershipGate` and keeps
//! it until `finish`, `cancel`, idle expiry or drop. Each `step` records one fixed window through
//! an `EnrollRecorder` (production: the `speaker_enroll` binding of the atomic
//! `AudioRecordingManager::try_start_recording` admission) and keeps only the in-memory embedding.
//! Nothing reaches the `VoiceprintStore` before `finish`.
//!
//! Locking: the session mutex is never held across a recording window or an embedding. A step
//! marks the session `stepping`; `cancel` during a step only flags it, and the step itself stops
//! its recording and releases the lease when it settles.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;

use crate::managers::ownership::{Lease, Owner, OwnershipGate};
use crate::managers::speaker_id::{self, VoiceprintStore};

/// The recorder already holds a recording (another owner's or this one's). Also used for a
/// microphone-open failure: the admission API reports every start failure as the same busy
/// signal, so the controller cannot tell them apart.
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
    /// A step is in flight: it owns the recorder and will settle the session when it ends.
    stepping: bool,
    /// Cancelled while a step was in flight; the step drops the session when it settles.
    cancelled: bool,
    /// Handed to the embedder so an in-flight embedding stops early on cancel.
    cancel: Arc<AtomicBool>,
}

pub struct EnrollmentController {
    gate: Arc<OwnershipGate>,
    recorder: Arc<dyn EnrollRecorder>,
    embedder: Arc<dyn EnrollEmbedder>,
    store: Arc<VoiceprintStore>,
    clock: Arc<dyn EnrollClock>,
    cfg: EnrollConfig,
    state: Mutex<Option<SessionState>>,
    next_id: AtomicU64,
}

/// Locks the session slot, recovering the inner value after a panic elsewhere. The guarded
/// value is plain data and stays consistent (every write is a whole-field assignment).
fn lock_session(mutex: &Mutex<Option<SessionState>>) -> MutexGuard<'_, Option<SessionState>> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
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
        EnrollmentController {
            gate,
            recorder,
            embedder,
            store,
            clock,
            cfg,
            state: Mutex::new(None),
            next_id: AtomicU64::new(0),
        }
    }

    /// Acquires the Enrollment lease and opens a session. `Busy` when any owner holds the gate or
    /// a session exists. `UnsupportedPlatform` when owner-private files cannot be guaranteed
    /// (checked before the lease is taken). Returns the new session id.
    pub fn begin(&self) -> Result<u64, EnrollError> {
        self.begin_with_platform(cfg!(unix))
    }

    /// Same contract as `begin`, with the platform check passed in so tests can simulate a host
    /// without owner-private files (`private_files_supported == false`). Production `begin`
    /// passes `cfg!(unix)`.
    pub(crate) fn begin_with_platform(
        &self,
        private_files_supported: bool,
    ) -> Result<u64, EnrollError> {
        if !private_files_supported {
            return Err(EnrollError::UnsupportedPlatform);
        }
        let mut guard = lock_session(&self.state);
        if guard.is_some() {
            return Err(EnrollError::Busy);
        }
        let lease = self
            .gate
            .try_acquire(Owner::Enrollment)
            .map_err(|_| EnrollError::Busy)?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        *guard = Some(SessionState {
            id,
            lease,
            next_step: 0,
            embeddings: Vec::new(),
            last_activity_ms: self.clock.now_ms(),
            stepping: false,
            cancelled: false,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        Ok(id)
    }

    /// Records one window for `step` (must equal the next expected step: 0, then 1, then 2).
    /// Starts the recorder, sleeps exactly `window_ms` on the clock, stops, embeds in memory.
    /// Fewer than `min_speech_ms` of speech => `TooLittleSpeech` (not counted, retry allowed).
    /// Recorder busy => `Busy` (session kept). Embedder `None` => `EmbedFailed` (not counted).
    pub fn step(&self, session_id: u64, step: u8) -> Result<StepResult, EnrollError> {
        let cancel = self.claim_step(session_id, step)?;
        if self.recorder.start().is_err() {
            return self.settle(session_id, Err(EnrollError::Busy));
        }
        // The window is measured around this sleep only; the recorder's own stop work is not
        // part of it.
        self.clock.sleep_ms(self.cfg.window_ms);
        let buffer = self.recorder.stop();
        let speech_ms = speaker_id::buffer_ms(buffer.len());
        let outcome = if speech_ms < self.cfg.min_speech_ms {
            Err(EnrollError::TooLittleSpeech)
        } else {
            self.embedder
                .embed(&buffer, &cancel)
                .ok_or(EnrollError::EmbedFailed)
                .map(|embedding| (speech_ms, embedding))
        };
        drop(buffer);
        self.settle(session_id, outcome)
    }

    /// Cancels the session: clears the in-memory embeddings and drops the lease. A step in
    /// flight is flagged instead and settles itself (it owns its recording, so cancel never
    /// stops one). Never touches the store.
    pub fn cancel(&self, session_id: u64) -> Result<(), EnrollError> {
        let mut guard = lock_session(&self.state);
        let session = guard
            .as_mut()
            .filter(|s| s.id == session_id)
            .ok_or(EnrollError::StaleSession)?;
        if session.stepping {
            session.cancelled = true;
            session.cancel.store(true, Ordering::SeqCst);
            session.embeddings.clear();
            return Ok(());
        }
        *guard = None;
        Ok(())
    }

    /// Needs `steps` ok steps (else `NotEnoughSteps`). Calls `speaker_id::enroll`; `Inconsistent`
    /// keeps the session open for a retry of the steps. On success saves via the store and drops
    /// the lease.
    pub fn finish(&self, session_id: u64) -> Result<(), EnrollError> {
        let mut guard = lock_session(&self.state);
        let session = guard
            .as_mut()
            .filter(|s| s.id == session_id)
            .ok_or(EnrollError::StaleSession)?;
        if session.stepping {
            return Err(EnrollError::Busy);
        }
        if session.next_step < self.cfg.steps {
            return Err(EnrollError::NotEnoughSteps);
        }
        let voiceprint =
            match speaker_id::enroll(&session.embeddings, speaker_id::SPEAKER_SPEC.sha256) {
                Ok(voiceprint) => voiceprint,
                Err(speaker_id::EnrollError::TooFewClips) => {
                    return Err(EnrollError::NotEnoughSteps)
                }
                Err(err) => {
                    // The enrollment is unusable: discard its embeddings and restart the steps.
                    session.embeddings.clear();
                    session.next_step = 0;
                    return Err(match err {
                        speaker_id::EnrollError::Inconsistent => EnrollError::Inconsistent,
                        _ => EnrollError::EmbedFailed,
                    });
                }
            };
        match self.store.save(&voiceprint) {
            Ok(()) => {
                *guard = None;
                Ok(())
            }
            Err(speaker_id::StoreError::UnsupportedPlatform) => {
                Err(EnrollError::UnsupportedPlatform)
            }
            Err(_) => Err(EnrollError::StoreFailed),
        }
    }

    /// Cancels the session when idle for at least `idle_expiry_ms` on the clock. A session with
    /// a step in flight is not idle.
    pub fn expire_if_idle(&self) {
        let now = self.clock.now_ms();
        let mut guard = lock_session(&self.state);
        let expired = match guard.as_ref() {
            Some(session) => {
                !session.stepping
                    && now.saturating_sub(session.last_activity_ms) >= self.cfg.idle_expiry_ms
            }
            None => false,
        };
        if expired {
            *guard = None;
        }
    }

    /// True while a session (and its lease) exists.
    pub fn has_session(&self) -> bool {
        lock_session(&self.state).is_some()
    }

    /// Validates the step and marks the session as stepping. Returns the cancel flag for the
    /// embedder. Checks run in order: session id, no step in flight, expected step number.
    fn claim_step(&self, session_id: u64, step: u8) -> Result<Arc<AtomicBool>, EnrollError> {
        let now = self.clock.now_ms();
        let mut guard = lock_session(&self.state);
        let session = guard
            .as_mut()
            .filter(|s| s.id == session_id)
            .ok_or(EnrollError::StaleSession)?;
        if session.stepping {
            return Err(EnrollError::Busy);
        }
        if step != session.next_step || session.next_step >= self.cfg.steps {
            return Err(EnrollError::BadStep);
        }
        session.stepping = true;
        session.last_activity_ms = now;
        Ok(Arc::clone(&session.cancel))
    }

    /// Ends a step: clears `stepping`, then either drops the session (cancelled meanwhile) or
    /// counts the embedded window. `outcome` carries `(speech_ms, embedding)` on success.
    fn settle(
        &self,
        session_id: u64,
        outcome: Result<(u32, Vec<f32>), EnrollError>,
    ) -> Result<StepResult, EnrollError> {
        let now = self.clock.now_ms();
        let mut guard = lock_session(&self.state);
        let session = guard
            .as_mut()
            .filter(|s| s.id == session_id)
            .ok_or(EnrollError::StaleSession)?;
        session.stepping = false;
        session.last_activity_ms = now;
        if session.cancelled {
            *guard = None;
            return Err(EnrollError::Cancelled);
        }
        let (speech_ms, embedding) = outcome?;
        session.embeddings.push(embedding);
        session.next_step = session.next_step.saturating_add(1);
        Ok(StepResult {
            speech_ms,
            ok: true,
        })
    }
}

#[cfg(test)]
#[path = "speaker_enroll_tests.rs"]
mod tests;
