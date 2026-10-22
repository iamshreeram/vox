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

- Conversations are still turn-based. You cannot interrupt the assistant while
  it is speaking; wait for the reply to finish, then talk.
- The wake word detector is muted while a conversation session runs, and for
  1 second after it ends.
- Ambient listening must be off while a conversation runs. The wake word must
  be off as well, unless its action is set to talk with the assistant.

## Roadmap / TODO

- [ ] (a) Overlay live transcript: show what you said and the reply on the
      recording overlay.
- [ ] (b) Dedicated Assistant chat window. Typing input is deferred.
- [ ] (c) Barge-in on headphones: talk over the assistant to interrupt it.
- [ ] (d) Speakers, using echo cancellation. Starts only after a measured spike.
- [ ] (e) Cross-session memory. This needs its own security design first.
