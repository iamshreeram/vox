# Third-party notices

Vox does not ship the models listed here. They are downloaded only when you choose to install them, and you can uninstall them at any time. See [model-provenance.md](model-provenance.md) for the sources, the pinned checksums, and the evidence for each licence.

## Phonon text-to-speech model (`gradium/phonon-31m-en`)

- **Project:** Gradium Phonon, Hugging Face model `gradium/phonon-31m-en` (commit `541f5752bc3c79583068bb205bfda5eb39aaa08a`)
- **Licence:** Apache-2.0 (model weights)
- **Links:** https://huggingface.co/gradium/phonon-31m-en
- **Attribution:** Phonon text-to-speech model (gradium/phonon-31m-en), Gradium. Weights licensed under the Apache License 2.0. Built on Kyutai Pocket TTS and the Mimi codec.

## Phonon runtime (`ptts`)

- **Project:** `ptts`, a Rust crate from the `xn-ptts` repository
- **Licence:** MIT OR Apache-2.0
- **Links:** https://github.com/gradium-ai/xn-ptts
- **Attribution:** `ptts` runtime for Phonon, licensed under MIT OR Apache-2.0. Built on Kyutai Pocket TTS and the Mimi codec.

## Smart Turn v3.2 (`smart-turn-v3.2-cpu.onnx`)

- **Project:** Smart Turn, pipecat-ai
- **Licence:** BSD 2-Clause License
- **Links:** https://github.com/pipecat-ai/smart-turn · https://huggingface.co/pipecat-ai/smart-turn-v3
- **Attribution:** Smart Turn v3.2 model (smart-turn-v3.2-cpu.onnx), Copyright (c) 2024–2025, Daily. Licensed under the BSD 2-Clause License. Source: https://github.com/pipecat-ai/smart-turn

## WeSpeaker ResNet34, trained on VoxCeleb (`wespeaker_en_voxceleb_resnet34.onnx`)

- **Project:** WeSpeaker (https://github.com/wenet-e2e/wespeaker), ONNX file published in sherpa-onnx (https://github.com/k2-fsa/sherpa-onnx)
- **Licence:** UNVERIFIED. No licence could be established for this model. See the open questions in [model-provenance.md](model-provenance.md).
- **Links:** https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-recongition-models
- **Attribution:** WeSpeaker ResNet34 speaker model, trained on VoxCeleb, by the WeSpeaker authors (https://github.com/wenet-e2e/wespeaker). The ONNX file is published by the sherpa-onnx project (https://github.com/k2-fsa/sherpa-onnx). This credit names the authors and states no licence; the licence is still being settled (see above).

## Tamil voice: Pocket TTS Tamil int4 ONNX pack (`prasadvittaldev/pocket-tts-tamil-onnx-int4`)

- **Project:** Pocket TTS Tamil, int4 ONNX pack, by Prasad Vittaldev (https://huggingface.co/prasadvittaldev/pocket-tts-tamil-onnx-int4)
- **Licence:** CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/)
- **Links:** https://huggingface.co/prasadvittaldev/pocket-tts-tamil-onnx-int4 · https://creativecommons.org/licenses/by/4.0/
- **Attribution:** Pocket TTS Tamil by Prasad Vittaldev, based on Kyutai (CC BY 4.0). The model weights are used unmodified. The Rust inference code in Vox is an independent implementation of the published algorithm. The Tamil pack is downloaded by `scripts/download_tamil_tts.sh` only when you run it. Vox does not ship the pack.
- **Test data:** `src-tauri/tests/fixtures/tamil` contains only derived test numbers (token ids, frame counts, and similar values). It contains no model weights and no audio.

## Kyutai Pocket TTS and Mimi codec

- **Project:** Pocket TTS and the Mimi codec, by Kyutai (https://kyutai.org/), Hugging Face model `kyutai/pocket-tts` (https://huggingface.co/kyutai/pocket-tts)
- **Licence:** CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/)
- **Links:** https://huggingface.co/kyutai/pocket-tts · https://creativecommons.org/licenses/by/4.0/
- **Attribution:** Pocket TTS and the Mimi codec, Kyutai. Licensed under CC BY 4.0. The Tamil voice and the Phonon model above are built on these.

## Reference runtime for the Tamil voice: `pocket_tts_onnx.py` (KevinAHM)

- **Project:** `pocket_tts_onnx.py`, a reference ONNX runtime for Pocket TTS, published by KevinAHM (https://huggingface.co/KevinAHM/pocket-tts-onnx)
- **Licence:** As stated in the published README: code under Apache 2.0, models under CC BY 4.0 (inherited from kyutai/pocket-tts). The Apache 2.0 licence text is at https://www.apache.org/licenses/LICENSE-2.0. The file itself has no licence header and no copyright line.
- **Links:** https://huggingface.co/KevinAHM/pocket-tts-onnx
- **Attribution:** Reference runtime `pocket_tts_onnx.py`, as published by KevinAHM. Vox used it to check its own Tamil inference output. The Rust code in Vox is an independent implementation of the published algorithm.
