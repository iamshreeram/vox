# Phase 5 (Wake Word) — Task Cards

Parent plan: `../PHASE-5-6-TASK-QUEUE.md`. Spec of record: `../PHASE-5-wakeword.md`
(all `T<n>` references below point to that file's §6 test tables unless
marked "(new)").

General rules for every task below:
- Work only inside the listed "Files touched." If you need to touch
  anything else, stop and report back to the Validator instead of
  guessing.
- Write every listed test FIRST, watch it fail, then implement.
- `#![allow(dead_code)]` on new modules/functions that nothing calls yet
  is expected and correct until a later task wires them in — do not
  delete it prematurely, and do not invent a fake caller just to silence
  the warning.
- No new third-party crate without flagging it to the Validator first
  (phase doc's own NFR2/NFR3 already name what should be reused: `ort`,
  `rustfft`, the existing `managers/model/download.rs` pattern).

---

## W1 — Settings + scaffolding

**Depends on:** nothing. **Blocks:** W2, W3, W5, W6.

Create the shape everything else builds against, with no real detection
logic yet.

**Files touched:**
- `src-tauri/src/managers/wakeword.rs` (new)
- `src-tauri/src/managers/mod.rs` (add `pub mod wakeword;`)
- `src-tauri/src/settings.rs` (add 3 fields + defaults)

**Implement:**
- `WakeWordDetection { model_name: String, confidence: f32, timestamp_ms: u64 }`
- `WakeWordEngine` trait exactly as specified in the phase doc §3
  (`process_frame`, `reset`, `pause`, `resume`) — method bodies can
  `unimplemented!()` or return a fixed stub value, this task is about the
  shape, not the logic.
- A `NullWakeWordEngine` that implements the trait by always returning
  `None` from `process_frame` and no-op for everything else — this is
  both the cross-platform non-macOS production fallback AND useful as a
  test fake for later tasks.
- Settings: `wake_word_enabled: bool` (default `false`),
  `wake_word_model_name: String` (default `"hey_jarvis"`),
  `wake_word_confidence_threshold: f32` (default `0.5`).

**Tests (write first):**
- T12: `wake_word_enabled = false` → a feature-flag-check function (even
  a trivial `fn wakeword_enabled(settings: &AppSettings) -> bool`) returns
  `false`, and document that this is what later tasks must gate on.
- T13: default value of `wake_word_enabled` on fresh settings is `false`.
- (new) default value of `wake_word_model_name` is `"hey_jarvis"`.
- (new) default value of `wake_word_confidence_threshold` is `0.5`.
- (new) legacy-settings-missing-field test mirroring the existing
  `memory_enabled_defaults_false_when_missing_from_legacy_settings`
  pattern in `settings.rs` — confirms old settings files without this key
  still parse.

**Acceptance criteria:** crate compiles on macOS and (via `NullWakeWordEngine`
being the only concrete type exercised) the code is structured so a
non-macOS build will also compile once W4 adds the real macOS-only engine
behind `cfg(target_os = "macos")`. `cargo clippy -- -D warnings` clean.

---

## W2 — Frame buffering / input validation

**Depends on:** W1. **Parallel-safe with:** W3, W5, W6.

Implement the buffering contract WITHOUT a real ONNX model — process_frame
should buffer input into the correct window size and call an injectable
"classify this 1280-sample window" closure/trait method that this task
stubs out (returns a fixed dummy confidence), so the framing logic is
fully testable in isolation before W4 wires in the real model.

**Files touched:**
- `src-tauri/src/managers/wakeword.rs` only.

**Implement:**
- Internal accumulation buffer: accept arbitrary-sized `&[f32]` chunks,
  accumulate until exactly 1280 samples (80ms @ 16kHz) are available,
  then hand that window to an injected classifier function/trait.
- Resampling hook: if the phase doc's assumption of 16kHz input doesn't
  hold for the real caller, note where `rubato` would plug in (reuse
  pattern from the existing STT pipeline) — stub is acceptable here if
  the real caller (W7) always supplies 16kHz already; document the
  assumption explicitly in a code comment either way.
- `reset()` clears the accumulation buffer.

**Tests (write first):**
- T4: empty slice → no panic, no classification attempt, returns `None`.
- T5: a slice of `f32::NAN`/`f32::INFINITY` → no panic; if a detection
  somehow still returns, assert `!confidence.is_nan()`.
- T6: a buffer shorter than 1280 samples → no panic, returns `None`,
  buffered internally; feed the remainder in a follow-up call and confirm
  the classifier is invoked exactly once with a correctly-assembled
  1280-sample window.
- T7: `reset()` mid-window (feed 600 samples, reset, feed 600 more) → the
  classifier must NOT be invoked with a window mixing pre- and
  post-reset samples (assert via a spy closure recording what it was
  called with).
- T11 (partial — real confidence-range test is W4's job, but confirm the
  stub classifier's returned value is clamped into `[0.0, 1.0]` by this
  layer regardless of what the classifier returns, including an
  out-of-range stub value like `1.5` or `-0.2`).

**Acceptance criteria:** `cargo clippy -- -D warnings` clean on this file.
No ONNX/`ort` dependency touched yet — that's W4.

---

## W3 — Model download & cache

**Depends on:** W1. **Parallel-safe with:** W2, W5, W6.

openWakeWord's pipeline needs 3 ONNX model files (melspectrogram,
embedding, classifier — read `src/vox/wakeword/openwakeword_engine.py` in
the vox Python repo for the exact filenames/URLs it downloads). These are
NOT bundled; they download on first use and cache locally, matching the
existing STT model manager's pattern.

**Files touched:**
- `src-tauri/src/managers/wakeword.rs` (add a download/cache submodule or
  function set — follow the existing split style of
  `managers/model.rs` + `managers/model/download.rs` if that reuse is
  practical; ask the Validator before creating a new top-level file if
  the existing `managers/model/download.rs` helpers can't be reused
  directly, since NFR2 of the phase doc requires justifying any new
  mechanism).

**Implement:**
- A function to resolve the wake-word model directory under the app's
  data dir (reuse `crate::portable::app_data_dir`, matching how
  `MemoryManager`/`ModelManager` do it).
- A function to ensure the configured model's files are present locally,
  downloading if missing, no-op if already cached (idempotent).
- A clear, distinct error/state for "model not downloaded yet" — do not
  let a missing file surface as a raw ONNX runtime file-not-found error.

**Tests (write first, using a fake HTTP layer / local fixture files —
never hit the real network in `cargo test`):**
- (new) Fresh cache dir, model not present → download function is
  invoked, file ends up at the expected path.
- (new) Model already present at the expected path → download function is
  NOT invoked again (idempotent no-op), verified via a spy/fake.
- (new) Download fails (simulate via a fake that errors) → returns a
  clear, distinct error type/message, does not panic, does not leave a
  partially-written file at the final path (same partial-download
  safety concern the existing STT model downloader already handles —
  read `managers/model/download.rs`'s existing tests for the pattern to
  mirror).
- (new) Querying "is this model ready?" before any download attempt
  returns a clear "not downloaded" state distinguishable from "download
  failed."

**Acceptance criteria:** no real network calls in the test suite. Mirrors
(does not duplicate) the existing STT download infrastructure's safety
properties (atomic/no-partial-file-on-failure).

---

## W4 — Real ONNX inference pipeline

**Depends on:** W2, W3. **Sequential** — this is the core, hardest task;
do not parallelize it further.

Wire the real melspectrogram → embedding → classifier cascade using `ort`
(already pulled in via `transcribe-rs`) and `rustfft` (already a
dependency) behind the `WakeWordEngine` trait and the buffering contract
W2 built. Convert to int16 PCM only at the ONNX call boundary (the trait's
own interface stays `f32` throughout, per the phase doc's explicit
guidance).

**Files touched:**
- `src-tauri/src/managers/wakeword.rs` only (replace W2's stub classifier
  with the real one; the buffering logic from W2 should not need to
  change).

**Read first:** `src/vox/wakeword/openwakeword_engine.py` in the sibling
Python vox repo — match its framing/hop size and normalization exactly,
per the phase doc's own instruction not to reinvent this from scratch.

**Tests (write first):**
- T1: synthetic wake-word audio (generate via macOS `say` command the
  same way vox's own `test_openwakeword_engine_real.py` does, or a
  bundled fixture WAV) fed frame-by-frame → at least one `process_frame`
  call returns `Some(detection)` with `confidence >= threshold`.
- T2: 10 seconds of all-zero (silence) frames → no detection ever.
- T3: 10 seconds of synthesized unrelated speech (`say "the quick brown
  fox"`) → no detection (or, if the placeholder model genuinely false-
  positives on this exact phrase, swap the fixture phrase and document
  the limitation rather than weakening the assertion — per the phase
  doc's own instruction).
- T9: confidence threshold set very high (e.g. 0.99) with borderline
  audio → no detection (proves the threshold is actually applied).
- T10: confidence threshold set very low (e.g. 0.01) with the same
  borderline audio → detection fires.
- T11 (full version): every detection across all the above tests has
  `0.0 <= confidence <= 1.0`.
- (new) NFR1 measurement: document (in the task's final report, not
  necessarily a hard-failing test) idle RSS with the engine constructed
  vs. not constructed, per the phase doc's own NFR1 ask.

**Acceptance criteria:** `cargo clippy -- -D warnings` clean. Mel-
spectrogram parameters documented in a code comment citing what
openWakeWord/vox's usage expects (phase doc §7 explicit requirement).

---

## W5 — pause/resume + multi-instance isolation

**Depends on:** W1 only (needs the trait shape, not the real model).
**Parallel-safe with:** W2, W3, W4, W6.

**Files touched:** `src-tauri/src/managers/wakeword.rs` only.

**Implement:** `pause()`/`resume()` as plain state flags on the engine
(per the phase doc: "not exercised beyond 'exists and is a plain state
flag' in this phase — the actual pause-on-recording wiring is W7's job").
When paused, `process_frame` should short-circuit to `None` without
running the classifier (cheap no-op, not just ignoring the result).

**Tests (write first):**
- (new) Fresh engine, `pause()` called, then `process_frame` fed genuine
  wake-word-shaped audio (or, if run before W4 lands, the W2 stub
  classifier spy) → no detection, and the classifier/spy was never
  invoked while paused.
- (new) `resume()` after `pause()` → `process_frame` behaves normally
  again.
- T8: two `WakeWordEngine` instances constructed with different
  `model_name`s (or, pre-W4, two instances with different injected stub
  classifiers), each fed audio appropriate to only one of them → each
  only reports its own detections, proving no shared/global state leaks
  between instances.

**Acceptance criteria:** works correctly whether this lands before or
after W4 (test against the W2 stub classifier if W4 hasn't merged yet;
Implementer should coordinate with the Validator on which is current).

---

## W6 — Tauri commands + event emission

**Depends on:** W1 only. **Parallel-safe with:** W2–W5.

Build the IPC surface against `NullWakeWordEngine`/a fake — this does not
need W4's real model to be useful, and unblocks W8 (Settings UI) early.

**Files touched:**
- `src-tauri/src/commands/wakeword.rs` (new)
- `src-tauri/src/commands/mod.rs` (register the new module)
- `src-tauri/src/lib.rs` (register the Tauri command(s) in
  `generate_handler!`, mirror how `commands/memory.rs` was registered)

**Implement:**
- `wakeword_set_enabled(enabled: bool) -> Result<(), String>` — writes
  the `wake_word_enabled` setting (mirror `memory_set_enabled`'s shape
  exactly).
- Event `wakeword://detected` carrying the `WakeWordDetection` payload,
  emitted wherever the (eventually real, for now stubbed) engine reports
  a hit — for this task, it's sufficient to prove the emission plumbing
  works against a manually-triggered fake detection in a test, since the
  live mic loop doesn't exist until W7.

**Tests (write first):**
- (new) `wakeword_set_enabled(true)` then reading settings back shows
  `wake_word_enabled == true` (mirror the existing
  `enabling_memory_updates_the_setting`-style test in `commands/memory.rs`).
- (new) `wakeword_set_enabled(false)` → setting flips back.
- (new) a unit test proving the event-emission helper function, given a
  fake `WakeWordDetection`, calls `app.emit("wakeword://detected", ...)`
  with the correct payload shape (use the same emit-testing approach
  already used elsewhere in this codebase, e.g. how `memory-remembered`
  emission is exercised, or a Tauri mock app context if that's the
  established pattern — check `tray.rs`'s or `actions.rs`'s existing
  tests for the house style before inventing a new one).

**Acceptance criteria:** regenerate `src/bindings.ts` (see
`AGENTS.md`/this repo's CLI-run instructions — run the app briefly in
debug mode per the documented pattern, do not hand-edit bindings.ts) so
the frontend can call `wakeword_set_enabled` once generated.

---

## W7 — Mic-flow integration (scope extension)

**Depends on:** W4, W5, W6. **Sequential, assign to the most senior
available Implementer; the Validator should review this one extra
carefully** since it touches the live recording pipeline.

This task does not exist in the original phase doc (explicitly deferred
there) — it's here because Ram wants a genuinely working, demoable
feature, not inert library code.

**Files touched:**
- `src-tauri/src/actions.rs` (or a new small coordinator module if
  `actions.rs` is getting too large — flag to the Validator if so, per
  this repo's 600-line file-size guidance)
- `src-tauri/src/lib.rs` (register the wake-word engine as managed state,
  mirror how `CommandRouter`/`MemoryManager` are registered)
- `src-tauri/src/settings.rs` only if an additional coordination flag is
  genuinely needed beyond what W1 already added (flag to the Validator
  before adding new settings fields here)

**Implement:**
- When `wake_word_enabled` is true, open a continuous low-rate mic stream
  feeding 1280-sample frames to the engine (reuse the existing
  `AudioRecordingManager`/cpal plumbing — do not invent a second audio
  capture path).
- Mic arbitration: pause the wake-word engine immediately whenever a
  normal dictation recording is active (reuse the existing
  `cancel_current_operation`/recording-state signals already in
  `utils.rs`/`actions.rs`), resume after recording finishes plus a short
  cooldown (reuse the `wake_word_confidence_threshold`-adjacent settings
  pattern — add a `wake_word_cooldown_ms` setting if needed, default a
  couple hundred ms, matching the phase doc's own NFR3 discussion).
- On detection: emit the `wakeword://detected` event (W6) AND
  automatically start a normal dictation recording (reuse
  `TranscribeAction::start`'s existing path) so the user can then speak
  their command hands-free — there is no agent bridge yet (Phase 3 is
  not built) so "start a recording" is the correct MVP action, not a
  reply.

**Tests (write first, as much as is unit-testable without real hardware
— the live-mic-loop wiring itself will need a manual smoke test too,
documented in the task's final report, same spirit as the phase doc's
own `_real`-suffixed tests):**
- (new) a pure decision function `fn should_feed_wakeword_frame(settings:
  &AppSettings, is_recording: bool, is_speaking: bool) -> bool` (mirrors
  the `decide_voice_command`/`decide_memory_fact` pattern already
  established in `actions.rs` from the Phase 1/2 wiring) — unit tested
  directly: disabled setting → false always; enabled + currently
  recording → false; enabled + idle → true.
- (new) a pure decision function for "should a detection auto-start a
  recording right now" (e.g. don't double-trigger if already recording).
- (new) manual smoke test, documented in the report: say the wake phrase
  for real, confirm a recording actually starts.

**Acceptance criteria:** existing dictation hotkey flow (Phase 1/2's
wiring from the prior session) has zero regressions — rerun the FULL test
suite, not just new tests, and manually re-verify the hotkey dictation
path still works (build + install + real test, same method used to
verify Phase 1/2's wiring).

---

## W8 — Settings UI (scope extension)

**Depends on:** W6 (can start once W6's commands/events exist, using a
fake/manually-triggered event for development); should be finalized
(real end-to-end test) only after W7 lands.

**Files touched:**
- `src/components/settings/WakeWordToggle.tsx` (new, mirror
  `MemoryToggle.tsx`'s structure exactly)
- `src/components/settings/index.ts` (export it)
- `src/components/settings/advanced/AdvancedSettings.tsx` (add it to the
  Experimental group, same placement convention as `MemoryToggle`)
- `src/i18n/locales/en/translation.json` (add the new label/description
  keys under `settings.advanced`)
- `src/App.tsx` (add a `listen<WakeWordDetection>("wakeword://detected",
  ...)` toast handler, mirroring the existing `memory-remembered`/
  `voice-command-executed` handlers added in the prior session)

**Implement:** a toggle bound to `wake_word_enabled`, plus a toast on
detection (e.g. " Wake word detected — listening..."). Do not build a
separate settings page for this — follow the established
Experimental-group pattern exactly, for consistency with Memory/Voice
Commands.

**Tests:** `bun run lint` and `bun run build` must be clean — this
component has no backend logic of its own to unit test, it's a thin
binding layer (same as `MemoryToggle`/`VoiceCommandsToggle` had no
dedicated tests beyond lint/build).

**Acceptance criteria:** Settings → Advanced → Experimental shows the new
toggle; toggling it persists (confirm via the same settings_store.json
inspection technique used earlier in this session, or build+install+
manual click-through).
