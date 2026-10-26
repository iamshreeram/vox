# Vox Full Architecture: speak to it, it does things, it talks back

Status: proposal. Nothing in sections 4 to 9 is built except where marked "exists".
Each phase in section 10 gets its own design and review before any code is written.

## 1. Goal

You speak. Vox understands, acts on your computer, and answers aloud. Examples:

- "Open google.com in Firefox."
- "Scroll down, then find pricing on this page."
- "Summarize this page." (answered aloud in one or two sentences)
- "Close the other tabs." (Vox asks to confirm first)

"Anything" means anything that has a registered, safety-checked tool. It does not
mean unrestricted control of the machine.

## 2. Design principles

1. **Cheapest path first.** Simple commands never wait for a language model.
2. **Understanding and doing are separate.** Embeddings and models only choose an
   action. Only registered tools execute it.
3. **Every action passes a safety gate.** Voice is misheard. Risk decides whether
   Vox acts, announces, or asks first.
4. **Local by default.** Speech recognition, rules and intent matching run on the
   device. Anything that leaves the device is explicit and configurable.
5. **Talking back is part of the loop.** Confirmations, progress and answers are
   spoken, and the microphone reopens when Vox expects a reply.
6. **Nothing sensitive is stored by default.** Transcripts, replies and screen
   content are not logged. The audit log records action names and outcomes only.

## 3. Overview

```
 Mic -> Wake word / hotkey / ambient -> VAD -> Local STT --------------+
  ^                                                                    v
  |                                                       +--- ORCHESTRATOR ---+
  |                                                       | 1. rules (instant)   |
  |                                                       | 2. embedding intent  |
  |                                                       | 3. planner LLM       |
  |                                                       | 4. long-task agent   |
  |                                                       +---------+------------+
  |                                           typed tool calls      |
  |                                        +------------------------v-----------+
  |                                        | SAFETY / POLICY GATE               |
  |                                        | risk tier, voice confirm, audit    |
  |                                        +------------------------+-----------+
  |                                                                  v
  |                          TOOL REGISTRY: open app/URL, keystrokes/scroll/find,
  |                          AppleScript, window management, browser automation,
  |                          files, calendar/mail, sandboxed shell, agent CLI
  |                                                                  | results, page/screen state
  |                                                                  v
  +-- Speaker <- TTS <- Speech composer <- Dialog manager <---------+
            (barge-in, follow-up listening, pending confirmations, memory)
```

## 4. Components

### 4.1 Audio input (exists)

Global hotkey, wake word and ambient listening feed a voice activity detector and a
local speech-to-text model. Ambient mode needs an echo guard so Vox does not
transcribe its own voice (see 10, phase 1).

### 4.2 Orchestrator (new)

Four tiers, tried in order. Each tier either handles the request or passes it on.

| Tier | Mechanism                                                          | Typical latency    | Handles                                                                                     |
| ---- | ------------------------------------------------------------------ | ------------------ | ------------------------------------------------------------------------------------------- |
| 1    | Rules and patterns (exists in part: the command router)            | under 1 s          | "open X", "open X in Firefox", "scroll down", "find X", "new tab", "back", media and volume |
| 2    | Embedding similarity to known intents, with a confidence threshold | under 1 s          | Fuzzy phrasings ("go down a bit", "page down") that map to a known action                   |
| 3    | Planner LLM with tool-calling over a long-lived connection         | 1 to 3 s           | Open-ended requests and multi-step chains ("scroll down, then find pricing")                |
| 4    | Long-task agent, run in the background                             | seconds to minutes | Research, multi-site work. Vox acknowledges at once and speaks progress                     |

Notes:

- Tier 2 only selects an action and fills arguments. It never executes anything.
- Argument extraction ("find **pricing**") is done by rules for tier 1 and by the
  planner for tier 3.
- A low-confidence tier 2 match falls through to tier 3 or asks a clarifying
  question. It never guesses on a risky action.
- Do not start a fresh command-line agent process per request. A cold start takes
  15 to 35 seconds. The planner must be a persistent connection: the user's agent
  CLI if it can run as a long-lived server, or a direct API call to a fast model.
  The slow agent is reserved for tier 4.

### 4.3 Tool registry (new)

Every capability is a tool with:

- a name and a typed argument schema,
- a risk level (see 4.4),
- a timeout,
- a pure description that the planner and the intent matcher can both read.

The planner can only call registered tools. Adding a capability means adding a tool,
not changing the orchestrator.

Initial tool families:

| Family                          | Tools                                                         | Executor                                                                              |
| ------------------------------- | ------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| Launch                          | open app, open file, open URL, open URL in a named browser    | `open` / `open -a` (exists in part)                                                   |
| Page and window control         | scroll, find in page, new tab, back, close tab, switch window | synthesized keystrokes (needs the Accessibility permission Vox already uses)          |
| Browser app control             | read URL, run script, switch tab                              | AppleScript for Safari and Chrome. Firefox's support is weak, so use keystrokes there |
| Page understanding and clicking | "click the pricing link", "read the headline"                 | browser automation (Playwright or the browser's remote-control protocol)              |
| System                          | volume, media, do-not-disturb                                 | fixed `osascript` scripts (exists for media)                                          |
| Personal data                   | calendar, mail, reminders, notes                              | app APIs or AppleScript                                                               |
| Files and shell                 | search, read, run an allowlisted command                      | sandboxed, allowlisted only                                                           |
| Agent                           | hand a task to the user's agent CLI                           | subprocess or persistent server                                                       |

### 4.4 Safety and policy gate (new, extends the existing `safety_policy`)

Sits between "decided" and "did". Nothing reaches an executor without passing it.

| Risk                          | Examples                                                                                           | Behavior                                                      |
| ----------------------------- | -------------------------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| Read-only                     | scroll, find, read the page, tell the time                                                         | run immediately                                               |
| Reversible                    | open a tab, change volume, open an app                                                             | run, then announce briefly                                    |
| Destructive or outward-facing | close tabs or windows, delete, send mail or messages, submit a form, purchase, run a shell command | ask aloud ("Do you want me to...?") and wait for a spoken yes |

Rules:

- A confirmation expires after a few seconds. Silence means no.
- "Stop" and "cancel" are a global kill switch that halts the current plan.
- Chained plans are checked per step, not once for the whole plan.
- An allowlist limits which apps, URL schemes, folders and commands are reachable.
- Raw unrestricted shell access is never a tool.
- The audit log stores tool name, risk tier, outcome and time. It never stores
  transcripts, page text or secrets.

### 4.5 Perception (new, later phase)

For "what's on this page" and "click that":

- Prefer the macOS Accessibility tree or the browser DOM. It is fast and exact.
- Use screenshots plus a vision model only as a fallback, with explicit user opt-in.
- Page and screen text is passed to a model only when the request needs it.

### 4.6 Dialog manager (partly exists)

Owns turn-taking and conversation state:

- Conversation context with bounds and expiry (exists, opt-in).
- Follow-up listening: reopen the microphone after Vox speaks (in design).
- Pending confirmations and clarifying questions.
- Barge-in: speaking over Vox stops its speech and takes the new request.
- A task queue so background work can report back without blocking new requests.

### 4.7 Spoken output (exists)

The speech composer turns results into one or two short sentences. Add short status
phrases for slow work ("On it", "Still working"), and stream text-to-speech where
the engine supports it.

### 4.8 Memory (exists)

Opt-in facts and conversation context. Unchanged by this proposal.

## 5. Example flows

**"Open google.com in Firefox"**

1. Speech to text.
2. Tier 1 rule matches `open_url(url, browser=Firefox)`.
3. Risk: reversible. The gate lets it through.
4. `open -a Firefox https://google.com`.
5. Vox says "Opened."

**"Scroll down, then find pricing"**

1. Tier 1 splits on "then" into `scroll_down` and `find_in_page("pricing")`.
2. Both are read-only. The gate lets them through.
3. Keystrokes: Page Down, then Cmd+F and typing "pricing".
4. Vox says "Found it" or "I couldn't find pricing on this page."

**"Close the other tabs"**

1. Tier 3 plans `close_other_tabs`.
2. Risk: destructive. Vox asks "Close the other tabs?"
3. The microphone reopens. A spoken yes runs it. Silence cancels it.

**"Find the pricing page and tell me the cheapest plan"**

1. Tier 4 background agent with browser automation.
2. Vox says "On it" immediately, then speaks progress and the final answer.
3. The user can say "stop" at any time.

## 6. Latency budget

| Path                       | Target                                       |
| -------------------------- | -------------------------------------------- |
| Speech to text (local)     | under 1 s after you stop speaking            |
| Tier 1 or 2 plus execution | under 1 s                                    |
| Tier 3 planner round trip  | 1 to 3 s                                     |
| Tier 4                     | acknowledge within 1 s, then report progress |

If a request will take longer than about 3 seconds, Vox acknowledges aloud first.

## 7. Privacy and trust

- Speech recognition and tiers 1 and 2 run locally.
- Tiers 3 and 4 send text to whatever model the user configures. This is visible and
  opt-in.
- Page and screen content is sent only when a request needs it.
- The microphone reopens automatically only with the follow-up listening setting on,
  always with a visible indicator, and with a hard time limit.
- No audio or transcript is written to disk for automatically opened recordings.

## 8. What already exists

| Capability                                           | Where                            |
| ---------------------------------------------------- | -------------------------------- |
| Speech to text, hotkey, wake word, ambient listening | audio and transcription managers |
| Command router for apps, files and URLs              | command router                   |
| Media and volume control                             | media control                    |
| User-defined phrase shortcuts                        | voice shortcuts                  |
| Handing text to a configured agent CLI               | agent bridge                     |
| Speech composer and text-to-speech                   | speech pipeline and TTS manager  |
| Conversation context and memory                      | conversation log, memory         |
| Ambient echo suppressor (partly wired)               | ambient listener                 |

## 9. Gaps to close

- No tool registry or typed tool schemas.
- No page or window control (scroll, find, tabs, back).
- The command router does not yet understand "in Firefox".
- No planner with a persistent connection. The current agent bridge is one request
  per process and too slow for interactive use.
- The safety policy does not yet cover spoken confirmations or per-step plan checks.
- Ambient mode does not yet reach the agent, and its echo guard is a stub.
- No perception layer.

## 10. Phased plan

| Phase | Deliverable                                                      | Notes                                                      |
| ----- | ---------------------------------------------------------------- | ---------------------------------------------------------- |
| 1     | Follow-up listening and a real ambient echo guard                | Turn-taking foundation. Design in progress, review pending |
| 2     | Tool registry, safety gate, keystroke tools, "open in <browser>" | Covers most simple commands fast and offline               |
| 3     | Planner with tool-calling and spoken confirmations               | Needs a persistent model connection                        |
| 4     | Browser automation and Accessibility-tree perception             | Controlled browser profile, opt-in                         |
| 5     | Background tasks with spoken progress, barge-in                  | Task queue and interruption handling                       |

Embedding-based intent matching (tier 2) can ship with phase 2 or 3. It is
optional: tiers 1 and 3 work without it.

## 11. Risks and open decisions

- **Misrecognition.** Voice errors are the main safety risk. Mitigation: risk tiers,
  confirmation for destructive actions, an allowlist and a kill switch.
- **Firefox control.** AppleScript is weak there. Keystrokes and browser automation
  are the realistic routes.
- **Planner model choice.** Persistent agent server, cloud API or local model. This
  needs a product decision and affects latency, cost and privacy.
- **Permissions.** Accessibility, Automation and Screen Recording each require an
  explicit macOS grant and a clear explanation in the UI.
- **Scope growth.** Every new tool widens the blast radius. Each one needs a risk
  level and tests before it is registered.
- **Cross-platform.** Keystrokes and scripting differ by operating system. The
  first target is macOS.
