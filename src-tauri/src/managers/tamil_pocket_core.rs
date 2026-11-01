//! ONNX-independent core of the Pocket TTS (Tamil) engine (DESIGN.md Rev 5, S3).
//!
//! The generation loop is a port of `_run_flow_lm_chunk` and friends in
//! KevinAHM's `pocket_tts_onnx.py` (Apache License 2.0). This module holds the pure
//! Rust parts only: bundle parsing, flow/mimi state tensors, the
//! [`PocketBackend`] trait that the ONNX session wrapper implements, and the
//! latent generation and decoding loops. No `ort`, no tokenizer.

use std::collections::HashSet;
use std::path::Path;

/// KV positions reserved after the voice and text (reference margin).
const KV_MARGIN: usize = 8;

const SCHEMA_VERSION: u32 = 2;
const SAMPLE_RATE: u32 = 24_000;
const LATENT_DIM: usize = 32;
const CONDITIONING_DIM: usize = 1024;

/// Element type of a state tensor, from the bundle manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dtype {
    /// 32-bit float.
    F32,
    /// 64-bit signed integer.
    I64,
    /// Boolean.
    Bool,
}

impl Dtype {
    fn parse(text: &str) -> Result<Dtype, String> {
        match text {
            "float32" => Ok(Dtype::F32),
            "int64" => Ok(Dtype::I64),
            "bool" => Ok(Dtype::Bool),
            other => Err(format!("unsupported state dtype '{other}'")),
        }
    }
}

/// How a state tensor is initialised, from the bundle manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    /// Every element is NaN (f32 only; other dtypes become zero).
    Nan,
    /// Every element is one (`true` for bool).
    Ones,
    /// Every element is zero (`false` for bool). Also used for `"empty"`.
    Zeros,
}

impl Fill {
    fn parse(text: &str) -> Fill {
        match text {
            "nan" => Fill::Nan,
            "ones" => Fill::Ones,
            _ => Fill::Zeros,
        }
    }
}

/// One entry of a flow-LM or mimi state manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct StateSpec {
    /// Position in the manifest; equals the array position after parsing.
    pub index: usize,
    /// ONNX input name of the state tensor (for example `state_3`).
    pub input_name: String,
    /// ONNX output name that carries the updated state.
    pub output_name: String,
    /// Element type.
    pub dtype: Dtype,
    /// Initial fill.
    pub fill: Fill,
    /// Tensor shape; may contain zeros.
    pub shape: Vec<usize>,
    /// Module the tensor belongs to (informational).
    pub module: String,
    /// Tensor key inside the module (for example `cache`).
    pub key: String,
}

/// Parsed and validated `bundle.json` of the Pocket TTS pack.
#[derive(Clone, Debug, PartialEq)]
pub struct Bundle {
    /// Output sample rate (always 24000 after parsing).
    pub sample_rate: u32,
    /// Latent frames per second.
    pub frame_rate: f32,
    /// Audio samples produced per latent frame.
    pub samples_per_frame: usize,
    /// Latent width (always 32 after parsing).
    pub latent_dim: usize,
    /// Text and voice embedding width (always 1024 after parsing).
    pub conditioning_dim: usize,
    /// Maximum text tokens per chunk recommended by the reference.
    pub max_token_per_chunk: usize,
    /// Whether a BOS embedding is prepended to the voice embedding.
    pub insert_bos_before_voice: bool,
    /// Flow-LM state manifest, in manifest order.
    pub flow_state: Vec<StateSpec>,
    /// Mimi decoder state manifest, in manifest order.
    pub mimi_state: Vec<StateSpec>,
    /// KV cache positions: dimension 2 of the first `cache` entry of `flow_state`.
    pub kv_capacity: usize,
    /// Frames to keep generating after the first EOS, when the pack recommends a value
    /// (bundle.json `model_recommended_frames_after_eos`). `None` (null or absent) means the
    /// caller applies its own default, `frames_after_eos_guess + 2` in the reference.
    pub recommended_frames_after_eos: Option<usize>,
}

#[derive(serde::Deserialize)]
struct RawState {
    dtype: String,
    fill: String,
    index: usize,
    input_name: String,
    output_name: String,
    key: String,
    module: String,
    shape: Vec<usize>,
}

fn default_max_token_per_chunk() -> usize {
    50
}

#[derive(serde::Deserialize)]
struct RawBundle {
    schema_version: u32,
    sample_rate: u32,
    frame_rate: f32,
    samples_per_frame: usize,
    latent_dim: usize,
    conditioning_dim: usize,
    #[serde(default = "default_max_token_per_chunk")]
    max_token_per_chunk: usize,
    #[serde(default)]
    insert_bos_before_voice: bool,
    flow_lm_state_manifest: Vec<RawState>,
    mimi_state_manifest: Vec<RawState>,
    #[serde(default)]
    model_recommended_frames_after_eos: Option<serde_json::Value>,
}

/// Number of elements of a shape, or `None` on overflow.
fn numel(shape: &[usize]) -> Option<usize> {
    shape
        .iter()
        .try_fold(1usize, |acc, &dim| acc.checked_mul(dim))
}

fn parse_manifest(name: &str, raw: &[RawState]) -> Result<Vec<StateSpec>, String> {
    if raw.is_empty() {
        return Err(format!("{name} is empty"));
    }
    let mut names = HashSet::new();
    let mut specs = Vec::with_capacity(raw.len());
    for (position, entry) in raw.iter().enumerate() {
        if entry.index != position {
            return Err(format!(
                "{name}: entry {position} has index {} (indices must be 0..n in array order)",
                entry.index
            ));
        }
        if !names.insert(entry.input_name.as_str()) {
            return Err(format!(
                "{name}: duplicate input name '{}'",
                entry.input_name
            ));
        }
        if numel(&entry.shape).is_none() {
            return Err(format!("{name}: shape of '{}' overflows", entry.input_name));
        }
        specs.push(StateSpec {
            index: entry.index,
            input_name: entry.input_name.clone(),
            output_name: entry.output_name.clone(),
            dtype: Dtype::parse(&entry.dtype)
                .map_err(|e| format!("{name}: '{}': {e}", entry.input_name))?,
            fill: Fill::parse(&entry.fill),
            shape: entry.shape.clone(),
            module: entry.module.clone(),
            key: entry.key.clone(),
        });
    }
    Ok(specs)
}

fn kv_capacity_of(flow_state: &[StateSpec]) -> Result<usize, String> {
    let cache = flow_state
        .iter()
        .find(|spec| spec.key == "cache")
        .ok_or_else(|| "flow_lm_state_manifest has no 'cache' entry".to_string())?;
    cache
        .shape
        .get(2)
        .copied()
        .ok_or_else(|| format!("cache entry '{}' has no KV dimension", cache.input_name))
}

impl Bundle {
    /// Parse and validate `bundle.json` text.
    pub fn parse(json: &str) -> Result<Bundle, String> {
        let raw: RawBundle =
            serde_json::from_str(json).map_err(|e| format!("invalid bundle.json: {e}"))?;
        if raw.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported schema_version {} (expected {SCHEMA_VERSION})",
                raw.schema_version
            ));
        }
        if raw.sample_rate != SAMPLE_RATE {
            return Err(format!(
                "unsupported sample_rate {} (expected {SAMPLE_RATE})",
                raw.sample_rate
            ));
        }
        if raw.latent_dim != LATENT_DIM {
            return Err(format!(
                "unsupported latent_dim {} (expected {LATENT_DIM})",
                raw.latent_dim
            ));
        }
        if raw.conditioning_dim != CONDITIONING_DIM {
            return Err(format!(
                "unsupported conditioning_dim {} (expected {CONDITIONING_DIM})",
                raw.conditioning_dim
            ));
        }
        let flow_state = parse_manifest("flow_lm_state_manifest", &raw.flow_lm_state_manifest)?;
        let mimi_state = parse_manifest("mimi_state_manifest", &raw.mimi_state_manifest)?;
        let kv_capacity = kv_capacity_of(&flow_state)?;
        let recommended_frames_after_eos = match raw.model_recommended_frames_after_eos {
            None => None,
            Some(value) => {
                let frames = value
                    .as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| {
                        format!("model_recommended_frames_after_eos must be a non-negative integer, got {value}")
                    })?;
                Some(frames)
            }
        };
        Ok(Bundle {
            sample_rate: raw.sample_rate,
            frame_rate: raw.frame_rate,
            samples_per_frame: raw.samples_per_frame,
            latent_dim: raw.latent_dim,
            conditioning_dim: raw.conditioning_dim,
            max_token_per_chunk: raw.max_token_per_chunk,
            insert_bos_before_voice: raw.insert_bos_before_voice,
            flow_state,
            mimi_state,
            kv_capacity,
            recommended_frames_after_eos,
        })
    }

    /// Read and parse a `bundle.json` file.
    pub fn load(path: &Path) -> Result<Bundle, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Bundle::parse(&text)
    }
}

/// Typed, row-major element storage of a [`Tensor`].
#[derive(Clone, Debug, PartialEq)]
pub enum TensorData {
    /// 32-bit float elements.
    F32(Vec<f32>),
    /// 64-bit integer elements.
    I64(Vec<i64>),
    /// Boolean elements.
    Bool(Vec<bool>),
}

/// Dense row-major tensor with a plain shape.
#[derive(Clone, Debug, PartialEq)]
pub struct Tensor {
    /// Dimensions; a zero dimension gives an empty tensor.
    pub shape: Vec<usize>,
    /// Element storage; its length equals the product of `shape`.
    pub data: TensorData,
}

fn check_len(shape: &[usize], len: usize) -> Result<(), String> {
    match numel(shape) {
        Some(expected) if expected == len => Ok(()),
        _ => Err(format!(
            "shape {shape:?} needs a different element count than {len}"
        )),
    }
}

impl Tensor {
    /// Build an f32 tensor; errors if the element count does not match the shape.
    pub fn f32(shape: Vec<usize>, data: Vec<f32>) -> Result<Tensor, String> {
        check_len(&shape, data.len())?;
        Ok(Tensor {
            shape,
            data: TensorData::F32(data),
        })
    }

    /// Build an i64 tensor; errors if the element count does not match the shape.
    pub fn i64(shape: Vec<usize>, data: Vec<i64>) -> Result<Tensor, String> {
        check_len(&shape, data.len())?;
        Ok(Tensor {
            shape,
            data: TensorData::I64(data),
        })
    }

    /// Build a bool tensor; errors if the element count does not match the shape.
    pub fn bool(shape: Vec<usize>, data: Vec<bool>) -> Result<Tensor, String> {
        check_len(&shape, data.len())?;
        Ok(Tensor {
            shape,
            data: TensorData::Bool(data),
        })
    }

    /// Number of elements.
    pub fn numel(&self) -> usize {
        match &self.data {
            TensorData::F32(d) => d.len(),
            TensorData::I64(d) => d.len(),
            TensorData::Bool(d) => d.len(),
        }
    }

    /// Borrow the f32 elements, or error if the tensor is not f32.
    pub fn as_f32(&self) -> Result<&[f32], String> {
        match &self.data {
            TensorData::F32(d) => Ok(d),
            _ => Err("expected an f32 tensor".to_string()),
        }
    }
}

/// Build the initial state tensors for a manifest, in manifest order.
pub fn init_state(specs: &[StateSpec]) -> Vec<Tensor> {
    specs.iter().map(init_one).collect()
}

fn init_one(spec: &StateSpec) -> Tensor {
    let n = numel(&spec.shape).unwrap_or(0);
    let data = match spec.dtype {
        Dtype::F32 => {
            let value = match spec.fill {
                Fill::Nan => f32::NAN,
                Fill::Ones => 1.0,
                Fill::Zeros => 0.0,
            };
            TensorData::F32(vec![value; n])
        }
        Dtype::I64 => {
            let value = i64::from(spec.fill == Fill::Ones);
            TensorData::I64(vec![value; n])
        }
        Dtype::Bool => TensorData::Bool(vec![spec.fill == Fill::Ones; n]),
    };
    Tensor {
        shape: spec.shape.clone(),
        data,
    }
}

/// Result of one flow-LM step.
#[derive(Clone, Debug, PartialEq)]
pub struct LmOut {
    /// Conditioning vector for the flow head, length `conditioning_dim`.
    pub conditioning: Vec<f32>,
    /// End-of-speech logit.
    pub eos_logit: f32,
    /// Updated flow-LM state, in manifest order.
    pub state: Vec<Tensor>,
}

/// Model operations the generation loop needs. The ONNX implementation lives
/// elsewhere; tests use a scripted fake.
pub trait PocketBackend {
    /// Embed token ids into `[1, L, conditioning_dim]` f32.
    fn text_embed(&mut self, token_ids: &[i64]) -> Result<Tensor, String>;
    /// One flow-LM step. The state is taken by value so a real backend can move it.
    fn lm_step(
        &mut self,
        sequence: &Tensor,
        text_embeddings: &Tensor,
        state: Vec<Tensor>,
    ) -> Result<LmOut, String>;
    /// Flow direction for conditioning `c`, times `s` and `t`, and current `x`.
    /// Returns `latent_dim` values.
    fn flow(&mut self, conditioning: &[f32], s: f32, t: f32, x: &[f32])
        -> Result<Vec<f32>, String>;
    /// Decode `[1, S, latent_dim]` latents to audio, returning the new mimi state.
    fn decode(
        &mut self,
        latents: &Tensor,
        state: Vec<Tensor>,
    ) -> Result<(Vec<f32>, Vec<Tensor>), String>;
}

/// Deterministic generator: xorshift64* with Box-Muller normals. No `rand` crate.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
    spare: Option<f32>,
}

impl Rng {
    /// Seeded generator. Seed 0 is remapped to a fixed non-zero state.
    pub fn new(seed: u64) -> Rng {
        let state = if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        };
        Rng { state, spare: None }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform sample strictly inside (0, 1).
    fn next_unit(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// Standard normal sample (Box-Muller; the second value is cached).
    pub fn normal(&mut self) -> f32 {
        if let Some(value) = self.spare.take() {
            return value;
        }
        let u1 = self.next_unit();
        let u2 = self.next_unit();
        let radius = (-2.0 * u1.ln()).sqrt();
        let angle = 2.0 * std::f64::consts::PI * u2;
        self.spare = Some((radius * angle.sin()) as f32);
        (radius * angle.cos()) as f32
    }
}

/// Sampling parameters for one generation.
#[derive(Clone, Debug, PartialEq)]
pub struct GenParams {
    /// Noise temperature; 0 means zero noise (deterministic).
    pub temperature: f32,
    /// Number of flow (LSD) integration steps; must be at least 1.
    pub lsd_steps: usize,
    /// EOS fires when the logit is strictly greater than this.
    pub eos_threshold: f32,
}

impl Default for GenParams {
    fn default() -> Self {
        GenParams {
            temperature: 0.7,
            lsd_steps: 1,
            eos_threshold: -4.0,
        }
    }
}

/// Cached flow-LM state after the voice prefix.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceState {
    /// Flow-LM state tensors in manifest order.
    pub state: Vec<Tensor>,
    /// KV positions used by the voice prefix (BOS if inserted, plus the voice frames).
    pub positions: usize,
}

fn expect_shape(tensor: &Tensor, want: &[usize], what: &str) -> Result<(), String> {
    if tensor.shape == want {
        Ok(())
    } else {
        Err(format!(
            "{what}: expected shape {want:?}, got {:?}",
            tensor.shape
        ))
    }
}

fn check_state_len(state: &[Tensor], want: usize, what: &str) -> Result<(), String> {
    if state.len() == want {
        Ok(())
    } else {
        Err(format!(
            "{what}: expected {want} state tensors, got {}",
            state.len()
        ))
    }
}

/// Condition the flow LM on the voice embedding, once. The result is cached by
/// the caller and cloned per chunk.
pub fn prepare_voice_state(
    backend: &mut dyn PocketBackend,
    bundle: &Bundle,
    voice_embeddings: &Tensor,
    bos: Option<&Tensor>,
) -> Result<VoiceState, String> {
    let dim = bundle.conditioning_dim;
    if voice_embeddings.shape.len() != 3
        || voice_embeddings.shape[0] != 1
        || voice_embeddings.shape[2] != dim
    {
        return Err(format!(
            "voice embeddings must be [1, T, {dim}], got {:?}",
            voice_embeddings.shape
        ));
    }
    let frames = voice_embeddings.shape[1];
    if frames == 0 {
        return Err("voice embeddings are empty".to_string());
    }
    let voice = voice_embeddings.as_f32()?;

    let (shape, data) = if bundle.insert_bos_before_voice {
        let bos = bos.ok_or_else(|| {
            "bundle inserts a BOS embedding before the voice, but none was supplied".to_string()
        })?;
        expect_shape(bos, &[1, 1, dim], "BOS embedding")?;
        let mut data = bos.as_f32()?.to_vec();
        data.extend_from_slice(voice);
        (vec![1, frames + 1, dim], data)
    } else {
        (vec![1, frames, dim], voice.to_vec())
    };
    let positions = shape[1];
    let text = Tensor::f32(shape, data)?;
    let empty_sequence = Tensor::f32(vec![1, 0, bundle.latent_dim], Vec::new())?;
    let out = backend
        .lm_step(&empty_sequence, &text, init_state(&bundle.flow_state))
        .map_err(|e| format!("voice prefix failed: {e}"))?;
    check_state_len(&out.state, bundle.flow_state.len(), "voice prefix")?;
    Ok(VoiceState {
        state: out.state,
        positions,
    })
}

/// Draw the initial flow noise for one frame: zeros when temperature is 0,
/// otherwise N(0, temperature) per element.
fn sample_noise(temperature: f32, dim: usize, rng: &mut Rng) -> Vec<f32> {
    if temperature > 0.0 {
        let std = temperature.sqrt();
        (0..dim).map(|_| rng.normal() * std).collect()
    } else {
        vec![0.0; dim]
    }
}

/// Generate the latent frames for one text chunk, starting from the cached
/// voice state (which is cloned and never mutated). Returns `frames * latent_dim`
/// values, row-major.
#[allow(clippy::too_many_arguments)]
pub fn generate_chunk_latents(
    backend: &mut dyn PocketBackend,
    bundle: &Bundle,
    voice: &VoiceState,
    token_ids: &[i64],
    frames_after_eos: usize,
    max_frames: usize,
    params: &GenParams,
    rng: &mut Rng,
) -> Result<Vec<f32>, String> {
    if token_ids.is_empty() {
        return Err("cannot generate speech from an empty token sequence".to_string());
    }
    if params.lsd_steps == 0 {
        return Err("lsd_steps must be at least 1".to_string());
    }
    let text_len = token_ids.len();
    let dim = bundle.latent_dim;
    let cond_dim = bundle.conditioning_dim;

    let used = voice
        .positions
        .saturating_add(text_len)
        .saturating_add(KV_MARGIN);
    if used >= bundle.kv_capacity {
        return Err("text too long for the KV cache".to_string());
    }
    let kv_room = bundle.kv_capacity - used;
    let frame_limit = max_frames.min(kv_room);
    let dt = 1.0 / params.lsd_steps as f32;

    let text = backend
        .text_embed(token_ids)
        .map_err(|e| format!("text embedding failed: {e}"))?;
    expect_shape(&text, &[1, text_len, cond_dim], "text embeddings")?;

    let empty_sequence = Tensor::f32(vec![1, 0, dim], Vec::new())?;
    let empty_text = Tensor::f32(vec![1, 0, cond_dim], Vec::new())?;
    let prefix = backend
        .lm_step(&empty_sequence, &text, voice.state.clone())
        .map_err(|e| format!("chunk prefix failed: {e}"))?;
    check_state_len(&prefix.state, bundle.flow_state.len(), "chunk prefix")?;
    let mut state = prefix.state;

    let mut current = Tensor::f32(vec![1, 1, dim], vec![f32::NAN; dim])?;
    let mut latents: Vec<f32> = Vec::new();
    let mut eos_step: Option<usize> = None;
    let mut stopped_on_eos = false;

    for step in 0..frame_limit {
        let out = backend
            .lm_step(&current, &empty_text, state)
            .map_err(|e| format!("flow LM failed at frame {step}: {e}"))?;
        check_state_len(&out.state, bundle.flow_state.len(), "flow LM step")?;
        state = out.state;

        if eos_step.is_none() && out.eos_logit > params.eos_threshold {
            eos_step = Some(step);
        }
        if let Some(first_eos) = eos_step {
            if step >= first_eos.saturating_add(frames_after_eos) {
                stopped_on_eos = true;
                break;
            }
        }

        let mut x = sample_noise(params.temperature, dim, rng);
        for j in 0..params.lsd_steps {
            let s = j as f32 / params.lsd_steps as f32;
            let t = s + dt;
            let flow = backend
                .flow(&out.conditioning, s, t, &x)
                .map_err(|e| format!("flow head failed at frame {step}: {e}"))?;
            if flow.len() != dim {
                return Err(format!(
                    "flow returned {} values, expected {dim}",
                    flow.len()
                ));
            }
            for (xi, fi) in x.iter_mut().zip(flow.iter()) {
                *xi += fi * dt;
            }
        }
        latents.extend_from_slice(&x);
        current = Tensor::f32(vec![1, 1, dim], x)?;
    }

    if !stopped_on_eos && frame_limit < max_frames {
        log::warn!(
            "pocket tts: KV cache limit reached at {frame_limit} frames before EOS; \
             the chunk may be cut short"
        );
    }
    Ok(latents)
}

/// Decode latent frames to audio, `group_frames` frames per backend call.
/// The mimi state is threaded through every call, starting from a fresh state.
pub fn decode_latents(
    backend: &mut dyn PocketBackend,
    bundle: &Bundle,
    latents: &[f32],
    group_frames: usize,
) -> Result<Vec<f32>, String> {
    if group_frames == 0 {
        return Err("group_frames must be at least 1".to_string());
    }
    let dim = bundle.latent_dim;
    if dim == 0 || !latents.len().is_multiple_of(dim) {
        return Err(format!(
            "latent count {} is not a multiple of latent_dim {dim}",
            latents.len()
        ));
    }
    let group_len = group_frames
        .checked_mul(dim)
        .ok_or_else(|| "group size overflows".to_string())?;
    let mut state = init_state(&bundle.mimi_state);
    let mut audio = Vec::new();
    for group in latents.chunks(group_len) {
        let frames = group.len() / dim;
        let tensor = Tensor::f32(vec![1, frames, dim], group.to_vec())?;
        let (samples, next_state) = backend
            .decode(&tensor, state)
            .map_err(|e| format!("decoder failed: {e}"))?;
        check_state_len(&next_state, bundle.mimi_state.len(), "decoder state")?;
        state = next_state;
        audio.extend(samples);
    }
    Ok(audio)
}
