# Automatic language detection (experimental)

Some dictation models only understand English. If you speak Hindi or Tamil into one
of them, you get a confident but wrong English transcript. This opt-in feature checks
short dictations that look wrong and, when it finds another language, transcribes them
again with a model that knows that language.

It is **off by default**. Turn it on under Settings, Models, "Automatic language
detection".

## What it does

1. The normal model transcribes the clip as usual.
2. If the transcript looks suspicious (many sound units per word), Vox asks a multilingual Whisper model which language was spoken.
3. If the answer is a non-Latin-script language, the clip is transcribed again:
   - Tamil goes to the Tamil speech pack when it is installed and ready.
   - Other languages use the Whisper model's own transcript.
4. Otherwise the original transcript is kept, unchanged.

Custom words, filler-word removal and Chinese script conversion are applied once, to
the transcript that is finally used.

## When it runs

All of these must be true:

- The setting is on.
- Language is **Auto** and **Translate to English** is off.
- The active model has no language detection of its own and reports token rows (for
  example Parakeet Unified EN).
- A multilingual Whisper model is downloaded and is **not** the active model. (If the
  active model is itself a multilingual Whisper model, it already detects the language;
  Vox only hands Tamil to the Tamil pack.)
- The dictation is batch dictation, not live streaming.

When the setting is off, none of this code runs.

## Cost

- The first check after the Whisper model loads takes about 2 seconds; later checks
  about 0.8 seconds. Only dictations that look non-English pay this cost.
- The extra model stays in memory (roughly 0.6 GB for Whisper Large v3 Turbo Q5)
  according to your model unload setting. Turning the feature off releases it at the
  next idle check (within about 10 seconds).

## Limits

- One- or two-word replies are not detected.
- Only non-Latin-script results replace the original. Spanish, French and other Latin
  script languages are not rerouted.
- Clips longer than 29 seconds are never replaced by the Whisper model's text. Tamil
  clips of any length can still go to the Tamil speech pack when it is ready.
- It was measured on synthetic speech (system voices), not on a large set of recorded
  human speech. Treat the accuracy as unproven and report wrong switches.
- Live streaming push-to-talk is unchanged.

## Privacy

Everything runs on this device. The log records only the decision (gate, source,
language code, timing), never the audio or the text.

## Turning it off

Switch the setting off. Dictation returns to exactly the previous behaviour.
