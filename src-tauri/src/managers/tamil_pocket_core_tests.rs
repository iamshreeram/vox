//! Contract tests for the ONNX-independent Pocket TTS core (DESIGN.md Rev 5, S3).
//! These are the authoritative spec: do not weaken or delete assertions.
//! Everything runs against a scripted `FakeBackend`; no ONNX, no tokenizer.

use super::tamil_pocket_core::*;
use std::path::Path;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Trimmed copy of the real pack's bundle.json shape: two flow entries
/// (cache, step) and two mimi entries (bool, and float32 with a zero dim).
const REAL_TRIMMED: &str = r#"{
  "bos_before_voice_file": "bos_before_voice.npy",
  "bundle_name": "tamil",
  "conditioning_dim": 1024,
  "flow_lm_state_manifest": [
    {"dtype": "float32", "fill": "nan", "index": 0, "input_name": "state_0", "key": "cache", "module": "transformer.layers.0.self_attn", "output_name": "out_state_0", "path": "transformer.layers.0.self_attn/cache", "shape": [2, 1, 1000, 16, 64]},
    {"dtype": "int64", "fill": "zeros", "index": 1, "input_name": "state_1", "key": "step", "module": "transformer.layers.0.self_attn", "output_name": "out_state_1", "path": "transformer.layers.0.self_attn/step", "shape": [1]}
  ],
  "mimi_state_manifest": [
    {"dtype": "bool", "fill": "ones", "index": 0, "input_name": "state_0", "key": "first", "module": "decoder.model.0", "output_name": "out_state_0", "path": "decoder.model.0/first", "shape": [1]},
    {"dtype": "float32", "fill": "empty", "index": 1, "input_name": "state_1", "key": "previous", "module": "decoder.model.8", "output_name": "out_state_1", "path": "decoder.model.8/previous", "shape": [1, 128, 0]}
  ],
  "frame_rate": 12.5,
  "insert_bos_before_voice": true,
  "latent_dim": 32,
  "max_token_per_chunk": 50,
  "sample_rate": 24000,
  "samples_per_frame": 1920,
  "schema_version": 2,
  "tokenizer_file": "tokenizer.model"
}"#;

fn base_value() -> serde_json::Value {
    serde_json::from_str(REAL_TRIMMED).expect("fixture json is valid")
}

fn mutated(f: impl FnOnce(&mut serde_json::Value)) -> String {
    let mut value = base_value();
    f(&mut value);
    value.to_string()
}

fn parse_err(json: &str) -> String {
    match Bundle::parse(json) {
        Ok(_) => panic!("expected bundle parse to fail"),
        Err(e) => e,
    }
}

fn spec(
    index: usize,
    input: &str,
    dtype: Dtype,
    fill: Fill,
    shape: Vec<usize>,
    key: &str,
) -> StateSpec {
    StateSpec {
        index,
        input_name: input.to_string(),
        output_name: format!("out_{input}"),
        dtype,
        fill,
        shape,
        module: "m".to_string(),
        key: key.to_string(),
    }
}

/// Small engine bundle: kv capacity is configurable, dims are the real ones.
fn test_bundle(kv: usize, insert_bos: bool) -> Bundle {
    Bundle {
        sample_rate: 24_000,
        frame_rate: 12.5,
        samples_per_frame: 1920,
        latent_dim: 32,
        conditioning_dim: 1024,
        max_token_per_chunk: 50,
        insert_bos_before_voice: insert_bos,
        flow_state: vec![
            spec(
                0,
                "state_0",
                Dtype::F32,
                Fill::Nan,
                vec![2, 1, kv, 16, 64],
                "cache",
            ),
            spec(1, "state_1", Dtype::I64, Fill::Zeros, vec![1], "step"),
        ],
        mimi_state: vec![
            spec(0, "state_0", Dtype::Bool, Fill::Ones, vec![1], "first"),
            spec(
                1,
                "state_1",
                Dtype::F32,
                Fill::Zeros,
                vec![1, 128, 0],
                "previous",
            ),
        ],
        kv_capacity: kv,
        recommended_frames_after_eos: None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Text = 0,
    Lm = 1,
    Flow = 2,
    Decode = 3,
}

/// Scripted backend. Records every call. Generation steps are lm_step calls
/// whose sequence is non-empty; `eos[i]` is the logit of generation step i
/// (default -10.0 = no EOS). Every LM call overwrites F32 state with a counter
/// so state flow is visible.
struct FakeBackend {
    eos: Vec<f32>,
    flow_value: f32,
    fail: Option<(Op, usize)>,
    counts: [usize; 4],
    text_calls: Vec<Vec<i64>>,
    lm_inputs: Vec<(Tensor, Tensor)>,
    prefix_states: Vec<Vec<Tensor>>,
    gen_calls: usize,
    lm_counter: f32,
    flow_calls: Vec<(f32, f32, Vec<f32>)>,
    decode_calls: Vec<(Tensor, Vec<Tensor>)>,
    decode_returned: Vec<Vec<Tensor>>,
}

impl FakeBackend {
    fn new() -> FakeBackend {
        FakeBackend {
            eos: Vec::new(),
            flow_value: 1.0,
            fail: None,
            counts: [0; 4],
            text_calls: Vec::new(),
            lm_inputs: Vec::new(),
            prefix_states: Vec::new(),
            gen_calls: 0,
            lm_counter: 0.0,
            flow_calls: Vec::new(),
            decode_calls: Vec::new(),
            decode_returned: Vec::new(),
        }
    }

    fn with_eos(mut self, eos: Vec<f32>) -> FakeBackend {
        self.eos = eos;
        self
    }

    fn total_calls(&self) -> usize {
        self.counts.iter().sum()
    }

    fn check(&mut self, op: Op) -> Result<(), String> {
        let index = op as usize;
        let nth = self.counts[index];
        self.counts[index] += 1;
        match self.fail {
            Some((failing, at)) if failing == op && at == nth => Err("boom".to_string()),
            _ => Ok(()),
        }
    }

    fn eos_at(&self, generation_step: usize) -> f32 {
        self.eos.get(generation_step).copied().unwrap_or(-10.0)
    }
}

impl PocketBackend for FakeBackend {
    fn text_embed(&mut self, token_ids: &[i64]) -> Result<Tensor, String> {
        self.check(Op::Text)?;
        self.text_calls.push(token_ids.to_vec());
        let len = token_ids.len();
        Tensor::f32(vec![1, len, 1024], vec![0.25; len * 1024])
    }

    fn lm_step(
        &mut self,
        sequence: &Tensor,
        text_embeddings: &Tensor,
        state: Vec<Tensor>,
    ) -> Result<LmOut, String> {
        self.check(Op::Lm)?;
        self.lm_inputs
            .push((sequence.clone(), text_embeddings.clone()));
        let is_generation = sequence.shape[1] > 0;
        if !is_generation {
            self.prefix_states.push(state.clone());
        }
        let eos_logit = if is_generation {
            let value = self.eos_at(self.gen_calls);
            self.gen_calls += 1;
            value
        } else {
            -10.0
        };
        self.lm_counter += 1.0;
        let mark = self.lm_counter;
        let new_state = state
            .into_iter()
            .map(|mut tensor| {
                if let TensorData::F32(values) = &mut tensor.data {
                    for v in values.iter_mut() {
                        *v = mark;
                    }
                }
                tensor
            })
            .collect();
        Ok(LmOut {
            conditioning: vec![0.5; 1024],
            eos_logit,
            state: new_state,
        })
    }

    fn flow(
        &mut self,
        conditioning: &[f32],
        s: f32,
        t: f32,
        x: &[f32],
    ) -> Result<Vec<f32>, String> {
        self.check(Op::Flow)?;
        assert_eq!(conditioning.len(), 1024, "conditioning must be 1024 wide");
        self.flow_calls.push((s, t, x.to_vec()));
        Ok(vec![self.flow_value; 32])
    }

    fn decode(
        &mut self,
        latents: &Tensor,
        state: Vec<Tensor>,
    ) -> Result<(Vec<f32>, Vec<Tensor>), String> {
        self.check(Op::Decode)?;
        let frames = latents.shape[1];
        self.decode_calls.push((latents.clone(), state.clone()));
        let mark = self.decode_calls.len() as f32;
        let new_state: Vec<Tensor> = state
            .into_iter()
            .map(|mut tensor| {
                if let TensorData::F32(values) = &mut tensor.data {
                    for v in values.iter_mut() {
                        *v = mark;
                    }
                }
                tensor
            })
            .collect();
        self.decode_returned.push(new_state.clone());
        Ok((vec![mark; frames * 4], new_state))
    }
}

/// Bit-exact fingerprint of a state (NaN-safe comparison).
fn bits(state: &[Tensor]) -> Vec<(Vec<usize>, Vec<u64>)> {
    state
        .iter()
        .map(|tensor| {
            let values: Vec<u64> = match &tensor.data {
                TensorData::F32(d) => d.iter().map(|v| u64::from(v.to_bits())).collect(),
                TensorData::I64(d) => d.iter().map(|v| *v as u64).collect(),
                TensorData::Bool(d) => d.iter().map(|v| u64::from(*v)).collect(),
            };
            (tensor.shape.clone(), values)
        })
        .collect()
}

fn voice_of(backend: &mut FakeBackend, bundle: &Bundle, frames: usize) -> VoiceState {
    let emb = Tensor::f32(vec![1, frames, 1024], vec![0.5; frames * 1024]).expect("emb");
    prepare_voice_state(backend, bundle, &emb, None).expect("prepare voice")
}

#[allow(clippy::too_many_arguments)]
fn run_chunk(
    backend: &mut FakeBackend,
    bundle: &Bundle,
    voice: &VoiceState,
    ids: &[i64],
    frames_after_eos: usize,
    max_frames: usize,
    params: &GenParams,
    seed: u64,
) -> Result<Vec<f32>, String> {
    let mut rng = Rng::new(seed);
    generate_chunk_latents(
        backend,
        bundle,
        voice,
        ids,
        frames_after_eos,
        max_frames,
        params,
        &mut rng,
    )
}

fn no_noise() -> GenParams {
    GenParams {
        temperature: 0.0,
        lsd_steps: 1,
        eos_threshold: -4.0,
    }
}

// ---------------------------------------------------------------------------
// Bundle parsing
// ---------------------------------------------------------------------------

#[test]
fn bundle_parses_trimmed_real_shape() {
    let b = Bundle::parse(REAL_TRIMMED).expect("trimmed real bundle parses");
    assert_eq!(b.sample_rate, 24_000);
    assert_eq!(b.frame_rate, 12.5);
    assert_eq!(b.samples_per_frame, 1920);
    assert_eq!(b.latent_dim, 32);
    assert_eq!(b.conditioning_dim, 1024);
    assert_eq!(b.max_token_per_chunk, 50);
    assert!(b.insert_bos_before_voice);
    assert_eq!(b.flow_state.len(), 2);
    assert_eq!(b.mimi_state.len(), 2);
    assert_eq!(b.kv_capacity, 1000);
    assert_eq!(b.recommended_frames_after_eos, None);

    let cache = &b.flow_state[0];
    assert_eq!(cache.index, 0);
    assert_eq!(cache.input_name, "state_0");
    assert_eq!(cache.output_name, "out_state_0");
    assert_eq!(cache.dtype, Dtype::F32);
    assert_eq!(cache.fill, Fill::Nan);
    assert_eq!(cache.key, "cache");
    assert_eq!(cache.shape, vec![2, 1, 1000, 16, 64]);
    assert_eq!(cache.module, "transformer.layers.0.self_attn");

    let first = &b.mimi_state[0];
    assert_eq!(first.dtype, Dtype::Bool);
    assert_eq!(first.fill, Fill::Ones);

    let previous = &b.mimi_state[1];
    assert_eq!(previous.dtype, Dtype::F32);
    assert_eq!(previous.fill, Fill::Zeros);
    assert_eq!(previous.shape, vec![1, 128, 0]);
}

#[test]
fn bundle_rejects_bad_schema_version() {
    let json = mutated(|v| v["schema_version"] = serde_json::json!(1));
    assert!(parse_err(&json).contains("schema_version"));
}

#[test]
fn bundle_rejects_wrong_sample_rate() {
    let json = mutated(|v| v["sample_rate"] = serde_json::json!(16000));
    assert!(parse_err(&json).contains("sample_rate"));
}

#[test]
fn bundle_rejects_wrong_latent_dim() {
    let json = mutated(|v| v["latent_dim"] = serde_json::json!(16));
    assert!(parse_err(&json).contains("latent_dim"));
}

#[test]
fn bundle_rejects_wrong_conditioning_dim() {
    let json = mutated(|v| v["conditioning_dim"] = serde_json::json!(512));
    assert!(parse_err(&json).contains("conditioning_dim"));
}

#[test]
fn bundle_rejects_missing_manifest() {
    let json = mutated(|v| {
        v.as_object_mut()
            .expect("object")
            .remove("mimi_state_manifest");
    });
    assert!(parse_err(&json).contains("mimi_state_manifest"));
}

#[test]
fn bundle_rejects_empty_manifest() {
    let json = mutated(|v| v["flow_lm_state_manifest"] = serde_json::json!([]));
    assert!(parse_err(&json).contains("flow_lm_state_manifest"));
}

#[test]
fn bundle_rejects_non_dense_index() {
    let json = mutated(|v| v["flow_lm_state_manifest"][0]["index"] = serde_json::json!(1));
    assert!(parse_err(&json).contains("index"));
    let json = mutated(|v| v["mimi_state_manifest"][1]["index"] = serde_json::json!(3));
    assert!(parse_err(&json).contains("index"));
}

#[test]
fn bundle_rejects_float16_dtype() {
    let json = mutated(|v| v["flow_lm_state_manifest"][0]["dtype"] = serde_json::json!("float16"));
    assert!(parse_err(&json).contains("float16"));
}

#[test]
fn bundle_rejects_duplicate_input_names_within_a_manifest() {
    let json =
        mutated(|v| v["flow_lm_state_manifest"][1]["input_name"] = serde_json::json!("state_0"));
    assert!(parse_err(&json).contains("state_0"));
    let json =
        mutated(|v| v["mimi_state_manifest"][1]["input_name"] = serde_json::json!("state_0"));
    assert!(parse_err(&json).contains("state_0"));
}

#[test]
fn bundle_allows_same_input_name_across_manifests() {
    // The real pack names both manifests state_0..; the two sessions are separate.
    assert!(Bundle::parse(REAL_TRIMMED).is_ok());
}

#[test]
fn bundle_rejects_missing_cache_entry() {
    let json = mutated(|v| v["flow_lm_state_manifest"][0]["key"] = serde_json::json!("cache_x"));
    assert!(parse_err(&json).contains("cache"));
}

#[test]
fn bundle_load_missing_file_is_an_error() {
    let missing = Path::new("/nonexistent/vox-tamil/bundle.json");
    assert!(Bundle::load(missing).is_err());
}

#[test]
fn bundle_recommended_frames_null_or_absent_is_none() {
    let null = mutated(|v| v["model_recommended_frames_after_eos"] = serde_json::Value::Null);
    assert_eq!(
        Bundle::parse(&null)
            .expect("null parses")
            .recommended_frames_after_eos,
        None
    );
    let absent = mutated(|v| {
        v.as_object_mut()
            .expect("object")
            .remove("model_recommended_frames_after_eos");
    });
    assert_eq!(
        Bundle::parse(&absent)
            .expect("absent parses")
            .recommended_frames_after_eos,
        None
    );
}

#[test]
fn bundle_recommended_frames_integer_is_some() {
    let json = mutated(|v| v["model_recommended_frames_after_eos"] = serde_json::json!(4));
    let b = Bundle::parse(&json).expect("integer parses");
    assert_eq!(b.recommended_frames_after_eos, Some(4));
    let zero = mutated(|v| v["model_recommended_frames_after_eos"] = serde_json::json!(0));
    assert_eq!(
        Bundle::parse(&zero)
            .expect("zero parses")
            .recommended_frames_after_eos,
        Some(0)
    );
}

#[test]
fn bundle_recommended_frames_wrong_type_is_an_error() {
    for bad in [
        serde_json::json!(-1),
        serde_json::json!(2.5),
        serde_json::json!("3"),
        serde_json::json!(true),
        serde_json::json!([3]),
    ] {
        let json = mutated(|v| v["model_recommended_frames_after_eos"] = bad.clone());
        let err = parse_err(&json);
        assert!(
            err.contains("model_recommended_frames_after_eos"),
            "error names the key for {bad}: {err}"
        );
    }
}

#[test]
#[ignore = "needs the Tamil pack; set VOX_TAMIL_DIR"]
fn real_bundle_json_from_pack() {
    let Ok(dir) = std::env::var("VOX_TAMIL_DIR") else {
        return;
    };
    let b = Bundle::load(&Path::new(&dir).join("bundle.json")).expect("load real bundle.json");
    assert_eq!(b.flow_state.len(), 18);
    assert_eq!(b.mimi_state.len(), 56);
    assert_eq!(b.kv_capacity, 1000);
    assert_eq!(b.sample_rate, 24_000);
    assert!(b.insert_bos_before_voice);
    assert_eq!(b.max_token_per_chunk, 50);
    assert_eq!(b.recommended_frames_after_eos, None);
}

// ---------------------------------------------------------------------------
// init_state
// ---------------------------------------------------------------------------

#[test]
fn init_state_fills_per_dtype_and_fill() {
    let specs = vec![
        spec(0, "a", Dtype::F32, Fill::Nan, vec![2, 2], "cache"),
        spec(1, "b", Dtype::F32, Fill::Ones, vec![3], "x"),
        spec(2, "c", Dtype::I64, Fill::Zeros, vec![2], "step"),
        spec(3, "d", Dtype::I64, Fill::Ones, vec![2], "y"),
        spec(4, "e", Dtype::Bool, Fill::Ones, vec![2], "first"),
        spec(5, "f", Dtype::Bool, Fill::Zeros, vec![2], "z"),
    ];
    let state = init_state(&specs);
    assert_eq!(state.len(), 6);

    match &state[0].data {
        TensorData::F32(d) => assert!(d.iter().all(|v| v.is_nan()) && d.len() == 4),
        other => panic!("expected f32, got {other:?}"),
    }
    assert_eq!(state[1].data, TensorData::F32(vec![1.0, 1.0, 1.0]));
    assert_eq!(state[2].data, TensorData::I64(vec![0, 0]));
    assert_eq!(state[3].data, TensorData::I64(vec![1, 1]));
    assert_eq!(state[4].data, TensorData::Bool(vec![true, true]));
    assert_eq!(state[5].data, TensorData::Bool(vec![false, false]));
    assert_eq!(state[0].shape, vec![2, 2]);
}

#[test]
fn init_state_zero_size_and_empty_fill() {
    let specs = vec![
        spec(0, "a", Dtype::F32, Fill::Zeros, vec![0], "current_end"),
        spec(1, "b", Dtype::F32, Fill::Zeros, vec![1, 128, 0], "previous"),
    ];
    let state = init_state(&specs);
    assert_eq!(state[0].shape, vec![0]);
    assert_eq!(state[0].data, TensorData::F32(Vec::new()));
    assert_eq!(state[1].shape, vec![1, 128, 0]);
    assert_eq!(state[1].numel(), 0);
}

#[test]
fn init_state_follows_real_manifest_order() {
    let b = Bundle::parse(REAL_TRIMMED).expect("parse");
    let state = init_state(&b.mimi_state);
    assert_eq!(state[0].data, TensorData::Bool(vec![true]));
    assert_eq!(state[1].numel(), 0);
}

#[test]
fn tensor_constructor_rejects_wrong_element_count() {
    assert!(Tensor::f32(vec![2, 2], vec![0.0; 3]).is_err());
    assert!(Tensor::i64(vec![0], Vec::new()).is_ok());
    assert!(Tensor::bool(vec![2], vec![true, false]).is_ok());
    assert!(Tensor::bool(vec![2], vec![true]).is_err());
}

// ---------------------------------------------------------------------------
// prepare_voice_state
// ---------------------------------------------------------------------------

#[test]
fn prepare_voice_state_prepends_bos_and_counts_positions() {
    let bundle = test_bundle(100, true);
    let mut fake = FakeBackend::new();
    let voice = Tensor::f32(vec![1, 2, 1024], vec![1.0; 2 * 1024]).expect("voice");
    let bos = Tensor::f32(vec![1, 1, 1024], vec![9.0; 1024]).expect("bos");

    let state = prepare_voice_state(&mut fake, &bundle, &voice, Some(&bos)).expect("prepare");
    assert_eq!(state.positions, 3);
    assert_eq!(state.state.len(), 2);

    assert_eq!(fake.lm_inputs.len(), 1);
    let (seq, text) = &fake.lm_inputs[0];
    assert_eq!(seq.shape, vec![1, 0, 32]);
    assert_eq!(text.shape, vec![1, 3, 1024]);
    let data = text.as_f32().expect("f32");
    assert!(data[..1024].iter().all(|v| *v == 9.0), "bos comes first");
    assert!(data[1024..].iter().all(|v| *v == 1.0), "voice follows bos");
}

#[test]
fn prepare_voice_state_missing_bos_is_an_error_before_any_call() {
    let bundle = test_bundle(100, true);
    let mut fake = FakeBackend::new();
    let voice = Tensor::f32(vec![1, 2, 1024], vec![1.0; 2 * 1024]).expect("voice");
    assert!(prepare_voice_state(&mut fake, &bundle, &voice, None).is_err());
    assert_eq!(fake.total_calls(), 0);
}

#[test]
fn prepare_voice_state_ignores_bos_when_not_inserted() {
    let bundle = test_bundle(100, false);
    let mut fake = FakeBackend::new();
    let voice = Tensor::f32(vec![1, 4, 1024], vec![1.0; 4 * 1024]).expect("voice");
    let bos = Tensor::f32(vec![1, 1, 1024], vec![9.0; 1024]).expect("bos");
    let state = prepare_voice_state(&mut fake, &bundle, &voice, Some(&bos)).expect("prepare");
    assert_eq!(state.positions, 4);
    assert_eq!(fake.lm_inputs[0].1.shape, vec![1, 4, 1024]);
}

#[test]
fn prepare_voice_state_rejects_bad_voice_shape() {
    let bundle = test_bundle(100, false);
    let mut fake = FakeBackend::new();
    let wrong_dim = Tensor::f32(vec![1, 2, 512], vec![0.0; 1024]).expect("voice");
    assert!(prepare_voice_state(&mut fake, &bundle, &wrong_dim, None).is_err());
    let empty = Tensor::f32(vec![1, 0, 1024], Vec::new()).expect("voice");
    assert!(prepare_voice_state(&mut fake, &bundle, &empty, None).is_err());
    assert_eq!(fake.total_calls(), 0);
}

// ---------------------------------------------------------------------------
// generate_chunk_latents
// ---------------------------------------------------------------------------

#[test]
fn voice_state_is_not_mutated_by_consecutive_chunks() {
    let bundle = test_bundle(1000, false);
    let mut fake = FakeBackend::new().with_eos(vec![-10.0, -10.0, 5.0]);
    let voice = voice_of(&mut fake, &bundle, 3);
    let before = bits(&voice.state);

    let first = run_chunk(
        &mut fake,
        &bundle,
        &voice,
        &[1, 2, 3],
        1,
        10,
        &no_noise(),
        1,
    )
    .expect("chunk a");
    let second =
        run_chunk(&mut fake, &bundle, &voice, &[4, 5], 1, 10, &no_noise(), 2).expect("chunk b");

    assert_eq!(
        bits(&voice.state),
        before,
        "voice state must never be mutated"
    );
    assert!(!first.is_empty() && !second.is_empty());
    // Each chunk's prefix call received a copy of the untouched voice state.
    let prefixes: Vec<_> = fake.prefix_states.iter().skip(1).collect();
    assert_eq!(prefixes.len(), 2);
    assert_eq!(bits(prefixes[0]), before);
    assert_eq!(bits(prefixes[1]), before);
}

#[test]
fn eos_exactly_at_threshold_does_not_trigger() {
    let bundle = test_bundle(1000, false);
    let mut fake = FakeBackend::new().with_eos(vec![-4.0; 4]);
    let voice = voice_of(&mut fake, &bundle, 3);
    let latents = run_chunk(&mut fake, &bundle, &voice, &[7], 0, 4, &no_noise(), 1).expect("chunk");
    assert_eq!(
        latents.len() / 32,
        4,
        "-4.0 is not strictly greater than -4.0"
    );
}

#[test]
fn eos_just_above_threshold_triggers() {
    let bundle = test_bundle(1000, false);
    let mut fake = FakeBackend::new().with_eos(vec![-3.99]);
    let voice = voice_of(&mut fake, &bundle, 3);
    let latents = run_chunk(&mut fake, &bundle, &voice, &[7], 0, 4, &no_noise(), 1).expect("chunk");
    assert!(
        latents.is_empty(),
        "eos at step 0 with 0 frames after eos gives no latents"
    );
}

#[test]
fn eos_at_step_k_yields_k_plus_n_latents() {
    let bundle = test_bundle(1000, false);
    let mut eos = vec![-10.0; 10];
    eos[2] = 2.0;
    let mut fake = FakeBackend::new().with_eos(eos);
    let voice = voice_of(&mut fake, &bundle, 3);
    let latents =
        run_chunk(&mut fake, &bundle, &voice, &[7], 3, 100, &no_noise(), 1).expect("chunk");
    assert_eq!(latents.len() / 32, 2 + 3, "k=2 plus n=3 frames after eos");
    assert_eq!(fake.flow_calls.len(), 5);
    // Breaking step runs the LM but produces no latent: 6 generation steps in total.
    assert_eq!(fake.gen_calls, 6);
}

#[test]
fn no_eos_yields_max_frames_latents() {
    let bundle = test_bundle(1000, false);
    let mut fake = FakeBackend::new();
    let voice = voice_of(&mut fake, &bundle, 3);
    let latents = run_chunk(&mut fake, &bundle, &voice, &[7], 1, 6, &no_noise(), 1).expect("chunk");
    assert_eq!(latents.len(), 6 * 32);
}

#[test]
fn kv_cache_clamp_limits_frames() {
    // kv 100, voice 10, text 5 => room = 100 - 10 - 5 - 8 = 77 (max_frames 200 is clamped).
    let bundle = test_bundle(100, false);
    let mut fake = FakeBackend::new();
    let voice = VoiceState {
        state: init_state(&bundle.flow_state),
        positions: 10,
    };
    let latents = run_chunk(
        &mut fake,
        &bundle,
        &voice,
        &[1, 2, 3, 4, 5],
        1,
        200,
        &no_noise(),
        1,
    )
    .expect("clamped chunk still succeeds");
    assert_eq!(latents.len() / 32, 77);
    assert_eq!(fake.gen_calls, 77);
}

#[test]
fn text_too_long_for_kv_errors_before_any_backend_call() {
    let bundle = test_bundle(100, false);
    let voice = VoiceState {
        state: init_state(&bundle.flow_state),
        positions: 73,
    };

    let mut fake = FakeBackend::new();
    let too_long: Vec<i64> = vec![1; 19]; // 19 + 73 + 8 = 100 >= 100
    let err = run_chunk(&mut fake, &bundle, &voice, &too_long, 1, 10, &no_noise(), 1)
        .expect_err("must refuse");
    assert!(err.contains("text too long for the KV cache"), "{err}");
    assert_eq!(fake.total_calls(), 0);

    let mut fake = FakeBackend::new();
    let fits: Vec<i64> = vec![1; 18]; // 18 + 73 + 8 = 99 < 100 => one frame of room
    let latents =
        run_chunk(&mut fake, &bundle, &voice, &fits, 1, 10, &no_noise(), 1).expect("fits");
    assert_eq!(latents.len(), 32);
}

#[test]
fn temperature_zero_feeds_zero_noise_and_latent_equals_flow_output() {
    let bundle = test_bundle(1000, false);
    let mut fake = FakeBackend::new();
    fake.flow_value = 0.25;
    let voice = voice_of(&mut fake, &bundle, 3);
    let latents = run_chunk(&mut fake, &bundle, &voice, &[7], 1, 2, &no_noise(), 1).expect("chunk");
    assert_eq!(fake.flow_calls[0].2, vec![0.0; 32], "x starts at zero");
    assert_eq!(latents[..32], [0.25; 32], "lsd 1: latent = 0 + flow * 1.0");
}

#[test]
fn temperature_positive_is_deterministic_per_seed_and_differs_across_seeds() {
    let bundle = test_bundle(1000, false);
    let params = GenParams::default();
    let mut fake_a = FakeBackend::new();
    let voice_a = voice_of(&mut fake_a, &bundle, 3);
    let a = run_chunk(&mut fake_a, &bundle, &voice_a, &[7], 1, 4, &params, 42).expect("a");
    let mut fake_b = FakeBackend::new();
    let voice_b = voice_of(&mut fake_b, &bundle, 3);
    let b = run_chunk(&mut fake_b, &bundle, &voice_b, &[7], 1, 4, &params, 42).expect("b");
    let mut fake_c = FakeBackend::new();
    let voice_c = voice_of(&mut fake_c, &bundle, 3);
    let c = run_chunk(&mut fake_c, &bundle, &voice_c, &[7], 1, 4, &params, 43).expect("c");

    assert_eq!(a, b, "same seed, same latents");
    assert_ne!(a, c, "different seed, different latents");
    assert!(
        fake_a.flow_calls[0].2.iter().any(|v| *v != 0.0),
        "noise is applied"
    );
}

#[test]
fn lsd_two_uses_half_step_pairs_and_dt_half() {
    let bundle = test_bundle(1000, false);
    let mut fake = FakeBackend::new();
    let voice = voice_of(&mut fake, &bundle, 3);
    let params = GenParams {
        temperature: 0.0,
        lsd_steps: 2,
        eos_threshold: -4.0,
    };
    let latents = run_chunk(&mut fake, &bundle, &voice, &[7], 1, 1, &params, 1).expect("chunk");

    assert_eq!(fake.flow_calls.len(), 2);
    assert_eq!((fake.flow_calls[0].0, fake.flow_calls[0].1), (0.0, 0.5));
    assert_eq!((fake.flow_calls[1].0, fake.flow_calls[1].1), (0.5, 1.0));
    assert_eq!(fake.flow_calls[1].2, vec![0.5; 32], "x after one half step");
    // x = 0 + 1.0*0.5 = 0.5, then 0.5 + 1.0*0.5 = 1.0
    assert_eq!(latents, vec![1.0; 32]);
}

#[test]
fn empty_ids_and_zero_lsd_steps_are_errors_without_calls() {
    let bundle = test_bundle(1000, false);
    let mut fake = FakeBackend::new();
    let voice = voice_of(&mut fake, &bundle, 3);
    let before = fake.total_calls();

    assert!(run_chunk(&mut fake, &bundle, &voice, &[], 1, 5, &no_noise(), 1).is_err());
    assert_eq!(fake.total_calls(), before);

    let zero_lsd = GenParams {
        temperature: 0.0,
        lsd_steps: 0,
        eos_threshold: -4.0,
    };
    assert!(run_chunk(&mut fake, &bundle, &voice, &[7], 1, 5, &zero_lsd, 1).is_err());
    assert_eq!(fake.total_calls(), before);
}

#[test]
fn backend_errors_propagate_with_context() {
    let bundle = test_bundle(1000, false);

    let mut fake = FakeBackend::new();
    let voice = voice_of(&mut fake, &bundle, 3);
    fake.fail = Some((Op::Flow, 2));
    let err = run_chunk(&mut fake, &bundle, &voice, &[7], 1, 10, &no_noise(), 1)
        .expect_err("flow failure must surface");
    assert!(err.contains("boom"), "{err}");
    assert!(err.contains("frame 2"), "{err}");

    let mut fake = FakeBackend::new();
    let voice = voice_of(&mut fake, &bundle, 3);
    fake.fail = Some((Op::Text, 0));
    let err = run_chunk(&mut fake, &bundle, &voice, &[7], 1, 10, &no_noise(), 1)
        .expect_err("text failure must surface");
    assert!(err.contains("boom") && err.contains("text"), "{err}");
}

// ---------------------------------------------------------------------------
// decode_latents
// ---------------------------------------------------------------------------

#[test]
fn decode_groups_of_twelve_carry_mimi_state_and_concatenate_audio() {
    let bundle = test_bundle(1000, false);
    let mut fake = FakeBackend::new();
    let latents = vec![0.1_f32; 25 * 32];

    let audio = decode_latents(&mut fake, &bundle, &latents, 12).expect("decode");

    assert_eq!(fake.decode_calls.len(), 3);
    assert_eq!(fake.decode_calls[0].0.shape, vec![1, 12, 32]);
    assert_eq!(fake.decode_calls[1].0.shape, vec![1, 12, 32]);
    assert_eq!(fake.decode_calls[2].0.shape, vec![1, 1, 32]);
    assert_eq!(
        bits(&fake.decode_calls[0].1),
        bits(&init_state(&bundle.mimi_state))
    );
    assert_eq!(
        bits(&fake.decode_calls[1].1),
        bits(&fake.decode_returned[0]),
        "call 2 must receive the state returned by call 1"
    );
    assert_eq!(
        bits(&fake.decode_calls[2].1),
        bits(&fake.decode_returned[1]),
        "call 3 must receive the state returned by call 2"
    );
    assert_eq!(audio.len(), 12 * 4 + 12 * 4 + 4);
    assert!(audio[..48].iter().all(|v| *v == 1.0));
    assert!(audio[48..96].iter().all(|v| *v == 2.0));
}

#[test]
fn decode_empty_latents_makes_no_calls() {
    let bundle = test_bundle(1000, false);
    let mut fake = FakeBackend::new();
    let audio = decode_latents(&mut fake, &bundle, &[], 12).expect("decode");
    assert!(audio.is_empty());
    assert_eq!(fake.total_calls(), 0);
}

#[test]
fn decode_rejects_misaligned_latents_and_zero_group() {
    let bundle = test_bundle(1000, false);
    let mut fake = FakeBackend::new();
    assert!(decode_latents(&mut fake, &bundle, &[0.0; 33], 12).is_err());
    assert!(decode_latents(&mut fake, &bundle, &[0.0; 32], 0).is_err());
    assert_eq!(fake.total_calls(), 0);
}

// ---------------------------------------------------------------------------
// Rng
// ---------------------------------------------------------------------------

#[test]
fn rng_normal_has_zero_mean_and_unit_variance() {
    let mut rng = Rng::new(7);
    let n = 10_000;
    let samples: Vec<f64> = (0..n).map(|_| f64::from(rng.normal())).collect();
    let mean = samples.iter().sum::<f64>() / n as f64;
    let var = samples.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / n as f64;
    assert!(mean.abs() < 0.05, "mean {mean}");
    assert!((var - 1.0).abs() < 0.05, "variance {var}");
}

#[test]
fn rng_same_seed_same_sequence() {
    let mut a = Rng::new(123);
    let mut b = Rng::new(123);
    let seq_a: Vec<f32> = (0..100).map(|_| a.normal()).collect();
    let seq_b: Vec<f32> = (0..100).map(|_| b.normal()).collect();
    assert_eq!(seq_a, seq_b);
}

#[test]
fn rng_seed_zero_is_usable() {
    let mut rng = Rng::new(0);
    let values: Vec<f32> = (0..16).map(|_| rng.normal()).collect();
    assert!(values.iter().any(|v| *v != 0.0));
    assert!(values.iter().all(|v| v.is_finite()));
}
