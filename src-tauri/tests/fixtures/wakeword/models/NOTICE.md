# Wake-word test model fixtures

These three ONNX files are redistributed, byte-for-byte, from the
[openWakeWord](https://github.com/dscripka/openWakeWord) project's
`v0.5.1` GitHub release assets, licensed under the Apache License 2.0
(see https://github.com/dscripka/openWakeWord/blob/main/LICENSE).

| File in this directory   | Upstream source                                                                 |
| ------------------------ | -------------------------------------------------------------------------------- |
| `melspectrogram.onnx`    | `https://github.com/dscripka/openWakeWord/releases/download/v0.5.1/melspectrogram.onnx` |
| `embedding_model.onnx`   | `https://github.com/dscripka/openWakeWord/releases/download/v0.5.1/embedding_model.onnx` |
| `classifier.onnx`        | `https://github.com/dscripka/openWakeWord/releases/download/v0.5.1/hey_jarvis_v0.1.onnx` (renamed to match this repo's generic per-model 3-file convention, see `managers::wakeword::WAKEWORD_MODEL_FILES`) |

They are used ONLY by `#[cfg(test)]` code in `src/managers/wakeword.rs`
to exercise the real ONNX inference cascade offline/deterministically in
`cargo test`, matching the phase doc's T1/T2/T3/T9/T10 requirements. They
are NOT bundled into the production app binary or distributed to end
users -- the production path downloads a model's files on first use (see
`ensure_model_downloaded` in `managers::wakeword`).
