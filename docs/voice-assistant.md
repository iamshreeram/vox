# Voice assistant: wake word actions

Vox can start voice work when it hears its wake phrase. What happens next is
set by the **wake word action**, found under the Wake Word settings (Advanced)
once the wake word is turned on.

## The two actions

**Run one voice request** (default). This is today's behaviour. Say the wake
word, then speak one request. Vox runs it as a voice command, sends it to the
agent bridge, or pastes it as text, the same as before.

**Talk with the assistant**. The wake word starts a spoken back-and-forth
conversation. You speak, the assistant replies out loud, and you can keep going
until the conversation ends.

## How conversation mode differs

- It uses the **Conversation** feature. Spoken replies must be turned on in the
  speech settings, and a reply backend must be set up there.
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
- Ambient listening must be off while a conversation runs. The wake word must
  be off as well, unless its action is set to talk with the assistant.

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
- [x] (c) Barge-in on headphones: talk over the assistant to interrupt it.
      Experimental and opt-in (headphones only); see the limits above.
- [ ] (d) Speakers, using echo cancellation. Starts only after a measured spike.
- [ ] (e) Cross-session memory. This needs its own security design first.
- [ ] (f) Show a start refusal (for example spoken replies being off) when the
      settings window is closed. Today the error only appears in the
      conversation settings, which is easy to miss when you are hands-free.
- [ ] (g) A start chime when the wake word opens a conversation.
- [ ] (h) List the wake word action in the settings search index.
