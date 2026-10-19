# Model provenance and licence attribution

This page covers the two optional voice models that Vox can download when you press **Install**. The values in the tables are copied by script from `src-tauri/src/managers/aux_model.rs`, the file that pins them.

**Date checked:** 2026-10-19 (UTC). Every licence fact below was read from the upstream source on that date. Check again before each release.

## What Vox does not ship

- The two models are **not bundled** in the Vox app, and they are **not stored** in this repository.
- Vox downloads a model **only when you choose to install it**. It does not download either model on its own.
- You can **uninstall** a model at any time from Settings. Vox then deletes the model's folder.
- Vox uses a downloaded file only after its size and SHA-256 match the values pinned in `aux_model.rs`.

## 1. Smart Turn v3.2 (CPU, ONNX)

| Field                        | Value                                                                                                                                                                                                                                                                                          |
| ---------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Purpose                      | Turn-end detection for voice conversations (decides whether you have finished speaking)                                                                                                                                                                                                        |
| File                         | `smart-turn-v3.2-cpu.onnx`                                                                                                                                                                                                                                                                     |
| Download URL 1 (tried first) | `https://raw.githubusercontent.com/pipecat-ai/pipecat/main/src/pipecat/audio/turn/smart_turn/data/smart-turn-v3.2-cpu.onnx`                                                                                                                                                                    |
| Download URL 2 (fallback)    | `https://huggingface.co/pipecat-ai/smart-turn-v3/resolve/main/smart-turn-v3.2-cpu.onnx`                                                                                                                                                                                                        |
| Size (bytes)                 | 8679182                                                                                                                                                                                                                                                                                        |
| SHA-256                      | `2bb026316b14a660486a75b1733cd3fbab8c2fd0314dc9af7be49f8cca967e4f`                                                                                                                                                                                                                             |
| Upstream project             | Smart Turn, by the pipecat-ai project. Repository: https://github.com/pipecat-ai/smart-turn . Model card: https://huggingface.co/pipecat-ai/smart-turn-v3                                                                                                                                      |
| Licence                      | **BSD 2-Clause License** (as printed in the model card and the repository LICENSE)                                                                                                                                                                                                             |
| Conversion note              | The model card lists "Checkpoint: 8 MB ONNX (int8 quantized), 32MB ONNX (unquantized)". It does not name the file `smart-turn-v3.2-cpu.onnx` or say who exported it. Which export this file is: **UNVERIFIED: the upstream source does not say which export this file is or who produced it.** |

Note on URL 1: it points at the `main` branch of the pipecat repository, and a branch can change over time. The pinned size and SHA-256 are the integrity check, not the URL.

### Licence evidence (Smart Turn)

| Claim                            | Source URL (fetched 2026-10-19)                                      | Licence as printed            | Exact quote                                                                 |
| -------------------------------- | -------------------------------------------------------------------- | ----------------------------- | --------------------------------------------------------------------------- |
| Model licence                    | https://huggingface.co/pipecat-ai/smart-turn-v3/raw/main/README.md   | BSD 2-Clause (`bsd-2-clause`) | `license: bsd-2-clause`                                                     |
| Model copyright and licence text | https://raw.githubusercontent.com/pipecat-ai/smart-turn/main/LICENSE | BSD 2-Clause License          | `Copyright (c) 2024–2025, Daily`                                            |
| Redistribution condition         | https://raw.githubusercontent.com/pipecat-ai/smart-turn/main/LICENSE | BSD 2-Clause License          | `Redistributions in binary form must reproduce the above copyright notice,` |
| Related code (not the model)     | https://raw.githubusercontent.com/pipecat-ai/pipecat/main/LICENSE    | BSD 2-Clause License          | `Copyright (c) 2024–2026, Daily`                                            |

Note: the pipecat code repository LICENSE gives the years 2024–2026. The smart-turn repository LICENSE gives 2024–2025. The attribution line below uses the smart-turn repository, because that is the source of the model.

Training data: the model card lists the datasets `pipecat-ai/smart-turn-data-v3.1-train` and `pipecat-ai/smart-turn-data-v3.1-test`, and names contributing organisations. **UNVERIFIED: the licence of these datasets was not checked.**

### Required attribution (Smart Turn)

Show this line where the model is credited:

> Smart Turn v3.2 model (smart-turn-v3.2-cpu.onnx), Copyright (c) 2024–2025, Daily. Licensed under the BSD 2-Clause License. Source: https://github.com/pipecat-ai/smart-turn

The BSD 2-Clause licence requires that redistributed copies keep the copyright notice and the licence conditions. Vox does not redistribute this file. The full licence text is in the smart-turn repository LICENSE, linked above.

## 2. WeSpeaker ResNet34 (VoxCeleb), speaker model, ONNX

| Field                     | Value                                                                                                                                                                                                                                                                          |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Purpose                   | Speaker check for the optional speaker filter (compares a voice against a saved voiceprint)                                                                                                                                                                                    |
| File                      | `wespeaker_en_voxceleb_resnet34.onnx`                                                                                                                                                                                                                                          |
| Download URL 1 (only URL) | `https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_en_voxceleb_resnet34.onnx`                                                                                                                                                       |
| Size (bytes)              | 26534365                                                                                                                                                                                                                                                                       |
| SHA-256                   | `5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94`                                                                                                                                                                                                             |
| Upstream project          | WeSpeaker (https://github.com/wenet-e2e/wespeaker), trained on VoxCeleb. The ONNX file is published in the k2-fsa sherpa-onnx release `speaker-recongition-models` (the spelling is upstream's): https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-recongition-models |
| Licence                   | **UNVERIFIED: no single licence for this model could be established from a source that states it for this file.** See the evidence and the open questions below.                                                                                                               |
| Conversion note           | **UNVERIFIED: the upstream release page does not say who exported this file to ONNX.** WeSpeaker's documentation says its runtime `.onnx` models "are exported by Onnxruntime on the checkpoint model". That is a general statement, not one about this file.                  |

### Licence evidence (speaker model)

| Claim                                                                     | Source URL (fetched 2026-10-19)                                                               | Licence as printed                                                                       | Exact quote                                                                                                          |
| ------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| Release says each model has its own licence                               | https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-recongition-models                 | none stated for this file                                                                | `Each model has its own license. Please see the corresponding repository for the specific license of a given model.` |
| File size in the release                                                  | https://api.github.com/repos/k2-fsa/sherpa-onnx/releases/tags/speaker-recongition-models      | (asset metadata)                                                                         | asset `wespeaker_en_voxceleb_resnet34.onnx`, size 26534365 (matches the pinned size)                                 |
| WeSpeaker says VoxCeleb models follow CC BY 4.0                           | https://raw.githubusercontent.com/wenet-e2e/wespeaker/master/docs/pretrained.md               | Creative Commons Attribution 4.0 International (as stated by WeSpeaker, citing VoxCeleb) | `the pretrained model on VoxCeleb follows Creative Commons Attribution 4.0 International License`                    |
| Hugging Face card for a WeSpeaker VoxCeleb ResNet34 model says Apache-2.0 | https://huggingface.co/Wespeaker/wespeaker-voxceleb-resnet34                                  | apache-2.0 (card metadata)                                                               | `"cardData":{"license":"apache-2.0"}`                                                                                |
| WeSpeaker code licence (code, not the model)                              | https://raw.githubusercontent.com/wenet-e2e/wespeaker/master/LICENSE                          | Apache License 2.0                                                                       | `Apache License Version 2.0, January 2004`                                                                           |
| sherpa-onnx code licence (code, not the model)                            | https://raw.githubusercontent.com/k2-fsa/sherpa-onnx/master/LICENSE                           | Apache License 2.0                                                                       | `Apache License Version 2.0, January 2004`                                                                           |
| VoxCeleb dataset terms                                                    | https://www.robots.ox.ac.uk/~vgg/data/voxceleb/ and https://mm.kaist.ac.kr/datasets/voxceleb/ | **UNVERIFIED**                                                                           | Not fetched. Both requests failed (no response from the network proxy), so no terms text was read.                   |

### Open questions for the maintainer (speaker model)

1. **Which licence applies to this file?** The WeSpeaker documentation says CC BY 4.0. The Hugging Face card for a WeSpeaker VoxCeleb ResNet34 model says Apache-2.0. The release page says to check the source repository for each model, and the sherpa-onnx docs page I fetched does not state a licence for this file. These sources conflict, and this page does not resolve the conflict.
2. **Do the VoxCeleb terms limit redistribution or commercial use of a model trained on VoxCeleb?** Open question for the maintainer. The VoxCeleb terms page could not be read, so this is not resolved here.

### Required attribution (speaker model)

**UNVERIFIED: the licence-specific attribution wording for this model.** It depends on the licence in question 1 above. [NOTICE.md](NOTICE.md) carries a neutral credit that names the WeSpeaker authors and the sherpa-onnx project and states no licence. The licence-specific wording waits until the maintainer settles question 1.

## How to verify a downloaded file

Run these in the folder that holds the file. Both values must match the tables above exactly.

```sh
shasum -a 256 smart-turn-v3.2-cpu.onnx      # expect 2bb026316b14a660486a75b1733cd3fbab8c2fd0314dc9af7be49f8cca967e4f
wc -c < smart-turn-v3.2-cpu.onnx            # expect 8679182

shasum -a 256 wespeaker_en_voxceleb_resnet34.onnx        # expect 5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94
wc -c < wespeaker_en_voxceleb_resnet34.onnx              # expect 26534365
```

A file that does not match is not used by Vox.
