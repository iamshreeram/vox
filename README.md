# Vox

Vox is a cross-platform desktop speech-to-text application built with Tauri, Rust, React, and TypeScript. It records from a configurable shortcut, transcribes locally with installed speech models, and pastes the result into the active application.

## Project status

This is Ram's personal Vox repository: [github.com/iamshreeram/vox](https://github.com/iamshreeram/vox). The repository currently documents building and testing from source; do not assume third-party package-manager listings, installers, web properties, or integrations exist unless they are linked from this repository.

## Build and run

Prerequisites: stable Rust and Bun. See [`BUILD.md`](BUILD.md) for platform prerequisites.

```bash
bun install
bun run tauri dev
```

On macOS, if CMake reports a policy-version error:

```bash
CMAKE_POLICY_VERSION_MINIMUM=3.5 bun run tauri dev
```

Build a local debug bundle with:

```bash
CMAKE_POLICY_VERSION_MINIMUM=3.5 bun run tauri build --debug
```

For the local macOS LaunchAgent install path used on this development machine:

```bash
./scripts/install-macos.sh
```

That script is a local development helper, not a signed or notarized distribution installer.

## Development checks

```bash
bun run lint
bun run format:check
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
```

## What Vox can do

In short: you talk, Vox types what you said. With the optional features below it can also run simple voice commands, talk back, remember things, and look up facts about your projects. Features marked **off by default** are switched on in Settings (most in the **Experimental** page).

### Dictation (the core feature)

- **Talk instead of type.** Press a shortcut, speak, and Vox pastes the text into whatever app you are using. Defaults: `Option+Space` on macOS, `Ctrl+Space` on Windows and Linux.
- **Works offline.** Speech is turned into text on your computer with models you install. You can download, switch, and delete models, and Vox can unload the model after a period of inactivity.
- **Cleanup options.** Remove filler words, translate speech to English, add a trailing space, or press Enter for you after pasting.
- **AI cleanup (post-processing).** A second shortcut (`Option+Shift+Space`) can tidy the text with a language model, when you turn this on.
- **Recording helpers.** A small on-screen overlay while recording (live text where the model supports it), `Escape` to cancel, optional mute of other audio while recording, optional sound cues, and a warning when the active field is a secure password box.
- **History.** Past dictations are saved locally so you can search, copy, retry, or delete them.

### Voice commands (off by default)

- **Simple spoken commands.** Say things like "pause", "next track", or "volume 40" to control media, or "open Safari" to open an app, folder, or website.
- **Your own phrases.** Map a phrase to a website, folder, or app (up to 50).
- **Wake word** _(partly built)_. A listening mode can start dictation when it hears a wake phrase.
- **Ambient mode** _(partly built)_. Vox can notice when you address it by name, but today it only shows a notification; it does not act on it yet.

### Talking back (off by default)

- **Spoken replies.** Vox reads answers aloud using the system voice, with a voice and speed you can choose.
- **Short spoken summaries.** Instead of reading a long answer word for word, Vox can ask a model, or a command-line tool you configure, to boil it down to a sentence or two first.
- **Hands-free conversation.** One shortcut (`Control+Option+C` on macOS, `Ctrl+Alt+C` elsewhere), the tray menu, or Settings starts a back-and-forth: Vox listens, answers out loud, then listens again. Say "stop", "bye", or "that's all" to end it. You can tune how long it waits for silence and how many earlier turns it remembers.
- **Realtime voice** _(experimental)_. A faster direct-to-model route for conversations. It has been tested only against mock servers so far.

### Memory and notes (off by default)

- **Remembered facts.** Ask Vox to remember, recall, list, or forget small facts.
- **Saved conversations.** Keep a record of each conversation, optionally as Markdown notes in an Obsidian-style folder (a "vault").
- **Notes as context.** Let a conversation read from that folder so answers can use your own notes.

### Assistant tools (off by default)

- **Agent bridge.** Send a spoken request to a command-line agent you set up and speak or paste its reply.
- **Project lookups.** In a conversation, Vox can look up your projects and answer from the results. It can only read:
  - list your project folders,
  - show a project's git status and recent commits,
  - read one file from a project,
  - open a page in Vox's own Settings,
  - recall titles and summaries of past conversations.
- **Built-in safety.** Project lookups only see folders you list in Settings. Secret files such as keys and `.env` files are never read, results are scrubbed of anything that looks like a password or token, and results are treated as data, not instructions. A protection switch (on by default) stops the model from pasting text or running a command for the rest of a session once it has read project data. Turning that switch off lets Vox act freely.
- **Spoken "OK?" before changes.** When assistant tools or coding jobs are turned on, Vox asks you out loud and waits for a clear "yes" before it types text, opens something or starts a coding job that the model planned. Anything other than a plain yes, including silence, an unclear answer or an error, means no. It asks once and never reads your answer back or saves it. Commands you say yourself, such as an exact phrase you configured, are not asked about. A switch in Settings turns the question off. With the assistant tools and coding jobs both off, nothing changes.
- **Background coding jobs (off by default, macOS and Linux).** Ask Vox to make a code change in one of your listed projects and it starts the coding command you set up for the agent bridge, in that project's folder, in the background. Only one job runs at a time and each has a time limit (30 minutes by default, adjustable from 1 to 120). Vox tells you when it starts and when it finishes, succeeds, fails, times out or is cancelled. It never reads the job's output aloud. Say "cancel the job", use the tray menu, or press Cancel in Settings to stop it, and quitting Vox stops it too. A job runs the coding command with your account's permissions, so turn this on only if you are comfortable with that.

### The app itself

- **Tray menu.** Open Settings, check for updates, copy the last transcript, start or stop a conversation, choose a model, switch listening mode, cancel, or quit.
- **Start options.** Launch at login, start hidden in the tray, or run without a tray icon.
- **Command line.** Flags such as `--toggle-transcription`, `--toggle-post-process`, `--cancel`, `--start-hidden`, `--no-tray`, and `--debug` control a running copy, and `--transcribe-file` transcribes a WAV file without opening the window.
- **Many languages.** The interface is translated into more than twenty languages.
- **Updates.** Check for updates from the app; builds installed from this repository's update script can rebuild from source.

### Not built yet

Vox is not a complete autonomous voice assistant. Reading your screen (OCR) and controlling other windows or the browser are planned but not implemented, and coding jobs are not available on Windows yet. Design notes live in [`docs/vox-phases/`](docs/vox-phases/); details for the talking features are in [`docs/spoken-replies.md`](docs/spoken-replies.md) and [`docs/conversation.md`](docs/conversation.md).

## Permissions and platform notes

### macOS

Microphone permission is required for recording. Accessibility permission is used for global shortcuts and text input. Grant permissions under **System Settings → Privacy & Security**. Local development app signatures can change during rebuilds, so macOS may require permissions to be granted again.

### Linux

Text input varies by display server. The app may use `xdotool` on X11 or `wtype`/`dotool` on Wayland, depending on configuration. See [`docs/troubleshooting/`](docs/troubleshooting/) for a tested Ubuntu Wayland setup.

A few persisted settings and environment variables retain historical names for compatibility with existing installations. Those are implementation identifiers, not current product branding; do not rename them without a migration.

### Existing model artifacts

Some existing model artifacts are hosted under their original publisher's domain. The model catalog is the source of truth for working URLs. Those legacy hosts are not Vox-owned services; do not infer or invent replacement hostnames.

## Issues and contributions

Report problems or discuss changes in the [Vox GitHub repository](https://github.com/iamshreeram/vox). Include reproduction steps and relevant redacted logs; never include private dictated text, credentials, or personal data.

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for development and personal-repository contribution rules.

## License

See [`LICENSE`](LICENSE).
