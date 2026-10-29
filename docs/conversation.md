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
  touched for ten minutes is forgotten. Nothing is kept after the conversation ends,
  except the optional records described under [Privacy](#privacy) and the transcript
  shown in the [conversation window](#conversation-window).
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

## Plans and specialist agents

Some requests need more than one kind of work. For example: "look up the
latest release notes, then write a summary into a file". The voice model can
split these into a **plan** of two to five steps. Vox runs the steps one at a
time, each as its own agent run.

### How a plan runs

1. The voice model decides that the request needs several kinds of work in
   sequence. It lists the steps. Each step has a type and one task.
2. Each step type is one of: **research**, **browser**, **code**, **files** or
   **general**.
3. Vox says one short line, for example "I will do this in 3 steps: research,
   coding, files." Then it starts the first step.
4. Each step runs as a separate agent run, one at a time. Vox does not start
   the next step until the one before has finished.
5. Each later step gets the results of the earlier steps it depends on. These
   results are passed as quoted text, marked as untrusted data.
6. Vox speaks a progress line when each later step starts: "Step 2 of 3: coding."
7. When the last step finishes, Vox speaks its result in short chunks and shows
   the full text on screen.

A plan is fixed once it is announced. Vox does not change, add, remove or
reorder steps while it runs. Steps do not run in parallel.

Requests that need only one kind of work still go to the agent as a single
request, as described in [Handing work to your agent](#handing-work-to-your-agent).

### Turning it on

- The planner setting is **Plan multi-step requests** (`conversation_planner`).
  It is **on by default**, but it does nothing unless agent hand-off is
  enabled. Hand-off must be set up first: **Hand open-ended requests to the
  agent in conversations** on, the ACP agent on, and a command set (see
  [ACP agent](acp-agent.md)). Without the hand-off, plans do not run.
- **Existing installs.** If you already use hand-off, upgrading turns on plan
  announcements for multi-step requests, because the planner is on by default.
  To keep single-agent hand-off, turn **Plan multi-step requests** off.
- The planner toggle and the four specialist rows (**Research steps**,
  **Browser steps**, **Code steps**, **File steps**) are on the same Agent
  settings page as the ACP agent (**Settings > Agents > ACP agent**).

### Specialist agents for step types

By default every step runs on your main ACP agent. You can name a different
agent for a step type: research, browser, code or files. Each one takes a
command and optional arguments, in the same form as the main agent.

- A blank command means that step type uses the main agent.
- **General** steps always use the main agent.
- Specialist commands follow the same rules as the main command. Relative
  paths and control characters are refused. Use an absolute path.
- Arguments are split on spaces. There is no quoting, as for the main agent.
  Do not put secrets, tokens or passwords in arguments.
- The working folder and the turn timeout always come from the main agent
  settings. Only the program and its arguments change per step type.
- A specialist command alone does not turn the hand-off on. The main agent
  must be on, with a command.
- Changing the program between steps starts a new process, which can take a
  few seconds.

### Cancelling a plan

- Say "cancel that", "cancel the request", "cancel the agent" or "stop the
  agent" at any time while a plan runs. Vox cancels the live step and says
  "Cancelled."
- Steps after the live one never start, even one that was about to start.
- Plain "stop" ends the conversation and also cancels the plan.

### Timeouts

- **Each step** may run for three times the turn timeout, plus 30 seconds. With
  the default turn timeout of 120 seconds, that is 390 seconds.
- **The whole plan** may run for 15 minutes. Time between steps counts too.
- A step that goes over its limit stops the plan. Vox says "Step 2 of 3 took
  too long. Stopping the plan." If the whole plan goes over 15 minutes, Vox
  says "The plan took too long. Stopping."
- Once the plan is past 15 minutes, no further step starts. A final result that
  arrives after that point is still shown and spoken.

### What you hear

| What you hear                                          | When                                                     |
| ------------------------------------------------------ | -------------------------------------------------------- |
| "I will do this in 3 steps: research, coding, files."  | The plan is announced.                                   |
| "Step 2 of 3: coding."                                 | Each later step starts.                                  |
| "Still working on it."                                 | A step has run for 20 seconds, then about once a minute. |
| "Step 2 of 3 failed. <reason>"                         | A step reports an error. The plan stops.                 |
| "Step 2 of 3 stopped unexpectedly. Stopping the plan." | The agent process stopped during a step.                 |
| "Step 2 of 3 took too long. Stopping the plan."        | A step went over its time limit.                         |
| "The plan took too long. Stopping."                    | The whole plan went over 15 minutes.                     |
| "The agent settings changed. Stopping the plan."       | An agent setting changed during the plan.                |
| "Cancelled."                                           | You cancelled the plan by voice.                         |

Intermediate step results are **not spoken and not kept**. Only the final
result is spoken, shown on screen and added to the conversation history.

### Safety

- **Agent output is untrusted.** Results from earlier steps are passed to later
  steps as quoted data, with a note that they must not be followed. Brackets
  and delimiters are removed from them, and they are capped in length.
- **Results cannot change the plan.** No agent output can add a step, change a
  step's type, or change an agent setting.
- **Local commands still win.** Opening apps, media keys, dictation, memory and
  ending the conversation stay local, as before.
- **Actions after agent output need a spoken yes.** If the voice model then
  proposes an action that changes something, such as pasting text, running a
  command, starting a job or running a plan, Vox reads back what it is about to
  do. Vox runs it only if you say exactly "yes". Any other answer cancels it.
  The "yes" check does not depend on the approval mode.
- **A confirmed plan runs as read back.** After "yes", Vox checks the agent
  settings again. If they changed, Vox asks you to ask again and runs nothing.
- **A settings change stops the plan.** If an agent setting or the planner
  setting changes during a plan, the next step does not start, and Vox says so.
- **Agent permissions are separate.** Each step runs with the approval mode of
  the main agent. See [ACP agent](acp-agent.md#approvals).

### Limits

- **Sequential only.** Steps run one after another. There is no parallel work.
- **No replanning.** If a step fails, the plan stops. Vox does not try another
  plan.
- **No memory between plans.** A later plan does not see the results of an
  earlier one.
- **Permission prompts are not spoken.** When the agent asks for permission, the
  prompt appears on screen, as described in [ACP agent](acp-agent.md#approvals).
  Spoken approval is not built yet.
- **Use headphones when you talk over the assistant.** Without them, the
  microphone can hear the spoken answer.

### Troubleshooting plans

- **Vox answers instead of running a plan.** Check that the planner setting is
  on, that **Hand open-ended requests to the agent in conversations** is on,
  and that the conversation is not in answer-only mode. Vox never runs a plan
  in answer-only mode.
- **A step says "The agent settings changed. Stopping the plan."** An agent
  setting or the planner setting changed while the plan was running. Ask again
  once the settings are final.
- **A plan says "took too long".** The step or the whole plan went over its
  limit. Check that the agent responds at all (use **Test connection** in
  [ACP agent](acp-agent.md)), and consider a longer turn timeout.

### Status

The plan flow, the specialist agent settings, and the cancel and timeout rules
are covered by automated tests in the code. They have **not yet been checked
with a real agent or with a microphone.** Treat plans as experimental until
that check is done.

## Conversation window

The conversation window shows the live transcript of a voice conversation.

Open it with the global shortcut: Control+Option+T on macOS, Ctrl+Alt+Shift+T
on other systems. You can change the shortcut under "Conversation window" in the
shortcut settings. You can also use the "Conversation window" item in the tray
menu, or the "Open conversation window" button in Settings > General, in the
Voice conversation group (shown when Experimental features are on).

When a conversation starts (for example after you say "Hey Jarvis"), the window
also opens by itself, without taking keyboard focus, so whatever you were typing
in keeps working. Turn this off with "Open the conversation window when a
conversation starts" in the same settings group. It does nothing while "Keep
conversation transcript" is off. If you closed the window, the next conversation
opens it again. If the window is minimised, it is brought back.

The window has a fixed dark look with the Vox icon. The orb and the status pill
change colour with the state: cyan while listening, gold while Vox speaks, violet
while thinking, blue while working, coral after an error and slate when the
session has ended. Motion stops when the system asks for reduced motion.

What it shows:

- What you said, as recognised.
- What Vox says, or began to say. Text that was accepted for speech and then cut
  off because you spoke over it stays shown.
- The plan announcement and the final result of a plan.
- Notes for the start and the end of each session.
- An animated avatar. Its face follows the state: listening, thinking, working
  or speaking. It is asleep after a session ends, and concerned after a session
  ends with an error. The speaking animation is not synced to the audio.

What it does not show:

- Raw agent output and intermediate plan results.
- Anything that was never spoken or typed, including the text of voices that were rejected.
- Dictation outside conversation mode.
- Background job announcements.

You can also type in the window. See [Typing](#typing-in-the-window).

Controls and settings:

- **Clear** empties the transcript.
- **Keep conversation transcript** (setting key `converse_chat_log`, on by
  default). When it is off, nothing new is recorded. Lines that were already
  queued a moment earlier may still appear. Existing lines stay until you press
  Clear or the next conversation starts.
- Closing the window only hides it. The transcript is kept.

Display notes:

- Text is plain. Nothing in the window is clickable.
- When the window is very busy, some lines can be skipped. A note says so.
- The window can lag a moment behind the conversation. It never slows the
  conversation down.

Privacy:

- While the setting is on, the transcript stays in memory after the conversation
  ends. It stays until the next conversation starts, you press Clear, or Vox
  quits. The window never writes it to disk or to logs.
- The optional **Save conversation records** feature is separate. See
  [Privacy](#privacy).

Safety:

- The window page is bundled with the app. It has no network access. It can read
  the transcript, clear it and send the message you typed. It cannot call
  anything else.

### Typing in the window

The box at the bottom of the window lets you chat by typing, at any time.

- Press **Enter** to send. **Shift+Enter** adds a new line. Messages can be up to
  2000 characters.
- A typed message is handled like something you said. Your shortcuts and
  commands run first (for example "bye" ends the chat), then your reply model
  answers, and work can be handed to your agent. Questions that need your
  approval can be answered by typing "yes" or "no". The window waits up to a
  minute for a typed answer.
- **No conversation running:** typing starts a text chat. It uses neither the
  microphone nor the speaker. Replies appear in the window only. Say "bye" or use
  your stop shortcut to end it. A text chat does not end by itself.
- **Voice conversation running:** your typed message joins it. The reply is
  spoken as usual.
- If you start a voice conversation while a text chat is open, Vox ends the text
  chat first and asks you to try again. Say your wake phrase a second time.
- Text chats cannot start background jobs and never paste text into another
  window, because both would act or speak without you watching the window.
- If no reply model is set up, the window tells you to do that in Settings.

Typing and privacy:

- Typed text stays in memory like the rest of the transcript. It is not written
  to a log.
- A conversation that included any typed message is never saved by **Save
  conversation records**, even when that setting is on. A text chat is never
  saved.
- Work you hand to an agent is stored by the agent the same way as when you speak
  the request.

## Context

Vox keeps the last N exchanges (the "turns kept in context" setting) and
sends them with each new turn. Older turns are dropped. The memory is in RAM
only and is cleared when the conversation ends.

Vault notes search can add matching passages from your notes to the context.
It is off by default; see [vault-rag.md](vault-rag.md).

## Privacy

What you say and Vox's replies are sent to your configured reply backend. That
is either a local command-line agent or a model API you configured. The
conversation context is kept in memory only and is cleared when the conversation
ends. The live transcript for the [conversation window](#conversation-window) is
also kept in memory only. When its setting is on, it stays until the next
conversation starts, you press Clear, or Vox quits. The window never writes it
to disk or to logs.

The one exception is the optional **Save conversation records** setting (off by
default). When it is on, Vox saves each finished conversation to its own local
database, with a short summary. A model writes the summary when one is
configured. Otherwise a simple fallback is used. If the vault option under that
setting is on and a vault folder is set, a Markdown note is written too.
Redaction of sensitive text is best-effort, so review what is saved.

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
- **Vox does not react to me:** turn off ambient listening (an Experimental
  feature; with Experimental off it has no effect). The wake word does not block
  a conversation: "Hey Jarvis" starts one (see [voice-assistant.md](voice-assistant.md)).
  Ambient listening must be off while a conversation runs.
- **Vox never hears me:** check that Vox has microphone permission in your
  system privacy settings, and that the right input device is selected.
- **Conversation stops by itself:** it reached the silent-turn or turn limit.
  Raise the values in Settings if you want longer sessions.
