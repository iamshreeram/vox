import assert from "node:assert/strict";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { VaultHitDto, VaultIndexStatusDto } from "@/bindings";
import { EXPERIMENTAL_ENTRIES } from "./search/index/experimental";
import {
  DISCLOSURE_KEYS,
  MAX_PREVIEW_HITS,
  VAULT_RAG_I18N_KEYS,
  VAULT_RAG_POLL_MS,
  canEnableVaultRag,
  canReindex,
  createStatusPoller,
  foldersOutcome,
  normalizeFolders,
  previewLines,
  probeMessage,
  shouldPollStatus,
  statusView,
} from "./vaultRagForm";

// Collect-and-continue harness: every scenario runs, failures are listed per group,
// exit code is 1 when anything is red. Logs go to stdout (tee to a file in CI/manual runs).

interface Result {
  group: string;
  name: string;
  ok: boolean;
  error?: string;
}
const results: Result[] = [];

async function scenario(
  group: string,
  name: string,
  fn: () => void | Promise<void>,
): Promise<void> {
  try {
    await fn();
    results.push({ group, name, ok: true });
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    results.push({
      group,
      name,
      ok: false,
      error: msg.split("\n").slice(0, 4).join(" | "),
    });
  }
}

function dto(over: Partial<VaultIndexStatusDto> = {}): VaultIndexStatusDto {
  return {
    notes: 3,
    chunks: 9,
    skipped: 1,
    last_refresh_ms: 5,
    refreshing: false,
    last_error_kind: null,
    tool_support: "untested",
    ...over,
  };
}

function hit(over: Partial<VaultHitDto> = {}): VaultHitDto {
  return {
    rel: "a.md",
    heading: "",
    snippet: "s",
    note_date: null,
    ...over,
  };
}

const HERE = path.dirname(fileURLToPath(import.meta.url));
const SRC = path.resolve(HERE, "..", "..");
const REPO = path.resolve(SRC, "..");
const FORM_FILE = path.join(HERE, "vaultRagForm.ts");
const COMPONENT_FILE = path.join(HERE, "VaultRagSettings.tsx");
const LOCALES_DIR = path.join(SRC, "i18n", "locales");
const SETTINGS_STORE = path.join(SRC, "stores", "settingsStore.ts");

const G_CANENABLE = "1 canEnableVaultRag";
const G_POLL = "2 shouldPollStatus";
const G_FOLDERS = "3 foldersOutcome";
const G_PREVIEW = "4 previewLines";
const G_STATUS = "5 statusView";
const G_REINDEX = "6 canReindex";
const G_PROBE = "7 probeMessage";
const G_POLLER = "8 createStatusPoller";
const G_KEYS = "9 i18n key constants";
const G_LOCALE = "10 locale parity";
const G_SCAN = "11 source scans";

// ---- 5 statusView --------------------------------------------------------
await scenario(
  G_STATUS,
  "toggle off maps to off whatever the status says",
  () => {
    assert.equal(statusView(false, dto()).key, "off");
    assert.equal(statusView(false, null).key, "off");
    assert.equal(statusView(false, dto({ refreshing: true })).key, "off");
  },
);

await scenario(G_STATUS, "enabled with no status DTO is unavailable", () => {
  assert.equal(statusView(true, null).key, "unavailable");
});

await scenario(G_STATUS, "ready passes counts and tool support through", () => {
  assert.deepEqual(statusView(true, dto({ tool_support: "supported" })), {
    key: "ready",
    notes: 3,
    chunks: 9,
    skipped: 1,
    toolSupport: "supported",
  });
});

await scenario(G_STATUS, "refreshing without an error kind is indexing", () => {
  assert.deepEqual(statusView(true, dto({ refreshing: true, notes: 4 })), {
    key: "indexing",
    notes: 4,
    chunks: 9,
    skipped: 1,
    toolSupport: "untested",
  });
});

await scenario(
  G_STATUS,
  "'applying' while refreshing (real backend value) is indexing",
  () => {
    assert.equal(
      statusView(true, dto({ refreshing: true, last_error_kind: "applying" }))
        .key,
      "indexing",
    );
  },
);

await scenario(
  G_STATUS,
  "busy wins over indexing (busy also sets refreshing)",
  () => {
    assert.deepEqual(
      statusView(true, dto({ refreshing: true, last_error_kind: "busy" })),
      { key: "busy", notes: 3, chunks: 9, skipped: 1, toolSupport: "untested" },
    );
  },
);

await scenario(G_STATUS, "busy is busy even if refreshing is false", () => {
  assert.equal(statusView(true, dto({ last_error_kind: "busy" })).key, "busy");
});

await scenario(
  G_STATUS,
  "other error kind while not refreshing is unavailable",
  () => {
    assert.deepEqual(
      statusView(true, dto({ last_error_kind: "io_failed", notes: 0 })),
      {
        key: "unavailable",
        notes: 0,
        chunks: 9,
        skipped: 1,
        toolSupport: "untested",
      },
    );
  },
);

await scenario(
  G_STATUS,
  "every tool_support string maps through; unknown strings become unknown",
  () => {
    for (const s of [
      "untested",
      "supported",
      "unsupported",
      "unknown",
    ] as const) {
      assert.equal(statusView(true, dto({ tool_support: s })).toolSupport, s);
    }
    assert.equal(
      statusView(true, dto({ tool_support: "bogus-future-value" })).toolSupport,
      "unknown",
    );
  },
);

// ---- 6 canReindex --------------------------------------------------------
await scenario(
  G_REINDEX,
  "allowed when enabled, status known and not refreshing",
  () => {
    assert.equal(canReindex(true, dto()), true);
    assert.equal(canReindex(true, dto({ tool_support: "supported" })), true);
  },
);

await scenario(G_REINDEX, "disabled while refreshing", () => {
  assert.equal(canReindex(true, dto({ refreshing: true })), false);
});

await scenario(
  G_REINDEX,
  "disabled while busy (busy counts as refreshing)",
  () => {
    assert.equal(
      canReindex(true, dto({ refreshing: true, last_error_kind: "busy" })),
      false,
    );
  },
);

await scenario(G_REINDEX, "disabled with no status DTO", () => {
  assert.equal(canReindex(true, null), false);
});

await scenario(G_REINDEX, "disabled when the toggle is off", () => {
  assert.equal(canReindex(false, dto()), false);
  assert.equal(canReindex(false, null), false);
});

// ---- 7 probeMessage ------------------------------------------------------
await scenario(
  G_PROBE,
  "ok with a known word maps to that kind with no text",
  () => {
    assert.deepEqual(probeMessage({ status: "ok", data: "supported" }), {
      kind: "supported",
      text: null,
    });
    assert.deepEqual(probeMessage({ status: "ok", data: "unsupported" }), {
      kind: "unsupported",
      text: null,
    });
    assert.deepEqual(probeMessage({ status: "ok", data: "unknown" }), {
      kind: "unknown",
      text: null,
    });
  },
);

await scenario(G_PROBE, "ok with an unexpected word is unknown", () => {
  assert.equal(probeMessage({ status: "ok", data: "banana" }).kind, "unknown");
  assert.equal(probeMessage({ status: "ok", data: "" }).kind, "unknown");
});

await scenario(
  G_PROBE,
  "error is kind error and the text is the error verbatim",
  () => {
    assert.deepEqual(
      probeMessage({ status: "error", error: "connection refused" }),
      {
        kind: "error",
        text: "connection refused",
      },
    );
  },
);

await scenario(
  G_PROBE,
  "error text with newlines and markup is kept verbatim",
  () => {
    const text = "HTTP 401\nunauthorized <b>x</b> & 'q'";
    assert.deepEqual(probeMessage({ status: "error", error: text }), {
      kind: "error",
      text,
    });
  },
);

// ---- 1 canEnableVaultRag -------------------------------------------------
await scenario(
  G_CANENABLE,
  "empty, blank and newline-only paths are refused",
  () => {
    assert.equal(canEnableVaultRag(""), false);
    assert.equal(canEnableVaultRag("   "), false);
    assert.equal(canEnableVaultRag("\n"), false);
  },
);

await scenario(G_CANENABLE, "a real path is accepted", () => {
  assert.equal(canEnableVaultRag("/v"), true);
});

// ---- 2 shouldPollStatus truth table (8 rows) -----------------------------
const POLL_TABLE: [boolean, boolean, boolean, boolean][] = [
  // [pageVisible, enabled, requestInFlight, expected]
  [false, false, false, false],
  [false, false, true, false],
  [false, true, false, false],
  [false, true, true, false],
  [true, false, false, false],
  [true, false, true, false],
  [true, true, false, true],
  [true, true, true, false],
];

await scenario(
  G_POLL,
  "truth table: polls only when visible AND enabled AND idle",
  () => {
    for (const [
      pageVisible,
      enabled,
      requestInFlight,
      expected,
    ] of POLL_TABLE) {
      assert.equal(
        shouldPollStatus({ pageVisible, enabled, requestInFlight }),
        expected,
        `visible=${pageVisible} enabled=${enabled} inFlight=${requestInFlight}`,
      );
    }
  },
);

// ---- 3 foldersOutcome ----------------------------------------------------
await scenario(
  G_FOLDERS,
  "ok persists the attempted value and clears the error",
  () => {
    assert.deepEqual(foldersOutcome("old", "new", { status: "ok" }), {
      value: "new",
      error: null,
    });
  },
);

await scenario(
  G_FOLDERS,
  "error keeps the current value and returns the backend text verbatim",
  () => {
    const text = "invalid folder: Projects/../People";
    assert.deepEqual(
      foldersOutcome("old", "bad", { status: "error", error: text }),
      {
        value: "old",
        error: text,
      },
    );
  },
);

await scenario(
  G_FOLDERS,
  "error text with HTML-ish content and newlines is untouched",
  () => {
    const text = '<img src=x onerror="alert(1)">\n  "quoted" & <b>bold</b>';
    const out = foldersOutcome("old", "<script>x</script>", {
      status: "error",
      error: text,
    });
    assert.equal(out.value, "old");
    assert.equal(out.error, text);
  },
);

// ---- 4 previewLines ------------------------------------------------------
await scenario(G_PREVIEW, "zero hits gives an empty list", () => {
  assert.deepEqual(previewLines([]), []);
});

await scenario(
  G_PREVIEW,
  "one hit with heading renders 'rel > heading: snippet'",
  () => {
    assert.deepEqual(
      previewLines([
        hit({
          rel: "Projects/Atlas.md",
          heading: "Status",
          snippet: "Ship it",
        }),
      ]),
      ["Projects/Atlas.md > Status: Ship it"],
    );
  },
);

await scenario(G_PREVIEW, "empty heading is omitted", () => {
  assert.deepEqual(
    previewLines([hit({ rel: "a.md", heading: "", snippet: "x" })]),
    ["a.md: x"],
  );
});

await scenario(
  G_PREVIEW,
  "caps at 8 hits, in order, for 8, 9 and 100 inputs",
  () => {
    assert.equal(MAX_PREVIEW_HITS, 8);
    const make = (n: number) =>
      Array.from({ length: n }, (_, i) =>
        hit({ rel: `n${i}.md`, snippet: `s${i}` }),
      );
    assert.equal(previewLines(make(8)).length, 8);
    const nine = previewLines(make(9));
    assert.equal(nine.length, 8);
    assert.equal(nine[0], "n0.md: s0");
    assert.equal(nine[7], "n7.md: s7");
    const hundred = previewLines(make(100));
    assert.equal(hundred.length, 8);
    assert.deepEqual(
      hundred,
      make(8).map((h) => `${h.rel}: ${h.snippet}`),
    );
  },
);

await scenario(
  G_PREVIEW,
  "script tags are returned byte-identical as plain text",
  () => {
    const out = previewLines([
      hit({
        rel: "a.md",
        heading: "<b>&amp;",
        snippet: "<script>alert(1)</script>",
      }),
    ]);
    assert.deepEqual(out, ["a.md > <b>&amp;: <script>alert(1)</script>"]);
  },
);

await scenario(G_PREVIEW, "null note_date does not crash", () => {
  assert.equal(previewLines([hit({ note_date: null })]).length, 1);
  assert.equal(previewLines([hit({ note_date: "2026-01-02" })]).length, 1);
});

// ---- 8 createStatusPoller (fake scheduler, manual queue, deferred runs) ---
interface Timer {
  id: number;
  fn: () => void;
  ms: number;
}

function fakeScheduler() {
  const queue: Timer[] = [];
  const cancelled: unknown[] = [];
  let nextId = 1;
  return {
    queue,
    cancelled,
    schedule(fn: () => void, ms: number): unknown {
      const handle = { id: nextId };
      nextId += 1;
      queue.push({ id: handle.id, fn, ms });
      return handle;
    },
    cancel(handle: unknown): void {
      cancelled.push(handle);
      const id = (handle as { id: number }).id;
      const index = queue.findIndex((t) => t.id === id);
      if (index >= 0) queue.splice(index, 1);
    },
  };
}

interface Deferred {
  resolve: () => void;
  reject: (e: unknown) => void;
}

function fakeRun() {
  const pending: Deferred[] = [];
  let calls = 0;
  const run = (): Promise<void> => {
    calls += 1;
    return new Promise<void>((resolve, reject) => {
      pending.push({ resolve, reject });
    });
  };
  return { run, pending, calls: () => calls };
}

function take<T>(items: T[]): T {
  const item = items.shift();
  assert.ok(item !== undefined, "expected an item but the list is empty");
  return item;
}

const flush = (): Promise<void> =>
  new Promise<void>((resolve) => setTimeout(resolve, 0));

function makePoller() {
  const sched = fakeScheduler();
  const r = fakeRun();
  const poller = createStatusPoller({
    run: r.run,
    schedule: sched.schedule,
    cancel: sched.cancel,
    intervalMs: 2000,
  });
  return { sched, r, poller };
}

await scenario(
  G_POLLER,
  "start runs once immediately and schedules nothing until run settles",
  async () => {
    const { sched, r, poller } = makePoller();
    assert.equal(poller.running(), false);
    poller.start();
    assert.equal(poller.running(), true);
    assert.equal(r.calls(), 1);
    assert.equal(r.pending.length, 1);
    assert.equal(sched.queue.length, 0);
    poller.stop();
  },
);

await scenario(G_POLLER, "second start while running is a no-op", async () => {
  const { r, poller } = makePoller();
  poller.start();
  poller.start();
  assert.equal(r.calls(), 1);
  assert.equal(r.pending.length, 1);
  poller.stop();
});

await scenario(
  G_POLLER,
  "next tick is scheduled only after run resolves; never overlaps",
  async () => {
    const { sched, r, poller } = makePoller();
    poller.start();
    take(r.pending).resolve();
    await flush();
    assert.equal(sched.queue.length, 1, "exactly one timer after settle");
    assert.equal(sched.queue[0]?.ms, 2000);
    assert.equal(r.pending.length, 0);
    take(sched.queue).fn();
    await flush();
    assert.equal(r.calls(), 2);
    assert.equal(r.pending.length, 1, "one run in flight");
    assert.equal(sched.queue.length, 0, "no timer while a run is in flight");
    take(r.pending).resolve();
    await flush();
    assert.equal(sched.queue.length, 1);
    poller.stop();
  },
);

await scenario(
  G_POLLER,
  "a rejected run still reschedules the next tick",
  async () => {
    const { sched, r, poller } = makePoller();
    poller.start();
    take(r.pending).reject(new Error("boom"));
    await flush();
    assert.equal(sched.queue.length, 1);
    assert.equal(poller.running(), true);
    take(sched.queue).fn();
    await flush();
    assert.equal(r.calls(), 2);
    assert.equal(r.pending.length, 1);
    poller.stop();
  },
);

await scenario(
  G_POLLER,
  "stop while a run is in flight: no schedule when it settles",
  async () => {
    const { sched, r, poller } = makePoller();
    poller.start();
    poller.stop();
    assert.equal(poller.running(), false);
    take(r.pending).resolve();
    await flush();
    assert.equal(sched.queue.length, 0);
    assert.equal(poller.running(), false);
    assert.equal(r.calls(), 1);
  },
);

await scenario(G_POLLER, "stop cancels the pending timer", async () => {
  const { sched, r, poller } = makePoller();
  poller.start();
  take(r.pending).resolve();
  await flush();
  assert.equal(sched.queue.length, 1);
  poller.stop();
  assert.equal(sched.queue.length, 0, "timer removed");
  assert.equal(sched.cancelled.length, 1, "cancel was called for the timer");
  assert.equal(poller.running(), false);
});

await scenario(
  G_POLLER,
  "restart after stop runs immediately and loops again",
  async () => {
    const { sched, r, poller } = makePoller();
    poller.start();
    take(r.pending).resolve();
    await flush();
    poller.stop();
    poller.start();
    assert.equal(poller.running(), true);
    assert.equal(r.calls(), 2);
    take(r.pending).resolve();
    await flush();
    assert.equal(sched.queue.length, 1);
    poller.stop();
  },
);

await scenario(
  G_POLLER,
  "100 start/stop cycles leave no timers and no runs in flight",
  async () => {
    const { sched, r, poller } = makePoller();
    for (let i = 0; i < 100; i += 1) {
      poller.start();
      if (i % 2 === 0) poller.stop();
      take(r.pending).resolve();
      await flush();
      if (i % 2 === 1) {
        assert.equal(sched.queue.length, 1, `cycle ${i}: one timer`);
        poller.stop();
      }
      assert.equal(sched.queue.length, 0, `cycle ${i}: no timers left`);
      assert.equal(poller.running(), false, `cycle ${i}: stopped`);
    }
    assert.equal(r.pending.length, 0);
    assert.equal(r.calls(), 100);
  },
);

await scenario(
  G_POLLER,
  "(a) stop then start while the old run is in flight: no immediate second run",
  async () => {
    const { sched, r, poller } = makePoller();
    poller.start();
    poller.stop();
    poller.start();
    assert.equal(poller.running(), true);
    assert.equal(
      r.calls(),
      1,
      "no extra immediate run while the old run is in flight",
    );
    assert.equal(r.pending.length, 1, "exactly one run in flight");
    assert.equal(
      sched.queue.length,
      0,
      "nothing scheduled before the old run settles",
    );
    take(r.pending).resolve();
    await flush();
    assert.equal(r.calls(), 1, "settling does not start a run by itself");
    assert.equal(r.pending.length, 0);
    assert.equal(
      sched.queue.length,
      1,
      "exactly one tick scheduled after the old run settles",
    );
    assert.equal(sched.queue[0]?.ms, 2000);
    take(sched.queue).fn();
    await flush();
    assert.equal(r.calls(), 2);
    assert.equal(r.pending.length, 1, "one run in flight after the tick");
    assert.equal(
      sched.queue.length,
      0,
      "no timer while the tick's run is in flight",
    );
    take(r.pending).resolve();
    await flush();
    assert.equal(sched.queue.length, 1, "normal loop resumes");
    poller.stop();
  },
);

await scenario(
  G_POLLER,
  "(b) stop, start, stop before the old run settles: nothing is ever scheduled",
  async () => {
    const { sched, r, poller } = makePoller();
    poller.start();
    poller.stop();
    poller.start();
    poller.stop();
    assert.equal(poller.running(), false);
    take(r.pending).resolve();
    await flush();
    assert.equal(sched.queue.length, 0, "no timer after the old run settles");
    assert.equal(r.pending.length, 0);
    assert.equal(r.calls(), 1, "no run was started by the restart");
    assert.equal(poller.running(), false);
  },
);

await scenario(
  G_POLLER,
  "(c) old run rejects after a stop-then-start: still schedules the next tick",
  async () => {
    const { sched, r, poller } = makePoller();
    poller.start();
    poller.stop();
    poller.start();
    assert.equal(r.calls(), 1);
    take(r.pending).reject(new Error("boom"));
    await flush();
    assert.equal(sched.queue.length, 1, "rejection still reschedules");
    assert.equal(poller.running(), true);
    assert.equal(r.pending.length, 0);
    take(sched.queue).fn();
    await flush();
    assert.equal(r.calls(), 2);
    assert.equal(r.pending.length, 1, "exactly one run in flight");
    poller.stop();
  },
);

// ---- 9 i18n key constants ------------------------------------------------
const KEY_PREFIX = "settings.experimental.vaultRag";
const DISCLOSURE_SUFFIXES = [
  "folders",
  "sentToProvider",
  "bestEffortRedaction",
  "notesMayInstruct",
  "spokenAloud",
  "toolsRequirement",
  "unsupportedFallback",
  "localCache",
];

await scenario(
  G_KEYS,
  "DISCLOSURE_KEYS is exactly the 8 disclosure keys under the prefix",
  () => {
    assert.equal(DISCLOSURE_KEYS.length, 8);
    for (const key of DISCLOSURE_KEYS) {
      assert.ok(
        key.startsWith(`${KEY_PREFIX}.disclosure.`),
        `wrong prefix: ${key}`,
      );
    }
    const suffixes = DISCLOSURE_KEYS.map((k) =>
      k.slice(`${KEY_PREFIX}.disclosure.`.length),
    );
    assert.deepEqual([...suffixes].sort(), [...DISCLOSURE_SUFFIXES].sort());
  },
);

await scenario(G_KEYS, "VAULT_RAG_I18N_KEYS has no duplicates", () => {
  assert.equal(new Set(VAULT_RAG_I18N_KEYS).size, VAULT_RAG_I18N_KEYS.length);
});

await scenario(
  G_KEYS,
  "every VAULT_RAG_I18N_KEYS key starts with settings.experimental.vaultRag.",
  () => {
    for (const key of VAULT_RAG_I18N_KEYS) {
      assert.ok(key.startsWith(`${KEY_PREFIX}.`), `wrong prefix: ${key}`);
    }
  },
);

await scenario(
  G_KEYS,
  "every disclosure key is also in VAULT_RAG_I18N_KEYS",
  () => {
    for (const key of DISCLOSURE_KEYS) {
      assert.ok(VAULT_RAG_I18N_KEYS.includes(key), `missing from list: ${key}`);
    }
  },
);

await scenario(G_KEYS, "poll interval is 2 s", () => {
  assert.equal(VAULT_RAG_POLL_MS, 2000);
});

// ---- 10 locale parity ----------------------------------------------------
function lookup(tree: unknown, key: string): unknown {
  let cur: unknown = tree;
  for (const part of key.split(".")) {
    if (cur === null || typeof cur !== "object") return undefined;
    cur = (cur as Record<string, unknown>)[part];
  }
  return cur;
}

const localeNames = readdirSync(LOCALES_DIR).filter((name) =>
  statSync(path.join(LOCALES_DIR, name)).isDirectory(),
);

await scenario(
  G_LOCALE,
  "every locale directory is checked (en + 25 others)",
  () => {
    assert.ok(localeNames.includes("en"));
    assert.ok(localeNames.length >= 26, `only ${localeNames.length} locales`);
  },
);

await scenario(
  G_LOCALE,
  "every key exists with a non-empty string in every locale",
  () => {
    const problems: string[] = [];
    for (const locale of localeNames) {
      const file = path.join(LOCALES_DIR, locale, "translation.json");
      const tree: unknown = JSON.parse(readFileSync(file, "utf8"));
      for (const key of VAULT_RAG_I18N_KEYS) {
        const value = lookup(tree, key);
        if (typeof value !== "string" || value.trim() === "") {
          problems.push(`${locale}: ${key}`);
        }
      }
    }
    assert.deepEqual(
      problems.slice(0, 20),
      [],
      `${problems.length} missing/empty`,
    );
  },
);

// ---- 11 source scans -----------------------------------------------------
function updaterBody(storeSource: string, key: string): string {
  const start = storeSource.indexOf("const settingUpdaters");
  assert.ok(start >= 0, "no `const settingUpdaters` in settingsStore.ts");
  const end = storeSource.indexOf("\n};", start);
  assert.ok(end > start, "end of settingUpdaters not found");
  const region = storeSource.slice(start, end);
  const head = region.indexOf(`\n  ${key}:`);
  assert.ok(head >= 0, `no settingUpdaters entry for ${key}`);
  const next = region.slice(head + 1).search(/\n  [a-z_0-9]+:/);
  return next < 0 ? region.slice(head) : region.slice(head, head + 1 + next);
}

await scenario(
  G_SCAN,
  "VaultRagSettings.tsx exists, <= 250 lines, no raw HTML injection",
  () => {
    assert.ok(existsSync(COMPONENT_FILE), `missing ${COMPONENT_FILE}`);
    const source = readFileSync(COMPONENT_FILE, "utf8");
    assert.ok(
      !source.includes("dangerouslySetInnerHTML"),
      "dangerouslySetInnerHTML used",
    );
    assert.ok(!source.includes("innerHTML"), "innerHTML used");
    const lines = source.split("\n").length;
    assert.ok(lines <= 250, `component is ${lines} lines (max 250)`);
  },
);

await scenario(
  G_SCAN,
  "settingUpdaters: vault_rag_enabled -> commands.changeVaultRagSetting",
  () => {
    const body = updaterBody(
      readFileSync(SETTINGS_STORE, "utf8"),
      "vault_rag_enabled",
    );
    assert.match(body, /changeVaultRagSetting\b/);
  },
);

await scenario(
  G_SCAN,
  "settingUpdaters: vault_rag_folders -> commands.changeVaultRagFoldersSetting",
  () => {
    const body = updaterBody(
      readFileSync(SETTINGS_STORE, "utf8"),
      "vault_rag_folders",
    );
    assert.match(body, /changeVaultRagFoldersSetting\b/);
  },
);

await scenario(
  G_SCAN,
  "VaultRagSettings is rendered by the Experimental page or ConversationMemorySettings",
  () => {
    const experimental = readFileSync(
      path.join(HERE, "placedRenderers.tsx"),
      "utf8",
    );
    const memory = readFileSync(
      path.join(HERE, "ConversationMemorySettings.tsx"),
      "utf8",
    );
    assert.ok(
      /<VaultRagSettings\b/.test(experimental) ||
        /<VaultRagSettings\b/.test(memory),
      "no <VaultRagSettings /> element found in the Experimental page or ConversationMemorySettings",
    );
  },
);

await scenario(
  G_SCAN,
  "settings-search: vaultRag entries exist for enable and folders, labels in the key list",
  () => {
    const vault = EXPERIMENTAL_ENTRIES.filter((e) => e.id.includes("vaultRag"));
    assert.ok(
      vault.some((e) => /enable/i.test(e.id)),
      "no vaultRag enable entry",
    );
    assert.ok(
      vault.some((e) => /folders/i.test(e.id)),
      "no vaultRag folders entry",
    );
    for (const e of vault) {
      assert.ok(
        VAULT_RAG_I18N_KEYS.includes(e.labelKey),
        `${e.id} labelKey not in VAULT_RAG_I18N_KEYS: ${e.labelKey}`,
      );
    }
  },
);

// ---- 12 normalizeFolders (V2) --------------------------------------------
const G_NORMALIZE = "12 normalizeFolders";

await scenario(G_NORMALIZE, "empty and separator-only input is empty", () => {
  assert.equal(normalizeFolders(""), "");
  assert.equal(normalizeFolders("   "), "");
  assert.equal(normalizeFolders(" , ,  ,"), "");
});

await scenario(G_NORMALIZE, "spaces around names are trimmed", () => {
  assert.equal(normalizeFolders(" Projects ,  Daily  "), "Projects,Daily");
});

await scenario(G_NORMALIZE, "repeated names are dropped, first kept", () => {
  assert.equal(normalizeFolders("A,B,A,C,B"), "A,B,C");
  assert.equal(normalizeFolders("Daily, Daily ,Daily"), "Daily");
});

await scenario(G_NORMALIZE, "the order of first appearance is kept", () => {
  assert.equal(normalizeFolders("Zeta,Alpha,Mid"), "Zeta,Alpha,Mid");
});

await scenario(G_NORMALIZE, "trailing and leading commas are dropped", () => {
  assert.equal(normalizeFolders("A,B,,"), "A,B");
  assert.equal(normalizeFolders(",A,B"), "A,B");
});

await scenario(
  G_NORMALIZE,
  "unicode names are kept whole and deduplicated",
  () => {
    assert.equal(
      normalizeFolders("Проекты, Дневник ,Проекты"),
      "Проекты,Дневник",
    );
    assert.equal(normalizeFolders("日記,メモ,日記"), "日記,メモ");
  },
);

await scenario(G_NORMALIZE, "names differing only by case are distinct", () => {
  assert.equal(normalizeFolders("Projects,projects"), "Projects,projects");
});

await scenario(
  G_NORMALIZE,
  "a single name with no separator is returned unchanged",
  () => {
    assert.equal(normalizeFolders("Memory"), "Memory");
  },
);

await scenario(
  G_NORMALIZE,
  "vault_rag_enabled updater throws on a backend error",
  () => {
    const body = updaterBody(
      readFileSync(SETTINGS_STORE, "utf8"),
      "vault_rag_enabled",
    );
    assert.match(body, /throw new Error\(result\.error\)/);
  },
);

// ---- summary -------------------------------------------------------------
const groups = [...new Set(results.map((r) => r.group))];
let failedTotal = 0;
for (const group of groups) {
  const rows = results.filter((r) => r.group === group);
  const failed = rows.filter((r) => !r.ok);
  failedTotal += failed.length;
  console.log(
    `[${failed.length === 0 ? "green" : "RED"}] ${group}: ${rows.length - failed.length}/${rows.length} pass`,
  );
  for (const f of failed)
    console.log(`    FAIL ${f.name}\n         ${f.error}`);
}
console.log(
  `${results.length - failedTotal}/${results.length} scenarios passed, ${failedTotal} failed`,
);
if (failedTotal > 0) process.exitCode = 1;
