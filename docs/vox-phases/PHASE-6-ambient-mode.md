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
/// Mirrors vox's TranscriptSegment exactly (fields vox's own
/// EngagementJudge.evaluate() and RollingTranscript both rely on).
pub struct TranscriptSegment {
    pub segment_id: String,
    pub text: String,
    pub start_time_ms: u64,
    pub end_time_ms: u64,
    pub confidence: f32,
    pub during_tts: bool,
}

pub struct RollingTranscript {
    // RAM-only ring buffer, ~90 seconds of segments, matching vox's own
    // default retention_seconds.
}

impl RollingTranscript {
    /// `now_ms` is caller-supplied (monotonic), never read from the wall
    /// clock internally -- mirrors vox's own design specifically so this
    /// stays trivially deterministic in tests (vox's own module docstring
    /// calls this out explicitly).
    pub fn add_segment(&mut self, text: &str, start_time_ms: u64, end_time_ms: u64, confidence: f32, during_tts: bool) -> TranscriptSegment;
    pub fn recent_segments(&mut self, now_ms: u64, within_ms: Option<u64>) -> Vec<TranscriptSegment>;
    pub fn clear(&mut self); // explicit privacy action
}
```

pub enum Engagement {
    Addressed { extracted_request: String }, // "Vox, what time is it" -> "what time is it"
    NotAddressed,
}

pub struct EngagementJudge { pub wake_names: Vec<String> } // ["vox"] by default -- vox's own
    // Python code defaults to ("vox", "jarvis") for its own historical
    // dual-brand transition reasons that don't apply here; a Vec lets a
    // future settings UI add aliases without an API change, but default
    // to just ["vox"].

impl EngagementJudge {
    /// **CORRECTED against vox's actual `src/vox/ambient/engagement.py`.**
    /// An earlier draft of this phase invented a 3-way Addressed/
    /// Mentioned/Unrelated split and two worked examples (T9/T10 below,
    /// now fixed) that do not match vox's real, shipped behavior. vox's
    /// `EngagementDecision` is binary (`addressed: bool`) -- it never
    /// distinguishes "mentions Vox" from "totally unrelated"; both
    /// collapse to "not addressed". The signal is PURELY POSITIONAL: does
    /// the wake name appear as literally the first word (optionally
    /// after "hey"/"okay"/"ok")? There is deliberately no understanding
    /// of "Vox is a good name" (statement) vs "Vox, do X" (command) --
    /// both start with the wake word, so vox's own heuristic treats BOTH
    /// as addressed. This is a known, accepted limitation of vox's
    /// design (documented in its own module docstring: "conservative by
    /// design... an unwanted response is a worse failure than a missed
    /// one" -- but conservative here means "err toward checking
    /// everything that starts with the name", not the opposite).
    ///
    /// Port vox's exact regex: `^\s*(?:hey|okay|ok)?[\s,]*(\w+)\b[,:]?\s*(.*)$`
    /// (case-insensitive). Group 1 is the candidate wake word; if it
    /// doesn't match (case-insensitively) a configured `wake_names`
    /// entry, this segment is not an address. Group 2 is the remainder/
    /// query text.
    ///
    /// Operates on a WINDOW of segments (oldest first), not a single
    /// string -- this is required to correctly handle "Hey Vox." as its
    /// own utterance followed by a pause, then the real request in a
    /// separate segment a moment later. Algorithm (ports vox's
    /// `evaluate()` exactly): scan the window from NEWEST to OLDEST;
    /// for the first segment whose text matches the address regex AND
    /// whose captured name is a known wake name, take its remainder as
    /// the query. If that remainder is empty (bare "Hey Vox." with
    /// nothing after it in the same segment), join every segment AFTER
    /// it in the window as the query instead. If no segment in the
    /// window matches, return `NotAddressed`.
    pub fn judge(&self, segments: &[TranscriptSegment]) -> Engagement;
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
- FR2: `RollingTranscript::recent_segments` never returns segments older
  than the requested window, and `add_segment` evicts anything older than
  the configured `ambient_window_ms` from internal storage (not just from
  query results - actually freed, provable via a memory-bound test: push
  far more than the window holds and assert internal storage size is
  capped). Mirrors vox's own `_expire()`, called on every `add_segment`
  AND on every `recent_segments` read (both sides self-clean, matching
  vox exactly).
- FR3: `clear()` immediately empties the transcript such that any
  `recent_segments` call right after returns empty, regardless of what
  was pushed before.
- FR4: `EngagementJudge::judge` on a single-segment window
  `["Vox, what time is it"]` returns
  `Addressed { extracted_request: "what time is it" }`.
- FR5: `EngagementJudge::judge` on a window where the most recent
  segment's first word does NOT match any configured wake name (e.g.
  `["what's the weather like today"]`) returns `NotAddressed`.
  **CORRECTED:** an earlier draft of this phase also expected a separate
  `Mentioned` outcome for text like "I was telling Vox about the
  project" -- vox's real `EngagementDecision` has no such third state;
  that case is also just `NotAddressed` (the word "Vox" appearing
  anywhere other than as the literal first word has no special handling
  at all). Do not implement a Mentioned-vs-Unrelated distinction unless
  you're deliberately scoping NEW, unvalidated behavior beyond vox --
  if you do, design and test it as its own clearly-labeled addition, not
  as if it were part of the ported heuristic.
- FR6: `EngagementJudge::judge` on a multi-segment window where a bare
  address ("Hey Vox.") has empty same-segment remainder pulls the
  following segment(s) in as the query -- ports vox's trailing-segment
  join exactly (see T14 below).
- FR7: The wake name list is configurable (`ambient_wake_name`, or a
  `Vec<String>` if supporting aliases), not hardcoded to the literal
  string "Vox" - a test must construct an `EngagementJudge` with a
  different name (e.g. "Computer") and confirm detection still works,
  proving no hardcoding slipped in.

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

| #   | Given                                                                                                              | Expect                                                                                                                       |
| --- | ----------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| T1  | Add 3 segments at t=0, t=1000, t=2000ms                                                                            | `recent_segments(now_ms=2000, None)` returns all 3, oldest first                                                              |
| T2  | Add a segment at t=0, then another at t=100_000 (past the 90s window)                                              | `recent_segments(now_ms=100_000, None)` no longer includes the t=0 segment (evicted by `_expire`, ported from vox exactly)     |
| T3  | Add 1000 segments spread across a much longer time range than the window                                           | Internal storage size stays bounded (does not grow unboundedly - assert via a size/len check, not just query-time filtering) |
| T4  | `clear()` after adding several segments                                                                             | `recent_segments` returns empty immediately after                                                                            |
| T5  | `recent_segments` called on a brand-new, empty transcript                                                           | Returns empty, no panic                                                                                                      |
| T6  | Concurrent `add_segment` from one thread and `recent_segments` reads from another, run for a short stress duration | No panic, no data race (use a loom-style or simple concurrent stress test with `std::thread`)                                |

### `EngagementJudge`

**CORRECTED against vox's actual `engagement.py`.** All cases below were
verified against vox's real regex
(`^\s*(?:hey|okay|ok)?[\s,]*(\w+)\b[,:]?\s*(.*)$`), not assumed --
several outcomes in an earlier draft of this table were wrong, most
notably T10.

| #   | Wake names    | Window (segments, oldest first)                              | Expect                                                                                                                   |
| --- | ------------- | ---------------------------------------------------------------| ----------------------------------------------------------------------------------------------------------------------- |
| T7  | `["vox"]`     | `["Vox, what time is it"]`                                     | `Addressed { extracted_request: "what time is it" }`                                                                     |
| T8  | `["vox"]`     | `["vox what time is it"]` (no comma, lowercase)                | `Addressed { extracted_request: "what time is it" }` -- vox's regex has no comma requirement, confirmed from the pattern itself |
| T9  | `["vox"]`     | `["I was telling Vox about the project"]`                      | `NotAddressed` -- first word is "I", not a wake name (mentioning Vox mid-sentence is never detected by this heuristic, by design) |
| T10 | `["vox"]`     | `["Vox is a good name for this"]`                              | **`Addressed { extracted_request: "is a good name for this" }`** -- CORRECTED: an earlier draft expected `Mentioned` here, but vox's heuristic is purely positional (first word == wake name), so this statement-that-happens-to-start-with-the-wake-word IS treated as addressed. This is a known, accepted limitation of the ported heuristic, not a Rust-side bug -- do not "fix" it to be smarter than vox's own validated behavior without that being a deliberate, separately-tracked enhancement. |
| T11 | `["vox"]`     | `["what's the weather like today"]`                            | `NotAddressed`                                                                                                            |
| T12 | `["vox"]`     | `[]` (empty window)                                             | `NotAddressed`, no panic -- ports vox's empty-segments-list early return                                                 |
| T13 | `["computer"]`| `["Computer, lights off"]`                                      | `Addressed { extracted_request: "lights off" }` - proves FR7 (no hardcoding)                                             |
| T14 | `["vox"]`     | `["Hey Vox.", "what time is it"]` (two segments: bare address, then the real request in a separate segment after a pause) | `Addressed { extracted_request: "what time is it" }` -- ports vox's trailing-segment join (FR6): the addressed segment's own remainder is empty, so the following segment is pulled in as the query |
| T15 | `["vox"]`     | `["VOX VOX VOX what time is it"]` (repeated)                    | `Addressed` -- regex only captures the first `\w+` as the candidate name ("VOX"), remainder is `"VOX VOX what time is it"` -- assert this exact string, don't guess a "cleaned up" version vox's regex doesn't actually produce |

### Mutual exclusion with wake word (FR1)

| #   | Given                                                      | Action                                       | Expect                                                                                                                                                                             |
| --- | ---------------------------------------------------------- | -------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| T16 | `wake_word_enabled = true`, `ambient_mode_enabled = false` | Attempt to set `ambient_mode_enabled = true` | Defined behavior occurs (either rejected with a clear error, or wake word auto-disables) - whichever is chosen, both settings' final state is asserted, not just the attempted one |
| T17 | Both `false` (fresh install default)                       | Enable ambient mode                          | Succeeds, wake word stays `false`                                                                                                                                                  |

### Settings defaults

| #   | Given          | Expect                                                                                       |
| --- | -------------- | -------------------------------------------------------------------------------------------- |
| T18 | Fresh settings | `ambient_mode_enabled == false`, `ambient_wake_name == "Vox"`, `ambient_window_ms == 90_000` |

## 7. Acceptance criteria

- [ ] Every test in SS6 exists, watched failing, then passing.
- [ ] `cargo clippy -- -D warnings` clean.
- [ ] No disk persistence of transcript content anywhere (NFR1 verified by
      code review/grep, noted explicitly in the PR description).
- [ ] Mutual exclusion with wake word is enforced and tested (T15/T16).
- [ ] `EngagementJudge`'s pattern set is cited back to vox's
      `src/vox/ambient/engagement.py` in a code comment, not reinvented
      from scratch.
