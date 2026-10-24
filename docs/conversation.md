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
- **Settings:** open Settings > General, then the Voice conversation group (shown
  when the Experimental switch is on), and use the Start conversation or Stop
  conversation button.

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
- **Roles** change who Vox is for the rest of the conversation. See below.

## Roles and lessons

Say it in your own words: "act like a teacher teaching computers to a six
year old", "pretend you are a quiz host", "you are now a pirate". Vox
confirms the role out loud and stays in it until you take it away or the
conversation ends. The role is kept apart from the recent exchanges, so it
does not fall out of context after a few turns. Vox adds no topic or role
filter of its own. The reply model's own rules still apply.

- **Start a task in the same breath.** "Act like a teacher teaching computers
  to a six year old, and start the lesson" confirms the role, then begins the
  lesson. A role with no task is only confirmed; Vox does not start talking.
- **Long answers are spoken in short parts** of up to three sentences, so you
  can interrupt between them. Turn on "Interrupt by speaking" (headphones)
  to cut in with a question.
- **Continue after a question.** Say "and then continue" when you set the
  role and Vox answers each interruption, says "Let's continue." and picks
  up where it stopped. Otherwise say "continue", "go on" or "carry on".
  A part that was cut off is repeated from its start. If a question's own
  answer is interrupted, the unspoken rest of that answer is dropped; the
  lesson itself is kept.
- **Stop the role** by saying "back to normal", "stop acting", "stop the
  role", "drop the role" or "end the role". These are matched only when they
  are the whole sentence, and they clear a paused lesson too. "I am back to
  normal" is treated as an ordinary sentence.
- **Limits.** A role description can be up to 1500 characters. A lesson that nobody has
  touched for ten minutes is forgotten. Nothing is saved after the conversation ends.
  With Vault context on, Vox still only answers: a role can be set but no
  paste, command or job runs.
- Open apps, media keys, dictation, ending the conversation and cancelling a
  job are still handled locally first, whatever the role.

## Handing work to your agent

Some requests need more than an answer. Vox can hand these to your ACP agent.
See [ACP agent](acp-agent.md) for how the agent is set up.

- **What goes to the agent.** In a Hey Jarvis conversation, an open-ended request
  that needs the web, files, a browser, running programs or several steps is
  sent to your ACP agent. Chat and general knowledge questions are still
  answered by the voice model.
- **Local commands still win.** Opening apps, media keys, dictation, memory and
  ending the conversation stay local. They never go to the agent.

### What you need

- An ACP agent that is turned on, with a command set (see [ACP agent](acp-agent.md)).
- **Settings > Agents > "Hand open-ended requests to the agent in conversations"** turned on.
- A reply backend set up. Vox needs it to speak the answer.

### How it works

1. You make a request. Vox says it is on it.
2. The conversation keeps going. You can ask something else.
3. If the agent is slow, Vox says a short progress line after 20 seconds, then
   about once a minute.
4. When the agent answers, Vox speaks the answer in short chunks. Each chunk is
   about 600 characters at most. The full text is shown on screen.
5. You can talk over the answer. Speaking interrupts it, and what you say becomes
   your next turn.

### Cancelling

- While a request is running, say "cancel that", "cancel the request", "cancel the
  agent" or "stop the agent". Vox cancels the request and says "Cancelled."
- Plain "stop" ends the conversation. That also cancels a running request.

### One request at a time

Vox runs one agent request at a time. If you ask for another while one is
running, Vox says: "Still working on the last request. Say cancel to stop it."

### Safety

- **The agent's answer is untrusted text.** After the agent has answered, Vox
  treats anything the voice model then wants to do as a risk. Examples are
  typing text, running a command, starting a job, asking the agent again, or
  taking a new role.
- **Those actions need a spoken "yes".** Vox reads back what it is about to do.
  It runs only if you say exactly "yes". Any other answer cancels it.
- **Local commands you say yourself are not asked about.** Only actions that
  come from the agent's answer need the "yes".
- **The approval mode is a separate setting.** The approval mode in
  [ACP agent](acp-agent.md#approvals) decides what the agent itself may do. Its
  default for new installs is "Allow everything". The "yes" check does not change
  when you change that mode.

### Spoken lines

| What you hear                                               | When                                                                      |
| ----------------------------------------------------------- | ------------------------------------------------------------------------- |
| "Still working on the last request. Say cancel to stop it." | You ask for a second request while one is running.                        |
| "Cancelled."                                                | You cancel a running request.                                             |
| "I can't reach the agent right now."                        | The agent cannot be started or reached.                                   |
| "The agent stopped unexpectedly."                           | The agent process stopped during a request.                               |
| "The agent took too long."                                  | The request ran longer than three times the turn timeout plus 30 seconds. |

### Limits

- **Permission prompts are not spoken yet.** With the "Ask" approval modes, a
  permission prompt from the agent appears on screen. You answer it there.
  Spoken approval is not built yet.
- **One conversation, no memory of agent sessions.** Each request starts a fresh
  agent session. The agent does not remember earlier requests from the
  conversation. Vox carries the role as text in the request.
- **Use headphones when you talk over the assistant.** Without them, the
  microphone can hear the spoken answer and treat it as your voice.
- **Not tested with every agent.** Results depend on the agent you choose.

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
they appear in Settings > General, in the Voice conversation group. That group
is shown only when Experimental features are turned on (Settings > Advanced >
Experimental features).

Optional features for conversations (smart turn detection, a latency panel,
personas, and a speaker filter) are described in [Optional voice features](voice-features.md).

## Interrupting by speaking (headphones, experimental)

Setting: **Interrupt by speaking** in Settings → General → Voice conversation. It is off by
default. Its only other option, **Headphones only**, is meant for headphones.
This feature is experimental and has **not yet been verified on real headsets**;
the live check below is how that gets confirmed.

When it is on:

- Your voice lowers the assistant's volume as soon as you start talking.
- About 600 ms of continuous speech cancels the current reply. Your sentence
  becomes the next turn in the same conversation.
- A short "yeah" restores the volume.
- The setting is read once, when a conversation starts. Changing it applies to
  the next conversation.

### Known limits

- The system voice cannot be ducked (lowered in volume), but it can be
  interrupted. The Phonon and Natural voices can be both ducked and
  interrupted.
- Speakers are not supported. The assistant would hear its own voice and
  interrupt itself.
- Bluetooth headsets may switch to a call profile when the second microphone
  stream opens, so a short audio glitch is possible when a conversation starts.
- Spoken replies must be on, and a reply backend must be set up. Without them
  the setting has no effect.
- If the interruption monitor cannot start, the conversation runs as it did
  before, without interruptions. No error is shown.
- If the normal microphone cannot open while the monitor is running, the
  monitor is closed and the conversation continues without interruptions.
- The monitor's microphone stream keeps its audio in memory until the
  conversation ends (roughly 4 MB per minute). Keep conversations short while
  this is experimental.

### Live check (headphones)

Run this once with a wired headset and once with a Bluetooth headset:

1. Say "Hey Jarvis", ask for a long answer, and talk over it for a second or
   more. The voice should dip at once and stop within about a second, and your
   sentence should be answered in the same conversation.
2. During a reply say a short "yeah". The volume should dip and come back and
   the reply should keep going.
3. Let ten replies play while you stay silent, with a normal background noise
   level. None should be interrupted or left quiet.
4. End the conversation during a reply. It should end normally.
5. Switch the setting back to **Off**. Conversations should behave as before.
6. With the system voice selected, step 1 should still stop the reply (no dip
   is expected).

Report anything that fails together with the headset model.

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
