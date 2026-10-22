# Voice fixtures (unit-test goldens)

Independent reference outputs for the Rust frontends. `manifest.json` lists the
size and sha256 of every file; a Rust test re-hashes them, so a changed or
corrupted fixture fails loudly.

* `smartturn/` - the official Smart Turn v3.2 preprocessing (numpy Whisper
  features, left-pad to 8 s, per-buffer normalisation) applied to locally
  synthesised speech (macOS `say`, voice Samantha), the edge cases, and
  deliberately WRONG variants (`negative/`) that the tests must tell apart.
  Layout of every `*.mel.f32`: little-endian float32, shape [80, 800],
  row-major ([mel][time]). `*.prob.txt` is the model probability.
* `speaker/` - WeSpeaker ResNet34 reference frontend (`wespeaker_ref`: samples
  x 32768, torchaudio Kaldi fbank, 80 bins, Hamming window, dither 0, then
  per-bin mean subtraction) on cropped inputs, plus the embeddings and
  negative controls. `*.fbank.f32` is [frames, 80] float32 row-major,
  `*.embedding.f32` is [256] float32. `tone_*` inputs are saved files; load
  them, do not regenerate them (libm may differ by 1 ulp).
* `speaker_capture_matrix.json` - expected decisions of the capture-shaped
  speaker evaluation (450 ms preroll + speech + 450 ms hangover buffers, four
  synthetic voices). The inputs of that evaluation are NOT committed
  (about 14 MB); the live gate reads them from the directory in the
  `VOX_LIVE_FIXTURES` environment variable and fails if it is missing.
* `gen/` - the Python generators, for provenance only. They read audio from a
  scratch directory and need torch/torchaudio/onnxruntime; the audio comes from
  the macOS `say` voices, whose output is not guaranteed to be bit-reproducible,
  which is why the generated artifacts, not the audio, are the reference.

All audio is synthetic; no personal recordings are included.
