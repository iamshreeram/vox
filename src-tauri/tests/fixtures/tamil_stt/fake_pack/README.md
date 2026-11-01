# Tamil STT fake pack (offline plumbing fixture)

This folder holds two tiny ONNX graphs that copy the real Tamil speech pack's tensor names,
dtypes and ranks, with trivial maths. They exist so the Rust backend (`indic_stt_ort.rs`) can
be tested in CI without the 652 MB encoder. They are NOT the real model and must never be
used to judge recognition quality.

## Files

| file             | size (bytes) | role                                                                                                                |
| ---------------- | ------------ | ------------------------------------------------------------------------------------------------------------------- |
| encoder.onnx     | 693          | `audio_signal` [1,80,T] f32, `length` [1] i64 -> `outputs` [1,16,T'] f32, `encoded_lengths` [1] i64, T' = ceil(T/8) |
| ctc_decoder.onnx | 835          | `encoder_output` [1,16,T'] f32 -> `logprobs` [1,T',5633] f32                                                        |

Behaviour (deterministic, no RNG):

- The encoder averages band 0 over blocks of 8 frames (AveragePool, ceil_mode) and repeats the
  result over 16 channels.
- The decoder reads channel 0 per step. Value > 0 selects column 4865; otherwise column 4869.
  `logprobs[t, v] = -(v - target[t])^2`, so the argmax over the Tamil columns (4864..5119 and 5632) is the target column.
- Known input: band 0 = +1 for 8 frames, then -1 for 8 frames, then 0 for 8 frames gives the
  argmax path [4865, 4869, 4869].

The pack has no `vocab.json` or `language_masks.json` on purpose. The pack-status gate therefore
rejects the folder, and the tests load the two sessions through `OrtIndicBackend::load_files`.

## How to regenerate

From the repository root, with the offline venv (needs the `onnx` package from public PyPI):

    /tmp/vox-work/venv3/bin/python scripts/tamil_stt_fake_pack.py

The script overwrites the two `.onnx` files. Keep each file under 60 KB; the fixture total must
stay under 150 KB. Versions used: onnx 1.23.2, opset 13, IR version 9.

## Checking

    cd src-tauri && cargo test --lib indic_stt_ort -- --test-threads=1

To print the graph facts of the fake pack (or of a real pack folder):

    /tmp/vox-work/venv3/bin/python scripts/tamil_stt_ref.py --inspect src-tauri/tests/fixtures/tamil_stt/fake_pack

## Real pack graph facts

Fill these in from the live run (`scripts/tamil_stt_ref.py --inspect <pack dir>`) once the owner
has the real pack. Until then every line is a TODO. The real encoder was NOT downloadable in the
development environment (the CDN is blocked there), so nothing below has been measured.

- TODO encoder.onnx: input names and dtypes (expect audio_signal FLOAT [1,80,T], length INT64 [1])
- TODO encoder.onnx: output names and dtypes (expect outputs FLOAT [1,D,T'], encoded_lengths INT64 [1])
- TODO encoder.onnx: opset imports and IR version
- TODO encoder.onnx: external-data references (expect none if self-contained; a 36.5 MB Constant file was seen in an earlier index, unverified)
- TODO ctc_decoder.onnx: input and output names and dtypes (expect encoder_output FLOAT [1,D,T'], logprobs FLOAT [1,T',5633])
- TODO ctc_decoder.onnx: opset imports, IR version, external-data references
- TODO measured encoder output width D and the frame-to-step ratio on a 2.7 s clip
