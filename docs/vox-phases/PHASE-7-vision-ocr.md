# Phase 7 - Screen OCR ("what's on my screen", Apple Vision)

**Branch:** `phase/7-vision-ocr` - **Worktree:** `.worktrees/phase-7-vision-ocr/`
**Depends on:** nothing (lowest priority phase - build last / assign to
whichever model is free)

## 1. Goal

A single, opt-in, observation-only voice command: capture the screen,
run on-device OCR, hand the extracted text back (as plain text for now;
speaking it aloud is a Phase 4 integration once both exist). Mirrors vox's
`src/vox/vision/` (`screen_capture.py` + Apple Vision OCR adapter).

## 2. Non-goals (read this twice - this phase is tightly scoped on purpose)

- **Observation only.** No click/move/type/scroll/hotkey actions. No
  Plan->Observe->Act->Verify loop. A real computer-use action loop is deliberately out of scope here.
  Do not add any action capability here, however small or tempting.
- No continuous/automatic screen monitoring - single-shot, user-triggered
  capture only.
- No integration with the agent bridge's reasoning in this phase - this
  phase delivers "capture + OCR text" as an isolated capability with its
  own tests; wiring OCR'd text into an agent prompt is later integration
  work.

## 3. Design

New file: `src-tauri/src/managers/vision.rs`, macOS-only
(`cfg(target_os = "macos")`), using `screencapture -x` (matching vox's own
approach of shelling out to the system tool rather than reimplementing
screen capture) piped into Apple's on-device Vision framework OCR via
`objc2` bindings (check for an existing `objc2-vision` crate before
hand-rolling FFI, same principle as Phase 4).

```rust
pub struct OcrResult { pub text: String, pub confidence: f32, pub captured_at_ms: u64 }
// confidence: average VNRecognizedTextObservation confidence across all
// recognized text candidates, 0.0 if none found -- mirrors vox's own
// OcrResult.confidence (src/vox/vision/ocr.py's _recognize_via_vision_framework
// averages topCandidates_(1).confidence() per observation).

#[derive(Debug)]
pub enum VisionError {
    Unsupported,          // non-macOS
    CaptureFailed(String),
    OcrFailed(String),
    Disabled,
}

pub trait ScreenOcr: Send + Sync {
    fn capture_and_read(&self) -> Result<OcrResult, VisionError>;
}
```

**Verified against vox's actual `screen_capture.py`:** the `screencapture
-x <path>` + 10-second timeout design below is exactly what vox already
ships (not just a reasonable guess) -- vox's own `_real_capture` uses
the identical command and timeout value. One concrete detail vox's error
messages get right that this phase must also include: **both the
"command not found" and "non-zero exit" failure paths explicitly mention
Screen Recording permission** (`System Settings -> Privacy & Security ->
Screen Recording`) in the error text, not just a generic
"capture failed" message -- a bare non-zero exit from `screencapture` is
the single most likely real-world cause and is often exactly this
permission being unset, per vox's own first-hand experience building
this. Given this whole project's own extensive experience this session
with how unhelpful generic permission failures are, treat this as a hard
requirement, not a nice-to-have.

### Settings addition

```rust
pub vision_enabled: bool, // default: false - matches vox's own off-by-default
```

### Tauri command

- `vision_read_screen() -> Result<String, String>` - checks
  `vision_enabled` first, returns the disabled-error if off, otherwise
  captures + OCRs + returns the text (or a clear error for capture/OCR
  failure, distinguished per `VisionError` variant, not collapsed into one
  generic message).

## 4. Functional requirements

- FR1: When `vision_enabled` is `false`, `vision_read_screen` returns
  `VisionError::Disabled` immediately - no `screencapture` process is
  spawned, no Vision framework call is made. This command is not even
  registered as matchable by any voice-command phrase in this state
  (mirrors vox: "the command is not registered/matchable at all unless
  explicitly enabled").
- FR2: On non-macOS, `capture_and_read` returns `VisionError::Unsupported`
  - crate still compiles cross-platform per the usual gating rule.
- FR3: A capture failure (e.g. `screencapture` binary missing/returns
  non-zero, simulate via a fake process runner) returns
  `VisionError::CaptureFailed` with a human-readable reason, distinct from
  an OCR failure. **The error text must explicitly mention Screen
  Recording permission** (`System Settings -> Privacy & Security ->
Screen Recording`) for both the "binary not found" and "non-zero exit"
  cases -- ports vox's own error messages verbatim-in-spirit, since a
  missing permission is the single most likely real-world cause of
  either failure.
- FR4: An OCR failure on a successfully-captured image (e.g. Vision
  framework returns an error, simulate via a fake OCR adapter) returns
  `VisionError::OcrFailed`, distinct from `CaptureFailed`.
- FR5: A screenshot with genuinely no text on it (e.g. a solid color
  image) is not an error - returns `Ok(OcrResult { text: "", .. })`, since
  "no text found" is a valid, successful outcome, not a failure.
- FR6: Temporary screenshot files (if any are written to disk as an
  intermediate step, matching vox's own `screencapture -x` -> temp file ->
  Vision framework flow) are always cleaned up, including when OCR fails
  partway through - use a scope guard / RAII pattern or explicit
  cleanup-on-every-path, and test both the success and failure paths leave
  no leftover temp file. **Deliberate improvement over vox, not a gap
  ported incorrectly:** vox's own `ScreenCaptureAdapter` does NOT clean up
  its captured PNGs at all (they accumulate in `output_dir` indefinitely).
  This phase's stricter zero-persistence stance (NFR1) is an intentional,
  stronger privacy posture for this port -- don't "fix" this requirement
  away thinking it's a porting mismatch.

## 5. Non-functional requirements

- NFR1: No screenshot image content is ever persisted beyond the
  transient temp file needed to hand it to the Vision framework API (if
  the chosen Vision API binding requires a file path rather than in-memory
  data - check whether `objc2-vision`/`objc2-app-kit` allow an in-memory
  `CGImage`/`NSImage` handoff instead, which would avoid touching disk at
  all; prefer that if available).
- NFR2: No new third-party crate beyond possibly one `objc2-*` subcrate for
  Vision framework bindings, same principle as Phase 4's NFR2.
- NFR3: `capture_and_read` must complete in well under a human-perceptible
  "it's broken" timeframe for a typical screen - set and test a generous
  but real timeout (e.g. 10 seconds) so a hung Vision framework call
  surfaces as a clear timeout error rather than hanging the app forever.

## 6. Test cases (write FIRST, red->green->refactor)

Use fakes for the process-spawn and Vision-framework-call boundaries for
all tests except the explicitly marked real-hardware smoke test, same
split pattern as Phases 3/4.

### Settings gate

| #   | Given                              | Expect                                                                                                 |
| --- | ---------------------------------- | ------------------------------------------------------------------------------------------------------ |
| T1  | `vision_enabled = false` (default) | `vision_read_screen` returns `VisionError::Disabled`; fake process runner asserts it was never invoked |
| T2  | Default value of `vision_enabled`  | `false`                                                                                                |

### Capture failure handling

| #   | Given                                                                                    | Expect                                                                                       |
| --- | ---------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| T3  | Fake capture process exits non-zero                                                      | `VisionError::CaptureFailed`, OCR step never attempted                                       |
| T3a | Fake capture process exits non-zero OR the capture binary is missing                     | Error text contains "Screen Recording" -- ports vox's own permission-guidance error messages |
| T4  | Fake capture process "succeeds" but produces no output file (simulate a filesystem race) | `VisionError::CaptureFailed`, not a confusing downstream OCR error                           |

### OCR failure handling

| #   | Given                                                                                 | Expect                                                                                                                     |
| --- | ------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| T5  | Capture succeeds, fake OCR adapter returns an error                                   | `VisionError::OcrFailed`, distinct variant from capture failures                                                           |
| T6  | Capture succeeds, fake OCR adapter returns empty text (simulating a blank screenshot) | `Ok(OcrResult { text: "", .. })` - not an error, per FR5                                                                   |
| T7  | Capture succeeds, fake OCR adapter returns a long multi-paragraph text                | Returned verbatim, no truncation introduced by this layer (any truncation is a caller's/UI's decision, not this manager's) |

### Cleanup (FR6)

| #   | Given                                                                            | Expect                                                                                                               |
| --- | -------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| T8  | Full success path (capture + OCR both succeed) using a real temp-file-based fake | No leftover temp file exists after `capture_and_read` returns                                                        |
| T9  | Capture succeeds but OCR fails                                                   | No leftover temp file exists after the error is returned (this is the easy-to-get-wrong case - verify it explicitly) |

### Cross-platform

| #   | Given                                                          | Expect                                             |
| --- | -------------------------------------------------------------- | -------------------------------------------------- |
| T10 | Non-macOS target (or a directly-exercised stub implementation) | `VisionError::Unsupported`, crate compiles cleanly |

### Timeout (NFR3)

| #   | Given                                                          | Expect                                                                                    |
| --- | -------------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| T11 | Fake Vision-framework call configured to hang past the timeout | Returns a timeout-flavored error within a bounded test time, does not hang the test suite |

### Real-hardware smoke test (documented in PR, not CI-asserted)

| #   | Scenario                                                                                                                      | Expect                                                                                                                     |
| --- | ----------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| T12 | Run on a real Mac, open a text-heavy window (e.g. this very markdown file in an editor), call `vision_read_screen()` for real | Returned text contains a recognizable fragment of what was actually on screen. Document in the PR that this was performed. |

## 7. Acceptance criteria

- [ ] Every test in SS6 exists, watched failing, then passing (T12
      performed manually, documented in the PR).
- [ ] `cargo clippy -- -D warnings` clean.
- [ ] No action capability (click/type/scroll) exists anywhere in this
      phase's diff - this is the single most important review point for
      this phase given vox's own explicit scope boundary here.
- [ ] `vision_enabled` defaults to `false` and gates both the command AND
      its registration as a matchable voice phrase (if a phrase registry
      from Phase 2 exists on `main` by the time this merges - if not yet,
      at minimum the Tauri command itself is fully gated).
- [ ] No leftover temp files on either the success or failure path (T8/T9
      passing is the proof).
