//! Smart Turn end-of-turn detector seam (design D3b). The pure policy lives in
//! `converse_audio::Endpointer`; the model runs on the single inference thread of
//! `voice_models::VoiceModelService`; this file holds the trait the capture worker uses and the
//! constants both sides share. The service-backed adapter is added by step S8b.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// A probability at or above this value means "the speaker finished their turn".
pub const DONE_THRESHOLD: f32 = 0.5;

/// How long the capture worker waits for the model before it falls back to the fixed
/// silence endpoint (measured inference on this model: about 50 ms).
pub const DECISION_DEADLINE: Duration = Duration::from_millis(250);

/// Length of the pre-VAD audio ring (8 s at 16 kHz); the model takes at most this much.
pub const RING_SAMPLES: usize = 128_000;

/// Decides whether the audio heard so far is a finished turn.
pub trait TurnDetector: Send + Sync {
    /// `tail16k` is the most recent audio (at most `RING_SAMPLES` samples of 16 kHz mono,
    /// the current pause included). Blocks at most `deadline`; returns the probability in
    /// `[0, 1]` that the turn is complete, or `None` on timeout, cancellation, an
    /// unavailable model or any failure. Must never panic and never keep the audio.
    fn done_probability(
        &self,
        tail16k: &[f32],
        deadline: Duration,
        cancel: &AtomicBool,
    ) -> Option<f32>;
}
