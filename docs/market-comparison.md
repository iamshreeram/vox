# Vox compared with similar tools

This page compares Vox with open-source and commercial tools that do part of the same job: talk to your computer, run tasks, or hand work to an agent.

It is a snapshot taken on 2026-10-25. Read the "How this was built" section before you rely on a cell.

## The short version

- Vox is a **desktop voice front end**. It listens, speaks, and decides whether a request is a local command, a spoken answer, or work to hand to an agent you already run.
- It does **not bundle an agent, a browser driver or a code sandbox**. Other tools below bundle those and have no voice-first desktop loop.
- Vox's strongest ground is the **voice loop plus a strict safety model**: local commands win, agent output is untrusted, effectful follow-ups need an exact spoken `yes`, and you can cancel by voice.
- Vox's weakest ground is **proof**: the planner and hand-off have not yet been tried with a real agent or a real microphone, and spoken approval of agent permission prompts is not built.

## Where each tool sits

| Tool                       | What it is                                                 | Main interface                        | Voice in the product      | Runs agents / actions itself       | Licence      |
| -------------------------- | ---------------------------------------------------------- | ------------------------------------- | ------------------------- | ---------------------------------- | ------------ |
| **Vox**                    | Desktop dictation and voice assistant (Tauri, Rust)        | Hotkey, tray, wake word, spoken reply | Yes, both directions      | No: hands work to your ACP agent   | MIT          |
| **AgenticSeek**            | Local "Manus-style" multi-agent assistant                  | Web UI, CLI                           | CLI only, English only    | Yes: browser, code, files, planner | GPL-3.0      |
| **Xiryn**                  | Self-hosted personal agent with a Docker desktop per agent | Browser UI, Telegram                  | Browser dictation only    | Yes, inside containers             | MIT          |
| Handy                      | Offline push-to-talk dictation (Vox's starting point)      | Desktop app, hotkey                   | Input only                | No                                 | MIT          |
| Open Interpreter (current) | Rust coding agent, fork of OpenAI Codex                    | Terminal UI, ACP                      | Not in the user docs      | Yes: shell, sandbox, MCP           | Apache-2.0   |
| Goose                      | General agent that runs on your machine                    | Desktop app, CLI, API                 | Not found                 | Yes: MCP extensions                | Apache-2.0   |
| Leon                       | Open-source personal assistant                             | Web app                               | Config switches, off      | Yes: skills, agent mode            | MIT          |
| OVOS (ovos-core)           | Voice assistant platform, successor to Mycroft             | Voice                                 | Yes (in sibling repos)    | Skills via intent matching         | Apache-2.0   |
| Pipecat                    | Python framework for voice agents                          | Developer framework                   | Yes: VAD, STT, TTS, turns | You build it                       | BSD-2-Clause |
| LiveKit Agents             | Python framework for voice agents on LiveKit               | Developer framework                   | Yes                       | You build it                       | Apache-2.0   |

## Vox against AgenticSeek and Xiryn

| Area                   | Vox                                                                                                                                                                | AgenticSeek                                                                                                | Xiryn                                                                            |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| Purpose                | Talk to your desktop; dictate anywhere                                                                                                                             | "100% local alternative to Manus": browse, code, plan                                                      | Personal agent that works in its own Linux desktop; you can watch and take over  |
| Stack                  | Tauri, Rust, React                                                                                                                                                 | Python, Docker, Selenium, React                                                                            | Next.js, SQLite, Docker                                                          |
| Platforms              | macOS, Windows, Linux (agent features: Unix only)                                                                                                                  | Docker on Linux/macOS/Windows                                                                              | Local server, Docker needed for code and desktop                                 |
| Speech input           | Local speech-to-text on the desktop, hotkey and wake word ("Hey Jarvis", partly built)                                                                             | Vosk, CLI only, English only, marked experimental                                                          | Browser speech recognition fills the message box                                 |
| Speech output          | Local voices (system voice, Kokoro, Phonon), barge-in (experimental)                                                                                               | Kokoro                                                                                                     | None found                                                                       |
| Hands-free loop        | Yes: conversation mode with stop phrases, speaker filter, smart turn detection (not mic-tested)                                                                    | Trigger word in the config; confirmation phrase ends the request                                           | No                                                                               |
| Local commands         | Yes: open apps and links, media keys, dictation prefixes, memory. They never reach an agent                                                                        | No equivalent found                                                                                        | No equivalent found                                                              |
| Routing a request      | Local intents first. Then an LLM planner decides: answer, single agent request, or 2–5 typed steps                                                                 | Language detect, complexity classifier (HIGH goes to the planner), then a BART zero-shot and LLM vote      | The model itself chooses to call a `delegate` tool inside a squad. No classifier |
| Multi-step plans       | Sequential, immutable once announced, one run live at a time (unmerged branch)                                                                                     | Planner agent, sequential loop, results passed forward                                                     | Squads and delegation, model-driven                                              |
| Specialist agents      | Optional command per step type; blank falls back to the main agent                                                                                                 | Fixed agents: casual, code, file, browser, planner, optional MCP                                           | Templates and squads, each with instructions and model                           |
| Browser and code       | Not built in. Your agent does it                                                                                                                                   | Selenium browser, Bash/Python/C/Go/Java interpreters                                                       | Chromium and shell inside the container                                          |
| Local models           | Speech models local; reply backend is a local CLI agent or a model API you configure                                                                               | Ollama, LM Studio, OpenAI-compatible, plus many cloud providers. States hardware needs (14B-class or more) | 11 providers including Ollama and LM Studio                                      |
| Approval model         | Permission modes (ask before changes, ask every call, never, allow all). Fresh install: allow all                                                                  | `safe_mode` command filter, described by the project as "not a sandbox"                                    | Payment approval enforced in code; other confirmations are prompt wording only   |
| Untrusted agent output | Quoted and neutralised before reaching the next step; cannot start a plan; follow-ups need a clear yes (fixed phrases, then a model judge for other short answers) | Not found                                                                                                  | Not found                                                                        |
| Cancel                 | By voice at any time; later steps never start                                                                                                                      | Not found                                                                                                  | Stop button / API call                                                           |
| Sandbox                | None. The agent is "fully trusted and not sandboxed"                                                                                                               | Docker privilege drop and secret scrubbing; host execution outside Docker                                  | Agents run only in containers; Docker required                                   |
| Install                | Build from source (no installers claimed)                                                                                                                          | `install.sh` / `install.bat`, Docker compose                                                               | One-line setup scripts                                                           |
| Maturity signals       | Version 0.1.0; large Rust test suite; planner and hand-off unproven with real agents                                                                               | Version 0.1.0; 27 test files; no CI; project says "zero roadmap and zero funding"                          | v0.6.0 (2026-10-24); CI runs typecheck, lint, build; no unit-test script         |
| Licence                | MIT                                                                                                                                                                | GPL-3.0 (copyleft: do not copy code into Vox)                                                              | MIT                                                                              |

## Other open-source tools

**Handy.** Offline push-to-talk dictation; Vox started from it. It has no agents, no wake word and no speech output. It is the cleanest reference for the dictation path and for cross-platform packaging (macOS, Windows, Linux installers).

**Open Interpreter (current).** The project is now a Rust coding agent forked from OpenAI Codex. The original Python project continues as a community fork. It has sandbox modes (read-only, workspace-write, full access), approval modes, MCP, and an ACP mode, so it is a plausible agent to run _behind_ Vox. A voice crate exists in the tree but is not described in the user docs.

**Goose.** A general agent with a desktop app, a CLI and many MCP extensions. Its permission modes (auto, approve every call, approve only sensitive calls, chat only) are a good comparison for Vox's permission modes. No voice found.

**Leon.** An assistant with three modes: smart, controlled (deterministic skills) and agent (step-by-step planning). Voice settings exist but are off by default. The docs for the new version are marked "not ready yet", and the changelog is stale.

**OVOS.** A voice-first platform with an ordered intent pipeline. It routes by intent matchers, not by an agent planner. Speech components live in other repositories. It is the closest match to Vox's "local intents first" idea, with a much larger skill ecosystem.

**Pipecat and LiveKit Agents.** Server-side Python frameworks for realtime voice agents. They are strong on turn detection, interruption and provider choice, and have no permission layer. They target servers, WebRTC rooms and phone lines, not a desktop hotkey app. Vox could borrow ideas (wake-phrase turn strategies, interruption handling), not code.

## Commercial and closed products

These rows are category-level descriptions from general product knowledge. They were not checked against source code, and features change often. Check the vendor's page before relying on one.

| Category              | Examples                                                    | How it relates to Vox                                                                                                   |
| --------------------- | ----------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| Dictation apps        | Superwhisper, Wispr Flow, MacWhisper, built-in OS dictation | Same dictation job as Vox's base mode. Mostly paid or cloud-assisted; none hand work to your own agent                  |
| Platform assistants   | Siri, Alexa, Google Assistant and Gemini                    | Deep OS integration and polished wake word. Closed, cloud-backed, and not extensible with your own agent                |
| Chat apps with voice  | ChatGPT voice, Gemini Live, Claude voice                    | Fluent spoken conversation. They live in an app, and do not run local commands or your own coding agent on your desktop |
| Hosted general agents | Manus, ChatGPT agent mode, Devin                            | The class AgenticSeek is a local alternative to. Cloud sandboxes, accounts, usage-based pricing                         |
| Coding agents         | Claude Code, OpenAI Codex CLI, Cursor, Aider                | Text-first. These are the kind of agent Vox can hand work to through ACP; they are complements, not rivals              |

## What Vox does differently

1. **Local-first routing with a strict order.** Local intents run first and never reach an agent; only unmatched requests go to the planner. AgenticSeek and Xiryn route everything through a model.
2. **An explicit trust boundary for agent output.** Output from an agent is untrusted, is quoted when it feeds a later step, cannot create a plan, and cannot authorise an action. The only approval is an exact spoken `yes`. None of the other tools surveyed document this.
3. **Voice control of the agent.** "Cancel that" kills the live run and prevents later steps. Timeouts apply per step and to the whole plan.
4. **Bring your own agent.** Vox talks ACP to a command you choose, with optional specialists per step type. It does not bundle agent code, so it does not inherit an agent's licence or sandbox choices.
5. **Desktop integration.** Paste into the active app, media keys, app and link launching, a menu-bar presence.

## Where Vox is behind

- **No proof with real agents or microphones yet.** The docs say so. Barge-in, speaker filter and smart turn are also untested on real hardware.
- **No sandbox.** An ACP agent runs with your permissions. Permission modes are not a sandbox. Xiryn (containers), Open Interpreter (OS sandbox) and AgenticSeek (Docker) all go further.
- **No spoken approval of agent permission prompts.** Prompts appear on screen. A headphones-only voice approval is planned.
- **No built-in browser or code tools.** That is by design, but a user without an agent gets less than with AgenticSeek.
- **Smaller ecosystem.** OVOS and Goose have far more skills or extensions.
- **Installation.** Vox documents building from source only.
- **Docs inconsistency to fix.** The ACP page says voice cancel and conversation hand-off are "not connected yet", while the conversation page says voice cancel works. The conversation page reflects the current behaviour.

## Ideas worth adopting

| Idea                                                                                   | Seen in                           | Fit for Vox                                                                                      | Effort |
| -------------------------------------------------------------------------------------- | --------------------------------- | ------------------------------------------------------------------------------------------------ | ------ |
| Bind each approval to one pending action (id and target)                               | Xiryn (payment approval)          | Stops a stale or replayed `yes` from approving a different action. Strengthens the existing rule | Small  |
| An explicit "I need you" state that ends the turn and asks for a fresh check on return | Xiryn (takeover)                  | Clean voice state for "I need your input"                                                        | Medium |
| Run ledger with skip reasons (done, failed, skipped-busy, skipped-missed)              | Xiryn (routines)                  | Useful if timed tasks are added. Unattended runs must never act without a live `yes`             | Medium |
| Trim old context before each agent step                                                | Xiryn                             | Keeps long plans cheap. Must not hide untrusted output from review                               | Small  |
| A "sensitive calls only" permission mode                                               | Goose (smart approve)             | A middle setting between ask-every-call and allow-all                                            | Small  |
| Named sandbox profiles for agents that support them                                    | Open Interpreter, Xiryn           | Lets the user pick stronger isolation per specialist                                             | Medium |
| Wake-phrase and interruption strategies as separate, testable modules                  | Pipecat                           | Cleaner way to tune wake word and barge-in                                                       | Medium |
| Mode switch: deterministic only, or planning allowed                                   | Leon (controlled / smart / agent) | A simple user-facing switch next to the planner toggle                                           | Small  |
| Secrets vault with masked hints for model context                                      | Xiryn                             | Keeps API keys out of prompts                                                                    | Medium |

Not worth adopting: a Docker desktop per agent (heavy, and ACP agents do not need computer use), automatic long-term memory with no review (it would persist untrusted output), prompt-only "wait for a yes" rules (not enforced in code), and a classifier-based router like AgenticSeek's (the LLM planner already routes, and a separate model adds download size and a second failure mode).

## How this was built

- **Open-source rows.** The repositories were shallow-cloned on 2026-10-25 and read for README, licence, manifest, security notes and the router or permission code. Cells say "not found" when a search found nothing; that is not proof the feature is absent.
  - AgenticSeek: https://github.com/Fosowl/agenticSeek (commit 4f5f924)
  - Xiryn: https://github.com/SouravBeraAkaSpeed/xiryn (v0.6.0)
  - Handy: https://github.com/cjpais/Handy
  - Open Interpreter: https://github.com/OpenInterpreter/open-interpreter
  - Goose: https://github.com/block/goose
  - Leon: https://github.com/leon-ai/leon
  - OVOS: https://github.com/OpenVoiceOS/ovos-core
  - Pipecat: https://github.com/pipecat-ai/pipecat
  - LiveKit Agents: https://github.com/livekit/agents
- **Vox rows.** Taken from the Vox documentation on the planner branch (`feature/voice-planner`). Planner, specialist agents and plan timeouts exist only on that branch until it is merged.
- **Commercial rows.** General product knowledge, not verified here.
- **Not measured.** No tool was run. Speed, accuracy and resource use are not compared.
- **Licences.** AgenticSeek is GPL-3.0, so copying its code into Vox is not appropriate. MIT and Apache-2.0 projects allow reuse if the notices are kept. Ideas are free to reuse in every case.
- **Refreshing.** Re-clone the repositories and re-read the same files. Update the date at the top.
