//! Service-backed turn detector and speaker gate (design D3b, D6, A2', A8').
//!
//! Both adapters talk to the single inference owner (`voice_models::VoiceModelService`) through
//! its non-blocking submit/poll/abandon API. They never block past their deadline by more than
//! one poll slice, never panic, and never keep or log the audio or the embedding.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::managers::smart_turn::TurnDetector;
use crate::managers::speaker_id::{
    accept_after, buffer_ms, verify, GateResult, SpeakerGate, Verdict, VoiceprintStore,
    SPEAKER_SPEC, VERIFY_DEADLINE,
};
use crate::managers::voice_models::{ModelKind, Reply, VoiceModelService};
use crate::settings::SpeakerSensitivity;

/// Poll interval while waiting for a turn-detection reply (keeps cancel latency well under 40 ms).
const TURN_POLL_SLICE: Duration = Duration::from_millis(10);
/// Poll interval while waiting for a speaker embedding.
const SPEAKER_POLL_SLICE: Duration = Duration::from_millis(20);

/// Outcome of waiting on one service request.
enum Waited {
    Replied(Reply),
    GaveUp,
}

/// Polls the service for `id` until a reply arrives, `deadline` passes or `cancel` is set.
/// On the last two the request is abandoned (never joined) and `GaveUp` is returned.
fn wait_for_reply(
    service: &VoiceModelService,
    id: u64,
    deadline: Duration,
    cancel: &AtomicBool,
    slice: Duration,
) -> Waited {
    let started = Instant::now();
    loop {
        if let Some(reply) = service.poll(id) {
            return Waited::Replied(reply);
        }
        let elapsed = started.elapsed();
        if cancel.load(Ordering::Relaxed) || elapsed >= deadline {
            service.abandon(id);
            return Waited::GaveUp;
        }
        std::thread::sleep(slice.min(deadline - elapsed));
    }
}

/// A single probability: exactly one finite value, clamped to [0, 1].
fn clamped_probability(values: &[f32]) -> Option<f32> {
    match values {
        [value] if value.is_finite() => Some(value.clamp(0.0, 1.0)),
        _ => None,
    }
}

/// End-of-turn detector backed by the Smart Turn session of the inference service (D3b).
pub struct ServiceTurnDetector {
    service: Arc<VoiceModelService>,
}

impl ServiceTurnDetector {
    pub fn new(service: Arc<VoiceModelService>) -> Self {
        Self { service }
    }
}

impl TurnDetector for ServiceTurnDetector {
    fn done_probability(
        &self,
        tail16k: &[f32],
        deadline: Duration,
        cancel: &AtomicBool,
    ) -> Option<f32> {
        if tail16k.is_empty() {
            return None;
        }
        let id = self
            .service
            .try_submit(ModelKind::SmartTurn, tail16k.to_vec())
            .ok()?;
        match wait_for_reply(&self.service, id, deadline, cancel, TURN_POLL_SLICE) {
            Waited::Replied(Reply::Output(values)) => clamped_probability(&values),
            Waited::Replied(Reply::Failed(_)) | Waited::GaveUp => None,
        }
    }
}

/// Speaker filter backed by the speaker session of the inference service (D6, A8').
///
/// Not authentication: it only decides whether an utterance is accepted for the conversation.
pub struct ServiceSpeakerGate {
    service: Arc<VoiceModelService>,
    store: Arc<VoiceprintStore>,
    sensitivity: Arc<dyn Fn() -> SpeakerSensitivity + Send + Sync>,
    model_sha256: String,
}

impl ServiceSpeakerGate {
    pub fn new(
        service: Arc<VoiceModelService>,
        store: Arc<VoiceprintStore>,
        sensitivity: Arc<dyn Fn() -> SpeakerSensitivity + Send + Sync>,
        model_sha256: String,
    ) -> Self {
        Self {
            service,
            store,
            sensitivity,
            model_sha256,
        }
    }
}

/// A verdict that did not come from an embedding: no timing is reported.
fn untimed(verdict: Verdict) -> GateResult {
    GateResult {
        verdict,
        verify_ms: None,
    }
}

impl SpeakerGate for ServiceSpeakerGate {
    fn check(&self, buffer16k: &[f32], cancel: &AtomicBool) -> GateResult {
        let started = self.store.generation();
        let Some(voiceprint) = self.store.load() else {
            return untimed(Verdict::Accept);
        };
        let duration_ms = buffer_ms(buffer16k.len());
        if duration_ms < SPEAKER_SPEC.min_verify_buffer_ms {
            return untimed(Verdict::SkippedTooShort);
        }
        let Ok(id) = self
            .service
            .try_submit(ModelKind::Speaker, buffer16k.to_vec())
        else {
            return untimed(Verdict::Accept);
        };
        let began = Instant::now();
        let waited = wait_for_reply(
            &self.service,
            id,
            VERIFY_DEADLINE,
            cancel,
            SPEAKER_POLL_SLICE,
        );
        let elapsed_ms = u32::try_from(began.elapsed().as_millis()).unwrap_or(u32::MAX);
        let embedding = match waited {
            Waited::Replied(Reply::Output(embedding)) => embedding,
            Waited::Replied(Reply::Failed(_)) | Waited::GaveUp => return untimed(Verdict::Accept),
        };
        let sensitivity = (self.sensitivity)();
        let verdict = verify(
            &voiceprint,
            &embedding,
            sensitivity,
            duration_ms,
            &self.model_sha256,
        );
        // A Reject survives only while the voiceprint it was judged against is still the
        // enrolled one; `accept_after` is true (=> Accept) when that is no longer so.
        let final_verdict = if accept_after(started, &self.store, verdict) {
            Verdict::Accept
        } else {
            verdict
        };
        GateResult {
            verdict: final_verdict,
            verify_ms: Some(elapsed_ms),
        }
    }
}

#[cfg(test)]
#[path = "voice_adapters_tests.rs"]
mod tests;
