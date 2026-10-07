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

## Capabilities

- Local speech transcription using supported installed models.
- Configurable global shortcuts, recording modes, and audio devices.
- Transcription history and local settings.
- Optional live recording overlay, including streaming transcription where supported.
- Platform-specific text insertion and system-tray controls.

Vox is not yet a complete autonomous voice assistant. Memory, deterministic command routing, a generic agent bridge, native TTS, wake-word detection, ambient mode, and screen OCR are tracked in [`docs/vox-phases/`](docs/vox-phases/).

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
