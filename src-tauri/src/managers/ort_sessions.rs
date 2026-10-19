//! ONNX sessions for the optional Smart Turn and speaker models (design A2', D3b, A10).
//!
//! `OrtLoader` builds the sessions that `VoiceModelService` owns on its single inference thread.
//! Every session is CPU-only, single-threaded (intra- and inter-op), and reads the model file
//! only from the path it is given. Load failures return `None`; nothing here panics.
//!
//! Verified I/O (from the pinned files, see the S8b notes):
//! - Smart Turn v3.2: input `input_features` [1, 80, 800] f32, output `logits` [1, 1] f32.
//! - WeSpeaker ResNet34: input `feats` [1, T, 80] f32, output `embs` [1, 256] f32.

use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;

use crate::audio_toolkit::dsp::kaldi_fbank::{wespeaker_features, FBANK_BINS};
use crate::audio_toolkit::dsp::whisper_mel::{
    smart_turn_features, SMART_TURN_FRAMES, SMART_TURN_MELS,
};
use crate::managers::speaker_id::SPEAKER_SPEC;
use crate::managers::voice_models::{ModelKind, ModelSession, SessionLoader};

const SMART_TURN_INPUT: &str = "input_features";
const SMART_TURN_OUTPUT: &str = "logits";
const SPEAKER_INPUT: &str = "feats";
const SPEAKER_OUTPUT: &str = "embs";
/// Fbank analysis window and frame shift in samples (25 ms / 10 ms at 16 kHz).
const FRAME_LEN_SAMPLES: usize = 400;
const FRAME_SHIFT_SAMPLES: usize = 160;
/// Frames the fbank produces for the minimum speaker clip (snip-edges count: 1 s => 98 frames).
const MIN_FRAMES: usize =
    (SPEAKER_SPEC.min_embed_samples - FRAME_LEN_SAMPLES) / FRAME_SHIFT_SAMPLES + 1;

/// Maps a raw Smart Turn output to a probability. A value already in [0, 1] is kept; any other
/// finite value goes through the logistic function; the result is clamped. Non-finite => `None`.
pub fn normalize_probability(raw: f32) -> Option<f32> {
    if !raw.is_finite() {
        return None;
    }
    let p = if (0.0..=1.0).contains(&raw) {
        raw
    } else {
        1.0 / (1.0 + (-raw).exp())
    };
    Some(p.clamp(0.0, 1.0))
}

/// Builds the sessions for the inference service. Stateless: each `load` is independent.
pub struct OrtLoader;

impl SessionLoader for OrtLoader {
    fn load(&self, kind: ModelKind, path: &Path) -> Option<Box<dyn ModelSession>> {
        // Telemetry is an environment-wide switch; the first committed environment wins and
        // later commits are no-ops.
        let _first_commit_wins = ort::environment::init().with_telemetry(false).commit();
        let session = build_session(path)?;
        let boxed: Box<dyn ModelSession> = match kind {
            ModelKind::SmartTurn => Box::new(SmartTurnSession { session }),
            ModelKind::Speaker => Box::new(SpeakerSession { session }),
        };
        Some(boxed)
    }
}

/// CPU provider only, one intra-op and one inter-op thread.
fn build_session(path: &Path) -> Option<Session> {
    let mut builder = Session::builder()
        .ok()?
        .with_execution_providers([ort::ep::CPU::default().build()])
        .ok()?
        .with_inter_threads(1)
        .ok()?
        .with_intra_threads(1)
        .ok()?;
    builder.commit_from_file(path).ok()
}

struct SmartTurnSession {
    session: Session,
}

impl ModelSession for SmartTurnSession {
    fn run(&mut self, audio16k: &[f32]) -> Option<Vec<f32>> {
        let features = smart_turn_features(audio16k);
        if features.len() != SMART_TURN_MELS * SMART_TURN_FRAMES {
            return None;
        }
        let shape = [1_usize, SMART_TURN_MELS, SMART_TURN_FRAMES];
        let input = Tensor::<f32>::from_array((shape, features)).ok()?;
        let outputs = self
            .session
            .run(ort::inputs![SMART_TURN_INPUT => input])
            .ok()?;
        let (_, values) = outputs
            .get(SMART_TURN_OUTPUT)?
            .try_extract_tensor::<f32>()
            .ok()?;
        match values {
            [raw] => normalize_probability(*raw).map(|p| vec![p]),
            _ => None,
        }
    }
}

struct SpeakerSession {
    session: Session,
}

impl ModelSession for SpeakerSession {
    fn run(&mut self, audio16k: &[f32]) -> Option<Vec<f32>> {
        if audio16k.len() < SPEAKER_SPEC.min_embed_samples {
            return None;
        }
        let fbank = wespeaker_features(audio16k);
        if fbank.frames < MIN_FRAMES || fbank.data.len() != fbank.frames * FBANK_BINS {
            return None;
        }
        let shape = [1_usize, fbank.frames, FBANK_BINS];
        let input = Tensor::<f32>::from_array((shape, fbank.data)).ok()?;
        let outputs = self
            .session
            .run(ort::inputs![SPEAKER_INPUT => input])
            .ok()?;
        let (_, values) = outputs
            .get(SPEAKER_OUTPUT)?
            .try_extract_tensor::<f32>()
            .ok()?;
        if values.len() != SPEAKER_SPEC.dim || values.iter().any(|v| !v.is_finite()) {
            return None;
        }
        Some(values.to_vec())
    }
}

#[cfg(test)]
#[path = "ort_sessions_tests.rs"]
mod tests;
