//! CI-safe tests for the text -> audio pipeline. They use a scripted fake [`PocketBackend`] and a
//! tiny synthetic SentencePiece model built in code, so they need no pack and no environment
//! variable.

use crate::managers::tamil_pocket_core::{
    prepare_voice_state, Bundle, Dtype, Fill, GenParams, LmOut, PocketBackend, Rng, StateSpec,
    Tensor, TensorData, VoiceState,
};
use crate::managers::tamil_pocket_pipeline::{peak_and_rms, synthesize_text};
use crate::managers::tamil_text::{estimate_max_frames, prepare_text_prompt, split_into_chunks};
use crate::managers::tamil_tokenizer::TamilTokenizer;
use std::cell::Cell;

const KIND_NORMAL: u64 = 1;
const KIND_UNKNOWN: u64 = 2;
const KIND_CONTROL: u64 = 3;
const MODEL_TYPE_BPE: u64 = 2;
const LATENT_DIM: usize = 32;
const DECODE_GROUP: usize = 12;

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let low = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(low);
            return;
        }
        out.push(low | 0x80);
    }
}

fn put_key(out: &mut Vec<u8>, field: u64, wire: u64) {
    put_varint(out, (field << 3) | wire);
}

fn put_bytes(out: &mut Vec<u8>, field: u64, data: &[u8]) {
    put_key(out, field, 2);
    put_varint(out, data.len() as u64);
    out.extend_from_slice(data);
}

fn put_uint(out: &mut Vec<u8>, field: u64, value: u64) {
    put_key(out, field, 0);
    put_varint(out, value);
}

fn put_float(out: &mut Vec<u8>, field: u64, value: f32) {
    put_key(out, field, 5);
    out.extend_from_slice(&value.to_le_bytes());
}

fn piece_msg(text: &str, score: f32, kind: u64) -> Vec<u8> {
    let mut msg = Vec::new();
    put_bytes(&mut msg, 1, text.as_bytes());
    put_float(&mut msg, 2, score);
    put_uint(&mut msg, 3, kind);
    msg
}

fn normalizer_msg() -> Vec<u8> {
    let mut msg = Vec::new();
    put_bytes(&mut msg, 1, b"nmt_nfkc");
    put_uint(&mut msg, 3, 1);
    put_uint(&mut msg, 4, 1);
    put_uint(&mut msg, 5, 1);
    msg
}

/// Serialized ModelProto: pieces, a BPE trainer spec with unk id 0, and a normalizer that adds
/// the dummy prefix, collapses whitespace and escapes spaces (no charsmap).
fn model_bytes(pieces: &[(String, u64)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (text, kind) in pieces {
        put_bytes(&mut out, 1, &piece_msg(text, 0.0, *kind));
    }
    let mut trainer = Vec::new();
    put_uint(&mut trainer, 3, MODEL_TYPE_BPE);
    put_uint(&mut trainer, 40, 0);
    put_bytes(&mut out, 2, &trainer);
    put_bytes(&mut out, 3, &normalizer_msg());
    out
}

const LATIN_SYMBOLS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.,;:!?";

/// One piece per character, no merges, so every character is exactly one token.
fn toy() -> TamilTokenizer {
    let mut pieces: Vec<(String, u64)> = vec![
        ("<unk>".to_string(), KIND_UNKNOWN),
        ("<s>".to_string(), KIND_CONTROL),
        ("</s>".to_string(), KIND_CONTROL),
        ("\u{2581}".to_string(), KIND_NORMAL),
    ];
    for ch in LATIN_SYMBOLS.chars() {
        pieces.push((ch.to_string(), KIND_NORMAL));
    }
    TamilTokenizer::from_bytes(&model_bytes(&pieces)).expect("synthetic model parses")
}

/// Engine bundle with the real dimensions and a small KV cache shape (capacity 1000 positions).
fn test_bundle() -> Bundle {
    let spec =
        |index: usize, name: &str, dtype: Dtype, fill: Fill, shape: Vec<usize>, key: &str| {
            StateSpec {
                index,
                input_name: name.to_string(),
                output_name: format!("out_{name}"),
                dtype,
                fill,
                shape,
                module: "m".to_string(),
                key: key.to_string(),
            }
        };
    Bundle {
        sample_rate: 24_000,
        frame_rate: 12.5,
        samples_per_frame: 1920,
        latent_dim: LATENT_DIM,
        conditioning_dim: 1024,
        max_token_per_chunk: 50,
        insert_bos_before_voice: false,
        flow_state: vec![
            spec(
                0,
                "state_0",
                Dtype::F32,
                Fill::Nan,
                vec![2, 1, 1000, 1, 1],
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
                vec![1, 4, 0],
                "previous",
            ),
        ],
        kv_capacity: 1000,
        recommended_frames_after_eos: None,
    }
}

fn no_noise() -> GenParams {
    GenParams {
        temperature: 0.0,
        lsd_steps: 1,
        eos_threshold: -4.0,
    }
}

/// Marker of a chunk: its first real character (token 1; token 0 is the dummy prefix) scaled to
/// (0, 1) so the audio is not clamped and different chunks give different values.
fn marker_of(token_ids: &[i64]) -> f32 {
    let key = token_ids.get(1).or(token_ids.first()).copied().unwrap_or(0);
    key as f32 / 1000.0
}

/// Scripted backend. Every text chunk gets a marker (see `marker_of`). The flow head returns
/// the marker, so with zero noise every generated latent of a chunk equals that chunk's marker,
/// and the decoder turns each latent frame into `samples_per_frame` samples of the same value.
/// No EOS is ever produced, so each chunk runs to its frame limit.
struct Scripted {
    samples_per_frame: usize,
    marker: f32,
    text_calls: Vec<Vec<i64>>,
    fail_text_at: Option<usize>,
    fail_decode: bool,
    /// Generation step (counted across chunks) whose EOS logit fires; `None` never fires.
    eos_at: Option<usize>,
    generation_steps: usize,
    flow_calls: usize,
    /// First F32 value of the state handed to every empty-sequence (prefix) step.
    prefix_inputs: Vec<f32>,
    /// Frames of each decode call, in order.
    decode_calls: Vec<usize>,
    /// When set, every decode call returns exactly these samples.
    decode_output: Option<Vec<f32>>,
}

impl Scripted {
    fn new() -> Scripted {
        Scripted {
            samples_per_frame: 1920,
            marker: -7.0,
            text_calls: Vec::new(),
            fail_text_at: None,
            fail_decode: false,
            flow_calls: 0,
            eos_at: None,
            generation_steps: 0,
            prefix_inputs: Vec::new(),
            decode_calls: Vec::new(),
            decode_output: None,
        }
    }
}

fn first_f32(state: &[Tensor]) -> f32 {
    state
        .iter()
        .find_map(|tensor| match &tensor.data {
            TensorData::F32(values) => values.first().copied(),
            _ => None,
        })
        .unwrap_or(f32::NAN)
}

impl PocketBackend for Scripted {
    fn text_embed(&mut self, token_ids: &[i64]) -> Result<Tensor, String> {
        if self.fail_text_at == Some(self.text_calls.len()) {
            return Err("boom".to_string());
        }
        self.text_calls.push(token_ids.to_vec());
        self.marker = marker_of(token_ids);
        let len = token_ids.len();
        Tensor::f32(vec![1, len, 1024], vec![self.marker; len * 1024])
    }

    fn lm_step(
        &mut self,
        sequence: &Tensor,
        _text_embeddings: &Tensor,
        mut state: Vec<Tensor>,
    ) -> Result<LmOut, String> {
        if sequence.shape[1] == 0 {
            self.prefix_inputs.push(first_f32(&state));
        }
        for tensor in state.iter_mut() {
            if let TensorData::F32(values) = &mut tensor.data {
                for value in values.iter_mut() {
                    *value = self.marker;
                }
            }
        }
        let eos_logit = if sequence.shape[1] > 0 {
            let step = self.generation_steps;
            self.generation_steps += 1;
            if self.eos_at == Some(step) {
                5.0
            } else {
                -10.0
            }
        } else {
            -10.0
        };
        Ok(LmOut {
            conditioning: vec![self.marker; 1024],
            eos_logit,
            state,
        })
    }

    fn flow(
        &mut self,
        conditioning: &[f32],
        _s: f32,
        _t: f32,
        _x: &[f32],
    ) -> Result<Vec<f32>, String> {
        self.flow_calls += 1;
        Ok(vec![
            conditioning.first().copied().unwrap_or(0.0);
            LATENT_DIM
        ])
    }

    fn decode(
        &mut self,
        latents: &Tensor,
        state: Vec<Tensor>,
    ) -> Result<(Vec<f32>, Vec<Tensor>), String> {
        if self.fail_decode {
            return Err("boom".to_string());
        }
        let frames = latents.shape[1];
        self.decode_calls.push(frames);
        let samples = match &self.decode_output {
            Some(fixed) => fixed.clone(),
            None => {
                let data = latents.as_f32()?;
                let mut out = Vec::new();
                for frame in 0..frames {
                    out.extend(vec![data[frame * LATENT_DIM]; self.samples_per_frame]);
                }
                out
            }
        };
        Ok((samples, state))
    }
}

/// Backend, bundle, tokenizer and a voice state made from a 4-frame voice embedding.
fn setup() -> (Scripted, Bundle, TamilTokenizer, VoiceState) {
    let bundle = test_bundle();
    let tok = toy();
    let mut backend = Scripted::new();
    let emb = Tensor::f32(vec![1, 4, 1024], vec![0.5; 4 * 1024]).expect("embedding");
    let voice = prepare_voice_state(&mut backend, &bundle, &emb, None).expect("voice state");
    (backend, bundle, tok, voice)
}

fn sentence(first: char) -> String {
    let mut text = String::from(first);
    text.push_str(&"a".repeat(29));
    text.push('.');
    text
}

fn three_sentences() -> String {
    format!("{} {} {}", sentence('A'), sentence('B'), sentence('C'))
}

fn ids_of(tok: &TamilTokenizer, chunk: &str) -> Vec<i64> {
    let (prepared, _) = prepare_text_prompt(chunk).expect("chunk prepares");
    tok.encode(&prepared).into_iter().map(i64::from).collect()
}

#[test]
fn single_short_text_audio_length_is_frames_times_samples_per_frame() {
    let (mut backend, bundle, tok, voice) = setup();
    let text = "Hello there.";
    let ids = ids_of(&tok, text);
    let frames = estimate_max_frames(ids.len());

    let audio = synthesize_text(
        &mut backend,
        &bundle,
        &tok,
        &voice,
        text,
        &no_noise(),
        &mut Rng::new(1),
        &|| false,
    )
    .expect("synthesis succeeds");

    assert_eq!(backend.text_calls, vec![ids]);
    assert_eq!(backend.decode_calls.iter().sum::<usize>(), frames);
    assert_eq!(audio.len(), frames * 1920);
}

#[test]
fn three_chunks_are_embedded_in_order_and_decoded_in_one_pass() {
    let (mut backend, bundle, tok, voice) = setup();
    let text = three_sentences();
    let chunks = split_into_chunks(&tok, &text, 50).expect("chunks");
    assert_eq!(chunks.len(), 3, "the test text must span three chunks");

    let audio = synthesize_text(
        &mut backend,
        &bundle,
        &tok,
        &voice,
        &text,
        &no_noise(),
        &mut Rng::new(1),
        &|| false,
    )
    .expect("synthesis succeeds");

    let expected_ids: Vec<Vec<i64>> = chunks.iter().map(|c| ids_of(&tok, c)).collect();
    assert_eq!(backend.text_calls, expected_ids);

    // Each chunk runs to its own frame limit and its latents all equal its marker. The audio is
    // therefore the chunks' blocks, in order.
    let mut expected_audio = Vec::new();
    let mut total_frames = 0usize;
    for ids in &expected_ids {
        let frames = estimate_max_frames(ids.len());
        total_frames += frames;
        let marker = marker_of(ids);
        assert!(marker > 0.0 && marker < 1.0, "marker must stay unclamped");
        expected_audio.extend(vec![marker; frames * 1920]);
    }
    assert_eq!(audio, expected_audio);

    // One decode_latents pass over the concatenated latents: every backend call is a full group
    // of 12 frames except possibly the last one. Per-chunk decoding would break this.
    let expected_calls = total_frames.div_ceil(DECODE_GROUP);
    assert_eq!(backend.decode_calls.len(), expected_calls);
    let (last, full) = backend.decode_calls.split_last().expect("decode calls");
    assert!(full.iter().all(|&frames| frames == DECODE_GROUP));
    assert_eq!(*last, total_frames - DECODE_GROUP * full.len());
}

#[test]
fn each_chunk_starts_from_the_untouched_voice_state() {
    let (mut backend, bundle, tok, voice) = setup();
    let before = format!("{voice:?}");
    let voice_value = voice.state[0].as_f32().expect("f32 cache")[0];

    synthesize_text(
        &mut backend,
        &bundle,
        &tok,
        &voice,
        &three_sentences(),
        &no_noise(),
        &mut Rng::new(1),
        &|| false,
    )
    .expect("synthesis succeeds");

    assert_eq!(format!("{voice:?}"), before, "voice state must not change");
    // The last three prefix steps are the three chunks; each must see the voice state, not the
    // state left behind by the previous chunk.
    let chunk_prefixes = &backend.prefix_inputs[backend.prefix_inputs.len() - 3..];
    assert_eq!(chunk_prefixes, &[voice_value; 3]);
}

#[test]
fn whitespace_and_empty_text_is_a_no_op() {
    for text in ["", "   ", "\n\t \r"] {
        let (mut backend, bundle, tok, voice) = setup();
        let audio = synthesize_text(
            &mut backend,
            &bundle,
            &tok,
            &voice,
            text,
            &no_noise(),
            &mut Rng::new(1),
            &|| false,
        )
        .expect("empty input is not an error");
        assert!(audio.is_empty(), "{text:?} must produce no audio");
        assert!(backend.text_calls.is_empty());
        assert_eq!(backend.flow_calls, 0);
        assert!(backend.decode_calls.is_empty());
    }
}

#[test]
fn cancel_before_the_first_chunk_makes_no_backend_call() {
    let (mut backend, bundle, tok, voice) = setup();
    let result = synthesize_text(
        &mut backend,
        &bundle,
        &tok,
        &voice,
        &three_sentences(),
        &no_noise(),
        &mut Rng::new(1),
        &|| true,
    );
    assert_eq!(result, Err("cancelled".to_string()));
    assert!(backend.text_calls.is_empty());
    assert!(backend.decode_calls.is_empty());
}

#[test]
fn cancel_before_chunk_two_stops_after_chunk_one_without_decoding() {
    let (mut backend, bundle, tok, voice) = setup();
    let checks = Cell::new(0usize);
    let cancelled = || {
        let seen = checks.get() + 1;
        checks.set(seen);
        seen >= 2
    };
    let result = synthesize_text(
        &mut backend,
        &bundle,
        &tok,
        &voice,
        &three_sentences(),
        &no_noise(),
        &mut Rng::new(1),
        &cancelled,
    );
    assert_eq!(result, Err("cancelled".to_string()));
    assert_eq!(backend.text_calls.len(), 1, "only chunk one is generated");
    assert!(backend.flow_calls > 0, "chunk one generation ran");
    assert!(backend.decode_calls.is_empty(), "no decode after cancel");
}

#[test]
fn non_finite_decoder_output_is_an_error() {
    for bad in [f32::NAN, f32::INFINITY] {
        let (mut backend, bundle, tok, voice) = setup();
        backend.decode_output = Some(vec![0.1, bad]);
        let result = synthesize_text(
            &mut backend,
            &bundle,
            &tok,
            &voice,
            "Hello there.",
            &no_noise(),
            &mut Rng::new(1),
            &|| false,
        );
        assert_eq!(result, Err("non-finite audio".to_string()));
    }
}

#[test]
fn samples_above_one_are_clamped() {
    let (mut backend, bundle, tok, voice) = setup();
    backend.decode_output = Some(vec![1.7, -3.0, 0.5]);
    let audio = synthesize_text(
        &mut backend,
        &bundle,
        &tok,
        &voice,
        "Hello there.",
        &no_noise(),
        &mut Rng::new(1),
        &|| false,
    )
    .expect("synthesis succeeds");

    let expected: Vec<f32> = [1.0, -1.0, 0.5].repeat(backend.decode_calls.len());
    assert_eq!(audio, expected);
    assert!(audio.iter().all(|s| (-1.0..=1.0).contains(s)));
}

#[test]
fn backend_text_error_mid_way_propagates_with_chunk_context() {
    let (mut backend, bundle, tok, voice) = setup();
    backend.fail_text_at = Some(1);
    let err = synthesize_text(
        &mut backend,
        &bundle,
        &tok,
        &voice,
        &three_sentences(),
        &no_noise(),
        &mut Rng::new(1),
        &|| false,
    )
    .expect_err("second chunk fails");
    assert!(err.contains("boom"), "cause kept: {err}");
    assert!(err.contains("chunk 2 of 3"), "context added: {err}");
    assert_eq!(backend.text_calls.len(), 1);
    assert!(backend.decode_calls.is_empty());
}

#[test]
fn decoder_error_propagates_with_context() {
    let (mut backend, bundle, tok, voice) = setup();
    backend.fail_decode = true;
    let err = synthesize_text(
        &mut backend,
        &bundle,
        &tok,
        &voice,
        "Hello there.",
        &no_noise(),
        &mut Rng::new(1),
        &|| false,
    )
    .expect_err("decoder fails");
    assert!(err.contains("boom"), "cause kept: {err}");
    assert!(err.contains("decoding failed"), "context added: {err}");
}

#[test]
fn frames_after_eos_default_is_guess_plus_two_and_bundle_recommendation_wins() {
    // Two words -> frames_after_eos_guess 3; seven words -> guess 1.
    let short = "Hello there.";
    let long = "Hello there friend how are you today.";
    assert_eq!(prepare_text_prompt(short).expect("prepares").1, 3);
    assert_eq!(prepare_text_prompt(long).expect("prepares").1, 1);

    // Frames generated = first EOS step k + frames_after_eos.
    let cases = [
        (short, 5, None, 5 + 3 + 2),
        (short, 0, None, 3 + 2),
        (long, 5, None, 5 + 1 + 2),
        (short, 5, Some(4), 5 + 4),
        (long, 5, Some(0), 5),
        (long, 5, Some(7), 5 + 7),
    ];
    for (text, eos_at, recommended, expected) in cases {
        let (mut backend, mut bundle, tok, voice) = setup();
        bundle.recommended_frames_after_eos = recommended;
        backend.eos_at = Some(eos_at);
        synthesize_text(
            &mut backend,
            &bundle,
            &tok,
            &voice,
            text,
            &no_noise(),
            &mut Rng::new(1),
            &|| false,
        )
        .expect("synthesis succeeds");
        let frames: usize = backend.decode_calls.iter().sum();
        assert_eq!(
            frames, expected,
            "text {text:?}, eos at {eos_at}, recommended {recommended:?}"
        );
    }
}

#[test]
fn peak_and_rms_of_known_samples() {
    let (peak, rms) = peak_and_rms(&[0.5, -1.0, 0.0]);
    assert_eq!(peak, 1.0);
    assert!((rms - (1.25f32 / 3.0).sqrt()).abs() < 1e-6, "rms {rms}");
    assert_eq!(peak_and_rms(&[]), (0.0, 0.0));
}
