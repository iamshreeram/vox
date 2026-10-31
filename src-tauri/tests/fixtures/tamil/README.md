# Tamil TTS parity fixtures

These three JSON files hold test numbers only: token ids, chunk texts, frame counts, a few latent values and an audio
RMS. They contain no audio and no model weights.

They were produced by running the published reference runtime (MIT licence, KevinAHM, `pocket_tts_onnx.py`) on the
Pocket TTS Tamil int4 ONNX pack by Prasad Vittaldev, which is derived from Kyutai's Pocket TTS (all CC-BY-4.0,
https://huggingface.co/prasadvittaldev/pocket-tts-tamil-onnx-int4, https://creativecommons.org/licenses/by/4.0/).
The model was used unmodified.

Regenerate with `scripts/tamil_tts_golden.py` (see its header). The tests that read these files are `#[ignore]`d and
need the pack: `VOX_TAMIL_DIR=<pack folder> cargo test --lib tamil -- --ignored`.
