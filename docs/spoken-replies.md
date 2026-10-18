# Spoken reply style

The spoken reply style (the "composer") turns an agent reply into a short
spoken sentence before Vox reads it aloud. It is optional. With the composer
off, Vox speaks the reply text as it is.

Spoken replies must be turned on first (Settings, Advanced, Spoken Replies).
The composer settings appear below that toggle.

## What the composer does

For each reply that Vox is about to speak, the composer asks a model or a
program to write one short sentence and to pick an emotion label. Vox then
speaks that sentence. The emotion changes the speaking speed and pitch a
little (see "Emotion" below).

If the composer cannot produce a sentence, the fallback option decides what
is spoken instead.

## Modes

Set the mode in "Composer mode".

- **Off.** Replies are spoken as they are. No data leaves the app for this
  purpose.
- **Model provider.** Vox sends the reply to a chat provider
  that you already configured in the post-processing settings. You choose the
  provider and enter a model name.
- **Command.** Vox runs a program that you choose (a command-line agent, for
  example). The program is started for every reply. This mode needs your
  acknowledgement (see "Command mode risk").

## What data is sent, and where

| Mode           | Sent to                                           | Contents                                                                                   |
| -------------- | ------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| Off            | Nowhere                                           | Nothing for the composer.                                                                  |
| Model provider | The provider's base URL shown in the settings     | The agent reply, the style text, and your request if "Send what I asked" is on.            |
| Command        | The program at the command path, on this computer | The same data, passed as one argument. What the program does with it is up to the program. |

The settings screen shows the provider name and base URL, or the command
path, next to the mode, so you can check where the data goes.

## Command mode risk

Command mode runs a program with your user permissions every time Vox speaks
a reply. The reply text, and your request if enabled, is passed to the program
as an argument. Text inside a reply could steer an agent into using tools or
sending data. Vox cannot enforce a tool-free mode, so the configuration of the
program is your responsibility.

Recommendations:

- Prefer a configuration with no tools and no network access.
- Do not put secrets in the extra arguments. Settings can appear in debug
  logs.

Before command mode runs, you must tick the acknowledgement. Changing the
command path, the extra arguments, or the prompt flag clears the
acknowledgement, so you must tick it again and save. "Save command" stays
disabled until the command fields differ from the saved values.

The readiness line shows one of:

- Needs a command path.
- Needs your acknowledgement before it can run.
- Ready.

## Timeouts and latency

- **Timeout.** Each composition has a timeout between 2 and 60 seconds. If it
  expires, the fallback is spoken.
- **Command mode** starts a program for every reply. A typical reply takes
  15 to 20 seconds.
- **Model provider** calls the provider directly. It is usually much faster
  than command mode, and the speed depends on the provider and the model.

## Fallback

"If composing fails" chooses what Vox speaks when the composer times out or
fails:

- **Short notice.** A fixed short sentence.
- **Speak the reply as is.** The reply text, as it would be without the
  composer.
- **Say nothing.**

## Style and "Send what I asked"

- **Style** is a short instruction for the sentence, for example "brief and
  friendly". It is saved when you leave the field or press Enter.
- **Send what I asked**, when on, sends your spoken request to the composer so
  the sentence can answer it. When off, only the agent reply and the style are
  sent.

## Emotion

The composer also picks one emotion label: neutral, cheerful, excited, calm,
concerned, apologetic, or serious. Vox maps each label to a small change in
the speaking speed and pitch of the system voice. The effect is subtle, and it
depends on the voice. Some voices change very little.

## Troubleshooting

- **Preview.** The Preview button composes a sample sentence with the current
  settings and shows it with its emotion label. Nothing is spoken. If Preview
  shows an error, the message is the error from the composer, for example a
  missing command path or a failed provider call.
- **Last spoken reply.** This line shows the outcome of the most recent
  reply. Common messages:
  - "Timed out, spoke the fallback." The composer took longer than the timeout.
    Raise the timeout, or switch to a faster model or provider.
  - "Not set up yet, spoke the fallback." The mode is not fully configured.
    Check the provider or the command path.
  - "Command not acknowledged, spoke the fallback." Tick the acknowledgement
    and save the command.
  - "The composer failed, spoke the fallback." Use Preview to see the error.
  - "The composer reply was too long, spoke the fallback." The composer output
    exceeded the size limit.
  - "The composer returned nothing, spoke the fallback." The program or
    provider gave an empty answer.
  - "The composer reply could not be read, spoke the fallback." The output
    did not contain a sentence Vox could use.
  - "A newer reply replaced this one." A newer reply arrived first, so this one
    was skipped.
- **Errors under a field** come from the settings save. The field goes back
  to its saved value.

## Natural voice (macOS, optional)

The natural voice is a local neural voice (Kokoro) that can read spoken
replies instead of the system voice. It speaks English only and offers 28
voices with American and British accents.

### Turning it on

1. Open Settings, Advanced, Spoken replies.
2. In "Voice engine", choose "Natural voice (beta)".
3. Pick a voice.
4. Press Download.

### The one-time download

- The download is about 115 MB: an 88 MB model and a 27 MB voice pack.
- Both files come from a public GitHub release. Vox checks the size and the
  SHA-256 checksum of each file before it uses them.
- Nothing else is downloaded.
- The model is stored in the app data folder, in `tts_models/kokoro-v1.0`.
  You can delete it from the same settings screen.

### Privacy

Speech is generated on this computer. The text that is spoken is never sent
anywhere and is never logged.

### Behaviour

- The system voice stays the default. The natural voice is used only when
  you choose it.
- If the model is missing or fails, the system voice speaks instead. A reply
  that starts in another script is spoken by the system voice; a sentence in
  another script inside a reply the natural voice is already speaking is
  skipped.
- After a failure, a Retry button appears in the settings.
- Starting to record stops speech immediately.

### Performance

Measured on an Apple-silicon Mac with a release build:

- The first sentence after a cold start (model not yet loaded) began playing
  after about 3 seconds. Checking the downloaded files takes about half a
  second of that.
- Memory use peaked at about 700 MB while the engine is loaded. The engine
  unloads itself after 5 minutes without speech.
- Starting to record silences speech within a few milliseconds.
- A sentence that is already being generated cannot be aborted, so the reply
  that follows an interruption can start a little late.

### Limitations

- English only.
- Emotion and pitch hints from the composer are ignored by the natural voice.
- The speaking rate follows the existing "Speaking rate" setting.
- Numbers, percentages, currency, ordinals and times are spoken out in English
  words; other symbols are read as words or skipped.
