# ACP agent (persistent agent for dictation)

This page describes the persistent agent connection that Vox uses for conversations and, when Send dictation to the agent is on, for dictation that no voice command matched. It replaces the old one-shot command-line bridge for that purpose. The old bridge still runs background coding jobs (see [Known limits](#known-limits-and-follow-ups)).

> **Trust warning.** The agent program you configure is **fully trusted and is not sandboxed**. While this feature is on, requests you hand over in conversations are sent to that program. Dictation that no built-in or custom voice command matched is sent to it only if you also turn on [Send dictation to the agent](#send-dictation-to-the-agent). Read [Trust model and privacy](#trust-model-and-privacy) before you turn it on.

## What it is

Vox starts **one long-lived child process** that speaks the open Agent Client Protocol (ACP) over its standard input and output. The protocol is JSON-RPC 2.0, one JSON message per line (newline-delimited JSON). Vox sends a prompt, reads the agent's streamed text, and speaks the reply through the existing agent-reply path.

Why this is faster than starting a process per request: starting a program often takes longer than the answer itself. The old one-shot bridge started a new process for every request. With ACP the process stays running between requests, so only the first request pays the start-up cost. In a test with one agent program, the first start took about 9 seconds to finish the handshake, and warm turns took about 1.4 to 2.8 seconds. These numbers depend on the agent and your machine.

Vox does not name or bundle any agent. You choose the program and its arguments.

## Requirements

- An agent program that implements ACP and can run in stdio mode. You give Vox its **absolute path**, plus optional arguments such as `--acp`. Check your agent's documentation for the exact flag.
- **Unix only for now** (macOS and Linux). On other platforms the feature reports `unsupported` and does not start a process.
- The agent must be installed and able to run on your machine. Vox does not install it.

## Configuration

Open **Settings > Agents > ACP agent**:

| Setting               | Value                                                                                                                                                                                                                                                                                                               |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Enable                | Off by default. Turning it on lets conversations and typed chat hand work to the agent. It does not send dictation: see **Send dictation to the agent** below.                                                                                                                                                      |
| Command               | Absolute path to the agent program, for example `/path/to/my-agent`. Relative paths are rejected. At most 1024 characters, no control characters.                                                                                                                                                                   |
| Arguments             | Space-separated, for example `--acp`. **No quoting and no shell parsing**: `"a b"` becomes two arguments, `"a` and `b"`. At most 8 arguments, each at most 256 characters, no control characters.                                                                                                                   |
| Turn timeout          | 10 to 900 seconds. Default 120. A turn that takes longer is cancelled (see [Timeouts and failures](#timeouts-and-failures)).                                                                                                                                                                                        |
| Approval mode         | Which permission requests from the agent are decided without asking you, and which ones reach you. Four modes: ask before changes, ask before every tool call, never allow, and allow everything. Fresh installs start on allow everything; existing settings keep the mode you saved. See [Approvals](#approvals). |
| Conversation hand-off | Used by conversations. On by default. When on, open-ended Hey Jarvis requests that need the web, files or running programs are sent to the agent. Local commands such as opening apps stay local. Used for each conversation request; see [Used by conversations](#used-by-conversations).                          |
| Approval timeout      | 10 to 300 seconds. Default 60. A request you do not answer in time is denied.                                                                                                                                                                                                                                       |
| Test connection       | Starts the agent if needed and runs the handshake. It reports success or the reason for failure. Use it after you change the command or arguments.                                                                                                                                                                  |

The agent runs with a working folder of its own: `acp_workspace` inside Vox's app data directory. Vox creates it with permissions `0700` (only your user can read it). The agent does not run in your home folder or in a project folder.

Vox starts the agent in the background at launch when the feature is enabled and the command is valid. This is called prewarming. It makes the first dictation faster.

## Send dictation to the agent

Dictation has its own switch, **Send dictation to the agent**. It is **off by default**.

- Dictation goes to the agent only when **both** the ACP agent and this switch are on.
- When this switch is off, dictation **always pastes**, even while the ACP agent is on. Turning the agent on for conversations does not change what dictation does.
- Conversation hand-off and typed chat do not need this switch. They work with the agent alone.
- **Upgrading:** if you relied on the old behaviour, where turning on the agent also sent unmatched dictation to it, turn on **Send dictation to the agent** again after the update.

## How a request flows

1. You dictate. Vox first tries its built-in and custom voice commands.
2. If none matched, and the ACP agent is enabled and **Send dictation to the agent** is on, Vox sends the **raw dictation text, unchanged**, to the agent as a prompt. Otherwise the text is pasted.
3. The agent streams text back. Vox collects it, removes terminal control characters, and caps its length.
4. Vox emits the reply through the existing agent-reply path, which shows it and speaks it.

Events used by the interface are the same as before: `agent-bridge-reply` for the reply, `agent-bridge-exchange` for the conversation log, and `agent-bridge-error` for failures.

## Sessions and context

The setting **Agent context** (in the same area as the other agent settings) controls whether the agent remembers earlier requests.

- **Agent context off (default).** Every request starts a fresh session inside the same warm process. The process stays running, but the agent has no memory of earlier requests.
- **Agent context on.** One session is reused across requests, so the agent keeps its conversation. Vox does not resend the history itself.
- **Clear history** and **turning context off** reset the session. The next request starts a fresh one.
- The agent program may keep its own sessions on disk or in its own storage. **Vox cannot clear those.** Clear them with the agent's own tools if you need to.
- Restarting Vox starts with no session.

### Used by conversations

Hey Jarvis conversations can hand open-ended requests to this agent. See [Handing work to your agent](conversation.md#handing-work-to-your-agent).

- Each conversation request is a **fresh session**. It does not reuse the session from **Agent context**, and the agent does not remember earlier conversation requests.
- The conversation's role is carried as plain text inside the request.
- The **Approval mode** applies to these requests, the same as to dictation.

### Specialist agents

Conversation plans can run different steps on different agents. See
[Plans and specialist agents](conversation.md#plans-and-specialist-agents).

- Four optional profiles exist: **research**, **browser**, **code** and
  **files**. Each has a command and arguments.
- A blank command means that step type uses the main agent. **General** steps
  always use the main agent.
- The main agent must still be on, with a command set. A specialist profile
  alone does not enable anything.
- The specialist rows sit on the same Agent settings page as the ACP agent,
  next to the **Plan multi-step requests** toggle. That toggle is on by default
  but does nothing unless hand-off is on. Turn it off to keep single-agent
  hand-off.
- Commands follow the same rules as the main command: absolute path, no control
  characters. Arguments are split on spaces, with no quoting.
- Do not put secrets in arguments.
- The working folder, the approval mode and the turn timeout come from the main
  agent settings.

## Timeouts and failures

- **Turn timeout.** When a turn exceeds the turn timeout, Vox cancels the request, discards any partial text, and **stops the agent process**. The next request starts a new process. Time spent waiting for your answer to an approval does not count toward this timeout, but a turn never runs longer than three times the turn timeout.
- **Agent exits.** If the agent process exits (for example after a crash), Vox reports the failure. It starts a new process automatically on the next request.
- **Idle shutdown.** If the agent receives no request for 30 minutes, Vox stops it. The next request starts it again.
- **Start-up time.** The first start can take several seconds, because the agent has to start and complete the handshake. Prewarming at launch hides most of this.

## Approvals

When the agent wants to use one of its own tools, it asks Vox for permission. Vox shows a dialog with the request's title, the operation details (when the agent gives them), a countdown and two buttons: **Allow once** and **Deny**. Deny has the focus, so Enter and Escape also deny. Allow needs an explicit click. If several requests are waiting, the dialog shows how many are queued.

The **Approval mode** setting decides which requests reach you. Each mode is one choice in **Settings > Agents > ACP agent**:

| Mode                         | Allowed without asking                                                                                                                              | Asks you                                        | Denied without asking |
| ---------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------- | --------------------- |
| Ask before changes           | Read, search and think requests, only when the agent offers an allow-once option for that request and the request input does not look like a change | Every other request                             | None                  |
| Ask before every tool call   | None                                                                                                                                                | Every request                                   | None                  |
| Never allow                  | None                                                                                                                                                | None                                            | Every request         |
| Allow everything (never ask) | Every request that offers an allow-once option, whatever its kind                                                                                   | Only a request that offers no allow-once option | None                  |

**Allow everything** is the default for a **fresh install only**. Existing settings are never switched to it on upgrade. A saved mode is kept as it is, and a settings file with no saved mode uses **Ask before changes**. Earlier versions denied every request; choose **Never allow** to get that behaviour back.

> **Allow everything is not a sandbox and removes the questions.** When you choose it, Settings shows this warning: "The agent can edit, delete, run and browse without asking. Web pages and files it reads can steer it. Use Ask before changes if you do not fully trust the agent." Vox shows a notice, once per launch, when an agent run starts under this mode. Choose **Ask before changes** to get the questions back.

These rules apply in every mode:

- Vox only ever grants **allow once**. It never grants "always allow" and never remembers an answer.
- If you do not answer within the **Approval timeout**, the request is denied.
- Waiting for your answer does not count toward the turn timeout. A turn still cannot run longer than three times its turn timeout, however long the approvals take.
- Cancelling a turn withdraws any request that is still open, and the dialog closes. Vox does not send an answer for a withdrawn request, so the agent is not told allow or deny. If the agent does not stop on its own, Vox retires its process after a short grace period. The engine supports cancelling. Voice cancel and hand-off from conversation mode are not connected yet.
- If Vox cannot send an answer back to the agent, the turn fails. You then see a notice with **Copy request**, which copies your original dictation. The agent may have partly run the request, so check what it did before you repeat it.

### What an approval does and does not protect

- **The agent reports what each tool does.** Its kind, title and operation details come from the agent, and Vox trusts that report. An agent that labels an action as a read will have that action allowed without a question. Approvals are a convenience policy, **not a sandbox**.
- **Use Ask before every tool call for agents you do not fully trust.**
- **The dialog is not a security boundary.** An agent that can control your desktop input could click Allow itself.
- **Titles and details are shown as plain text** and are never run. They are not written to the log.

## Trust model and privacy

Read this section before you enable the feature.

- **The agent program is fully trusted and is not sandboxed.** It runs as your user, with your permissions.
- **It inherits Vox's environment variables.** Anything Vox can see in its environment, the agent can also see.
- **Its own tools are not controlled by Vox.** If the agent can run commands, edit files or reach the network with its own tools, Vox cannot stop it. Vox does not see or approve those actions.
- **Vox answers the agent's permission requests using the Approval mode** (see [Approvals](#approvals)). This limits only what goes through Vox's protocol. **It does not limit what the agent does with its own tools.** Approvals are not a sandbox.
- **Recommendation:** configure the agent itself to be read-only, or to ask before it changes anything. Do this in the agent's own settings, not in Vox.
- **Process cleanup is best effort.** When Vox stops the agent, it signals the agent's whole process group. A child process that moves to a different process group can keep running after Vox stops the agent.
- **Privacy.** What the agent receives: requests you hand over in conversations and typed chat, and, only if **Send dictation to the agent** is on, dictation that no built-in or custom voice command matched. Vox does not filter what you say. The agent program may log, store or send that text according to its own rules.
- **Replies are not filtered for safety.** Vox removes terminal control characters from replies and caps their length. It does **not** check whether a reply contains harmful instructions. Do not let the agent's replies control anything you would not do yourself.
- **Authentication is not supported by Vox.** Vox does not perform the agent's login step. Authenticate the agent outside Vox, using its own login or setup, before you enable the feature.

## Known limits and follow-ups

- **No spoken approval yet.** Permission requests are answered on screen only, with the dialog described in [Approvals](#approvals). The agent can use its read, search and think tools without asking. It asks for everything else, or nothing at all if you choose **Never allow**.
- **No streaming into speech.** Vox speaks the reply once the turn is finished.
- **Background jobs and the spoken-reply composer still use the old one-shot command setting.** They still start a process per request, and their settings are unchanged. They are not part of this feature.
- **Platform testing.** The feature is Unix only. In this repository, `.github/workflows/test.yml` runs `cargo test` only on `ubuntu-24.04`. The manually started `build-test.yml` builds the app on macOS, Linux and Windows, but I found no workflow that runs the tests on macOS or Windows. Tests that use Unix process features are therefore checked on Linux only in CI.

## Troubleshooting

| Message           | Likely cause                                                                                                                                   | What to do                                                                                                                 |
| ----------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| Not configured    | The feature is on but the command is empty or invalid.                                                                                         | Enter an absolute path to the agent program in **Settings > Agents > ACP agent**.                                          |
| Command not found | The path does not exist, or it is not a file Vox can run.                                                                                      | Check the path. Use the full absolute path, not a name on your `PATH`.                                                     |
| Handshake failed  | The program started but did not answer the ACP handshake. It may not be an ACP agent, or the arguments are wrong.                              | Check the arguments (for example `--acp`). Use **Test connection**. The message includes a short, capped detail.           |
| Timeout           | The turn took longer than the turn timeout.                                                                                                    | Ask a shorter question, or raise the turn timeout (up to 900 seconds). The agent process is restarted on the next request. |
| Agent exited      | The agent process stopped during a request. The message shows the end of its error output (stderr), capped at 200 characters in the interface. | Read the message, fix the cause in the agent's setup, then try again. Vox restarts the agent on the next request.          |

## Related pages

- [Voice features](voice-features.md)
- [Voice assistant](voice-assistant.md)
- [Vox phases: agent bridge (superseded for interactive dictation)](vox-phases/PHASE-3-agent-bridge.md)
