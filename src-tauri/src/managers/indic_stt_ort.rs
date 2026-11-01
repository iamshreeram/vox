//! ONNX Runtime backend for the Tamil recognition pack (IndicConformer encoder + CTC head).
//!
//! [`OrtIndicBackend`] implements [`IndicBackend`] on two sessions of the pack:
//! `encoder.onnx` (`audio_signal` [1,80,T] f32 and `length` [1] i64 -> `outputs`
//! [1,D,T'] and `encoded_lengths` [1] i64) and `ctc_decoder.onnx` (`encoder_output`
//! [1,D,T'] -> `logprobs` [1,T',W]). The reference is `model_onnx.py` from the pack
//! (CTC path only). Sessions are built once; ORT is initialised the same way as the
//! Tamil TTS backend (`tamil_pocket_ort.rs`).

#![cfg_attr(not(test), allow(dead_code))]

use super::indic_stt_core::{
    indic_pack_status, load_lang_head, IndicBackend, IndicEngine, IndicLogits, IndicPackStatus,
};
use super::indic_stt_mel::{MelFrontend, N_MELS};
use super::indic_stt_slot::IndicSession;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::{Session, SessionInputValue, SessionOutputs};
use ort::value::{DynValue, PrimitiveTensorElementType, Tensor as OrtTensor};
use std::fmt::Debug;
use std::path::Path;

const ENCODER_FILE: &str = "encoder.onnx";
const CTC_FILE: &str = "ctc_decoder.onnx";
/// Upper bound on intra-op threads (the task asks for min(cpus, 4)).
const MAX_INTRA_OP_THREADS: usize = 4;
/// Full CTC output width of the multilingual head.
const FULL_WIDTH: usize = 5633;
/// Vocabulary size below which the CTC output width is not plausible for the pack.
const MIN_PLAUSIBLE_WIDTH: usize = 256;

/// Number of intra-op threads: min(available parallelism, 4), at least 1.
fn intra_op_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, MAX_INTRA_OP_THREADS)
}

/// Builds a CPU-only session with one inter-op thread and the intra-op count above.
/// Deliberately not `ort_sessions::build_session`, which uses one intra-op thread.
fn build_session(path: &Path) -> Result<Session, String> {
    if !path.is_file() {
        return Err(format!(
            "missing Tamil speech model file: {}",
            path.display()
        ));
    }
    // Telemetry is an environment-wide switch; the first committed environment wins.
    let _first_commit_wins = ort::environment::init().with_telemetry(false).commit();
    let mut builder = Session::builder()
        .map_err(|e| format!("cannot create ONNX session builder: {e}"))?
        .with_execution_providers([ort::ep::CPU::default().build()])
        .map_err(|e| format!("cannot select the CPU provider: {e}"))?
        .with_optimization_level(GraphOptimizationLevel::All)
        .map_err(|e| format!("cannot set graph optimisation level: {e}"))?
        .with_inter_threads(1)
        .map_err(|e| format!("cannot set inter-op threads: {e}"))?
        .with_intra_threads(intra_op_threads())
        .map_err(|e| format!("cannot set intra-op threads: {e}"))?;
    builder
        .commit_from_file(path)
        .map_err(|e| format!("cannot load {}: {e}", path.display()))
}

/// Errors unless every named input and output exists in the session.
fn check_names(
    session: &Session,
    file: &str,
    inputs: &[&str],
    outputs: &[&str],
) -> Result<(), String> {
    let have_inputs: Vec<&str> = session.inputs().iter().map(|o| o.name()).collect();
    for name in inputs {
        if !have_inputs.contains(name) {
            return Err(format!(
                "{file}: no input named '{name}' (pack and code disagree)"
            ));
        }
    }
    let have_outputs: Vec<&str> = session.outputs().iter().map(|o| o.name()).collect();
    for name in outputs {
        if !have_outputs.contains(name) {
            return Err(format!(
                "{file}: no output named '{name}' (pack and code disagree)"
            ));
        }
    }
    Ok(())
}

/// Removes a named output and copies its shape and elements out of ORT memory.
fn take_output<T>(
    outputs: &mut SessionOutputs<'_>,
    name: &str,
) -> Result<(Vec<usize>, Vec<T>), String>
where
    T: PrimitiveTensorElementType + Clone + Debug,
{
    let value: DynValue = outputs
        .remove(name)
        .ok_or_else(|| format!("model output '{name}' is missing"))?;
    let (shape, data) = value
        .try_extract_tensor::<T>()
        .map_err(|e| format!("output '{name}': {e}"))?;
    let dims = shape
        .iter()
        .map(|&d| {
            usize::try_from(d).map_err(|_| format!("output '{name}': negative dimension {d}"))
        })
        .collect::<Result<Vec<usize>, String>>()?;
    Ok((dims, data.to_vec()))
}

/// The encoder and CTC sessions of one Tamil speech pack.
pub struct OrtIndicBackend {
    encoder: Session,
    ctc: Session,
}

impl OrtIndicBackend {
    /// Loads the pack from `dir`. The four-file pack status is checked first (stat only), so a
    /// missing or wrong-sized file is reported by name before any session is built.
    pub fn load(dir: &Path) -> Result<Self, String> {
        match indic_pack_status(dir) {
            IndicPackStatus::Ready => {}
            IndicPackStatus::Missing(files) => {
                return Err(format!(
                    "Tamil speech pack in {} is missing: {}",
                    dir.display(),
                    files.join(", ")
                ));
            }
            IndicPackStatus::WrongSize {
                file,
                expected,
                actual,
            } => {
                return Err(format!(
                    "{file} has the wrong size: {actual} bytes, expected {expected} bytes ({})",
                    dir.join(&file).display()
                ));
            }
        }
        Self::load_files(&dir.join(ENCODER_FILE), &dir.join(CTC_FILE))
    }

    /// Builds both sessions from explicit paths and checks the tensor names the code uses.
    /// No size check here: tests load the fake pack through this path.
    pub(crate) fn load_files(encoder: &Path, ctc: &Path) -> Result<Self, String> {
        let encoder_session = build_session(encoder)?;
        check_names(
            &encoder_session,
            ENCODER_FILE,
            &["audio_signal", "length"],
            &["outputs", "encoded_lengths"],
        )?;
        let ctc_session = build_session(ctc)?;
        check_names(&ctc_session, CTC_FILE, &["encoder_output"], &["logprobs"])?;
        Ok(Self {
            encoder: encoder_session,
            ctc: ctc_session,
        })
    }
}

impl IndicBackend for OrtIndicBackend {
    fn run(&mut self, features: &[f32], frames: usize) -> Result<IndicLogits, String> {
        if frames == 0 {
            return Err("no feature frames to encode".to_string());
        }
        let expected = N_MELS
            .checked_mul(frames)
            .ok_or_else(|| "feature shape overflows".to_string())?;
        if features.len() != expected {
            return Err(format!(
                "features have {} values but {frames} frames x {N_MELS} bands need {expected}",
                features.len()
            ));
        }
        let frames_i64 =
            i64::try_from(frames).map_err(|_| format!("{frames} frames is too many"))?;

        let audio = OrtTensor::<f32>::from_array((vec![1usize, N_MELS, frames], features.to_vec()))
            .map_err(|e| format!("cannot create audio_signal tensor: {e}"))?;
        let length = OrtTensor::<i64>::from_array((vec![1usize], vec![frames_i64]))
            .map_err(|e| format!("cannot create length tensor: {e}"))?;
        let mut encoder_out = self
            .encoder
            .run(vec![
                ("audio_signal", SessionInputValue::from(audio)),
                ("length", SessionInputValue::from(length)),
            ])
            .map_err(|e| format!("{ENCODER_FILE} failed: {e}"))?;
        let (enc_shape, enc_data) = take_output::<f32>(&mut encoder_out, "outputs")?;
        let (_, enc_len) = take_output::<i64>(&mut encoder_out, "encoded_lengths")?;
        drop(encoder_out);

        if enc_shape.len() != 3 || enc_shape[0] != 1 || enc_shape[1] == 0 || enc_shape[2] == 0 {
            return Err(format!("encoder output has unexpected shape {enc_shape:?}"));
        }
        let encoded_frames = match enc_len.as_slice() {
            [value] => usize::try_from(*value)
                .map_err(|_| format!("encoded length {value} is negative"))?,
            other => {
                return Err(format!(
                    "encoded_lengths has {} values, expected 1",
                    other.len()
                ))
            }
        };

        let encoder_tensor = OrtTensor::<f32>::from_array((enc_shape, enc_data))
            .map_err(|e| format!("cannot create encoder_output tensor: {e}"))?;
        let mut ctc_out = self
            .ctc
            .run(vec![(
                "encoder_output",
                SessionInputValue::from(encoder_tensor),
            )])
            .map_err(|e| format!("{CTC_FILE} failed: {e}"))?;
        let (lp_shape, mut lp_data) = take_output::<f32>(&mut ctc_out, "logprobs")?;

        if lp_shape.len() != 3 || lp_shape[0] != 1 {
            return Err(format!("logprobs has unexpected shape {lp_shape:?}"));
        }
        let width = lp_shape[2];
        if width != FULL_WIDTH && width <= MIN_PLAUSIBLE_WIDTH {
            return Err(format!(
                "logprobs width {width} is neither {FULL_WIDTH} nor above {MIN_PLAUSIBLE_WIDTH}"
            ));
        }
        let model_steps = lp_shape[1];
        let steps = encoded_frames.min(model_steps);
        lp_data.truncate(steps * width);
        Ok(IndicLogits {
            logprobs: lp_data,
            steps,
            width,
        })
    }
}

/// Builds the Tamil engine from a pack directory: ORT backend, mel front end and the language head.
/// Errors name the missing or wrong file (pack status gate first).
pub fn load_engine(dir: &Path, lang: &str) -> Result<IndicEngine, String> {
    let backend = OrtIndicBackend::load(dir)?;
    let head = load_lang_head(dir, lang)?;
    Ok(IndicEngine::new(
        Box::new(backend),
        MelFrontend::new(),
        head,
    ))
}

impl IndicSession for IndicEngine {
    fn transcribe(&mut self, samples: &[f32]) -> Result<String, String> {
        IndicEngine::transcribe(self, samples)
    }
}
