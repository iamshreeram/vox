# Optional voice features

Four optional features change how voice conversations work. All four are off by
default and are switched on in Settings. Two of them need a model that you
download once. None of them sends your voice or your words anywhere, and each
one falls back to the normal behaviour when it cannot run.

For how conversations work in general, see [Conversation](conversation.md). For
the spoken reply style and natural voice, see [Spoken replies](spoken-replies.md).

## 1. Smart turn detection

Normally Vox decides that you have finished speaking after a fixed pause of
silence. The length of that pause is the "Silence before a turn ends" setting,
1200 ms by default.

Smart turn detection asks a small model whether what you have said so far
sounds like a finished thought. If it does, Vox ends your turn after about
0.2 seconds of silence. If it does not, Vox waits longer before it ends the
turn: about 2.5 seconds of silence instead of the fixed pause.

- **Off by default.**
- **One-time download.** The model is about 8.7 MB (8,679,182 bytes).
- **Runs on this computer only.** The model runs on your computer. Audio is not
  sent to any server.
- **Nothing downloads by itself.** The model is downloaded only when you press
  Download in Settings.
- **Missing or slow model.** If the model is not installed, fails, or does not
  answer within 250 ms, Vox uses the fixed silence rule for that turn.
- **Turning it off.** Switch off the Smart turn setting. Vox then uses the fixed
  silence rule for every turn. To free the space, delete the model from the same
  place in Settings.

## 2. Latency panel

The latency panel shows timings for recent conversation turns. Each turn has
these numbers:

- **Wait for end of speech:** the time from your last spoken sound until Vox
  decided your turn was over.
- **Transcription:** the time it took to turn your speech into text.
- **Thinking:** the time the reply backend took to answer.
- **Time to first speech:** the time from the end of your turn until Vox handed
  the first words to the voice. This is not the moment you hear the first sound.
- **Speaker check:** the time the voice check took. This appears only when the
  speaker filter is on.

The panel also shows a total for each turn, and a median and a 90th percentile
across recent turns.

The panel shows numbers only. It never shows or stores audio, and it never shows
or stores the text of what you said or what Vox replied.

The latency panel is **off by default**.

## 3. Personas

A persona is a named preset of reply style and voice. Applying a persona sets
both at once.

There are five built-in personas. They cannot be edited or deleted.

- **Default:** warm, upbeat and concise. It keeps your current voice.
- **Concise:** answers first, in one or two short sentences. Uses the natural
  voice af_sky.
- **Coach:** encouraging and practical, brief, and may end with one guiding
  question. Uses the natural voice am_adam.
- **Narrator:** a warm storytelling cadence with short, calm sentences. Uses the
  natural voice bf_emma.
- **Dry wit:** understated and dry, never mocks you, and stays accurate. Uses the
  natural voice bm_lewis.

**Custom personas.** You can save up to 12 of your own. Each one has:

- a name of up to 40 characters, which may repeat;
- a style of up to 300 characters, with no control characters;
- an optional natural voice, chosen from the list Vox provides.

**Applying, saving and deleting.**

- **Apply** copies the persona's style and voice into your current settings.
- **Save** stores a custom persona you have created or edited.
- **Delete** removes a custom persona. Built-in personas cannot be deleted.

**When a change takes effect.** A new voice applies from the next thing Vox
says. A new style applies from the next conversation, because a conversation
reads its style when it starts. The voice part applies to the natural voice (see
[Spoken replies](spoken-replies.md)). The Default persona has no voice, so it
leaves your current voice unchanged.

## 4. Speaker filter

The speaker filter is a convenience filter. It is **not security and it is not
authentication**. It does not prove who is speaking, and you should not rely on
it to keep anyone from using Vox or from controlling your computer.

What it does: while a conversation runs, Vox compares each utterance with a
voiceprint of your voice and ignores speech that does not sound like you. This
keeps a television or another person in the room from giving Vox commands.

What it cannot do:

- **It can be fooled.** A recording of you, or someone with a similar voice,
  can be accepted.
- **It can wrongly ignore you.** Your own voice can be rejected, for example
  when you have a cold, in a noisy room, with a different microphone, or after a
  long time since enrollment. If this happens, try the **Lenient** sensitivity,
  enroll again, or delete the voiceprint and start again.
- **Short speech is not checked.** Utterances shorter than about 2 seconds are
  accepted without a check.
- **A failed check accepts the speech.** If the check cannot run, Vox accepts
  the speech rather than ignoring it.
- **Repeated rejections are capped.** After five utterances in a row from other
  voices, the fifth is treated as a silent turn, so a conversation does not get
  stuck.
- **Confirmations are protected.** If Vox asks you to confirm an action out loud,
  an answer from a voice that does not match is declined, and the action does
  not run.

**Sensitivity.** The sensitivity setting has three levels:

- **Lenient:** lets more voices through, and wrongly ignores you less often.
- **Balanced:** the default.
- **Strict:** keeps out more other voices, and wrongly ignores you more often.

**Enrollment.** Enrollment takes three short recordings, each about 6 seconds
long. Speak normally in a quiet place. Each recording needs at least 2 seconds
of speech, and the three recordings must sound alike. If they do not, enrollment
fails and you can try again.

**What is stored.** One file, `voiceprint.json`, holds a numeric voiceprint of
your voice, a version number, a fingerprint of the speaker model, and the number
of recordings used. The file stays on this computer in a private folder that only
your user account can read. On macOS and Linux the folder and file are
owner-only. On other systems, enrollment is not offered. No audio is kept, and
nothing is uploaded.

**How to delete it.** Choose **Delete voiceprint** in the speaker filter
settings. This removes the file. Without a voiceprint the filter cannot run.

**Requirements.** The speaker filter is **off by default**. It needs a one-time
speaker model download of about 26.5 MB (26,534,365 bytes). The same rules apply
as for smart turn detection: the download happens only when you press Download,
and it runs on this computer only.

## What has been tested

- **Smart turn detection.** The audio preparation was checked against a reference
  implementation, using computer-generated speech and edge cases. Its decisions
  were checked on computer-generated utterances.
- **Speaker filter.** Accuracy was checked with a capture matrix made by hand from
  four computer-generated voices. The matrix follows the way Vox captures speech,
  including the padding around each utterance. The margin between your voice and
  other voices is thin for shorter clips, which is why checks need at least 2
  seconds of speech.
- **Not yet tested with real microphones or rooms.** None of the accuracy results
  above come from real microphones or real rooms. The real-world false-reject
  rate (your own voice being ignored) is unknown. The real-world false-accept rate
  (other voices getting through) is also unknown.

## 5. Privacy summary

| Feature        | Data used                                                           | Stored where                                                        | Sent anywhere | How to delete                                                   |
| -------------- | ------------------------------------------------------------------- | ------------------------------------------------------------------- | ------------- | --------------------------------------------------------------- |
| Smart turn     | Up to 8 seconds of recent audio, held in memory during a turn       | Nowhere. Audio is not written to disk.                              | No            | Turn the setting off. Delete the model in Settings.             |
| Latency panel  | Timing numbers only                                                 | Shown on screen. No audio or text is stored.                        | No            | Turn the setting off.                                           |
| Personas       | The style text and voice name you choose                            | Your settings on this computer                                      | No (see note) | Delete the custom persona. Built-in personas stay.              |
| Speaker filter | Audio held in memory while a check runs, and one numeric voiceprint | One file, `voiceprint.json`, in a private folder. No audio is kept. | No            | Choose Delete voiceprint. Delete the speaker model in Settings. |

Notes on the table:

- The model downloads fetch model files from the internet, but only after you
  press Download. They send nothing about you.
- When you use personas, the style text is part of the instructions given to the
  reply backend during a conversation, and to the spoken reply composer if it is
  turned on. This is the only way a persona reaches another program.

## 6. Troubleshooting

**The filter ignores me.**

- Try the **Lenient** sensitivity.
- Speak for at least 2 seconds in each turn. Shorter speech is not checked.
- Check that the speaker model is installed and that a voiceprint exists.
- Enroll again in a quiet room, using the microphone you normally use.
- If it still ignores you, turn the speaker filter off.

**Smart turn waits too long.**

- When the model thinks you have not finished, Vox waits for about 2.5 seconds of
  silence before it ends the turn. Pause for less time, or turn Smart turn
  detection off.
- If the model is slow, Vox uses the fixed silence rule after 250 ms. It does
  not keep waiting for the model.

**The model download failed.**

- Check your internet connection, then press Download again.
- Vox uses a downloaded file only after its size and checksum match the expected
  values. A file that does not match is not used.
- Until the model is installed, the feature runs in its fallback mode. Other
  features are not affected.
