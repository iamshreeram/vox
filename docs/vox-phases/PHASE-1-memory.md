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

**CORRECTED against vox's actual shipped + tested implementation**
(`src/vox/memory/{fact_extractor,store,recall_gate,_text}.py` and their
test suites) -- a prior draft of this section was written from the
CONTEXT.md narrative rather than the real source and got several things
wrong. Read this section fully before implementing; it replaces the
original design below it.

New file: `src-tauri/src/managers/memory.rs` (new `MemoryManager`,
registered in `lib.rs` setup the same way `ModelManager`/`HistoryManager`
are registered as Tauri-managed state).

### Schema (new migration)

vox's actual `facts` table only stores the extracted content + a dedup
hash -- no `source_transcript`, no `superseded_by` (there's no fact-update
concept yet, just add/remove). Rust adds `source_transcript` back
deliberately (useful for a future "why did it remember this" UI,
vox has no equivalent) -- keep it, but add the dedup hash vox actually
uses, since silently storing "remember that I prefer tea" twice as two
rows is a real, proven-against bug class in vox's own test suite
(`test_add_fact_dedupes_identical_content`, case-insensitively per
`test_add_fact_dedupes_case_insensitively`).

```sql
CREATE TABLE facts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    text TEXT NOT NULL,              -- the fact itself, e.g. "User prefers tea"
    content_hash TEXT NOT NULL UNIQUE, -- sha256(text.lower()), dedup key
    source_transcript TEXT NOT NULL, -- original utterance it was extracted from
    created_at INTEGER NOT NULL      -- unix millis
);
CREATE INDEX idx_facts_created_at ON facts(created_at);
```

### Fact extraction -- NOT a simple 5-phrase passthrough

The original draft of this phase described a flat list of 5 trigger
phrases ("remember that ", "remember I ", ...) with the captured text
returned verbatim. **That is not what vox actually does.** vox's
`fact_extractor.py` has 8 patterns checked in a specific priority order
(most-specific first), each with its own labeling template, and uses
regex `.search()` (match anywhere in the text) rather than a
start-of-string/whole-transcript prefix check:

```
1. r"\bmy name is (.+)"           -> "User's name is {0}"
2. r"\bi prefer (.+)"             -> "User prefers {0}"
3. r"\bmy favorite (.+)"          -> "User's favorite {0}"
4. r"\bi am allergic to (.+)"     -> "User is allergic to {0}"
5. r"\bi work at (.+)"            -> "User works at {0}"
6. r"\bi live in (.+)"            -> "User lives in {0}"
7. r"\bremember that (.+)"        -> "{0}" (verbatim)
8. r"\bremember (.+)"             -> "{0}" (verbatim)
```

First pattern (in the above order) that matches wins -- stop there, don't
also apply later/broader patterns underneath. This is WHY order matters:
"remember that my name is Ram" must produce `"User's name is Ram"` via
pattern 1, not `"my name is Ram"` via pattern 7 -- pattern 1 is checked
first and `.search()` finds "my name is Ram" as a substring regardless of
the "remember that " prefix in front of it.

All captured groups get trailing `.!?` and whitespace stripped
(`_TRAILING_PUNCTUATION_RE = r"[.!?\s]+$"`).

Rust signature should mirror vox's real return type -- `Vec<String>`,
not `Option<String>` -- even though in practice (first-match-wins, no
pattern depends on another) it only ever produces 0 or 1 results today.
Matching the type now avoids an API break if a future pattern set ever
legitimately produces more than one.

```rust
/// Deterministic trigger-phrase extraction -- NO LLM call. Checks the 8
/// patterns above in order, first match wins. Mirrors vox's
/// fact_extractor.py exactly (see this section's prose above for why
/// order matters and why this differs from an earlier, incorrect draft
/// of this phase doc).
pub fn extract_facts(transcript: &str) -> Vec<String>;
```

### Core API (Rust, on `MemoryManager`)

```rust
pub struct Fact { pub id: i64, pub text: String, pub source_transcript: String, pub created_at: i64 }

impl MemoryManager {
    pub fn extract_facts(transcript: &str) -> Vec<String>; // see above

    /// Store a fact, deduped by content hash (case-insensitive) -- mirrors
    /// vox's `MemoryStore.add_fact`. Returns `Ok(Some(id))` for a newly
    /// stored fact, `Ok(None)` if this exact text (case-insensitive) was
    /// already known -- NOT an error, same as vox's `add_fact` returning
    /// `False` rather than raising.
    pub fn remember(&self, fact_text: &str, source_transcript: &str) -> Result<Option<i64>>;

    /// Word-overlap search over stored facts -- mirrors vox's
    /// `MemoryStore.search_facts` exactly: tokenize query and each fact's
    /// text into content words (lowercased, stopworded, len > 2), rank by
    /// raw overlap count (ties broken by recency -- facts scanned
    /// newest-first, stable sort preserves that order), return the top
    /// `limit`. Deterministic, no LLM call, NO STEMMING -- "prefers" and
    /// "prefer" are different tokens and do NOT match each other, exactly
    /// as in vox (an earlier draft of this phase's own test table assumed
    /// stemming that vox never actually implements; fixed in §7 below).
    pub fn recall(&self, query: &str, limit: usize) -> Result<Vec<Fact>>;

    /// Soft-delete / list for future UI phases.
    pub fn list_all(&self) -> Result<Vec<Fact>>;
    pub fn forget(&self, id: i64) -> Result<()>;
}

fn content_words(text: &str) -> HashSet<String>; // shared by remember's
    // dedup-adjacent normalization and recall's overlap scoring, mirrors
    // vox's _text.py content_words() -- lowercase, regex `[a-z0-9']+`
    // tokenize, drop stopwords (mirror vox's exact list below), drop
    // words with len <= 2.
```

vox's stopword list (`_text.py`) is larger than the original draft's --
use this exact set so recall behavior matches vox's validated cases:

```
the, a, an, is, are, was, were, do, does, did, to, of, in, on, and, or,
i, you, my, me, it, what, that, this, for, with, about, please, can,
could, would, will, your, yours, okay, ok, yeah, yep, nope, thanks,
thank, hmm, um, uh
```

### `should_recall` gate -- a SEPARATE concept, out of scope for this phase

vox also has `recall_gate.should_recall(query, recent_context) -> bool`
-- a cheap pre-filter that decides whether it's even worth calling
`search_facts` at all (skips when the query has no content words, or
when recent conversation context already covers >=60% of the query's
content words). This is a genuinely separate concept from fact search --
don't conflate the two the way an earlier draft of this doc implicitly
did by only describing one `recall()` function. `should_recall` needs
"recent conversation context," which doesn't exist yet without the
dictation-flow wiring this phase's own non-goals explicitly defer --
so it is correctly out of scope here too, but implement `recall()` as
the `search_facts` equivalent (unconditional search), not as a combined
gate+search, so a later phase can cleanly add the gate in front of it
without reshaping this phase's API.

### Settings addition (`settings.rs`)

```rust
pub memory_enabled: bool,  // default: false (opt-in; vox defaults this on,
                           // but Vox-rs defaults new subsystems off per the
                           // phase-doc ground rules until a UI exists to
                           // manage them)
```

### Tauri commands (new `commands/memory.rs`, mirror `commands/history.rs` conventions)

- `memory_remember(text: String) -> Result<Option<i64>, String>` (see
  dedup note above -- `None` means "already known", not an error)
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

## 7. Test cases (write these FIRST, one at a time, red->green->refactor)

**CORRECTED against vox's actual test suites** (`test_fact_extractor.py`,
`test_memory_store.py`) -- several cases in an earlier draft of this
table described behavior vox doesn't actually have (verbatim-only
extraction, word-stemming in recall). Ported directly from vox's own
validated test cases where possible, with a note wherever a case was
changed and why.

### `extract_facts`

| #   | Given                                                                            | Expect                                                                                                                         | vox source                                         |
| --- | -------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------- |
| T1  | `"remember that I have a dentist appointment on Friday"`                         | `["I have a dentist appointment on Friday"]`                                                                                   | `test_remember_that_phrase_extracts_fact_verbatim` |
| T2  | `"remember my wifi password is on the fridge"`                                   | `["my wifi password is on the fridge"]`                                                                                        | `test_bare_remember_phrase_extracts_fact`          |
| T3  | `"my name is Ram"`                                                               | `["User's name is Ram"]`                                                                                                       | `test_my_name_is_produces_labeled_fact`            |
| T4  | `"I prefer dark mode everywhere"`                                                | `["User prefers dark mode everywhere"]`                                                                                        | `test_i_prefer_produces_labeled_fact`              |
| T5  | `"my favorite language is Python"`                                               | `["User's favorite language is Python"]`                                                                                       | `test_my_favorite_produces_labeled_fact`           |
| T6  | `"I am allergic to peanuts"`                                                     | `["User is allergic to peanuts"]`                                                                                              | `test_allergic_to_produces_labeled_fact`           |
| T7  | `"I work at Walmart"`                                                            | `["User works at Walmart"]`                                                                                                    | `test_i_work_at_produces_labeled_fact`             |
| T8  | `"I live in Austin"`                                                             | `["User lives in Austin"]`                                                                                                     | `test_i_live_in_produces_labeled_fact`             |
| T9  | `"my name is Ram."` (trailing punctuation)                                       | `["User's name is Ram"]` (punctuation stripped)                                                                                | `test_trailing_punctuation_is_stripped`            |
| T10 | `"what's the weather like today"` / `"open iterm"` / `"thanks that was helpful"` | `[]` for each (no trigger pattern matches)                                                                                     | `test_ordinary_conversation_extracts_nothing`      |
| T11 | `"remember that my name is Ram"` (two patterns could apply)                      | `["User's name is Ram"]` -- the MORE SPECIFIC "my name is" pattern wins, not the broader "remember that" pattern underneath it | `test_most_specific_pattern_wins_not_both`         |
| T12 | `""` (empty string)                                                              | `[]`                                                                                                                           | `test_empty_text_extracts_nothing`                 |
| T13 | `"remember"` (trigger word, nothing after it)                                    | `[]` (no fact text to extract)                                                                                                 | new -- real edge case, not in vox's suite          |
| T14 | `"   remember that   i like coffee  "` (extra whitespace)                        | `["i like coffee"]` (trimmed)                                                                                                  | new, mirrors vox's `.strip()` on captured group    |

### `remember` / persistence / dedup

| #   | Given                                                                                                | Expect                                                                                                                          |
| --- | ---------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| T15 | `remember("User prefers tea", "...")`                                                                | Returns `Ok(Some(id))`, `id > 0`                                                                                                |
| T16 | After T15, call `list_all()`                                                                         | Contains exactly one fact with `text == "User prefers tea"`                                                                     |
| T17 | Store a fact, close and reopen the `MemoryManager` against the same DB file (simulating app restart) | The fact is still present via `list_all()`                                                                                      |
| T18 | Store 3 facts in sequence                                                                            | `list_all()` returns them ordered most-recent-first                                                                             |
| T19 | `remember("User prefers tea", ...)` then `remember("User prefers tea", ...)` again (identical text)  | Second call returns `Ok(None)`; `list_all()` still has exactly one row -- ports vox's `test_add_fact_dedupes_identical_content` |
| T20 | `remember("User prefers tea", ...)` then `remember("USER PREFERS TEA", ...)` (different case)        | Second call returns `Ok(None)` -- dedup is case-insensitive, ports `test_add_fact_dedupes_case_insensitively`                   |
| T21 | `remember("   ", ...)` (whitespace-only text)                                                        | Returns `Ok(None)`, no row stored -- ports `test_add_fact_ignores_empty_content`                                                |

### `recall`

All overlap examples below use words that are IDENTICAL after
tokenization (no stemming, matching vox exactly -- "prefers" and
"prefer" are different tokens and must NOT be treated as overlapping;
an earlier draft of this table had a case assuming they would, which
does not match vox's real, tested behavior).

| #   | Given facts                                                                                 | Query                                       | Expect                                                                              | vox source                                         |
| --- | ------------------------------------------------------------------------------------------- | ------------------------------------------- | ----------------------------------------------------------------------------------- | -------------------------------------------------- |
| T22 | `["User prefers tea over coffee", "User's favorite color is blue", "User lives in Austin"]` | `"what does the user drink, tea or coffee"` | first result is `"User prefers tea over coffee"` (highest overlap: user/tea/coffee) | `test_search_facts_ranks_by_word_overlap`          |
| T23 | `["User prefers tea"]`                                                                      | `"tell me about the weather today"`         | `[]` (no overlap)                                                                   | `test_search_facts_returns_empty_for_no_overlap`   |
| T24 | `["garage code is 4521"]`                                                                   | `"what's my garage code"`                   | overlap on "garage"/"code" -> returned                                              | new, exact-token overlap (no stemming needed here) |
| T25 | `["User prefers tea", "User is allergic to peanuts"]`                                       | `"tea"`                                     | only the tea fact returned, not the peanut one                                      | new                                                |
| T26 | 10 facts each containing "hobby", `limit = 3`                                               | `"tell me about hobby"`                     | exactly 3 returned                                                                  | `test_search_facts_respects_limit`                 |
| T27 | No facts stored at all                                                                      | any query                                   | `[]`, not an error                                                                  | new                                                |
| T28 | A fact that is only stopwords after tokenization (e.g. a fact literally `"it is"`)          | any query                                   | does not crash; matches nothing (every word filtered as a stopword)                 | new edge case                                      |
| T29 | Query with different casing than the stored fact (`"TEA"` vs stored `"User prefers tea"`)   | --                                          | still matches (case-insensitive)                                                    | new                                                |
| T30 | Query with no content words at all (e.g. `"the a an"`, all stopwords)                       | --                                          | `[]` -- mirrors vox's `search_facts` returning `[]` when `query_words` is empty     | new, ports the early-return in `search_facts`      |

### `forget`

| #   | Given                                            | Expect                                         |
| --- | ------------------------------------------------ | ---------------------------------------------- |
| T31 | Store a fact, `forget(id)`, then `list_all()`    | Fact no longer present                         |
| T32 | `forget(99999)` (id that was never used)         | Returns `Ok(())`, does not error               |
| T33 | `forget` a fact, then `forget` the same id again | Second call also returns `Ok(())` (idempotent) |

### Settings / enablement gate

| #   | Given                                                    | Expect                                                                                                                                              |
| --- | -------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| T34 | `memory_enabled = false` (default)                       | `memory_remember` Tauri command returns an error containing "disabled" (or equivalent clear message), and does NOT write a row to the `facts` table |
| T35 | `memory_enabled = false`                                 | `memory_recall` Tauri command also returns the same clear disabled-error, not an empty-results success                                              |
| T36 | Flip `memory_enabled` to `true` via `memory_set_enabled` | Subsequent `memory_remember`/`memory_recall` calls work normally                                                                                    |
| T37 | Default value of `memory_enabled` on fresh settings      | `false`                                                                                                                                             |

### Concurrency / integration sanity

| #   | Given                                                                                                      | Expect                                                       |
| --- | ---------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ |
| T38 | Two `remember()` calls issued concurrently from two threads (simulate with `std::thread::spawn` in a test) | Both facts are persisted, no row lost, no panic, no deadlock |
| T39 | 10,000 stored facts, `recall()` with a query matching one of them                                          | Returns in under 50ms (NFR2)                                 |

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
