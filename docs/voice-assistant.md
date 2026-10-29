# Voice assistant: the wake phrase

Vox has one way to start a voice conversation: say **"Hey Jarvis"** (the wake
word), or use the conversation shortcut. The wake word is in Settings > General,
in the Conversation group, and is always available. It is not an Experimental
feature.

## What the wake word does

A wake word detection always starts a conversation. You speak, the assistant
replies out loud, and you can keep going until the conversation ends. The
earlier one-shot "run one voice request" choice has been removed. Older settings
that stored it load as a conversation, and the stored value is rewritten once to
"conversation" when the settings are upgraded. A downgraded build keeps
"conversation"; the old one-shot preference is not restored.

## How conversation mode works

- It uses the **Conversation** feature. Spoken replies must be turned on in
  Settings > Models, and a reply backend must be set up there.
- It does **not** use the agent bridge path. Replies come from the conversation
  reply backend only.
- Context is kept for that conversation only. A new conversation starts fresh.
  There is no memory carried across sessions.
- Nothing is stored beyond the existing opt-in conversation records setting.

## Current limits

- Conversations are turn-based by default. Interrupting the assistant while it
  speaks is experimental and headphones only; see
  [Interrupting by speaking](conversation.md#interrupting-by-speaking-headphones-experimental).
- The wake word detector is muted while a conversation session runs, and for
  1 second after it ends.
- Ambient listening must be off while a conversation runs. The wake word does
  not block a conversation.
- Ambient listening is hidden under Experimental (Settings > General, Listening
  Mode). With Experimental off, a stored ambient setting does nothing: it does
  not block the wake word or the conversation shortcut.

### Known limits of interrupting by speaking

- The system voice can be interrupted but not ducked (lowered in volume).
- Speakers are not supported: the assistant would hear itself.
- Bluetooth headsets may switch profile when the second microphone stream
  opens, so a short audio glitch is possible.
- Spoken replies and a reply backend are required.

## Roadmap / TODO

- [ ] (a) Overlay live transcript: show what you said and the reply on the
      recording overlay.
- [ ] (b) Dedicated Assistant chat window. Typing input is deferred.
- [ ] (c) Barge-in on headphones: talk over the assistant to interrupt it.
      Implemented and experimental; tick this once the live headset check in
      [conversation.md](conversation.md) has passed on a wired and a Bluetooth headset.
      Experimental and opt-in (headphones only); see the limits above.
- [ ] (d) Speakers, using echo cancellation. Starts only after a measured spike.
- [ ] (e) Cross-session memory. This needs its own security design first.
- [ ] (f) Show a start refusal (for example spoken replies being off) when the
      settings window is closed. Today the error only appears in the
      conversation settings, which is easy to miss when you are hands-free.
- [ ] (g) A start chime when the wake word opens a conversation.
- [x] (h) The wake word action is gone: every wake word detection starts a
      conversation, so there is nothing to list in the settings search index.
