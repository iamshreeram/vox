# Phase 4 - Native TTS (macOS AVSpeechSynthesizer)

**Branch:** `phase/4-tts` - **Worktree:** `.worktrees/phase-4-tts/`
**Depends on:** nothing

## 1. Goal

Give Vox a voice. A `TtsManager` that can speak text aloud on macOS via
`AVSpeechSynthesizer`, mirroring vox's Apple TTS backend
(`src/vox/tts/apple_engine.py`) - the simplest of vox's three TTS backends,
and the right first target since Handy already depends on `objc2` +
`objc2-app-kit` + `objc2-foundation` for its existing macOS paste path.
Porting Kokoro inference to Rust is explicitly deferred to a later phase,
not this one (see Non-goals).

## 2. Non-goals

- No Kokoro (ONNX) TTS in this phase - native AVSpeechSynthesizer only.
  Track a future "Phase 4b: Kokoro TTS" separately if/when needed; do not
  scope-creep this phase into it.
- No Windows (SAPI) or Linux (speech-dispatcher) backends in this phase -
  macOS only, matching vox's own scope. The `TtsManager` trait must still
  compile cross-platform (a no-op/`Unsupported` implementation on
  non-macOS), per the platform-gating ground rule.
- No TTS-echo suppression / mic-pause-while-speaking logic yet - that's an
  integration concern for whichever phase wires TTS into the live
  recording flow (relevant once Phase 5/6 wake-word and ambient mode
  exist). This phase delivers "given text, speak it, know when it's done,
  be able to stop it early" as an isolated subsystem.

## 3. Design

New file: `src-tauri/src/managers/tts.rs`, macOS implementation behind
`cfg(target_os = "macos")` using `objc2`/`objc2-foundation` bindings to
`AVFoundation`'s `AVSpeechSynthesizer`/`AVSpeechUtterance`. If an
`objc2-avf-audio` (or equivalently named) crate exists and covers
`AVSpeechSynthesizer`, use it instead of hand-rolling raw `objc2::msg_send!`
calls - check before writing manual FFI bindings.

```rust
pub trait TtsEngine: Send + Sync {
    /// Blocks the calling thread until speech finishes OR `stop()` is
    /// called from another thread. Must be called off the Tauri
    /// command/async-runtime thread (spawn_blocking), mirroring how
    /// `transcription.rs` handles its own blocking model work.
    fn speak(&self, text: &str) -> Result<(), TtsError>;

    /// Interrupts in-flight speech immediately. No-op if nothing is
    /// speaking. Safe to call from any thread.
    fn stop(&self);

    fn is_speaking(&self) -> bool;
}

#[derive(Debug)]
pub enum TtsError {
    Unsupported,       // non-macOS, or macOS but the engine failed to init
    EmptyText,
    SynthesisFailed(String),
}
```

### Settings addition

```rust
pub tts_enabled: bool,        // default: false
pub tts_voice_id: Option<String>, // default: None -> system default voice
pub tts_rate: f32,            // default: 0.5 (AVSpeechUtterance's own
                               // normalized 0.0-1.0 rate scale)
```

### Tauri commands

- `tts_speak(text: String) -> Result<(), String>`
- `tts_stop() -> Result<(), String>`
- `tts_is_speaking() -> Result<bool, String>`
- `tts_list_voices() -> Result<Vec<VoiceInfo>, String>` (id + display name
  - language code, from `AVSpeechSynthesisVoice.speechVoices()`)

## 4. Functional requirements

- FR1: `speak("")` returns `TtsError::EmptyText` immediately, never calls
  into AVFoundation with empty text.
- FR2: `speak(text)` on a non-macOS target returns `TtsError::Unsupported`
  - the crate must still compile and this code path must still be tested
    there (via the trait + a dummy/stub implementation), per the
    cross-platform-must-keep-compiling ground rule.
- FR3: Calling `stop()` while `speak()` is blocked on another thread
  causes that `speak()` call to return promptly (bounded by a test
  timeout, e.g. under 2 seconds) rather than waiting for the full
  utterance to finish naturally.
- FR4: `is_speaking()` returns `true` strictly between a `speak()` call
  starting and it finishing/being stopped, `false` otherwise - including
  `false` before the first ever `speak()` call.
- FR5: `tts_voice_id` set to an invalid/nonexistent voice id falls back to
  the system default voice rather than erroring (AVFoundation's own
  behavior when given `nil`/not-found - confirm and lock in via test; if
  AVFoundation instead errors, this manager must convert that into a
  graceful fallback itself).
- FR6: Multiple back-to-back `speak()` calls queue sequentially (the
  second doesn't start until the first finishes or is stopped) - no
  overlapping/garbled simultaneous speech.

## 5. Non-functional requirements

- NFR1: `speak()` must run off the Tauri async runtime's worker threads
  (blocking call pattern, same note as Phase 3's NFR2).
- NFR2: No new third-party crate beyond possibly one `objc2-*` subcrate for
  AVFoundation bindings if `objc2-app-kit`/`objc2-foundation` alone don't
  cover `AVSpeechSynthesizer` - check the `objc2` project's crate family
  first (it ships per-framework crates, e.g. `objc2-avf-audio`) before
  hand-writing raw message-send FFI.
- NFR3: Must not regress existing macOS paste functionality - this phase
  adds AVFoundation usage alongside, not instead of, the existing
  `objc2-app-kit` paste-path usage; run the full existing test suite, not
  just new tests, before considering this phase done (this is also in the
  global ground rules, repeated here because FFI code is exactly the kind
  of change most likely to have spooky-action-at-a-distance effects).

## 6. Test cases (write FIRST, red->green->refactor)

Tests that require actually hearing/verifying audio output are hard to
assert on in CI. Split tests into "state machine" tests (assertable without
real audio hardware) and a small number of manually-verified real-synthesis
smoke tests, mirroring vox's own `test_apple_tts_engine.py` split between
contract tests and real-engine tests.

### State machine / contract tests (run in normal `cargo test`, macOS CI)

| #   | Given                                                                                             | Expect                                                                                                                                                                            |
| --- | ------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| T1  | Fresh `TtsEngine`, never spoken                                                                   | `is_speaking() == false`                                                                                                                                                          |
| T2  | `speak("")`                                                                                       | Returns `Err(TtsError::EmptyText)`, `is_speaking()` stays `false`                                                                                                                 |
| T3  | `speak("hello")` on a background thread, poll `is_speaking()` shortly after starting              | `true` while in progress                                                                                                                                                          |
| T4  | After the utterance naturally finishes (short text, wait with a generous timeout)                 | `is_speaking()` returns to `false`, and the `speak()` call returns `Ok(())`                                                                                                       |
| T5  | `speak("a very long paragraph...")` on a background thread, then call `stop()` almost immediately | `speak()` returns (success or an explicit "interrupted" signal - define which in code and assert it) well before the natural completion time, and `is_speaking()` becomes `false` |
| T6  | `stop()` called when nothing is speaking                                                          | No-op, no panic, no error                                                                                                                                                         |
| T7  | Two sequential `speak()` calls from the same thread, one after the other completes                | Both complete successfully, no overlap (hard to assert overlap directly without audio capture - at minimum assert both return `Ok(())` and `is_speaking()` is `false` after both) |
| T8  | `tts_voice_id` set to `"definitely-not-a-real-voice-id"`                                          | `speak()` still succeeds (falls back to default voice per FR5), does not error                                                                                                    |
| T9  | `tts_list_voices()` on a macOS machine                                                            | Returns a non-empty list; every entry has a non-empty `id` and non-empty `language` code                                                                                          |

### Settings gate

| #   | Given                                            | Expect                                                                               |
| --- | ------------------------------------------------ | ------------------------------------------------------------------------------------ |
| T10 | `tts_enabled = false` (default)                  | `tts_speak` Tauri command returns a clear disabled-error, never reaches AVFoundation |
| T11 | Default value of `tts_enabled` on fresh settings | `false`                                                                              |

### Cross-platform compile/behavior

| #   | Given                                                                                                                                         | Expect                                                                                                                 |
| --- | --------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| T12 | Built/tested on a non-macOS target (or via a `cfg`-gated stub implementation exercised directly in a unit test even when developing on macOS) | `speak()` returns `TtsError::Unsupported`; crate compiles cleanly, no macOS-only symbols leak into the non-macOS build |

### Real-synthesis manual smoke test (documented in PR, not asserted by CI)

| #   | Scenario                                                                         | Expect                                                                   |
| --- | -------------------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| T13 | Run the app, call `tts_speak("Vox is online")` for real on a real Mac with audio | You actually hear it. Document in the PR description that this was done. |

## 7. Acceptance criteria

- [ ] Every test in SS6 exists, watched failing, then passing (T13
      performed manually and documented in the PR).
- [ ] `cargo clippy -- -D warnings` clean on macOS, and the crate still
      builds clean on Windows/Linux targets (CI matrix, or at minimum
      `cargo check --target <other-os-triple>` if cross-compilation is
      available locally).
- [ ] `tts_enabled` defaults to `false`.
- [ ] No regression in any existing test (full `cargo test` run, not just
      new tests).
