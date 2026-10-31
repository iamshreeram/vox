//! ONNX Runtime backend for the Pocket TTS (Tamil) engine (DESIGN.md Rev 5, S4).
//!
//! [`OrtPocket`] implements [`PocketBackend`] on four sessions of the pack
//! (text conditioner, flow LM main, flow head, mimi decoder). The mimi encoder
//! is loaded only while a voice is encoded and dropped right after. State
//! tensors are wired by the names in `bundle.json`: inputs use `input_name`,
//! outputs are looked up by `output_name`, never by position.
//!
//! The session handling and tensor wiring follow KevinAHM's
//! `pocket_tts_onnx.py` (Apache License 2.0). The Rust code is an independent port.
//!
//! Known deviation: `bos_before_voice.npy` is read by its fixed file name,
//! because [`Bundle`] does not carry the `bos_before_voice_file` entry.

use std::fmt::Debug;
use std::path::Path;

use ort::memory::Allocator;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::{Session, SessionInputValue, SessionOutputs};
use ort::value::{DynValue, PrimitiveTensorElementType, Tensor as OrtTensor};

use super::natural_audio::resample_mono;
use super::tamil_pocket_core::{
    prepare_voice_state, Bundle, Dtype, LmOut, PocketBackend, StateSpec, Tensor, TensorData,
    VoiceState,
};
use super::tamil_tokenizer::TamilTokenizer;

const BUNDLE_FILE: &str = "bundle.json";
const TOKENIZER_FILE: &str = "tokenizer.model";
const TEXT_CONDITIONER_FILE: &str = "text_conditioner.onnx";
const FLOW_LM_MAIN_FILE: &str = "flow_lm_main_int4.onnx";
const FLOW_LM_FLOW_FILE: &str = "flow_lm_flow_int8.onnx";
const MIMI_DECODER_FILE: &str = "mimi_decoder_int8.onnx";
const MIMI_ENCODER_FILE: &str = "mimi_encoder_int8.onnx";
const VOICE_WAV_FILE: &str = "sample_geetha.wav";
const BOS_FILE: &str = "bos_before_voice.npy";

/// Sample rate the voice WAV is resampled to, and the model output rate.
const SAMPLE_RATE: u32 = 24_000;
/// Upper bound on intra-op threads (the task asks for min(cpus, 4)).
const MAX_INTRA_OP_THREADS: usize = 4;
/// Largest npy header accepted. Real headers are well under 1 KiB.
const NPY_MAX_HEADER: usize = 64 * 1024;
/// Scale of 16-bit PCM samples.
const PCM16_SCALE: f32 = 32_768.0;
/// Scale of 24-bit PCM samples (2^23).
const PCM24_SCALE: f32 = 8_388_608.0;

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
        return Err(format!("missing Tamil TTS model file: {}", path.display()));
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
                "{file}: no input named '{name}' (bundle and model disagree)"
            ));
        }
    }
    let have_outputs: Vec<&str> = session.outputs().iter().map(|o| o.name()).collect();
    for name in outputs {
        if !have_outputs.contains(name) {
            return Err(format!(
                "{file}: no output named '{name}' (bundle and model disagree)"
            ));
        }
    }
    Ok(())
}

/// Product of the dimensions, or `None` on overflow.
fn element_count(shape: &[usize]) -> Option<usize> {
    shape.iter().try_fold(1usize, |acc, &d| acc.checked_mul(d))
}

/// Builds an ORT input value from owned data. Shapes with a zero dimension go through
/// `Tensor::new`, because `from_array` only accepts dimensions of at least 1.
fn to_input<T>(
    allocator: &Allocator,
    name: &str,
    shape: &[usize],
    data: Vec<T>,
) -> Result<SessionInputValue<'static>, String>
where
    T: PrimitiveTensorElementType + Debug + Clone + 'static,
{
    let expected =
        element_count(shape).ok_or_else(|| format!("'{name}': shape {shape:?} overflows"))?;
    if expected != data.len() {
        return Err(format!(
            "'{name}': shape {shape:?} needs {expected} elements, got {}",
            data.len()
        ));
    }
    if expected == 0 {
        let dims = shape
            .iter()
            .map(|&d| i64::try_from(d).map_err(|_| format!("'{name}': dimension {d} too large")))
            .collect::<Result<Vec<i64>, String>>()?;
        let tensor = OrtTensor::<T>::new(allocator, dims)
            .map_err(|e| format!("'{name}': cannot create empty tensor: {e}"))?;
        return Ok(SessionInputValue::from(tensor));
    }
    let tensor = OrtTensor::<T>::from_array((shape.to_vec(), data))
        .map_err(|e| format!("'{name}': cannot create tensor: {e}"))?;
    Ok(SessionInputValue::from(tensor))
}

/// Converts a crate tensor into an ORT input, moving its buffer (no copy).
fn tensor_input(
    allocator: &Allocator,
    name: &str,
    tensor: Tensor,
) -> Result<SessionInputValue<'static>, String> {
    let Tensor { shape, data } = tensor;
    match data {
        TensorData::F32(values) => to_input(allocator, name, &shape, values),
        TensorData::I64(values) => to_input(allocator, name, &shape, values),
        TensorData::Bool(values) => to_input(allocator, name, &shape, values),
    }
}

/// Errors unless a state tensor carries the element type the manifest declares.
fn check_state_dtype(spec: &StateSpec, tensor: &Tensor) -> Result<(), String> {
    let ok = matches!(
        (spec.dtype, &tensor.data),
        (Dtype::F32, TensorData::F32(_))
            | (Dtype::I64, TensorData::I64(_))
            | (Dtype::Bool, TensorData::Bool(_))
    );
    if ok {
        Ok(())
    } else {
        Err(format!(
            "state input '{}': expected element type {:?}",
            spec.input_name, spec.dtype
        ))
    }
}

/// Errors unless the tensor holds f32 data.
fn require_f32(tensor: &Tensor, name: &str) -> Result<(), String> {
    if matches!(tensor.data, TensorData::F32(_)) {
        Ok(())
    } else {
        Err(format!("'{name}': expected f32 data"))
    }
}

/// Removes a named output and copies its elements and shape out of ORT memory.
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

/// Takes the updated state tensors for a manifest, in manifest order.
fn take_state(
    outputs: &mut SessionOutputs<'_>,
    specs: &[StateSpec],
) -> Result<Vec<Tensor>, String> {
    specs
        .iter()
        .map(|spec| take_state_one(outputs, spec))
        .collect()
}

fn take_state_one(outputs: &mut SessionOutputs<'_>, spec: &StateSpec) -> Result<Tensor, String> {
    let name = spec.output_name.as_str();
    let built = match spec.dtype {
        Dtype::F32 => {
            let (shape, data) = take_output::<f32>(outputs, name)?;
            Tensor::f32(shape, data)
        }
        Dtype::I64 => {
            let (shape, data) = take_output::<i64>(outputs, name)?;
            Tensor::i64(shape, data)
        }
        Dtype::Bool => {
            let (shape, data) = take_output::<bool>(outputs, name)?;
            Tensor::bool(shape, data)
        }
    };
    built.map_err(|e| format!("state output '{name}': {e}"))
}

/// Reads one little-endian f32 `.npy` file (version 1, 2 or 3, C order).
/// Other dtypes, big-endian data, Fortran order and malformed files are errors.
pub fn read_npy_f32(path: &Path) -> Result<Tensor, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_npy_f32(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn parse_npy_f32(bytes: &[u8]) -> Result<Tensor, String> {
    const MAGIC: &[u8] = b"\x93NUMPY";
    if !bytes.starts_with(MAGIC) {
        return Err("not a .npy file (bad magic)".to_string());
    }
    let version = *bytes.get(6).ok_or("truncated .npy preamble")?;
    let (header_len, header_start) = match version {
        1 => {
            let raw: [u8; 2] = bytes
                .get(8..10)
                .and_then(|s| s.try_into().ok())
                .ok_or("truncated .npy header length")?;
            (usize::from(u16::from_le_bytes(raw)), 10usize)
        }
        2 | 3 => {
            let raw: [u8; 4] = bytes
                .get(8..12)
                .and_then(|s| s.try_into().ok())
                .ok_or("truncated .npy header length")?;
            let len =
                usize::try_from(u32::from_le_bytes(raw)).map_err(|_| "header length overflows")?;
            (len, 12usize)
        }
        other => return Err(format!("unsupported .npy version {other}")),
    };
    if header_len > NPY_MAX_HEADER {
        return Err(format!(
            "header length {header_len} exceeds the {NPY_MAX_HEADER}-byte limit"
        ));
    }
    let header_end = header_start
        .checked_add(header_len)
        .ok_or("header length overflows")?;
    let header_bytes = bytes
        .get(header_start..header_end)
        .ok_or("truncated .npy header")?;
    let header = std::str::from_utf8(header_bytes).map_err(|_| "header is not valid text")?;
    let (descr, fortran, shape) = parse_npy_header(header)?;
    if descr != "<f4" {
        return Err(format!(
            "unsupported dtype '{descr}' (only little-endian float32 '<f4')"
        ));
    }
    if fortran {
        return Err("fortran_order arrays are not supported".to_string());
    }
    let count = element_count(&shape).ok_or("shape overflows")?;
    let expected = count.checked_mul(4).ok_or("data size overflows")?;
    let data = bytes.get(header_end..).ok_or("truncated .npy data")?;
    if data.len() < expected {
        return Err(format!(
            "truncated data: expected {expected} bytes, found {}",
            data.len()
        ));
    }
    if data.len() > expected {
        return Err(format!(
            "size mismatch: expected {expected} data bytes, found {}",
            data.len()
        ));
    }
    let (words, _tail) = data.as_chunks::<4>();
    let values: Vec<f32> = words.iter().map(|w| f32::from_le_bytes(*w)).collect();
    Tensor::f32(shape, values)
}

/// Returns the text after `key:` in a Python dict literal, with leading spaces removed.
fn value_after<'a>(header: &'a str, key: &str) -> Result<&'a str, String> {
    let at = header
        .find(key)
        .ok_or_else(|| format!("header has no {key} entry"))?;
    let rest = header.get(at + key.len()..).unwrap_or("");
    let colon = rest
        .find(':')
        .ok_or_else(|| format!("header entry {key} has no value"))?;
    Ok(rest.get(colon + 1..).unwrap_or("").trim_start())
}

/// Parses `descr`, `fortran_order` and `shape` from a .npy header dict.
fn parse_npy_header(header: &str) -> Result<(String, bool, Vec<usize>), String> {
    let after_descr = value_after(header, "'descr'")?;
    let quoted = after_descr
        .strip_prefix('\'')
        .ok_or("header descr is not a string")?;
    let descr = quoted.split('\'').next().unwrap_or("").to_string();

    let after_fortran = value_after(header, "'fortran_order'")?;
    let fortran = if after_fortran.starts_with("True") {
        true
    } else if after_fortran.starts_with("False") {
        false
    } else {
        return Err("header fortran_order is not a boolean".to_string());
    };

    let after_shape = value_after(header, "'shape'")?;
    let inner = after_shape
        .strip_prefix('(')
        .and_then(|s| s.split(')').next())
        .ok_or("header shape is not a tuple")?;
    let shape = inner
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<usize>()
                .map_err(|_| format!("bad shape entry '{s}'"))
        })
        .collect::<Result<Vec<usize>, String>>()?;
    Ok((descr, fortran, shape))
}

/// Loads a voice WAV as mono f32 at 24 kHz. Accepts 16-bit int, 24-bit int and
/// 32-bit float; multi-channel input is averaged; other rates are resampled.
/// Empty and zero-length files are errors.
pub fn load_voice_wav(path: &Path) -> Result<Vec<f32>, String> {
    let mut reader =
        hound::WavReader::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels);
    if channels == 0 {
        return Err(format!("{}: WAV has no channels", path.display()));
    }
    if spec.sample_rate == 0 {
        return Err(format!("{}: WAV sample rate is zero", path.display()));
    }
    let interleaved: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|s| s.map(|v| f32::from(v) / PCM16_SCALE))
            .collect::<Result<Vec<f32>, hound::Error>>(),
        (hound::SampleFormat::Int, 24) => reader
            .samples::<i32>()
            .map(|s| s.map(|v| v as f32 / PCM24_SCALE))
            .collect::<Result<Vec<f32>, hound::Error>>(),
        (hound::SampleFormat::Float, 32) => reader.samples::<f32>().collect::<Result<Vec<f32>, hound::Error>>(),
        (format, bits) => {
            return Err(format!(
                "{}: unsupported WAV format {format:?} {bits}-bit (16-bit int, 24-bit int or 32-bit float only)",
                path.display()
            ))
        }
    }
    .map_err(|e| format!("{}: cannot read samples: {e}", path.display()))?;

    if interleaved.is_empty() {
        return Err(format!("{}: WAV contains no samples", path.display()));
    }
    if !interleaved.len().is_multiple_of(channels) {
        return Err(format!(
            "{}: {} samples is not a whole number of {channels}-channel frames",
            path.display(),
            interleaved.len()
        ));
    }
    let mono: Vec<f32> = if channels == 1 {
        interleaved
    } else {
        let scale = channels as f32;
        interleaved
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / scale)
            .collect()
    };
    let mono = if spec.sample_rate == SAMPLE_RATE {
        mono
    } else {
        resample_mono(&mono, spec.sample_rate, SAMPLE_RATE)
            .map_err(|e| format!("{}: {e}", path.display()))?
    };
    if mono.is_empty() {
        return Err(format!(
            "{}: WAV has no audio after resampling",
            path.display()
        ));
    }
    Ok(mono)
}

/// The Pocket TTS ONNX sessions of one pack plus the bundle that wires them.
pub struct OrtPocket {
    bundle: Bundle,
    allocator: Allocator,
    text_conditioner: Session,
    flow_lm_main: Session,
    flow_lm_flow: Session,
    mimi_decoder: Session,
}

impl OrtPocket {
    /// Loads `bundle.json` and the four sessions the generation loop needs.
    /// The mimi encoder is not loaded here. Errors name the missing file.
    pub fn load(dir: &Path) -> Result<OrtPocket, String> {
        let bundle = Bundle::load(&dir.join(BUNDLE_FILE))?;
        let text_conditioner = build_session(&dir.join(TEXT_CONDITIONER_FILE))?;
        check_names(
            &text_conditioner,
            TEXT_CONDITIONER_FILE,
            &["token_ids"],
            &["embeddings"],
        )?;

        let flow_lm_main = build_session(&dir.join(FLOW_LM_MAIN_FILE))?;
        let mut lm_inputs: Vec<&str> = vec!["sequence", "text_embeddings"];
        lm_inputs.extend(bundle.flow_state.iter().map(|s| s.input_name.as_str()));
        let mut lm_outputs: Vec<&str> = vec!["conditioning", "eos_logit"];
        lm_outputs.extend(bundle.flow_state.iter().map(|s| s.output_name.as_str()));
        check_names(&flow_lm_main, FLOW_LM_MAIN_FILE, &lm_inputs, &lm_outputs)?;

        let flow_lm_flow = build_session(&dir.join(FLOW_LM_FLOW_FILE))?;
        check_names(
            &flow_lm_flow,
            FLOW_LM_FLOW_FILE,
            &["c", "s", "t", "x"],
            &["flow_dir"],
        )?;

        let mimi_decoder = build_session(&dir.join(MIMI_DECODER_FILE))?;
        let mut dec_inputs: Vec<&str> = vec!["latent"];
        dec_inputs.extend(bundle.mimi_state.iter().map(|s| s.input_name.as_str()));
        let mut dec_outputs: Vec<&str> = vec!["audio_frame"];
        dec_outputs.extend(bundle.mimi_state.iter().map(|s| s.output_name.as_str()));
        check_names(&mimi_decoder, MIMI_DECODER_FILE, &dec_inputs, &dec_outputs)?;

        Ok(OrtPocket {
            bundle,
            allocator: Allocator::default(),
            text_conditioner,
            flow_lm_main,
            flow_lm_flow,
            mimi_decoder,
        })
    }

    /// Encodes a voice WAV to its `[1, T, 1024]` embedding (before the BOS is
    /// prepended). Loads the mimi encoder for this call only and drops it afterwards.
    pub fn encode_voice(&mut self, wav_path: &Path, dir: &Path) -> Result<Tensor, String> {
        let samples = load_voice_wav(wav_path)?;
        let len = samples.len();
        let mut encoder = build_session(&dir.join(MIMI_ENCODER_FILE))?;
        check_names(&encoder, MIMI_ENCODER_FILE, &["audio"], &["latents"])?;
        let audio = to_input(&self.allocator, "audio", &[1, 1, len], samples)?;
        let mut outputs = encoder
            .run(vec![("audio", audio)])
            .map_err(|e| format!("voice encoder failed: {e}"))?;
        let (shape, data) = take_output::<f32>(&mut outputs, "latents")?;
        Tensor::f32(shape, data).map_err(|e| format!("voice latents: {e}"))
    }
}

impl PocketBackend for OrtPocket {
    fn text_embed(&mut self, token_ids: &[i64]) -> Result<Tensor, String> {
        let ids = to_input(
            &self.allocator,
            "token_ids",
            &[1, token_ids.len()],
            token_ids.to_vec(),
        )?;
        let mut outputs = self
            .text_conditioner
            .run(vec![("token_ids", ids)])
            .map_err(|e| format!("text conditioner failed: {e}"))?;
        let (shape, data) = take_output::<f32>(&mut outputs, "embeddings")?;
        Tensor::f32(shape, data).map_err(|e| format!("text embeddings: {e}"))
    }

    fn lm_step(
        &mut self,
        sequence: &Tensor,
        text_embeddings: &Tensor,
        state: Vec<Tensor>,
    ) -> Result<LmOut, String> {
        require_f32(sequence, "sequence")?;
        require_f32(text_embeddings, "text_embeddings")?;
        let specs = &self.bundle.flow_state;
        if state.len() != specs.len() {
            return Err(format!(
                "flow LM state: expected {} tensors, got {}",
                specs.len(),
                state.len()
            ));
        }
        let mut inputs: Vec<(String, SessionInputValue<'static>)> =
            Vec::with_capacity(state.len() + 2);
        inputs.push((
            "sequence".to_string(),
            tensor_input(&self.allocator, "sequence", sequence.clone())?,
        ));
        inputs.push((
            "text_embeddings".to_string(),
            tensor_input(&self.allocator, "text_embeddings", text_embeddings.clone())?,
        ));
        for (spec, tensor) in specs.iter().zip(state) {
            check_state_dtype(spec, &tensor)?;
            let value = tensor_input(&self.allocator, &spec.input_name, tensor)?;
            inputs.push((spec.input_name.clone(), value));
        }

        let mut outputs = self
            .flow_lm_main
            .run(inputs)
            .map_err(|e| format!("flow LM failed: {e}"))?;
        let (_, conditioning) = take_output::<f32>(&mut outputs, "conditioning")?;
        if conditioning.len() != self.bundle.conditioning_dim {
            return Err(format!(
                "'conditioning': expected {} values, got {}",
                self.bundle.conditioning_dim,
                conditioning.len()
            ));
        }
        let (_, eos) = take_output::<f32>(&mut outputs, "eos_logit")?;
        let eos_logit = *eos
            .first()
            .ok_or_else(|| "'eos_logit' is empty".to_string())?;
        let state = take_state(&mut outputs, &self.bundle.flow_state)?;
        Ok(LmOut {
            conditioning,
            eos_logit,
            state,
        })
    }

    fn flow(
        &mut self,
        conditioning: &[f32],
        s: f32,
        t: f32,
        x: &[f32],
    ) -> Result<Vec<f32>, String> {
        let c = to_input(
            &self.allocator,
            "c",
            &[1, conditioning.len()],
            conditioning.to_vec(),
        )?;
        let s_in = to_input(&self.allocator, "s", &[1, 1], vec![s])?;
        let t_in = to_input(&self.allocator, "t", &[1, 1], vec![t])?;
        let x_in = to_input(&self.allocator, "x", &[1, x.len()], x.to_vec())?;
        let mut outputs = self
            .flow_lm_flow
            .run(vec![("c", c), ("s", s_in), ("t", t_in), ("x", x_in)])
            .map_err(|e| format!("flow head failed: {e}"))?;
        let (_, direction) = take_output::<f32>(&mut outputs, "flow_dir")?;
        Ok(direction)
    }

    fn decode(
        &mut self,
        latents: &Tensor,
        state: Vec<Tensor>,
    ) -> Result<(Vec<f32>, Vec<Tensor>), String> {
        require_f32(latents, "latent")?;
        let specs = &self.bundle.mimi_state;
        if state.len() != specs.len() {
            return Err(format!(
                "mimi decoder state: expected {} tensors, got {}",
                specs.len(),
                state.len()
            ));
        }
        let mut inputs: Vec<(String, SessionInputValue<'static>)> =
            Vec::with_capacity(state.len() + 1);
        inputs.push((
            "latent".to_string(),
            tensor_input(&self.allocator, "latent", latents.clone())?,
        ));
        for (spec, tensor) in specs.iter().zip(state) {
            check_state_dtype(spec, &tensor)?;
            let value = tensor_input(&self.allocator, &spec.input_name, tensor)?;
            inputs.push((spec.input_name.clone(), value));
        }
        let mut outputs = self
            .mimi_decoder
            .run(inputs)
            .map_err(|e| format!("mimi decoder failed: {e}"))?;
        let (_, audio) = take_output::<f32>(&mut outputs, "audio_frame")?;
        let next = take_state(&mut outputs, &self.bundle.mimi_state)?;
        Ok((audio, next))
    }
}

/// A loaded Pocket TTS model: bundle, tokenizer, ONNX backend and the cached voice state.
pub struct PocketModel {
    /// Parsed `bundle.json`.
    pub bundle: Bundle,
    /// SentencePiece BPE tokenizer from `tokenizer.model`.
    pub tokenizer: TamilTokenizer,
    /// ONNX sessions for the generation loop.
    pub backend: OrtPocket,
    /// Flow-LM state after the BOS and the voice prefix; cloned per chunk.
    pub voice: VoiceState,
}

/// Conditions the flow LM on a voice embedding (`[1, T, 1024]`, before the BOS),
/// prepending `bos_before_voice.npy` from `dir` exactly like the reference does.
pub fn condition_voice(
    backend: &mut OrtPocket,
    bundle: &Bundle,
    embedding: &Tensor,
    dir: &Path,
) -> Result<VoiceState, String> {
    let bos = read_npy_f32(&dir.join(BOS_FILE))?;
    prepare_voice_state(backend, bundle, embedding, Some(&bos))
}

impl PocketModel {
    /// Loads the pack from `dir`: sessions, tokenizer, and the voice state computed
    /// from `sample_geetha.wav` and `bos_before_voice.npy`. Errors name the file.
    pub fn load(dir: &Path) -> Result<PocketModel, String> {
        let mut backend = OrtPocket::load(dir)?;
        let embedding = backend.encode_voice(&dir.join(VOICE_WAV_FILE), dir)?;
        Self::with_embedding(dir, backend, &embedding)
    }

    /// Builds the model from already-loaded sessions and a voice embedding. Used by
    /// [`PocketModel::load`], and by tests that pin the voice embedding to a reference.
    pub fn with_embedding(
        dir: &Path,
        mut backend: OrtPocket,
        embedding: &Tensor,
    ) -> Result<PocketModel, String> {
        let bundle = backend.bundle.clone();
        let tokenizer = TamilTokenizer::load(&dir.join(TOKENIZER_FILE))?;
        let voice = condition_voice(&mut backend, &bundle, embedding, dir)?;
        Ok(PocketModel {
            bundle,
            tokenizer,
            backend,
            voice,
        })
    }
}
