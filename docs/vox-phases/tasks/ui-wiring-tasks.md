# Cross-Cutting UI Wiring — Task Cards

Parent plan: `../PHASE-5-6-TASK-QUEUE.md`. These tasks exist only because
Ram's instruction for this round is explicit: the UI must be wired for
both Wake Word and Ambient Mode, and the two must present as one coherent
"how does Vox listen to me" choice, not two independent toggles that
happen to fight each other in the background.

---

## U1 — Shared "Listening Mode" settings section

**Depends on:** W8 (Phase 5's toggle component), A8 (Phase 6's toggle
component).

Right now W8/A8 each describe adding their own toggle to the flat
Experimental settings list, same as `MemoryToggle`/`VoiceCommandsToggle`
were added. That is acceptable as a starting placement, but since these
two features are mutually exclusive alternatives to the same underlying
question, this task's job is to compose them into one clearly-grouped UI
unit rather than leaving them as two unrelated rows a user has to
mentally connect themselves.

**Files touched:**
- `src/components/settings/ListeningModeSection.tsx` (new) — a small
  `SettingsGroup`-wrapped component containing both `WakeWordToggle` and
  `AmbientModeToggle` with a short explanatory blurb above them (reuse
  the existing `SettingsGroup`/`SettingContainer` components, do not
  invent new layout primitives).
- `src/components/settings/advanced/AdvancedSettings.tsx` (replace the
  two separate toggle placements from W8/A8 with this one composed
  section, still inside the Experimental gate).
- `src/components/settings/index.ts` (export the new section component;
  `WakeWordToggle`/`AmbientModeToggle` can remain exported too since
  other code — like a future tray menu — may want them individually).
- `src/i18n/locales/en/translation.json` (a short section-level
  label/description key, e.g. `settings.advanced.listeningMode.*`).

**Implement:** the section should make the mutual exclusion visually
obvious — e.g. radio-button-style presentation, or two toggles where
turning one on visibly disables/greys the other with a tooltip explaining
why, matching whatever the backend (Phase 6's A5) actually enforces. Do
not implement a DIFFERENT exclusivity rule in the UI than what the
backend enforces — read A5's final chosen behavior before building this.

**Tests:** `bun run lint` and `bun run build` clean. No new backend logic
— this is pure composition of already-tested pieces.

**Acceptance criteria:** a user can look at Settings and immediately
understand "I can have Wake Word OR Ambient Mode, not both, and here's
why" without needing to read documentation.

---

## U2 — Permission / onboarding copy update

**Depends on:** W7 or A7 (either is sufficient to start — both introduce
a genuinely continuous/always-on microphone use case that today's
onboarding copy doesn't mention).

**Files touched:**
- Whatever onboarding/permission component currently explains microphone
  access (check `src/components/onboarding/` and
  `src/components/AccessibilityPermissions.tsx` for the current copy
  before editing — this task is a copy/messaging update, not a new
  permission flow; macOS's microphone permission is already a single
  system-level grant, this is purely about setting user expectations).
- `src/i18n/locales/en/translation.json` (any new/updated copy keys).

**Implement:** add a short, honest note wherever microphone permission is
explained, to the effect of: enabling Wake Word or Ambient Mode means the
microphone stays open continuously while that mode is on (Wake Word:
listening for the trigger phrase only, nothing is transcribed or stored
until it fires; Ambient Mode: everything nearby is transcribed into a
short-lived, RAM-only buffer, never written to disk). This mirrors the
honest framing already used earlier in this conversation when explaining
the feature to Ram — the UI copy should not undersell what "continuous
listening" actually means.

**Tests:** `bun run lint` and `bun run build` clean.

**Acceptance criteria:** the distinction between Wake Word's narrow/cheap
listening and Ambient Mode's broad/continuous transcription is stated
plainly somewhere a user will actually see before turning either on (the
Settings toggle's own tooltip/description is an acceptable place if a
dedicated onboarding screen update is judged too heavy for this task's
scope — flag which approach was taken in the report).

---

## U3 — i18n pass + final lint/build verification

**Depends on:** W8, A8, U1 (all new UI copy must exist first).

**Files touched:**
- `src/i18n/locales/en/translation.json` only (verification/cleanup
  pass — this task does not translate into other locales, matching the
  precedent already set with Memory/Voice Commands in the prior session;
  non-English locales fall back to English for untranslated keys, which
  is acceptable for this round).

**Implement:** nothing new — this is a verification task. Confirm:
- every new translation key introduced across W8/A8/U1/U2 exists in
  `en/translation.json` and is actually referenced via `t("...")` in the
  component that needs it (no orphaned keys, no missing keys).
- `bun run lint` passes (the i18next ESLint rule catches hardcoded
  strings — this is the final safety net).
- `bun run build` passes.
- Run `bun run check:translations` (the existing script in
  `package.json`) and read its output — if it flags the new keys as
  missing from other locales, that is expected and fine for this round;
  confirm it does not report anything else newly broken.

**Acceptance criteria:** a clean `bun run lint && bun run build` with
every Phase 5/6 UI task's components present, right before the two phase
integration branches are considered UI-complete.
