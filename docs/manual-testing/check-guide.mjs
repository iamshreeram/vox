#!/usr/bin/env node
// Structural checks for the manual test guide (experimental-features.html).
// Usage: node docs/manual-testing/check-guide.mjs
// Exits 1 and lists every problem when the guide is inconsistent. It does not
// start a browser; it evaluates only the data tables at the top of the page script.
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const html = fs.readFileSync(
  path.join(here, "experimental-features.html"),
  "utf8",
);

const start = html.indexOf("var SECTIONS = [");
const end = html.indexOf("function whereFor(");
if (start < 0 || end < 0) {
  console.error(
    "Cannot find the data tables (var SECTIONS ... function whereFor).",
  );
  process.exit(1);
}
const sandbox = {};
vm.createContext(sandbox);
vm.runInContext(
  html.slice(start, end) + "\nthis.__d = { SECTIONS, NAV, PATCH, ADD, PRI };",
  sandbox,
);
const { SECTIONS, NAV, PATCH, ADD, PRI } = sandbox.__d;

const problems = [];
const bad = (m) => problems.push(m);
const text = (v) => (typeof v === "string" ? v : "");

// --- collect tests ---
// The page script already applied PATCH and ADD to SECTIONS (function applyRevisions
// sits before whereFor), so SECTIONS is the final list the browser renders.
const tests = [];
const sectionOf = {};
for (const s of SECTIONS) {
  if (!s.id || !s.title)
    bad(`section without id/title: ${JSON.stringify(s.id)}`);
  for (const t of s.tests || []) {
    tests.push(t);
    sectionOf[t.id] = s.id;
  }
}
const ids = new Set(tests.map((t) => t.id));
for (const a of ADD) {
  if (!a.test || !ids.has(a.test.id))
    bad(`ADD entry was not inserted (unknown 'after' id?): ${a.after}`);
}

// --- unique ids ---
const seen = new Set();
for (const t of tests) {
  if (!t.id) {
    bad("a test has no id");
    continue;
  }
  if (seen.has(t.id)) bad(`duplicate test id: ${t.id}`);
  seen.add(t.id);
}

// --- PATCH keys exist ---
for (const k of Object.keys(PATCH))
  if (!seen.has(k)) bad(`PATCH refers to unknown test id: ${k}`);

// --- every PATCH field must equal the final object's field (R7): the page applies PATCH once ---
for (const [id, fields] of Object.entries(PATCH)) {
  const t = tests.find((x) => x.id === id);
  if (!t) continue; // reported above
  for (const [k, v] of Object.entries(fields))
    if (JSON.stringify(t[k]) !== JSON.stringify(v))
      bad(`${id}: PATCH field '${k}' differs from the final test object`);
}

// --- every test is complete (final objects) ---
for (const t of tests) {
  const p = t; // final object: the page already applied ADD then PATCH
  if (!text(p.title).trim()) bad(`${t.id}: empty title`);
  if (
    !Array.isArray(p.steps) ||
    p.steps.length === 0 ||
    p.steps.some((x) => !text(x).trim())
  )
    bad(`${t.id}: needs at least one non-empty step`);
  if (!text(p.expect).trim()) bad(`${t.id}: needs an expect text`);
}

// --- navigation help rows are complete ---
for (const row of NAV) {
  if (
    !Array.isArray(row) ||
    row.length < 3 ||
    row.slice(0, 3).some((c) => !text(c).trim())
  )
    bad(`NAV row incomplete: ${JSON.stringify(row).slice(0, 80)}`);
}

// --- the Experimental page no longer exists: no instructions may send people there ---
const stale = [
  /\bExperimental (page|item|tab)\b/i,
  /\bExperimental card\b/i,
  /\bcards? on the Experimental\b/i,
];
const everything = [];
for (const s of SECTIONS)
  everything.push([`section ${s.id} note`, text(s.note)]);
for (const t of tests) {
  const p = t; // final object: the page already applied ADD then PATCH
  everything.push(
    [`${t.id} title`, text(p.title)],
    [`${t.id} needs`, text(p.needs)],
    [`${t.id} expect`, text(p.expect)],
    [`${t.id} warn`, text(p.warn)],
  );
  for (const st of p.steps || []) everything.push([`${t.id} step`, text(st)]);
}
for (const row of NAV) everything.push([`NAV ${row[0]}`, row.join(" | ")]);
for (const [where, s] of everything) {
  for (const re of stale)
    if (re.test(s))
      bad(`stale navigation (${re}) in ${where}: ${s.slice(0, 90)}`);
  if (/^Experimental, /.test(s))
    bad(`stale section note start in ${where}: ${s.slice(0, 60)}`);
}
for (const row of NAV) {
  if (row[1] === "Experimental")
    bad(`NAV row still points at the removed Experimental tab: ${row[0]}`);
}
const validTabs = new Set([
  "General",
  "Models",
  "Advanced",
  "Agents",
  "Post Process",
  "History",
  "About",
  "Debug",
  "Tray",
  "Menu bar",
  "Brain",
]);
for (const row of NAV)
  if (!validTabs.has(row[1]))
    bad(`NAV row has an unknown tab '${row[1]}': ${row[0]}`);

// --- duplicate NAV labels: a test that bolds one of them must say where (R8) ---
const navLabelRows = {};
for (const row of NAV) {
  const key = row[0].toLowerCase();
  (navLabelRows[key] = navLabelRows[key] || []).push(row);
}
for (const t of tests) {
  const all = [t.title, t.needs || "", t.warn || ""]
    .concat(t.steps || [])
    .concat([t.expect || ""])
    .join(" ");
  const labels = [
    ...new Set(
      (all.match(/<b>(.*?)<\/b>/g) || []).map((m) =>
        m
          .replace(/<[^>]+>/g, "")
          .replace(/&amp;/g, "&")
          .trim()
          .toLowerCase(),
      ),
    ),
  ];
  for (const lbl of labels) {
    const rows = navLabelRows[lbl] || [];
    if (rows.length < 2) continue;
    if (!t.where) {
      bad(
        `${t.id}: bolds '${lbl}', which has ${rows.length} setting rows: add where:[tab, group]`,
      );
      continue;
    }
    if (!rows.some((r) => r[1] === t.where[0] && r[2] === t.where[1]))
      bad(
        `${t.id}: where ${JSON.stringify(t.where)} matches no NAV row for '${lbl}'`,
      );
  }
}
for (const t of tests) {
  if (t.where === undefined) continue;
  if (
    !Array.isArray(t.where) ||
    t.where.length < 2 ||
    t.where.length > 3 ||
    t.where.slice(0, 2).some((x) => typeof x !== "string")
  )
    bad(`${t.id}: where must be [tab, group, optional note]`);
  else if (!validTabs.has(t.where[0]))
    bad(`${t.id}: where has an unknown tab '${t.where[0]}'`);
}

// --- developer-only tests say so in the warning ---
for (const t of tests) {
  if (t.dev && !/debug build/i.test(text(t.warn)))
    bad(`${t.id}: a dev test needs a warn that says 'debug build'`);
}

// --- required coverage (new features that must have manual test cases) ---
const mustHaveIds = [
  // ACP agent and approvals
  "acp-setup",
  "acp-bad-path",
  "acp-no-double-paste",
  "acp-read-auto",
  "acp-write-asks",
  "acp-allow-once",
  "acp-deny",
  "acp-timeout",
  "acp-mode-ask-all",
  "acp-mode-deny",
  "acp-mode-persist",
  "acp-timeout-clamp",
  "acp-hidden-window",
  "acp-queue",
  "acp-failed-copy",
  "acp-contradiction",
  "acp-no-details",
  "acp-keyboard",
  // models tab and search
  "models-language",
  "models-translate",
  "models-unload",
  "models-vad-engine",
  "search-wake-action",
  "search-barge-in",
  "search-approval-rows",
  "search-gated-off",
  // voice features that were not in the guide
  "persona-pick",
  "persona-custom",
  "barge-in",
  "speaker-filter",
  "smart-turn",
  "phonon-voice",
  "natural-voice",
  // install and update scripts
  "update-script",
  "setup-voice-models-script",
  // generic assistant: spoken roles are built; hand-off, voice approval and voice cancel are planned
  "jarvis-role-spoken",
  "role-set",
  "role-continue-voice",
  "role-clear",
  "role-long-chunks",
  "role-local-first",
  "role-limits",
  "jarvis-agent-handoff",
  "jarvis-voice-approval",
  "jarvis-voice-cancel",
  // release-blocking UAT additions (cmd-ack, Hey Jarvis single trigger, speech-model failures, accuracy, clipping, natural flow, reply setup, paste)
  "cmd-ack-reset",
  "hey-jarvis-single-trigger",
  "stt-unusable-speech",
  "stt-model-missing-start",
  "stt-accuracy-long",
  "tts-first-word-and-clipping",
  "cv-natural-flow",
  "setup-reply-backend",
  "paste-default-restores",
  "tl-outside",
  "history-auto-delete-options",
  "history-retranscribe",
  "history-open-recordings-folder",
];
for (const id of mustHaveIds)
  if (!seen.has(id)) bad(`missing required test id: ${id}`);
const mustHaveSections = [
  "setup",
  "dictation",
  "acp-agent",
  "models-tab",
  "search",
  "install-update",
  "generic-assistant",
];
const secIds = new Set(SECTIONS.map((s) => s.id));
for (const id of mustHaveSections)
  if (!secIds.has(id)) bad(`missing required section: ${id}`);

// planned tests must be clearly marked so nobody reports them as failures
const plannedIds = new Set(["jarvis-voice-approval"]);
for (const t of tests.filter((x) => plannedIds.has(x.id))) {
  const blob = JSON.stringify(t);
  if (!/not built yet|planned/i.test(blob))
    bad(`${t.id}: planned test must say 'not built yet' or 'planned'`);
}

// --- priority buckets: PRI literal (R1, R2, R4), P0 size (R3), P0 self-contained text (R9), no dev-only P0 (R10) ---
const PRI_KEYS = ["P0", "P1", "P2", "X"];
const priShapeOk =
  PRI &&
  typeof PRI === "object" &&
  PRI_KEYS.every(
    (k) => Array.isArray(PRI[k]) && PRI[k].every((x) => typeof x === "string"),
  );
if (!priShapeOk) {
  bad(
    "PRI must be an object with arrays of test id strings for P0, P1, P2 and X",
  );
} else {
  // R2: no id in two literal buckets, no duplicate inside a bucket, no unknown id
  const listedIn = {};
  for (const k of ["P0", "P1", "X"]) {
    const local = new Set();
    for (const id of PRI[k]) {
      if (!seen.has(id)) bad(`PRI.${k} refers to unknown test id: ${id}`);
      if (local.has(id)) bad(`${id} appears twice in PRI.${k}`);
      local.add(id);
      if (listedIn[id] && listedIn[id] !== k)
        bad(
          `${id} is in two priority buckets (PRI.${listedIn[id]} and PRI.${k})`,
        );
      listedIn[id] = listedIn[id] || k;
    }
  }
  // R2 after applyPriority: every test in exactly one bucket (P2 is the complement)
  for (const t of tests) {
    if (!PRI_KEYS.includes(t.pri)) {
      bad(`${t.id}: not in exactly one priority bucket after applyPriority`);
      continue;
    }
    if (listedIn[t.id] && listedIn[t.id] !== t.pri)
      bad(
        `${t.id}: listed in PRI.${listedIn[t.id]} but applyPriority put it in ${t.pri}`,
      );
  }
  // R4: the not-built bucket is exactly the planned tests
  const xIds = new Set(PRI.X);
  for (const id of plannedIds)
    if (!xIds.has(id)) bad(`${id}: planned test must be listed in PRI.X`);
  for (const id of xIds)
    if (!plannedIds.has(id)) bad(`${id}: in PRI.X but not a planned test`);
  // R3: P0 size
  const p0Count = tests.filter((t) => t.pri === "P0").length;
  if (p0Count < 15 || p0Count > 40)
    bad(`P0 must have 15 to 40 tests, found ${p0Count}`);
  // R9: a P0 test must stand on its own (no pointer to another test or section)
  const backRef =
    /\b(previous|earlier|above|other|last) (test|section)\b|\bsee the [^.]{0,60}\b(test|section)\b|\bas the [\w -]{1,30} test\b|\bfrom the [\w -]{1,30} test\b|\brepeat the (same )?request\b/i;
  for (const t of tests.filter((x) => x.pri === "P0")) {
    const fields = [
      ["title", text(t.title)],
      ["needs", text(t.needs)],
      ["steps", (t.steps || []).map(text).join(" ")],
      ["expect", text(t.expect)],
      ["warn", text(t.warn)],
    ];
    for (const [name, s] of fields) {
      const hit = s.match(backRef);
      if (hit)
        bad(
          `${t.id}: P0 ${name} points at another test or section: '${hit[0]}'`,
        );
    }
  }
  // R10: a P0 test must run on the normal release build
  for (const t of tests.filter((x) => x.pri === "P0" && x.dev))
    bad(`${t.id}: a P0 test cannot be developer-only (dev)`);
}

// --- no internal names in a personal open-source repo ---
const banned = /walmart|code[-_ ]puppy|wcnp|ram-ai|\/Users\/|s0v06yb/i;
if (banned.test(html))
  bad("the page mentions an internal name or a home-directory path");

if (problems.length) {
  console.error(`${problems.length} problem(s) in the guide:`);
  for (const p of problems) console.error(" - " + p);
  process.exit(1);
}
const inBucket = (k) => tests.filter((t) => t.pri === k).length;
console.log(
  `guide ok: ${SECTIONS.length} sections, ${tests.length} tests (P0 ${inBucket("P0")}, P1 ${inBucket("P1")}, P2 ${inBucket("P2")}, not built ${inBucket("X")}), ${NAV.length} setting rows`,
);
