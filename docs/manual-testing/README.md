# Manual test guide

`experimental-features.html` is a step-by-step guide for testing Vox by hand.
It is one page with a built-in script. Tests are grouped in sections, and
your answers are saved per test id.

## Start the guide

Open `docs/manual-testing/experimental-features.html` directly in a browser
(double-click it, or run `open docs/manual-testing/experimental-features.html`
on macOS). It needs no server and no extra tools.

## Where your answers are saved

Answers are saved in your browser, for that exact file location. If you move or
rename the page, or clear the browser data, the page starts empty again.

- **Save progress file** downloads all results and notes as a file. Press it
  now and then, and always before you close the browser for the day.
- **Load progress file...** puts a saved file back.
- The page keeps automatic browser backups; **Restore this backup** brings one back.
- **Download report (.md)** and **Download data (.json)** produce the reports.

Progress files and reports are personal. Do not commit them.

## Priorities

Every test has a priority, shown as a badge on its card:

- **P0 - UAT: must pass (start here).** The talk-and-listen loop (hearing you,
  talking back, a natural conversation, interrupting, stopping) and anything that
  could act or leak without asking (confirmations, secret files, agent write
  approvals, logs). Each P0 test must stand on its own: it names its setup and
  cleanup, and it never points at another test or section.
- **P1 - Main features.**
- **P2 - Edge cases and options.** Everything not listed in P0, P1 or the not-built list.
- **Not built yet (do not test).** Planned work. A not-built test is never a failure.

Rules the checker enforces:

- Every test is in exactly one bucket. P0 has 15 to 40 tests.
- A P0 test is never developer-only (`dev`). `dev` tests say "debug build" in `warn`.
- The not-built bucket holds exactly the planned tests.

The guide has two views, chosen with the **Priority view / Section view** menu
(saved in this browser). The priority view opens P0 first and lists each test
once, in run order, with a "From" line naming its section. The section view is
the original layout. A **P0 only** / **P0 + P1** filter sits next to the status
filter.

The **UAT (P0)** line at the top counts P0 results. Only **Pass** is a pass;
Skip, Blocked and not run are not. It shows `ALL n P0 TESTS PASSED` when every
P0 test passes, otherwise `NOT READY: k of n P0 tests not passed`. The Markdown
report repeats this, and the JSON file has `summaryByPriority` and `uatVerdict`.

### How to change a priority

The priority lists are the `PRI` literal in the page script (`P0`, `P1`, `X`).
`P2` stays empty in the source: `applyPriority()` fills it with every test that is
in no list, in section order. To move a test, edit its entry in `PRI` (keep the
list order, because P0 runs in list order). Add a new test id to `P0`, `P1` or
`X`, or leave it out to make it P2. Then run the checker. Never put an id in two
lists.

## Priorities

Every test has a priority, shown as a badge on its card:

- **P0 - UAT: must pass (start here).** The talk-and-listen loop (hearing you,
  talking back, a natural conversation, interrupting, stopping) and anything that
  could act or leak without asking (confirmations, secret files, agent write
  approvals, logs). Each P0 test must stand on its own: it names its setup and
  cleanup, and it never points at another test or section.
- **P1 - Main features.**
- **P2 - Edge cases and options.** Everything not listed in P0, P1 or the not-built list.
- **Not built yet (do not test).** Planned work. A not-built test is never a failure.

Rules the checker enforces:

- Every test is in exactly one bucket. P0 has 15 to 40 tests.
- A P0 test is never developer-only (`dev`). `dev` tests say "debug build" in `warn`.
- The not-built bucket holds exactly the planned tests.

The guide has two views, chosen with the **Priority view / Section view** menu
(saved in this browser). The priority view opens P0 first and lists each test
once, in run order, with a "From" line naming its section. The section view is
the original layout. A **P0 only** / **P0 + P1** filter sits next to the status
filter.

The **UAT (P0)** line at the top counts P0 results. Only **Pass** is a pass;
Skip, Blocked and not run are not. It shows `ALL n P0 TESTS PASSED` when every
P0 test passes, otherwise `NOT READY: k of n P0 tests not passed`. The Markdown
report repeats this, and the JSON file has `summaryByPriority` and `uatVerdict`.

### How to change a priority

The priority lists are the `PRI` literal in the page script (`P0`, `P1`, `X`).
`P2` stays empty in the source: `applyPriority()` fills it with every test that is
in no list, in section order. To move a test, edit its entry in `PRI` (keep the
list order, because P0 runs in list order). Add a new test id to `P0`, `P1` or
`X`, or leave it out to make it P2. Then run the checker. Never put an id in two
lists.

## Check the guide

```sh
bun run test:guide      # node docs/manual-testing/check-guide.mjs
bun run test:guide:ui   # Playwright, loads the page from a file:// URL
```

`test:guide` prints `guide ok: N sections, N tests (P0 a, P1 b, P2 c, not built d),
N setting rows` and exits 0 when the guide is consistent. Otherwise it lists each
problem and exits 1. It does not start a browser and needs no server.

`test:guide:ui` runs the browser tests for the priority view, filters, guide order
and reports. It needs no server (the page opens from its file path), needs
Node 18 or newer, and needs Playwright installed. It is run by hand only: it is
not part of CI.

## Add or change a test

The data lives in the page script:

- `SECTIONS`: sections and their tests. Each test has `id`, `title`, `steps`,
  `expect`, and optionally `needs`, `warn`, `mic` (you speak to Vox) and
  `opt` (optional or slow).
- `PATCH`: corrections to an existing test id. A PATCH entry overrides the
  fields of the SECTIONS entry with the same id. If a test already has a PATCH
  entry, change the PATCH entry; otherwise change the SECTIONS entry.
- `ADD`: extra tests inserted after a given id.
- `NAV`: the "Where is each setting?" table. Each row is
  `[label, tab, group, note, keywords]`. The tab must be one of General, Models,
  Advanced, Agents, Post Process, History, About, Debug, Tray, Menu bar or Brain, and
  the group is the real group title shown in Vox. Test steps that name a
  setting in bold (`<b>Label</b>`) get a "Where to click" box from this table.

When a bold label appears in more than one NAV row (for example a label on both
the General and Brain tabs), the test must say which row it means with
`where: [tab, group, optional note]`. The checker enforces this.

Tabs: General, Models, Advanced, Agents, Post Process, History, About, Debug,
Tray, Menu bar and Brain (the Brain page in the sidebar).

Rules:

- Keep test ids stable. People keep saved answers keyed by id: edit the text of
  an existing test, never rename its id.
- Write expected results only from what the docs or the code say. Mark anything
  uncertain with `warn`.
- Do not name internal tools, employers or personal paths in the page or here.
  The checker enforces this.
