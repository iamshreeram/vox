# Conversation mode

Conversation mode is a hands-free way to talk with Vox. Vox listens, sends
what you said to your reply backend, speaks the answer, and then listens
again. You do not press a key for every turn.

## What it does

- Vox listens for a turn, then waits for a short pause (the silence setting)
  before it treats the turn as finished.
- Your words are sent to the reply backend. Vox speaks the reply and returns
  to listening.
- Short exchanges are kept as context, so follow-up questions make sense.

## Start and stop

- **Shortcut:** press the conversation shortcut (default `control+option+c`
  on macOS). Press it again to stop.
- **Tray:** choose "Start or stop conversation" from the tray menu.
- **Settings:** open Settings, then Experimental, then the Spoken replies group,
  and use the Start conversation or Stop conversation button.

## What you can say

- **End phrases** are handled locally and never sent to the reply backend:
  "stop", "stop listening", "stop conversation", "end conversation",
  "goodbye", "bye", "that's all", "we're done", "never mind", "exit".
- **Dictation** types or pastes what follows a prefix: "type this ...",
  "type out ...", "write this down ...", "write down ...", "paste this ...",
  "paste ...", "dictate ...".
- **Actions** can open apps and web links, and send media keys (play, pause,
  next). See the safety notes below.
- **Questions** get a spoken answer from the reply backend.

## Context

Vox keeps the last N exchanges (the "turns kept in context" setting) and
sends them with each new turn. Older turns are dropped. The memory is in RAM
only and is cleared when the conversation ends.

Vault notes search can add matching passages from your notes to the context.
It is off by default; see [vault-rag.md](vault-rag.md).

## Privacy

What you say and Vox's replies are sent to your configured reply backend. That
is either a local command-line agent or a model API you configured. Nothing is
stored beyond the in-memory session. Conversation text is not written to disk
by Vox.

## Safety

- Generated text is pasted through the clipboard only. It is never submitted
  automatically: no Enter key is sent.
- Multiline text is refused into terminal-like apps and into apps Vox does not
  recognise, so it cannot run commands by accident.
- Commands proposed by the model are limited to opening apps, http(s) links,
  and media keys. Anything else is refused.

## Settings

| Setting                      | Range       | Default | What it does                                |
| ---------------------------- | ----------- | ------- | ------------------------------------------- |
| Silence before a turn ends   | 600-4000 ms | 1200 ms | Pause length that ends your turn.           |
| Silent turns before stopping | 1-5         | 2       | Stops after this many turns with no speech. |
| Turns kept in context        | 0-12        | 6       | Recent exchanges sent with each turn.       |

The three values are clamped to these ranges. Values outside a range are
set to the nearest limit. They are saved as soon as you move a slider, and
they appear in the Experimental settings page, in the Spoken replies group,
below the speaking-rate setting. That page is shown only when Experimental
features are turned on.

Optional features for conversations (smart turn detection, a latency panel,
personas, and a speaker filter) are described in [Optional voice features](voice-features.md).

## Troubleshooting

- **Nothing happens when I start:** turn on Spoken replies in the speech
  settings first. The start button shows the reason if it cannot start.
- **"No reply backend" or "not set up":** choose and save a reply backend
  (a local command-line agent or a model API) in the spoken reply settings.
- **Vox does not react to me:** turn off ambient listening, and turn off the
  wake word unless its action is set to "Talk with the assistant" (see
  [voice-assistant.md](voice-assistant.md)).
  They must be off while a conversation runs.
- **Vox never hears me:** check that Vox has microphone permission in your
  system privacy settings, and that the right input device is selected.
- **Conversation stops by itself:** it reached the silent-turn or turn limit.
  Raise the values in Settings if you want longer sessions.
