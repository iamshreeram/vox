# Manual test guide

`experimental-features.html` is a step-by-step guide for testing Vox by hand.
It is one page with a built-in script. Tests are grouped in sections, and
your answers are saved per test id.

## Start the guide

```sh
python3 docs/manual-testing/serve.py          # default port 8765
python3 docs/manual-testing/serve.py 8798     # or pick a port
```

Open the address it prints (for example `http://127.0.0.1:8765/`).
The server listens on 127.0.0.1 only and needs only the Python standard library.

You can also open `experimental-features.html` directly in a browser. In that
case answers stay in that browser only, so use **Save progress file** from time
to time.

## Where feedback is saved

With `serve.py` running, every answer is also written to disk:

- `docs/manual-testing/feedback.json`: the current answers.
- `docs/manual-testing/feedback.prev.json`: the copy before the last save.
- `docs/manual-testing/feedback-history/`: timestamped copies, at most one every
  10 minutes, last 30 kept.

These files are personal and are listed in `.gitignore`. Never commit them.

## Check the guide

```sh
node docs/manual-testing/check-guide.mjs
```

It prints `guide ok: N sections, N tests, N setting rows` and exits 0 when the
guide is consistent. Otherwise it lists each problem and exits 1. It does not
start a browser.

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
  Advanced, Agents, Post Process, History, About, Debug, Tray or Menu bar, and
  the group is the real group title shown in Vox. Test steps that name a
  setting in bold (`<b>Label</b>`) get a "Where to click" box from this table.

Rules:

- Keep test ids stable. People keep saved answers keyed by id: edit the text of
  an existing test, never rename its id.
- Write expected results only from what the docs or the code say. Mark anything
  uncertain with `warn`.
- Do not name internal tools, employers or personal paths in the page or here.
  The checker enforces this.
