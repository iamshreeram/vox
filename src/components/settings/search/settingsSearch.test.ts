import assert from "node:assert/strict";
import { matchRange, normalize, resolveTarget, search } from "./settingsSearch";
import { SECTION_LABEL_KEY } from "./settingsSearchIndex";
import type {
  Resolve,
  SearchContext,
  SearchEntry,
  SearchPlatform,
  SearchSection,
  TextPair,
} from "./types";

// ---------- fixture (independent of the real index) ----------
const pair = (s: string): TextPair => ({ local: s, en: s });
const TEXT: Record<string, TextPair> = {
  "g.app": pair("App"),
  "g.engine": pair("Engine & input"),
  "g.general": pair("General"),
  "g.debug": pair("Debug"),
  "g.postproc": pair("Hotkey"),
  "l.expToggle": pair("Experimental Features"),
  "d.expToggle": pair("Turns on the Experimental page."),
  "l.lazy": pair("Keep Mic Open Between Transcriptions"),
  "d.lazy": pair(
    "Keeps the microphone stream open for 30 seconds after recording stops.",
  ),
  "l.accelT": pair("transcribe.cpp Acceleration"),
  "d.accelT": pair("Hardware acceleration for Whisper-family models."),
  "l.accelO": pair("ONNX Acceleration"),
  "d.accelO": pair("Hardware acceleration for ONNX models."),
  "l.vad": pair("Voice Activity Backend"),
  "d.vad": pair("Choose the detector used to filter silence."),
  "l.postProc": pair("Post Processing"),
  "l.provider": pair("Provider"),
  "l.logLevel": pair("Log Level"),
  "l.appLang": pair("Application Language"),
  "l.lang": pair("Language"),
  "l.cafe": { local: "Café Mode", en: "Cafe Mode" },
  "l.fi": pair("\uFB01le options"), // "ﬁle options" (ligature)
  "l.cjk1": { local: "麦克风设置", en: "Microphone Settings" },
  "l.cjk2": { local: "打开麦克风", en: "Open microphone" },
  "l.cancel": pair("Cancel Shortcut"),
  "l.echo": pair("Echo Test"),
  "d.echo": pair("Checks your mic."),
  "l.zebra": pair("Zebra"),
  "d.zebra": pair("Avoid the quokka."),
  "l.cycA": pair("Cycle A"),
  "l.cycB": pair("Cycle B"),
  "l.orphan": pair("Orphan Row"),
  "sidebar.general": pair("General"),
  "sidebar.history": pair("History"),
  "sidebar.models": pair("Models"),
  "sidebar.advanced": pair("Advanced"),
  "sidebar.postProcessing": pair("Post Process"),
  "sidebar.debug": pair("Debug"),
  "sidebar.agents": pair("Agents"),
  "sidebar.about": pair("About"),
};
for (let n = 0; n < 40; n++) TEXT[`l.item.${n}`] = pair(`Item ${n}`);
const resolve: Resolve = (key) => {
  const v = TEXT[key];
  if (!v) throw new Error(`fixture has no text for ${key}`);
  return v;
};
// The section label keys used by the real index must be resolvable by this fixture.
for (const key of Object.values(SECTION_LABEL_KEY)) {
  assert.ok(TEXT[key], `fixture lacks section label key ${key}`);
}

const E = (
  o: Pick<SearchEntry, "id" | "section" | "labelKey"> & Partial<SearchEntry>,
): SearchEntry => o;
const ENTRIES: SearchEntry[] = [
  E({
    id: "general.language",
    section: "general",
    groupKey: "g.general",
    labelKey: "l.lang",
  }),
  E({
    id: "general.cancelShortcut",
    section: "general",
    groupKey: "g.general",
    labelKey: "l.cancel",
    platforms: ["macos", "windows"],
  }),
  E({
    id: "general.cafe",
    section: "general",
    groupKey: "g.general",
    labelKey: "l.cafe",
  }),
  E({
    id: "general.fi",
    section: "general",
    groupKey: "g.general",
    labelKey: "l.fi",
  }),
  E({
    id: "general.cjk1",
    section: "general",
    groupKey: "g.general",
    labelKey: "l.cjk1",
  }),
  E({
    id: "general.cjk2",
    section: "general",
    groupKey: "g.general",
    labelKey: "l.cjk2",
  }),
  E({
    id: "general.echo",
    section: "general",
    groupKey: "g.general",
    labelKey: "l.echo",
    descriptionKey: "d.echo",
  }),
  E({
    id: "general.zebra",
    section: "general",
    groupKey: "g.general",
    labelKey: "l.zebra",
    descriptionKey: "d.zebra",
  }),
  E({
    id: "advanced.experimentalToggle",
    section: "advanced",
    groupKey: "g.app",
    labelKey: "l.expToggle",
    descriptionKey: "d.expToggle",
    keywords: ["experimental"],
  }),
  E({
    id: "postprocessing.provider",
    section: "postprocessing",
    groupKey: "g.postproc",
    labelKey: "l.provider",
    unlockedBy: "experimental.postProcessing",
  }),
  E({
    id: "debug.logLevel",
    section: "debug",
    groupKey: "g.debug",
    labelKey: "l.logLevel",
  }),
  E({
    id: "experimental.postProcessing",
    section: "agents",
    groupKey: "g.engine",
    groupId: "engine",
    labelKey: "l.postProc",
    unlockedBy: "advanced.experimentalToggle",
  }),
  E({
    id: "experimental.lazyStreamClose",
    section: "agents",
    groupKey: "g.engine",
    groupId: "engine",
    labelKey: "l.lazy",
    descriptionKey: "d.lazy",
    keywords: ["lazy stream close", "microphone latency"],
    unlockedBy: "advanced.experimentalToggle",
  }),
  E({
    id: "experimental.accelT",
    section: "agents",
    groupKey: "g.engine",
    groupId: "engine",
    labelKey: "l.accelT",
    descriptionKey: "d.accelT",
    keywords: ["gpu", "metal", "whisper"],
    unlockedBy: "advanced.experimentalToggle",
  }),
  E({
    id: "experimental.accelO",
    section: "agents",
    groupKey: "g.engine",
    groupId: "engine",
    labelKey: "l.accelO",
    descriptionKey: "d.accelO",
    keywords: ["gpu", "onnx"],
    unlockedBy: "advanced.experimentalToggle",
  }),
  E({
    id: "experimental.vad",
    section: "agents",
    groupKey: "g.engine",
    groupId: "engine",
    labelKey: "l.vad",
    descriptionKey: "d.vad",
    keywords: ["vad", "silero", "earshot"],
    unlockedBy: "advanced.experimentalToggle",
  }),
  E({
    id: "experimental.cycA",
    section: "agents",
    labelKey: "l.cycA",
    unlockedBy: "experimental.cycB",
  }),
  E({
    id: "experimental.cycB",
    section: "agents",
    labelKey: "l.cycB",
    unlockedBy: "experimental.cycA",
  }),
  E({
    id: "experimental.orphan",
    section: "agents",
    labelKey: "l.orphan",
    unlockedBy: "nope.missing",
  }),
  E({ id: "about.appLanguage", section: "about", labelKey: "l.appLang" }),
];
for (let n = 0; n < 40; n++) {
  ENTRIES.push(
    E({
      id: `general.item${n}`,
      section: "general",
      groupKey: "g.general",
      labelKey: `l.item.${n}`,
    }),
  );
}

const DEFAULT_ON: SearchSection[] = [
  "general",
  "history",
  "models",
  "advanced",
  "about",
];
const ALL_ON: SearchSection[] = [
  ...DEFAULT_ON,
  "postprocessing",
  "debug",
  "agents",
];
const ctx = (
  on: SearchSection[],
  platform: SearchPlatform = "macos",
): SearchContext => ({
  isSectionEnabled: (s) => on.includes(s),
  platform,
});
const ids = (
  query: string,
  c: SearchContext = ctx(ALL_ON),
  limit?: number,
): string[] =>
  search(
    ENTRIES,
    query,
    resolve,
    c,
    limit === undefined ? undefined : { limit },
  ).map((h) => h.entry.id);
const first = (query: string, c: SearchContext = ctx(ALL_ON)) =>
  search(ENTRIES, query, resolve, c)[0];

// ---------- normalize ----------
assert.equal(
  normalize("  Café   Mode "),
  "cafe mode",
  "diacritics, case, whitespace",
);
assert.equal(normalize("\uFB01"), "fi", "compatibility ligature");
assert.equal(normalize("e\u0301"), "e", "combining mark stripped");
assert.equal(normalize(""), "");
assert.equal(normalize("   "), "");
assert.equal(normalize("麦克风"), "麦克风", "CJK unchanged");

// ---------- empty, huge, metacharacters ----------
assert.deepEqual(ids(""), [], "empty query");
assert.deepEqual(ids("   \t "), [], "whitespace query");
for (const q of [
  "(",
  "[",
  "\\",
  "*",
  "+",
  "?",
  ".*",
  "^$",
  "(a|b)",
  "[a-z]+",
]) {
  assert.doesNotThrow(
    () => search(ENTRIES, q, resolve, ctx(ALL_ON)),
    `metachar ${q} must not throw`,
  );
  assert.deepEqual(ids(q), [], `metachar ${q} matches nothing literally`);
}
assert.deepEqual(ids("x".repeat(10000)), [], "huge query is harmless");
assert.deepEqual(
  ids("keep " + "a".repeat(200)),
  [],
  "101+ char query is capped, not an error",
);
assert.deepEqual(
  ids("  keep   mic  "),
  ids("keep mic"),
  "extra whitespace between tokens is irrelevant",
);

// ---------- the user's real examples ----------
assert.equal(
  first("lazy stream")?.entry.id,
  "experimental.lazyStreamClose",
  "synonym finds Keep Mic Open",
);
assert.equal(
  first("keep mic")?.entry.id,
  "experimental.lazyStreamClose",
  "multi-token AND",
);
assert.deepEqual(ids("keep zebra"), [], "AND: every token must match");
const accel = ids("acceleration").slice(0, 2).sort();
assert.deepEqual(
  accel,
  ["experimental.accelO", "experimental.accelT"],
  "acceleration finds both selectors first",
);
assert.deepEqual(
  ids("gpu").sort(),
  ["experimental.accelO", "experimental.accelT"],
  "keyword only match",
);
assert.equal(first("vad")?.entry.id, "experimental.vad");
assert.equal(first("silero")?.entry.id, "experimental.vad");

// ---------- ranking ----------
assert.deepEqual(ids("quokka"), ["general.zebra"], "description-only match");
{
  const mic = ids("mic");
  const echoAt = mic.indexOf("general.echo");
  for (const id of [
    "experimental.lazyStreamClose",
    "general.cjk1",
    "general.cjk2",
  ]) {
    assert.ok(mic.includes(id), `mic finds ${id}`);
    assert.ok(
      mic.indexOf(id) < echoAt,
      `label match ${id} outranks description-only match`,
    );
  }
}
{
  const lang = ids("language");
  assert.ok(
    lang.includes("general.language") && lang.includes("about.appLanguage"),
    "duplicate-ish labels both returned",
  );
  assert.ok(
    lang.indexOf("general.language") < lang.indexOf("about.appLanguage"),
    "label equal to the query ranks first",
  );
}
{
  const hits = search(ENTRIES, "engine", resolve, ctx(ALL_ON));
  const got = hits.map((h) => h.entry.id);
  for (const id of [
    "experimental.postProcessing",
    "experimental.lazyStreamClose",
    "experimental.accelT",
    "experimental.accelO",
    "experimental.vad",
  ]) {
    assert.ok(got.includes(id), `group title match includes ${id}`);
  }
  assert.ok(
    hits.every((h) => h.score <= 30 + 25),
    "group-title matches carry low scores",
  );
}
{
  const items = ids("item");
  assert.equal(items.length, 30, "default limit is 30");
  assert.deepEqual(
    items,
    Array.from({ length: 30 }, (_, n) => `general.item${n}`),
    "ties keep index order",
  );
  assert.equal(ids("item", ctx(ALL_ON), 5).length, 5, "explicit limit");
}

// ---------- unicode ----------
assert.equal(
  first("cafe")?.entry.id,
  "general.cafe",
  "unaccented query finds accented label",
);
assert.equal(
  first("café")?.entry.id,
  "general.cafe",
  "accented query finds it too",
);
assert.equal(first("CAFÉ MODE")?.entry.id, "general.cafe", "case-insensitive");
assert.equal(
  first("file")?.entry.id,
  "general.fi",
  "ligature label matches plain letters",
);
assert.deepEqual(
  ids("麦克风").slice(0, 2),
  ["general.cjk1", "general.cjk2"],
  "CJK: prefix outranks substring",
);
assert.deepEqual(ids("风设"), ["general.cjk1"], "CJK substring only");
assert.equal(
  first("microphone settings")?.entry.id,
  "general.cjk1",
  "English text matches while UI language is not English",
);

// ---------- gating ----------
{
  const fresh = ctx(DEFAULT_ON);
  const lazy = first("lazy stream", fresh);
  assert.equal(lazy.entry.id, "experimental.lazyStreamClose");
  assert.equal(lazy.locked, true, "experimental hidden => locked");
  assert.deepEqual(lazy.unlockChain, ["advanced.experimentalToggle"]);
  assert.equal(
    resolveTarget(lazy, ENTRIES, fresh).id,
    "advanced.experimentalToggle",
    "locked hit routes to the enabler",
  );

  const unlocked = first("lazy stream", ctx(ALL_ON));
  assert.equal(unlocked.locked, false);
  assert.deepEqual(unlocked.unlockChain, []);
  assert.equal(
    resolveTarget(unlocked, ENTRIES, ctx(ALL_ON)).id,
    "experimental.lazyStreamClose",
  );

  const provider = first("provider", fresh);
  assert.equal(provider.locked, true);
  assert.deepEqual(
    provider.unlockChain,
    ["experimental.postProcessing", "advanced.experimentalToggle"],
    "two-level chain",
  );
  assert.equal(
    resolveTarget(provider, ENTRIES, fresh).id,
    "advanced.experimentalToggle",
  );

  const expOnly = ctx([...DEFAULT_ON, "agents"]);
  const provider2 = first("provider", expOnly);
  assert.deepEqual(
    provider2.unlockChain,
    ["experimental.postProcessing"],
    "chain stops at the first reachable enabler",
  );
  assert.equal(
    resolveTarget(provider2, ENTRIES, expOnly).id,
    "experimental.postProcessing",
  );

  assert.deepEqual(
    ids("log level", fresh),
    [],
    "debug hidden while debug mode is off",
  );
  const dbg = first("log level", ctx([...DEFAULT_ON, "debug"]));
  assert.equal(dbg.entry.id, "debug.logLevel");
  assert.equal(dbg.locked, false);

  assert.deepEqual(ids("cycle", fresh), [], "cyclic unlockedBy is dropped");
  assert.deepEqual(
    ids("orphan", fresh),
    [],
    "missing unlockedBy target is dropped",
  );
  assert.deepEqual(
    ids("cycle", ctx(ALL_ON)).sort(),
    ["experimental.cycA", "experimental.cycB"],
    "…but both show when their page is enabled",
  );
  assert.deepEqual(ids("orphan", ctx(ALL_ON)), ["experimental.orphan"]);
}

// ---------- platform ----------
assert.deepEqual(ids("cancel", ctx(ALL_ON, "macos")), [
  "general.cancelShortcut",
]);
assert.deepEqual(
  ids("cancel", ctx(ALL_ON, "linux")),
  [],
  "entry not available on this platform",
);

// ---------- matchRange ----------
assert.deepEqual(matchRange("Keep Mic Open", "mic"), { start: 5, end: 8 });
assert.deepEqual(
  matchRange("Café Mode", "cafe"),
  { start: 0, end: 4 },
  "accent-insensitive",
);
assert.deepEqual(matchRange("Café Mode", "CAFÉ"), { start: 0, end: 4 });
assert.deepEqual(
  matchRange("\uFB01le options", "file"),
  { start: 0, end: 3 },
  "ligature: one display char covers two normalized chars",
);
assert.deepEqual(
  matchRange("e\u0301x", "ex"),
  { start: 0, end: 3 },
  "combining mark sits inside the range",
);
assert.deepEqual(
  matchRange("麦克风设置", "克风"),
  { start: 1, end: 3 },
  "CJK multi-character",
);
assert.deepEqual(
  matchRange("Keep Mic Open", "keep open"),
  { start: 0, end: 4 },
  "first matching token is emphasised",
);
assert.equal(
  matchRange("Keep  Mic", "mic"),
  null,
  "double space => no safe mapping => no emphasis",
);
assert.equal(
  matchRange(" Keep Mic", "mic"),
  null,
  "leading space => no safe mapping",
);
assert.equal(matchRange("Keep Mic Open", "zzz"), null, "no match");
assert.equal(matchRange("Keep Mic Open", ""), null, "empty query");
assert.equal(matchRange("Keep Mic Open", "("), null, "metachar query");

console.log("settingsSearch tests: ok");
