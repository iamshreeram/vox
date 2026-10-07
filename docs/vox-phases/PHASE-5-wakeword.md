# Phase 5 - Wake Word ("Hey Vox"-style, ONNX-based)

**Branch:** `phase/5-wakeword` - **Worktree:** `.worktrees/phase-5-wakeword/`
**Depends on:** nothing to start (uses existing VAD infra); Phase 6
(Ambient Mode) depends on this phase being merged first.

## 1. Goal

Opt-in, continuous-listening wake-word detection mirroring vox's
`src/vox/wakeword/` (openWakeWord, ONNX-based). Handy already has `ort`
(via `transcribe-rs`) for ONNX inference and `rustfft` for spectral math,
plus `vad-rs`/`earshot` for voice-activity detection - this phase is mostly
assembling existing capability, not inventing new infrastructure.

## 2. Non-goals

- No custom "Hey Vox" trained model in this phase. Vox's own `ARCH.md`
  flags training a custom wake word as a distinct, separate undertaking
  (synthetic TTS dataset + training pipeline) - **not scoped here.** This
  phase ships with openWakeWord's existing pretrained models (e.g. "hey
  jarvis", whatever ships with the openWakeWord project) as a placeholder,
  with the model file configurable so swapping in a real "Hey Vox" model
  later requires zero code changes (matches vox's own
  `wake_word.model_name` config-swap design).
- No barge-in/interrupt handling while something else is speaking/
  listening - vox explicitly deferred this as YAGNI; do the same here.
- No wiring into the actual recording-trigger flow yet - this phase
  delivers a `WakeWordEngine` that emits a detection event with a
  confidence score; a later integration phase decides what to DO when
  triggered (likely reusing Phase 3's agent bridge).

## 3. Design

New file: `src-tauri/src/managers/wakeword.rs`.

```rust
pub struct WakeWordDetection { pub model_name: String, pub confidence: f32, pub timestamp_ms: u64 }

pub trait WakeWordEngine: Send + Sync {
    /// Feed one frame of mono audio (sample rate must match the model's
    /// expected rate - resample via the existing `rubato` dependency if
    /// the input stream doesn't already match, same as the STT pipeline
    /// does). Returns Some(detection) if this frame's rolling window
    /// crosses the configured confidence threshold.
    fn process_frame(&mut self, samples: &[f32]) -> Option<WakeWordDetection>;

    fn reset(&mut self);
}
```

Feature extraction (mel-spectrogram) is the part most worth getting right:
openWakeWord's own preprocessing pipeline (melspectrogram -> embedding
model -> wake-word classifier, a 3-model cascade) must be reproduced
faithfully enough that confidence scores are meaningful. Read
`src/vox/wakeword/openwakeword_engine.py` in the vox repo first - it
documents the exact preprocessing vox's Python implementation relies on
(via the `openwakeword` PyPI package); match its framing/hop size and
normalization, don't reinvent the pipeline from scratch.

### Settings addition

```rust
pub wake_word_enabled: bool,       // default: false (matches vox's own default-off)
pub wake_word_model_name: String,  // default: whatever placeholder model ships
pub wake_word_confidence_threshold: f32, // default: 0.5, matching openWakeWord's own common default
```

### Tauri commands / events

- `wakeword_set_enabled(enabled: bool) -> Result<(), String>`
- Emits a Tauri event `wakeword://detected` with the `WakeWordDetection`
  payload when triggered (event-based, not a polled command, since
  detection is asynchronous/continuous) - mirrors the existing
  command-event architecture documented in this repo's own `AGENTS.md`.

## 4. Functional requirements

- FR1: When `wake_word_enabled` is `false`, no microphone stream is opened
  for wake-word purposes and `process_frame` is never called by the live
  audio pipeline - this is a resource/privacy requirement (vox's own docs
  stress wake word means "the mic listens continuously", opt-in only).
- FR2: `process_frame` never panics on malformed input: empty slice,
  all-zero (silence) buffer, a buffer shorter than one full analysis
  window, NaN/Inf samples (simulate a corrupted audio source).
- FR3: Feeding genuine silence (all zeros) for an extended period (e.g. 10
  seconds of frames) never produces a detection, regardless of threshold.
- FR4: Confidence scores are in `[0.0, 1.0]` - clamp/validate, never
  propagate a raw model output outside that range as if it were valid.
- FR5: `reset()` clears any internal rolling buffer/state such that audio
  fed before `reset()` cannot contribute to a detection after it (useful
  for tests and for clean state after a false-positive cooldown).
- FR6: Changing `wake_word_model_name` via settings takes effect on the
  next engine (re)initialization without requiring an app restart (a
  later integration phase may relax this if genuinely impractical, but
  the manager itself must support swapping cleanly - prove it with a test
  that constructs two engines with different model names side by side).

## 5. Non-functional requirements

- NFR1: CPU overhead must be bounded - vox's own measurement (see
  `ARCH.md` SS8) found Python + onnxruntime + openwakeword adds ~127MB RSS
  over baseline; the whole point of doing this in Rust is to beat that.
  Add a test/benchmark note (not necessarily a hard-failing CI gate, but a
  documented measurement in the PR) comparing idle RSS with wake word on
  vs off.
- NFR2: Uses `ort` (already pulled in via `transcribe-rs`) for inference
  and `rustfft` (already a dependency) for the mel-spectrogram FFT step -
  no new heavyweight ML dependency.
- NFR3: Runs on a background thread, never blocks the UI or the existing
  dictation-hotkey recording path - the two must be able to coexist or, if
  truly mutually exclusive (vox's wake word and ambient mode are mutually
  exclusive with each other, but vox's wake word is NOT exclusive with
  hotkey dictation - verify this against vox's source and match it, don't
  assume).

## 6. Test cases (write FIRST, red->green->refactor)

### Core detection behavior

| #   | Given                                                                                                                                                                                                                   | Expect                                                                                                                                                                                                                                               |
| --- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| T1  | Synthetic audio clip of actual recorded/synthesized wake-word speech (generate via macOS `say` command the same way vox's own `test_openwakeword_engine_real.py` does, or use a bundled fixture WAV) fed frame-by-frame | At least one `process_frame` call returns `Some(detection)` with `confidence >= threshold`                                                                                                                                                           |
| T2  | 10 seconds of all-zero (silence) frames                                                                                                                                                                                 | No detection ever returned                                                                                                                                                                                                                           |
| T3  | 10 seconds of synthesized unrelated speech (e.g. `say "the quick brown fox"`)                                                                                                                                           | No detection returned (or if the placeholder pretrained model has false positives on this phrase, document that as a known limitation - do not weaken the test, instead document the limitation and choose a confirmed-clean phrase for the fixture) |
| T4  | Empty slice passed to `process_frame`                                                                                                                                                                                   | No panic, returns `None`                                                                                                                                                                                                                             |
| T5  | Slice of `f32::NAN`/`f32::INFINITY` values                                                                                                                                                                              | No panic; either returns `None` or a sanitized result - must not propagate NaN into a confidence score (assert `!confidence.is_nan()` if a detection is somehow returned)                                                                            |
| T6  | A buffer shorter than one full analysis window                                                                                                                                                                          | No panic, returns `None` (buffered internally for the next call, not discarded silently - verify by feeding the rest of the window in a follow-up call and confirming detection still works end-to-end across the split)                             |
| T7  | `reset()` called mid-utterance (feed half the wake word's audio, reset, feed silence)                                                                                                                                   | No detection (the reset utterance's partial audio must not "linger" and combine with anything after)                                                                                                                                                 |
| T8  | Two `WakeWordEngine` instances constructed with different `model_name`s, each fed the same audio appropriate to ITS model                                                                                               | Each only detects its own wake word's audio (proves no global/shared state leaks between instances)                                                                                                                                                  |

### Threshold / confidence validity

| #   | Given                                                                | Expect                                                                   |
| --- | -------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| T9  | Threshold set very high (e.g. 0.99) with borderline-confidence audio | No detection (confirms threshold is actually applied, not just logged)   |
| T10 | Threshold set very low (e.g. 0.01) with the same borderline audio    | Detection fires (confirms the threshold is configurable both directions) |
| T11 | Any detection returned, across all tests                             | `0.0 <= confidence <= 1.0` always                                        |

### Settings gate

| #   | Given                                 | Expect                                                                                                                                                                                                           |
| --- | ------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| T12 | `wake_word_enabled = false` (default) | The live audio pipeline integration point (even if just a feature-flag check function in this phase, since full pipeline wiring is a later phase) returns "disabled", and no engine is constructed/no mic opened |
| T13 | Default value of `wake_word_enabled`  | `false`                                                                                                                                                                                                          |

## 7. Acceptance criteria

- [ ] Every test in SS6 exists, watched failing, then passing.
- [ ] `cargo clippy -- -D warnings` clean.
- [ ] No new heavyweight dependency - `ort`/`rustfft` reused.
- [ ] `wake_word_enabled` defaults to `false`.
- [ ] A documented (in the PR) idle-RSS measurement comparing wake-word
      on vs off exists, per NFR1.
- [ ] Mel-spectrogram/feature-extraction parameters are documented in a
      code comment with a citation back to what `openwakeword`/vox's own
      usage of it expects, so a future contributor can verify correctness
      without reverse-engineering magic numbers.
