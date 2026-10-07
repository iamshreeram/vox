# Phase 1 — Memory (SQLite facts + deterministic recall)

**Branch:** `phase/1-memory` · **Worktree:** `.worktrees/phase-1-memory/`
**Depends on:** nothing · **Blocks:** nothing

## 1. Goal

Give Vox a durable, local, zero-LLM-call memory of user-stated facts
("remember that I prefer tea"), recalled automatically when a later
transcript seems related — mirroring vox's `src/vox/memory/store.py` +
`recall_gate.py`, reimplemented as a new Rust manager.

## 2. Non-goals (explicitly out of scope for this phase)

- No semantic/vector/embedding search (vox doesn't have this either —
  word-overlap only).
- No knowledge graph, no diary summarizer.
- No UI for browsing/editing facts yet (a Settings → Memory tab is a
  separate, later phase — this phase is backend-only, exposed via Tauri
  commands + unit/integration tests).
- No wiring into the actual dictation hotkey flow yet (that's where Phase 3
  — Agent Bridge — will consume recalled facts to augment a transcript
  before sending it to an agent). This phase stops at: facts can be stored,
  queried, and recalled correctly, with tests proving it.

## 3. Before you write any code

Read `src-tauri/src/managers/history.rs` in full. It already does
SQLite + `rusqlite_migration` for conversation/transcription history —
**reuse its connection-handling and migration conventions**, don't invent
a second pattern. If it uses a single shared `Connection` behind a
`Mutex`/manager struct, do the same for memory. If it has its own
migrations list, add a new migration to that same list rather than
creating a second migrations runner.

## 4. Design

New file: `src-tauri/src/managers/memory.rs` (new `MemoryManager`,
registered in `lib.rs` setup the same way `ModelManager`/`HistoryManager`
are registered as Tauri-managed state).

### Schema (new migration)

```sql
CREATE TABLE facts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    text TEXT NOT NULL,              -- the fact itself, e.g. "prefers tea"
    source_transcript TEXT NOT NULL, -- original utterance it was extracted from
    created_at INTEGER NOT NULL,     -- unix millis
    superseded_by INTEGER REFERENCES facts(id) DEFAULT NULL
);
CREATE INDEX idx_facts_created_at ON facts(created_at);
```

### Core API (Rust, on `MemoryManager`)

```rust
pub struct Fact { pub id: i64, pub text: String, pub source_transcript: String, pub created_at: i64 }

impl MemoryManager {
    /// Deterministic trigger-phrase extraction — NO LLM call. Mirrors vox's
    /// fact_extractor.py: look for phrases like "remember that ", "remember I ",
    /// "don't forget that ", "note that " (case-insensitive) at/near the start
    /// of the transcript; the fact text is whatever follows the trigger phrase,
    /// trimmed. Returns None if no trigger phrase is present.
    pub fn extract_fact(transcript: &str) -> Option<String>;

    /// Store a fact. Returns the new row's id.
    pub fn remember(&self, fact_text: &str, source_transcript: &str) -> Result<i64>;

    /// Word-overlap recall gate — mirrors vox's recall_gate.py. Returns facts
    /// whose text shares at least `min_overlap_words` (default 1, configurable)
    /// significant words (lowercased, stopwords excluded) with `query`, most
    /// recent first, capped at `limit`. Deterministic, no LLM call.
    pub fn recall(&self, query: &str, limit: usize) -> Result<Vec<Fact>>;

    /// Soft-delete / list for future UI phases.
    pub fn list_all(&self) -> Result<Vec<Fact>>;
    pub fn forget(&self, id: i64) -> Result<()>;
}
```

### Settings addition (`settings.rs`)

```rust
pub memory_enabled: bool,  // default: false (opt-in; vox defaults this on,
                           // but Vox-rs defaults new subsystems off per the
                           // phase-doc ground rules until a UI exists to
                           // manage them)
```

### Tauri commands (new `commands/memory.rs`, mirror `commands/history.rs` conventions)

- `memory_remember(text: String) -> Result<i64, String>`
- `memory_recall(query: String, limit: u32) -> Result<Vec<Fact>, String>`
- `memory_list_all() -> Result<Vec<Fact>, String>`
- `memory_forget(id: i64) -> Result<(), String>`
- `memory_set_enabled(enabled: bool) -> Result<(), String>` (writes settings)

All must be registered in the `tauri::generate_handler!` list and exported
through `tauri-specta` like the rest of the codebase (check how
`commands/history.rs` does this and match it exactly).

## 5. Functional requirements

- FR1: When `memory_enabled` is `false`, `extract_fact` is never called
  from any call site added by a later phase, and all memory commands
  return a clear "memory is disabled" error rather than silently no-oping
  — callers must be able to tell the difference between "no fact found"
  and "memory is off."
- FR2: `extract_fact` recognizes at least these trigger phrases
  (case-insensitive, leading/trailing whitespace tolerant):
  `"remember that "`, `"remember I "`, `"remember my "`,
  `"don't forget that "`, `"note that "`.
- FR3: `remember()` persists across process restart (it's SQLite on disk,
  not in-memory).
- FR4: `recall()` never calls an LLM and never makes a network request.
- FR5: `recall()` is case-insensitive and ignores a small built-in English
  stopword list (`the, a, an, is, are, i, my, that, to, of, in, on`) when
  computing overlap, so "What's my favorite drink" can still match a fact
  "prefers tea" via the word "favorite"/"drink" — wait, these are
  unrelated words. Be precise during implementation: the overlap test
  cases in §7 define the exact expected matches/non-matches; the stopword
  list exists only to stop trivial words like "the"/"my" from causing
  false-positive matches, not to invent semantic understanding.
- FR6: Facts are returned most-recent-first by default.
- FR7: `forget(id)` is a hard delete (no soft-delete UI exists yet to need
  otherwise); deleting a nonexistent id is not an error (idempotent).

## 6. Non-functional requirements

- NFR1: All DB access on a background thread / via the same async pattern
  `history.rs` already uses — must never block the UI thread.
- NFR2: `recall()` on a table of 10,000 facts must return in under 50ms on
  a developer laptop (word-overlap over an indexed, in-memory-cached SQLite
  table is trivially fast; this is a sanity ceiling, not a stress target).
- NFR3: No new third-party crate. `rusqlite` + `rusqlite_migration` are
  already dependencies.

## 7. Test cases (write these FIRST, one at a time, red→green→refactor)

### `extract_fact`

| #   | Given                                                                         | Expect                                                                                                                                                                                                |
| --- | ----------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| T1  | `"remember that I prefer tea"`                                                | `Some("I prefer tea")`                                                                                                                                                                                |
| T2  | `"Remember That I Prefer Tea"` (mixed case)                                   | `Some("I Prefer Tea")` (trigger match is case-insensitive; captured fact text preserves original casing)                                                                                              |
| T3  | `"don't forget that the garage code is 4521"`                                 | `Some("the garage code is 4521")`                                                                                                                                                                     |
| T4  | `"note that my dentist appointment is Tuesday"`                               | `Some("my dentist appointment is Tuesday")`                                                                                                                                                           |
| T5  | `"what's the weather today"` (no trigger phrase)                              | `None`                                                                                                                                                                                                |
| T6  | `""` (empty string)                                                           | `None`                                                                                                                                                                                                |
| T7  | `"remember"` (trigger word with nothing after it)                             | `None` (no fact text to extract)                                                                                                                                                                      |
| T8  | `"   remember that   i like coffee  "` (extra whitespace)                     | `Some("i like coffee")` (trimmed)                                                                                                                                                                     |
| T9  | `"please remember that i like coffee"` (trigger phrase not at the very start) | `Some("i like coffee")` — match the trigger phrase anywhere in the transcript, not only at position 0 (vox's filler-word cleanup means transcripts often have a leading "please"/"hey" the STT added) |
| T10 | `"remember that remember that nested"` (trigger phrase appears twice)         | `Some("remember that nested")` — only the _first_ match's prefix is stripped                                                                                                                          |

### `remember` / persistence

| #   | Given                                                                                                | Expect                                                 |
| --- | ---------------------------------------------------------------------------------------------------- | ------------------------------------------------------ |
| T11 | `remember("prefers tea", "remember that I prefer tea")`                                              | Returns an `id > 0`                                    |
| T12 | After T11, call `list_all()`                                                                         | Contains exactly one fact with `text == "prefers tea"` |
| T13 | Store a fact, close and reopen the `MemoryManager` against the same DB file (simulating app restart) | The fact is still present via `list_all()`             |
| T14 | Store 3 facts in sequence                                                                            | `list_all()` returns them ordered most-recent-first    |

### `recall`

| #   | Given facts                                                                                                       | Query                            | Expect                                                                                                                                                                            |
| --- | ----------------------------------------------------------------------------------------------------------------- | -------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| T15 | `["prefers tea over coffee"]`                                                                                     | `"what do I prefer to drink"`    | overlap on "prefer" → returned                                                                                                                                                    |
| T16 | `["prefers tea over coffee"]`                                                                                     | `"what's the capital of France"` | no meaningful overlap → empty result                                                                                                                                              |
| T17 | `["garage code is 4521"]`                                                                                         | `"what's my garage code"`        | overlap on "garage"/"code" → returned                                                                                                                                             |
| T18 | `["prefers tea", "allergic to peanuts"]`                                                                          | `"tea"`                          | only the tea fact returned, not the peanut one                                                                                                                                    |
| T19 | 5 facts all matching a broad query, `limit = 2`                                                                   | query matching all 5             | exactly 2 returned, the 2 most recent of the matches                                                                                                                              |
| T20 | No facts stored at all                                                                                            | any query                        | empty result, not an error                                                                                                                                                        |
| T21 | A fact containing only stopwords after extraction (edge case, e.g. `"remember that it is"` → fact text `"it is"`) | any query                        | does not crash; matches nothing meaningful (every word is a stopword) — assert no panic and an empty/near-empty result, not a specific crash-free behavior beyond "doesn't error" |
| T22 | Query with different casing than the stored fact (`"TEA"` vs stored `"tea"`)                                      | —                                | still matches (case-insensitive)                                                                                                                                                  |

### `forget`

| #   | Given                                            | Expect                                         |
| --- | ------------------------------------------------ | ---------------------------------------------- |
| T23 | Store a fact, `forget(id)`, then `list_all()`    | Fact no longer present                         |
| T24 | `forget(99999)` (id that was never used)         | Returns `Ok(())`, does not error               |
| T25 | `forget` a fact, then `forget` the same id again | Second call also returns `Ok(())` (idempotent) |

### Settings / enablement gate

| #   | Given                                                    | Expect                                                                                                                                              |
| --- | -------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| T26 | `memory_enabled = false` (default)                       | `memory_remember` Tauri command returns an error containing "disabled" (or equivalent clear message), and does NOT write a row to the `facts` table |
| T27 | `memory_enabled = false`                                 | `memory_recall` Tauri command also returns the same clear disabled-error, not an empty-results success                                              |
| T28 | Flip `memory_enabled` to `true` via `memory_set_enabled` | Subsequent `memory_remember`/`memory_recall` calls work normally                                                                                    |
| T29 | Default value of `memory_enabled` on fresh settings      | `false`                                                                                                                                             |

### Concurrency / integration sanity

| #   | Given                                                                                                      | Expect                                                       |
| --- | ---------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ |
| T30 | Two `remember()` calls issued concurrently from two threads (simulate with `std::thread::spawn` in a test) | Both facts are persisted, no row lost, no panic, no deadlock |

## 8. Acceptance criteria

- [ ] Every test in §7 exists, was watched to fail, then watched to pass.
- [ ] `cargo clippy -- -D warnings` clean.
- [ ] No new dependency added to `Cargo.toml`.
- [ ] `memory_enabled` defaults to `false` and is respected by every public
      command (T26-T29 passing is the proof).
- [ ] This phase's commands are callable from the frontend via the
      generated TypeScript bindings (`bindings.ts` regenerates cleanly —
      run whatever command the repo uses to regenerate `tauri-specta`
      bindings and confirm no manual edits were needed).
- [ ] Nothing outside `managers/memory.rs`, `commands/memory.rs`, the new
      migration, and the `settings.rs`/`lib.rs` registration lines was
      modified.
