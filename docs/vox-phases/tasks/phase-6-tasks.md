# Phase 6 (Ambient Mode) — Task Cards

Parent plan: `../PHASE-5-6-TASK-QUEUE.md`. Spec of record:
`../PHASE-6-ambient-mode.md` (all `T<n>` references below point to that
file's SS6 test tables unless marked "(new)").

Same general rules as `phase-5-tasks.md` apply here (file boundaries,
TDD-first, no new crates without flagging, `allow(dead_code)` is expected
on unwired code).

**Cross-phase dependency:** A5 needs Phase 5's W1 merged into
`phase/5-wakeword` (reads the `wake_word_enabled` setting for mutual
exclusion). If W1 hasn't merged yet when A5 is ready to start, the
Implementer should branch A5 off a commit that includes W1's settings
change (ask the Validator which commit), not guess.

---

## A1 — Settings + scaffolding

**Depends on:** nothing. **Blocks:** A2, A3, A4, A6.

**Files touched:**
- `src-tauri/src/managers/ambient/mod.rs` (new)
- `src-tauri/src/managers/ambient/transcript.rs` (new, skeleton only)
- `src-tauri/src/managers/ambient/engagement.rs` (new, skeleton only)
- `src-tauri/src/managers/ambient/echo.rs` (new, skeleton only)
- `src-tauri/src/managers/ambient/coordinator.rs` (new, skeleton only)
- `src-tauri/src/managers/mod.rs` (add `pub mod ambient;`)
- `src-tauri/src/settings.rs` (add 3 fields + defaults)

**Implement:**
- `TranscriptSegment`, `RollingTranscript` (struct shape only, methods
  can `unimplemented!()`), `Engagement` enum, `EngagementJudge` (struct
  shape only), `EchoSuppressor` (struct shape only) — exactly as typed in
  the phase doc section 3.
- Settings: `ambient_mode_enabled: bool` (default `false`),
  `ambient_wake_name: String` (default `"Vox"`), `ambient_window_ms: u64`
  (default `90_000`).

**Tests (write first):**
- T18: fresh settings → `ambient_mode_enabled == false`,
  `ambient_wake_name == "Vox"`, `ambient_window_ms == 90_000`.
- (new) legacy-settings-missing-field test mirroring the existing
  `memory_enabled_defaults_false_when_missing_from_legacy_settings`
  pattern — old settings files without these 3 keys still parse.

**Acceptance criteria:** crate compiles. `cargo clippy -- -D warnings`
clean.

---

## A2 — RollingTranscript ring buffer

**Depends on:** A1. **Parallel-safe with:** A3, A4.

**Files touched:** `src-tauri/src/managers/ambient/transcript.rs` only.

**Implement:** the RAM-only ring buffer per the phase doc §3 — `now_ms`
is always caller-supplied, never read from the wall clock internally
(this is required for deterministic tests, not a style choice).
`add_segment` AND `recent_segments` both self-expire anything older than
`ambient_window_ms`, per vox's own `_expire()`-on-both-sides design.

**Tests (write first):**
- T1: add 3 segments at t=0, t=1000, t=2000ms →
  `recent_segments(now_ms=2000, None)` returns all 3, oldest first.
- T2: add a segment at t=0, then another at t=100_000 (past the 90s
  window) → `recent_segments(now_ms=100_000, None)` no longer includes
  the t=0 segment.
- T3: add 1000 segments spread across a much longer time range than the
  window → internal storage size stays bounded (assert a size/len check,
  not just query-time filtering — storage must actually be freed).
- T4: `clear()` after adding several segments → `recent_segments` returns
  empty immediately after.
- T5: `recent_segments` on a brand-new, empty transcript → returns empty,
  no panic.
- T6: concurrent `add_segment` from one thread and `recent_segments`
  reads from another, run for a short stress duration → no panic, no data
  race (mirror the concurrency-test style already used in this repo, e.g.
  `MemoryManager`'s `concurrent_remember_calls_persist_both_facts` test).

**Acceptance criteria:** NFR2 (safe to push from an audio-callback thread
while read from the UI/coordinator thread) — use a `Mutex`/`RwLock`-
guarded structure, proven by T6. `cargo clippy -- -D warnings` clean.

---

## A3 — EngagementJudge heuristic

**Depends on:** A1. **Parallel-safe with:** A2, A4.

**Files touched:** `src-tauri/src/managers/ambient/engagement.rs` only.

Port vox's exact regex: `^\s*(?:hey|okay|ok)?[\s,]*(\w+)\b[,:]?\s*(.*)$`
(case-insensitive). This is a **binary** addressed/not-addressed
decision, purely positional (does the wake name appear as literally the
first captured word) — there is no "mentioned vs. addressed" distinction.
Re-read the phase doc §3's full explanation before implementing; a prior
draft of this spec got this wrong and the corrected version matters.

**Tests (write first — all from the phase doc, already corrected against
vox's real behavior):**
- T7: wake names `["vox"]`, window `["Vox, what time is it"]` →
  `Addressed { extracted_request: "what time is it" }`.
- T8: wake names `["vox"]`, window `["vox what time is it"]` (no comma,
  lowercase) → `Addressed { extracted_request: "what time is it" }`.
- T9: wake names `["vox"]`, window `["I was telling Vox about the
  project"]` → `NotAddressed` (first word is "I", not a wake name).
- T10: wake names `["vox"]`, window `["Vox is a good name for this"]` →
  `Addressed { extracted_request: "is a good name for this" }` — this
  IS addressed per vox's real heuristic, do not "fix" it to be smarter.
- T11: wake names `["vox"]`, window `["what's the weather like today"]`
  → `NotAddressed`.
- T12: wake names `["vox"]`, empty window `[]` → `NotAddressed`, no
  panic.
- T13: wake names `["computer"]`, window `["Computer, lights off"]` →
  `Addressed { extracted_request: "lights off" }` (proves no hardcoding
  of "vox" anywhere in the implementation).
- T14: wake names `["vox"]`, window `["Hey Vox.", "what time is it"]`
  (two segments) → `Addressed { extracted_request: "what time is it" }`
  (the bare-address segment's own remainder is empty, so the following
  segment is pulled in as the query).
- T15: wake names `["vox"]`, window `["VOX VOX VOX what time is it"]` →
  `Addressed` with `extracted_request == "VOX VOX what time is it"`
  (the regex only captures the FIRST `\w+` as the name; assert this exact
  string, not a "cleaned up" guess).

**Acceptance criteria:** the pattern is cited back to vox's
`src/vox/ambient/engagement.py` in a code comment (phase doc's own
explicit acceptance criterion). `cargo clippy -- -D warnings` clean.

---

## A4 — EchoSuppressor

**Depends on:** A1. **Parallel-safe with:** A2, A3.

**Files touched:** `src-tauri/src/managers/ambient/echo.rs` only.

The phase doc describes this conceptually ("suppresses the ambient
listener's own TTS output from being re-transcribed as a human saying
it") but does not give it a numbered test table — write one now. Test
against a fake "currently speaking" boolean signal, NOT a real TTS
dependency (Phase 4 doesn't exist yet; this must be independently
buildable).

**Tests (write first, new — no T-numbers in the phase doc for this
piece):**
- (new) given a fake "is TTS currently speaking" signal reporting `true`,
  a segment added during that window is tagged/flagged as
  `during_tts: true` (reuses the `TranscriptSegment.during_tts` field
  already defined in A1's scaffolding).
- (new) given the signal reporting `false`, a segment is tagged
  `during_tts: false`.
- (new) a segment tagged `during_tts: true` is excluded when
  `EngagementJudge` evaluates a window for addressing (echo of Vox's own
  speech must never trigger a false "addressed" detection) — this test
  may need to live in A5 instead if it requires the Coordinator to wire
  `EchoSuppressor` + `EngagementJudge` together; if so, write a stub
  version here (pure flagging logic only) and note in your report that
  the integration assertion belongs to A5.

**Acceptance criteria:** zero dependency on Phase 4 (TTS) code — the
"is speaking" signal is a trait/closure/fake, not a real `TtsManager`
reference (Phase 4 isn't built; this must compile and test independent
of it, same principle the phase doc applies elsewhere).

---

## A5 — Coordinator + mutual exclusion with Wake Word

**Depends on:** A2, A3, A4, **and Phase 5's W1 merged** (reads
`wake_word_enabled`). **Sequential** — this composes everything above.

**Files touched:** `src-tauri/src/managers/ambient/coordinator.rs` only.

**Implement:** wires `RollingTranscript` + `EngagementJudge` +
`EchoSuppressor` together into one coordinator type, plus the mutual-
exclusion rule against Wake Word. Pick ONE conflict-resolution behavior
(the phase doc explicitly says "pick one, document it, test it" — do not
leave this ambiguous): recommended default is **enabling one
auto-disables the other** (friendlier UX than a hard error, and matches
how most "pick one mode" settings behave elsewhere in this app). Confirm
this choice with the Validator before implementing if there's any doubt.

**Tests (write first):**
- T16: `wake_word_enabled = true`, `ambient_mode_enabled = false`, then
  attempt to set `ambient_mode_enabled = true` → defined behavior occurs
  (per whichever resolution was chosen); assert BOTH settings' final
  state, not just the one being set.
- T17: both `false` (fresh install default), enable ambient mode →
  succeeds, wake word stays `false`.
- (new) symmetric case: `ambient_mode_enabled = true`, attempt to enable
  `wake_word_enabled` → same resolution rule applied in the other
  direction (the phase doc only gives the one-directional T16; the
  reverse direction must behave consistently, not be an untested gap).

**Acceptance criteria:** mutual exclusion is enforced at the settings-
write layer (wherever `wake_word_enabled`/`ambient_mode_enabled` get
written — likely a shared validation function called by both features'
`*_set_enabled` commands), not just in the UI (a raw settings-file edit
or a future second UI surface must not be able to bypass it).

---

## A6 — Tauri commands + events

**Depends on:** A1 only. **Parallel-safe with:** A2, A3, A4.

Build the IPC surface against fakes/stubs — unblocks A8 (Settings UI)
early, same reasoning as Phase 5's W6.

**Files touched:**
- `src-tauri/src/commands/ambient.rs` (new)
- `src-tauri/src/commands/mod.rs` (register it)
- `src-tauri/src/lib.rs` (register commands in `generate_handler!`)

**Implement:**
- `ambient_set_enabled(enabled: bool) -> Result<(), String>` (goes
  through A5's shared mutual-exclusion validation once A5 lands; for this
  task in isolation, a direct settings write is fine as a placeholder —
  flag to the Validator to confirm it gets swapped to the validated path
  once A5 merges).
- `ambient_clear() -> Result<(), String>` — calls `RollingTranscript::clear()`
  (the explicit privacy action from the phase doc).
- Event `ambient://addressed` carrying `{ extracted_request: String }`
  when the coordinator detects an addressed utterance.

**Tests (write first):**
- (new) `ambient_set_enabled(true)`/`(false)` round-trips through
  settings, mirroring `memory_set_enabled`'s test style.
- (new) `ambient_clear()` actually empties a `RollingTranscript` fixture.
- (new) event-emission helper test, same style as W6's.

**Acceptance criteria:** regenerate `src/bindings.ts` once this lands
(same documented process as W6).

---

## A7 — Mic-flow integration (scope extension)

**Depends on:** A5, A6. **Sequential, senior Implementer, careful
Validator review** — same caution level as W7.

Not in the original phase doc (explicitly deferred there) — added because
Ram wants a working, demoable feature.

**Files touched:**
- `src-tauri/src/actions.rs` or a new small coordinator-wiring module
  (flag file-size concerns to the Validator, same note as W7)
- `src-tauri/src/lib.rs` (register ambient coordinator as managed state)

**Implement:**
- When `ambient_mode_enabled` is true, continuously run VAD-segmented
  transcription (reuse the existing `TranscriptionManager`/VAD pipeline —
  do not build a second transcription path) and feed each finished
  segment into `RollingTranscript::add_segment`.
- After each new segment, run the Coordinator's engagement check over the
  recent window; on `Addressed`, emit `ambient://addressed` (A6). Per the
  phase doc's own non-goal, there is no reply/TTS wiring yet — the MVP
  action on detection is purely the event + a toast (W8/A8's job), it
  does not start a recording or call anything further.
- Respect NFR1 (zero disk persistence) — grep your own diff for any
  `File::create`/`rusqlite` touching transcript text before calling this
  done; there must be none.
- If Phase 5's W7 already landed and built shared "continuous mic
  capture" plumbing, reuse it rather than duplicating — check with the
  Validator which commit has that plumbing before starting.

**Tests (write first, as unit-testable as possible; a live-mic manual
smoke test is also required, documented in the report):**
- (new) a pure decision function `fn should_run_ambient_capture(settings:
  &AppSettings) -> bool` mirroring the Phase 1/2/5 gating pattern.
- (new) given a fixed sequence of fake transcribed segments fed through
  the full pipeline (RollingTranscript -> EchoSuppressor -> EngagementJudge),
  an "addressed" segment produces exactly one `ambient://addressed` emission
  with the correct extracted request text.
- (new) manual smoke test, documented in the report: say "Vox, what time
  is it" out loud with ambient mode on, confirm the toast/event fires with
  the right extracted text.

**Acceptance criteria:** zero regressions in the full existing test suite
and in the existing hotkey dictation flow (manual re-verification
required, same as W7). No disk persistence of transcript content
anywhere (grep-verified, noted in the report per NFR1).

---

## A8 — Settings UI (scope extension)

**Depends on:** A6 (can start early with fakes); finalize after A7.
**Coordinate with W8** — these two toggles need a consistent mutual-
exclusion UX (see `ui-wiring-tasks.md` U1 for how they get composed
together).

**Files touched:**
- `src/components/settings/AmbientModeToggle.tsx` (new, mirror
  `MemoryToggle.tsx`)
- `src/components/settings/index.ts` (export it)
- `src/components/settings/advanced/AdvancedSettings.tsx` (placement:
  coordinate with U1 — this may end up living in a new shared section
  instead of directly in the flat Experimental list; check
  `ui-wiring-tasks.md` before placing it)
- `src/i18n/locales/en/translation.json` (new keys)
- `src/App.tsx` (toast handler for `ambient://addressed`)
- Optionally a small debug-only rolling-transcript viewer component if
  time permits — NOT required for this task's completion; flag as a
  stretch item in your report rather than blocking on it.

**Implement:** a toggle bound to `ambient_mode_enabled`, disabled/greyed
out (with an explanatory tooltip) while Wake Word is already on, per
whatever mutual-exclusion UX A5 implemented on the backend — the UI must
reflect the same rule, not just rely on the backend silently overriding
it. A toast on `ambient://addressed` (e.g. "Vox heard: <request>"). A
"Clear ambient history" button calling `ambient_clear()`.

**Tests:** `bun run lint` and `bun run build` clean (same note as W8 — no
dedicated unit tests for this thin binding layer).

**Acceptance criteria:** toggling ambient mode on in Settings visibly
disables the Wake Word toggle (and vice versa) in the same session,
without needing a restart.
