# Text-to-speech engines

Vox can read spoken replies with one of three engines. You choose the engine in
Settings, Advanced, Spoken replies, under "Voice engine". The choice applies to
spoken replies only. It does not change how Vox listens.

For the spoken reply style, see [Spoken replies](spoken-replies.md). For the
other optional features, see [Optional voice features](voice-features.md). For
sources and checksums of the downloaded models, see
[Model provenance](model-provenance.md).

## The three engines

| Engine            | What it is                                                                                                                         | Download                                                                                      | Platforms          |
| ----------------- | ---------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------- | ------------------ |
| **System voice**  | The voice built into macOS, spoken with the `say` command.                                                                         | None.                                                                                         | macOS              |
| **Natural voice** | A local neural voice (Kokoro). English only, with 28 voices in American and British accents. Also shown as "Natural voice (beta)". | About 115 MB: an 88 MB model and a 27 MB voice pack. See [Spoken replies](spoken-replies.md). | macOS              |
| **Phonon**        | A local neural text-to-speech model from the Gradium Phonon family. English only, one built-in voice.                              | About 65 MB, in four files. See below.                                                        | macOS only for now |

- **System voice is the default.** Vox uses it until you choose another engine.
- **Phonon is marked "Recommended"** in the engine picker. It does not change the
  default.
- **Nothing downloads by itself.** Phonon's files are downloaded only when you
  press **Install**. Choosing the engine does not start a download.
- **On other platforms** Phonon is shown as unavailable. Linux and Windows have
  not been tested.

## Choosing an engine

1. Open Settings, Advanced, Spoken replies.
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
- If Phonon fails once, the sentence is retried on the fallback voice (the
  system voice).
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
Open Settings, Advanced, Spoken replies, and delete the Phonon voice model. Vox
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
