# Text-to-speech engines

Vox can read spoken replies with one of three engines. You choose the engine in
Settings > Models, Spoken replies, under "Voice engine". The choice applies to
spoken replies only. It does not change how Vox listens.

For the spoken reply style, see [Spoken replies](spoken-replies.md). For the
other optional features, see [Optional voice features](voice-features.md). For
sources and checksums of the downloaded models, see
[Model provenance](model-provenance.md).

## The three engines

| Engine            | What it is                                                                                                                                                                                                                                                                                                                                     | Download                                                                                      | Platforms          |
| ----------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------- | ------------------ |
| **System voice**  | The voice built into macOS, spoken with the `say` command.                                                                                                                                                                                                                                                                                     | None.                                                                                         | macOS              |
| **Natural voice** | A local neural voice (Kokoro). It speaks English and other Latin-script text, with 28 voices in American and British accents. Sentences in other scripts are spoken by the system voice, except that a later sentence in another script within a reply the Natural voice is already speaking is skipped. Also shown as "Natural voice (beta)". | About 115 MB: an 88 MB model and a 27 MB voice pack. See [Spoken replies](spoken-replies.md). | macOS              |
| **Phonon**        | A local neural text-to-speech model from the Gradium Phonon family. English only, one built-in voice.                                                                                                                                                                                                                                          | About 65 MB, in four files. See below.                                                        | macOS only for now |

- **System voice is the default.** Vox uses it until you choose another engine.
- **Phonon is marked "Recommended"** in the engine picker. It does not change the
  default.
- **Nothing downloads by itself.** Phonon's files are downloaded only when you
  press **Install**. Choosing the engine does not start a download.
- **On other platforms** Phonon is shown as unavailable. Linux and Windows have
  not been tested.

## Choosing an engine

1. Open Settings > Models, Spoken replies.
2. In "Voice engine", choose the engine you want.
3. If the engine is not installed, press **Install** (or **Install from folder**
   for Phonon). Until it is installed, Vox speaks with the system voice.

## Installing Phonon

**Install** downloads the four files over HTTPS from Hugging Face. Vox accepts a
download redirect only when it goes to a Hugging Face host. Each file is checked
against its pinned size and SHA-256 checksum before Vox uses it. A file that does
not match is not used.

### Installing from a folder

Use this when the download is blocked, for example by a proxy, or when you
already have the files.

1. Download the four files yourself from the Hugging Face page of the model at
   the pinned commit:
   `https://huggingface.co/gradium/phonon-31m-en/tree/541f5752bc3c79583068bb205bfda5eb39aaa08a`
2. Put the four files in one folder, `<folder>`. The names must be exact:

   | File                        | Size (bytes) |
   | --------------------------- | -----------: |
   | `model.q8.gguf`             |   64,512,352 |
   | `default-voice.safetensors` |      256,136 |
   | `tokenizer.json`            |      277,967 |
   | `config.json`               |        4,634 |

3. In Vox, choose **Install from folder** and select `<folder>`.

Vox re-checks the size and SHA-256 of each file during import. It ignores every
other file in the folder, so the folder can contain other files.

### Installing all voice models with one script

`scripts/setup-voice-models.sh` downloads Phonon, the Natural voice (Kokoro), the
Smart turn model and the speaker model, checks each file against its pinned size
and SHA-256, and puts each model in the folder Vox reads it from. It is useful
when the in-app download is blocked.

```sh
./scripts/setup-voice-models.sh --dry-run   # show what would happen, change nothing
./scripts/setup-voice-models.sh             # install everything that is missing
./scripts/setup-voice-models.sh --only phonon,smart-turn
```

By default it uses your shell's proxy settings. `--direct` ignores them and connects
straight to the model hosts; use it only if your network policy allows that. A model
that already verifies is skipped, an interrupted download resumes, and an existing
install is replaced only after the whole new set has verified. Each run appends to
`CONTEXT.md` in the repository root: the destination folder and the size, SHA-256,
source and result of every file. Proxy addresses and signed links are never logged.
After a run, quit Vox from the tray and open it again. The tests for the script are
in `scripts/test-setup-voice-models.sh`.

## Tamil voice

Tamil text is spoken by a local neural voice (Pocket TTS Tamil). English is
unchanged: English replies still use the engine you chose above. The Tamil voice
is a separate switch and applies only when its files are installed.

### Installing the Tamil voice

Run the download script from the repository root:

```sh
./scripts/download_tamil_tts.sh
```

The script downloads nine files, about 111 MB, from Hugging Face. It checks each
file against its published size and SHA-256 checksum, skips files that already
verify, and can be run again if a download fails. It puts the files in
`~/Library/Application Support/com.iamshreeram.vox/tts_models/pocket-tts-tamil-int4`.

If your network uses a proxy that blocks large downloads, download the files in a
browser from https://huggingface.co/prasadvittaldev/pocket-tts-tamil-onnx-int4 and
place them in that folder. The file names must match.

### The setting

Open Settings > Models, Spoken replies. The row **Tamil voice** has a switch and
a status line:

- **Installed**: the files are present and the voice is ready.
- **Not installed (missing files: N)**: run the download script.
- **Incomplete download, run the script again**: a file has the wrong size.
- **Tamil voice problem**, with the error: the voice could not start.

The switch is on by default. Turning it off makes Tamil text use the system
voice or the chosen engine, as before. The change applies without a restart.

### Limits

- **One voice.** Vox uses one Tamil voice (Geetha).
- **Digits and English words inside Tamil text are not spoken.** A Tamil
  sentence with a number in it is spoken without the number.
- **Mixed replies.** A full English sentence inside a Tamil reply is skipped, not
  spoken. Tamil sentences are spoken by the Tamil voice.
- **Offline.** After the files are installed, the Tamil voice needs no network.
- **Speed.** On CPU the real-time factor is about 0.25: generating a sentence takes
  about a quarter of the length of its audio.

### Credit

The Tamil voice is Pocket TTS Tamil by Prasad Vittaldev, based on Kyutai, under
CC BY 4.0. The credit and licence links are in
[Third-party notices](NOTICE.md).

## Privacy

- Phonon runs on this computer. The text that is spoken is not sent anywhere
  and is not logged.
- The only network use is the one-time download, when you press **Install**. It
  fetches model files from Hugging Face and sends nothing about you.
- At load time, Vox copies the verified files into a private snapshot and loads
  the model from that copy. A file swapped on disk after verification is
  therefore not loaded.

## English only

Phonon speaks English only. Vox uses Phonon only when the text looks English.
This check is best-effort and can be wrong, especially for short replies or
replies that mix languages. A reply that does not look English is spoken with
the system voice. Other languages need another engine or the system voice.

## Behaviour

- Phonon is used only when it is selected, installed, healthy, and the text
  looks English.
- A reply that is already being spoken does not switch engine part way through.
- A Phonon failure at submit time is retried once on the fallback voice (the
  system voice). A sentence whose background synthesis fails later is skipped, as
  with the Natural voice. The next sentences use the system voice while Phonon is
  marked failed, and the Phonon card shows a message.
- Pressing stop silences speech within about 11 ms. In testing with a fake
  output device, no stale audio played after the stop.
- The first spoken reply after the app starts includes the model load, about 3
  seconds in the measured build. Loading the model when the engine is selected
  is a known follow-up.
- Phonon uses one built-in voice and a fixed speaking rate.

## Troubleshooting

**Install fails with "blocked" or HTTP 407.**
A proxy is asking you to sign in. Vox reports this case separately. Either sign
in to the proxy in another tool and try again, or install from a folder (see
above).

**"The voice files did not pass verification."**
A file is corrupt, incomplete or not the pinned version. Delete the Phonon voice
model in Settings, then install it again, or install it from a folder with the
four files from the pinned commit.

**"The voice files could not be written. Check free disk space."**
Phonon's four files total about 65 MB. Free up space and install again.

**Phonon does not speak, or the system voice speaks instead.**
Check, in order: Phonon is selected; it shows as installed; the reply is in
English; the platform is macOS. If none of these explain it, delete the model and
install it again.

**To remove Phonon and free the space.**
Open Settings > Models, Spoken replies, and delete the Phonon voice model. Vox
asks you to confirm, then deletes the model. You can install it again later.

## Measured numbers

These numbers come from one Apple-silicon Mac, using a development build with
the numeric crates optimised. This is **not a release build**. It is one
machine, so treat the numbers as indicative, not as a guarantee.

| Measure                            | Result on that machine             |
| ---------------------------------- | ---------------------------------- |
| Model load                         | about 2.95 s                       |
| First synthesis                    | 0.34 to 0.41 s for 1.29 s of audio |
| Second synthesis                   | 0.29 to 0.43 s                     |
| Real-time factor (first synthesis) | about 3 to 4 times real time       |
| Output level                       | RMS 0.077, peak 0.35 (voiced)      |

The vendor publishes higher figures: 20 to 26 times real time, and 5 to 9 ms to
first audio, on one M5 core. **We did not reproduce those figures.** They are the
vendor's claims and are not verified by Vox.

## Limits

- **Text-to-speech only.** Phonon does not listen. Speech recognition, active
  listening, barge-in (interrupting by speaking) and echo cancellation are not
  part of this change.
- **macOS only for now.** Linux and Windows are untested.
- **English only,** with one built-in voice.
- **No preview of an engine you have not selected.**
- **Model load on the first reply.** See Behaviour above.
- **Automated tests.** The live and end-to-end Phonon tests are ignored by
  default and run on a Mac with the files available (see
  [Live acceptance](live-acceptance.md)). Continuous integration runs on Linux,
  so the macOS audio paths are covered only by local acceptance.

## Responsible use

Do not use Phonon to impersonate a real person, and do not use it to clone a
voice without that person's consent. This is the model's responsible-use
condition.

## Tamil speech recognition

Tamil speech is recognised by a local model, IndicConformer 600M multilingual
(MIT), run through ONNX Runtime. It is used for push-to-talk and for conversation
mode when the speech language is Tamil. English stays on the speech model you
selected, as before. Tamil speech recognition is separate from the Tamil voice
above: the voice speaks Tamil, and this recognises it.

### Installing the Tamil speech pack

Run the download script from the repository root:

```sh
./scripts/download_indic_stt.sh
```

The script downloads four files, about 676 MB, from Hugging Face. It checks each
file against its pinned size and SHA-256 checksum, skips files that already
verify, and can be run again if a download fails. It puts the files in
`~/Library/Application Support/com.iamshreeram.vox/aux_models/indic_conformer_int8`.

If your network uses a proxy that blocks large downloads, download the files in a
browser from https://huggingface.co/yashwork-byte/indic-conformer-600m-int8-onnx
(revision `9c590ef62f537c30de47df62e525539938549f13`, folder `assets/`) and place
them in that folder. The file names must match.

### How it is chosen

Tamil recognition is used when all three are true:

- the speech language is Tamil (Settings > Models > Speech-to-text, language),
- the **Tamil speech recognition** switch is on (on by default), and
- the pack is installed and the engine has not failed.

When these hold, Tamil appears in the language list even if the active model does
not list Tamil (for example an English-only model), and the picker shows a short
note that Tamil is recognised by the Tamil speech pack. If the pack is removed or
the switch is turned off, the picker shows the fallback language again. The
saved choice stays Tamil and is used again once the pack is back.

The Tamil row shows one of: installed and ready; not installed, with the folder to
install into; an incomplete download; or the engine error.

### Memory and speed

The model needs about 1.1 GB of memory while it is loaded. It loads on the first
Tamil use, not at startup, and unloads when it has been idle for a while. The
first Tamil turn after a load takes longer than later ones.

### Limits

- **Greedy CTC decoding only.** The model's CTC head is used with greedy decoding.
  The transducer head is not used.
- **No translation to English.** When translate-to-English is on, that turn does
  not use the Tamil model. It uses the normal speech model, as before.
- **English words in Tamil script.** English words spoken in a Tamil sentence are
  written in Tamil script. Speech is not translated and the language is not
  detected automatically: Tamil is chosen with the language setting.
- **Long audio is split.** Audio longer than 30 seconds is split at the quietest
  point, so a word spoken across a split can be damaged.
- **Barge-in.** A barge-in that overlaps a Tamil turn can report that recognition is
  busy.
- **Stop words.** The list of Tamil stop words is an initial list and is still being
  reviewed.

### Credit

The model is IndicConformer 600M multilingual by AI4Bharat (MIT). The int8 ONNX
files are a community repackage by yashwork-byte (MIT), pinned to one revision.
The licence texts and links are in [Third-party notices](NOTICE.md).

## Tamil replies after a spoken acknowledgement

In a live conversation Vox may speak a short fixed English acknowledgement ("One moment.")
while the real reply is still being written. The English voices (Phonon and Natural)
cannot speak Tamil. If a mostly-Tamil sentence arrives while one of them is still
speaking, Vox now stops that voice and starts the Tamil voice at once, so the reply is
heard. The acknowledgement is cut short in that case. The system voice is not changed.

If you do not hear Tamil, search the log for `speech route:`. Each decision is one line
with the voice chosen, whether a handover happened and the sentence length; the text
itself is never logged.
