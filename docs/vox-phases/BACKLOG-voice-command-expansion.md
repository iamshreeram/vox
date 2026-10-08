# Backlog — Voice Command Expansion ("text into action")

**Status:** ideas backlog, not a claimed/scoped phase. Nothing here follows
the `STATUS.md` claim protocol until someone promotes an item into an
actual numbered phase doc with its own design/FR/test tables, per
`WORKTREE-CONVENTION.md` and the existing `PHASE-N-*.md` format.

**Why this file exists:** after fixing the wake-word microphone-capture bug
(see `git log` around the `fix(wakeword)` commit), the natural next
question was "now that it hears me, how do spoken words turn into actions
beyond dictation?" Investigating the existing code turned up far more
already built than expected — this file separates "already works today",
"built but not wired up", and "not started anywhere yet" so nobody
re-discovers or re-implements what's already here.

## Already working today (no code changes needed)

- `CommandRouter` (`src-tauri/src/managers/command_router.rs`, Phase 2,
  done) matches `open/launch/start/switch to/go to <app or known folder>`
  against real installed apps and standard folders, and is already wired
  into the post-transcription hook in `actions.rs` — including for
  wake-word-triggered transcriptions, since `WakeWordListener::
  handle_detection` calls the same `send_transcription_input()` path as
  hotkey dictation.
- Gated by the `voice_commands_enabled` setting (Settings → Advanced →
  Experimental group → "Voice Commands"), default off.
- Leaving the dictated/transcribed text on the system clipboard instead of
  only auto-pasting it is also already built: `ClipboardHandling::
  CopyToClipboard` (Settings → Advanced → Output group → "Clipboard
  Handling"), default `DontModify`. No new code needed for either of
  these — they just need to be enabled.
- **`CommandAction::OpenUrl` is now wired up** (was "built but not wired"
  below as of the previous revision of this doc): `CommandRouter::route`
  recognizes an explicit `"open website <name>"` phrase (defaults to
  `.com` when the remainder has no dot) and a bare spoken/literal domain
  with the `"go to"`/`"open"` verbs (`"go to google dot com"`,
  `"open google.com"`). Only bare domains are accepted -- no scheme, path,
  port, or userinfo -- since the match is handed straight to the OS URL
  opener; see `match_url`/`is_valid_domain` in `command_router.rs` and its
  16 `u1`-`u16` unit tests for the exact accepted/rejected phrasing
  (including rejecting scheme-smuggling attempts like
  `"javascript:alert(1)"`).

## Built but not wired up yet (exists in code, inert)

| Item | Where | What's missing |
| --- | --- | --- |
| `SafetyPolicy` | `managers/safety_policy.rs` | Fully built and tested (`delete`/`send`/`push`/`deploy`/`rm `/`format`/`uninstall` keyword gate, whole-word matching, configurable list), but `#![allow(dead_code)]` — nothing destructive exists yet for it to gate. Needs an actual confirm/cancel UI flow (toast with Confirm/Cancel, or a voice "yes"/"cancel" follow-up with a timeout-to-deny) once any consequential action ships. |
| App alias matching via `CFBundleDisplayName`/`CFBundleName` | `command_router.rs` module docs, documented gap | Only indexes by `.app` folder stem today, so e.g. "VS Code" won't match `Visual Studio Code.app` (whose bundle display name is "Code"). Needs an Info.plist-parsing crate. |
| Curated STT-mishearing phrase allowlist (vox's "tier 1" YAML table — e.g. "eater" → iTerm) | documented gap in `command_router.rs` | Not ported yet; would reduce false `NoMatch`es for commonly-misheard app names. |

## Not started anywhere — needs real scoping

- **Phase 3: Agent Bridge** (`PHASE-3-agent-bridge.md`, status `not_started`
  in `STATUS.md`) is the existing, already-designed home for anything the
  deterministic `CommandRouter` can't match — e.g. "remind me to call mom
  tomorrow", "what's the weather", "search for X" — by handing the
  transcript to a user-configured external CLI agent. This is very likely
  where most of the "general assistant" asks below actually belong,
  rather than inventing a second catch-all system. Worth picking this up
  before building bespoke handling for any individual open-ended command.
- **A pluggable "skill" architecture** for new deterministic action
  categories beyond app/folder launching, so the router doesn't grow into
  one giant file — each skill owning its own trigger patterns + execution,
  tested with fakes the same way `CommandRouter` is. Candidate categories:
  - Media control (play/pause/next/volume)
  - System toggles (wifi, brightness, do-not-disturb) — needs a
    platform-specific execution backend (AppleScript/System Events on
    macOS), not just "open a path" like today's actions
  - Window management (move/resize/snap)
  - Reminders/notes (EventKit/Reminders.app integration on macOS)
  - User-defined custom macros/aliases
- **Per-category settings**, not just one global `voice_commands_enabled`
  switch — e.g. enable app-launching without enabling anything that sends
  messages or deletes things.
- **Discoverability**: a "what can I say" help surface in the UI, since a
  purely deterministic phrase-matching router is invisible otherwise —
  nobody will remember the exact accepted phrasing without a reference.

## Suggested order of attack

1. Validate today's "open X" + clipboard behavior actually satisfies real
   usage (in progress).
2. Validate the new `OpenUrl` phrasing ("go to X dot com" / "open website
   X") live, with `voice_commands_enabled` on (already the case on this
   machine).
3. Scope and claim Phase 3 (Agent Bridge) for the open-ended/general-query
   case, rather than building a parallel ad hoc system.
4. Only then consider the pluggable skill architecture for
   media/system/window control, once there's a second and third
   deterministic action category to justify the abstraction (YAGNI: one
   category — app/folder open — doesn't yet justify a plugin framework).
