# Phase 6 - Ambient Mode (rolling transcript + Engagement Judge)

**Branch:** `phase/6-ambient-mode` - **Worktree:** `.worktrees/phase-6-ambient-mode/`
**Depends on:** Phase 5 (Wake Word) must be merged to `main` first - this
phase reuses its always-on mic/VAD infrastructure and is mutually exclusive
with it at the settings level.

## 1. Goal

Opt-in, off-by-default "room mode": continuous VAD-segmented transcription
into a short RAM-only rolling buffer, with a deterministic Engagement Judge
that decides whether speech was _addressed to_ Vox ("Vox, what time is
it") versus merely _mentioning_ Vox ("I was telling Vox earlier...";
"Vox is a good idea actually"). Mirrors vox's `src/vox/ambient/` (`transcript.py`,
`engagement.py`, `echo.py`, `coordinator.py`).

## 2. Non-goals

- No trained classifier - the Engagement Judge is a deterministic regex/
  heuristic, exactly like vox's own (vox's feature matrix is explicit that
  this is "a deterministic regex heuristic, not a trained classifier").
  Do not scope-creep into ML here.
- Mutually exclusive with wake-word mode being enabled at the same time -
  enforce this at the settings layer (enabling one disables the other, or
  enabling one is blocked while the other is on - pick one UX and encode
  it as a test, don't leave it ambiguous).
- No persistence of the rolling transcript - RAM-only, with an explicit
  `clear()` as a privacy action, matching vox exactly.
- No actual reply/TTS wiring in this phase - this phase proves "detect
  that I was addressed, extract what was said" correctly; what happens
  next (agent bridge call, TTS reply) is integration work for a later
  phase once Phases 3 and 4 both exist on `main`.

## 3. Design

New files: `src-tauri/src/managers/ambient/transcript.rs`,
`src-tauri/src/managers/ambient/engagement.rs`,
`src-tauri/src/managers/ambient/echo.rs`,
`src-tauri/src/managers/ambient/coordinator.rs` (mirrors vox's own
sub-module layout 1:1, which makes porting the logic/tests far less
error-prone than inventing a new structure).

```rust
pub struct RollingTranscript {
    // RAM-only ring buffer, ~90 seconds of (text, timestamp) segments,
    // matching vox's own window length.
}

impl RollingTranscript {
    pub fn push(&mut self, text: &str, timestamp_ms: u64);
    pub fn recent_window(&self, window_ms: u64) -> Vec<(&str, u64)>;
    pub fn clear(&mut self); // explicit privacy action
}

pub enum Engagement {
    Addressed { extracted_request: String }, // "Vox, what time is it" -> "what time is it"
    Mentioned,                                // "I was telling Vox earlier"
    Unrelated,
}

pub struct EngagementJudge { pub wake_name: String } // "Vox" - configurable, not hardcoded

impl EngagementJudge {
    /// Deterministic heuristic. Read vox's `src/vox/ambient/engagement.py`
    /// for the exact pattern set to mirror (address-initial patterns like
    /// "^<name>[,:]?\\s+" vs mention patterns like "telling <name>",
    /// "<name> (is|was|said)" etc.) - do not invent a different heuristic
    /// from scratch, port the one that's already been validated.
    pub fn judge(&self, text: &str) -> Engagement;
}

pub struct EchoSuppressor {
    /// Suppresses the ambient listener's own TTS output from being
    /// re-transcribed as if a human said it. Needs to coordinate with
    /// Phase 4's TTS manager once both exist - for THIS phase, implement
    /// and test it against a fake "currently speaking" signal, not a real
    /// TTS dependency (keeps this phase buildable independent of Phase 4's
    /// merge status).
}
```

### Settings addition

```rust
pub ambient_mode_enabled: bool, // default: false; mutually exclusive with wake_word_enabled
pub ambient_wake_name: String,  // default: "Vox"
pub ambient_window_ms: u64,     // default: 90_000 (90s, matches vox)
```

## 4. Functional requirements

- FR1: `ambient_mode_enabled` and `wake_word_enabled` cannot both be
  `true` simultaneously - define and test the exact conflict-resolution
  behavior (e.g. setting one to `true` while the other is already `true`
  either errors or auto-disables the other; pick one, document it, test
  it).
- FR2: `RollingTranscript::recent_window` never returns segments older
  than the requested window, and `push` evicts anything older than the
  configured `ambient_window_ms` from internal storage (not just from
  query results - actually freed, provable via a memory-bound test: push
  far more than the window holds and assert internal storage size is
  capped).
- FR3: `clear()` immediately empties the transcript such that any
  `recent_window` call right after returns empty, regardless of what was
  pushed before.
- FR4: `EngagementJudge::judge` on `"Vox, what time is it"` returns
  `Addressed { extracted_request: "what time is it" }` (exact text per the
  ported heuristic's own normalization - lock in the precise expected
  string via the test, don't guess).
- FR5: `EngagementJudge::judge` on `"I was telling Vox about the project"`
  returns `Mentioned`, never `Addressed`.
- FR6: `EngagementJudge::judge` on text with no mention of the wake name at
  all returns `Unrelated`.
- FR7: The wake name is configurable (`ambient_wake_name`), not hardcoded
  to the literal string "Vox" - a test must construct an `EngagementJudge`
  with a different name (e.g. "Computer") and confirm detection still
  works, proving no hardcoding slipped in.

## 5. Non-functional requirements

- NFR1: Zero disk persistence of transcript content anywhere in this
  phase's code - this is a privacy requirement from vox's own design;
  grep the diff for any `File::create`/`rusqlite` usage touching
  transcript text and there should be none.
- NFR2: `RollingTranscript` must be safe to push to from an audio-callback
  thread while being read from the UI/coordinator thread concurrently (a
  `Mutex`/`RwLock`-guarded structure, tested with concurrent push+read).
- NFR3: No new third-party crate - this is pure Rust logic plus reuse of
  existing VAD/STT infra from Phase 5 and the existing transcription
  pipeline.

## 6. Test cases (write FIRST, red->green->refactor)

### `RollingTranscript`

| #   | Given                                                                                                     | Expect                                                                                                                       |
| --- | --------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| T1  | Push 3 segments at t=0, t=1000, t=2000ms                                                                  | `recent_window(3000)` returns all 3, in chronological order                                                                  |
| T2  | Push a segment at t=0, then another at t=100_000 (past the 90s window)                                    | `recent_window` anchored at the latest timestamp no longer includes the t=0 segment                                          |
| T3  | Push 1000 segments spread across a much longer time range than the window                                 | Internal storage size stays bounded (does not grow unboundedly - assert via a size/len check, not just query-time filtering) |
| T4  | `clear()` after pushing several segments                                                                  | `recent_window` returns empty immediately after                                                                              |
| T5  | `recent_window` called on a brand-new, empty transcript                                                   | Returns empty, no panic                                                                                                      |
| T6  | Concurrent `push` from one thread and `recent_window` reads from another, run for a short stress duration | No panic, no data race (use a loom-style or simple concurrent stress test with `std::thread`)                                |

### `EngagementJudge`

| #   | Wake name                          | Input                                         | Expect                                                                                                                                                                                                                                                                |
| --- | ---------------------------------- | --------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| T7  | "Vox"                              | `"Vox, what time is it"`                      | `Addressed { extracted_request: "what time is it" }`                                                                                                                                                                                                                  |
| T8  | "Vox"                              | `"vox what time is it"` (no comma, lowercase) | `Addressed` (case-insensitive, punctuation-tolerant - confirm against the ported heuristic's actual tolerance; if the ported heuristic requires the comma, this test instead asserts `Unrelated` or `Mentioned` and that must match vox's real behavior, not a guess) |
| T9  | "Vox"                              | `"I was telling Vox about the project"`       | `Mentioned`                                                                                                                                                                                                                                                           |
| T10 | "Vox"                              | `"Vox is a good name for this"`               | `Mentioned` (statement about Vox, not addressed to it)                                                                                                                                                                                                                |
| T11 | "Vox"                              | `"what's the weather like today"`             | `Unrelated`                                                                                                                                                                                                                                                           |
| T12 | "Vox"                              | `""`                                          | `Unrelated`, no panic                                                                                                                                                                                                                                                 |
| T13 | "Computer" (non-default wake name) | `"Computer, lights off"`                      | `Addressed { extracted_request: "lights off" }` - proves FR7 (no hardcoding)                                                                                                                                                                                          |
| T14 | "Vox"                              | `"VOX VOX VOX what time is it"` (repeated)    | `Addressed` with a sane extracted request (define exact expected string in the test once implemented, don't leave it vague)                                                                                                                                           |

### Mutual exclusion with wake word (FR1)

| #   | Given                                                      | Action                                       | Expect                                                                                                                                                                             |
| --- | ---------------------------------------------------------- | -------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| T15 | `wake_word_enabled = true`, `ambient_mode_enabled = false` | Attempt to set `ambient_mode_enabled = true` | Defined behavior occurs (either rejected with a clear error, or wake word auto-disables) - whichever is chosen, both settings' final state is asserted, not just the attempted one |
| T16 | Both `false` (fresh install default)                       | Enable ambient mode                          | Succeeds, wake word stays `false`                                                                                                                                                  |

### Settings defaults

| #   | Given          | Expect                                                                                       |
| --- | -------------- | -------------------------------------------------------------------------------------------- |
| T17 | Fresh settings | `ambient_mode_enabled == false`, `ambient_wake_name == "Vox"`, `ambient_window_ms == 90_000` |

## 7. Acceptance criteria

- [ ] Every test in SS6 exists, watched failing, then passing.
- [ ] `cargo clippy -- -D warnings` clean.
- [ ] No disk persistence of transcript content anywhere (NFR1 verified by
      code review/grep, noted explicitly in the PR description).
- [ ] Mutual exclusion with wake word is enforced and tested (T15/T16).
- [ ] `EngagementJudge`'s pattern set is cited back to vox's
      `src/vox/ambient/engagement.py` in a code comment, not reinvented
      from scratch.
