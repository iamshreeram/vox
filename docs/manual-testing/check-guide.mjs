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
const html = fs.readFileSync(path.join(here, "experimental-features.html"), "utf8");

const start = html.indexOf("var SECTIONS = [");
const end = html.indexOf("function whereFor(");
if (start < 0 || end < 0) {
  console.error("Cannot find the data tables (var SECTIONS ... function whereFor).");
  process.exit(1);
}
const sandbox = {};
vm.createContext(sandbox);
vm.runInContext(html.slice(start, end) + "\nthis.__d = { SECTIONS, NAV, PATCH, ADD };", sandbox);
const { SECTIONS, NAV, PATCH, ADD } = sandbox.__d;

const problems = [];
const bad = (m) => problems.push(m);
const text = (v) => (typeof v === "string" ? v : "");

// --- collect tests ---
// The page script already applied PATCH and ADD to SECTIONS (function applyRevisions
// sits before whereFor), so SECTIONS is the final list the browser renders.
const tests = [];
const sectionOf = {};
for (const s of SECTIONS) {
  if (!s.id || !s.title) bad(`section without id/title: ${JSON.stringify(s.id)}`);
  for (const t of s.tests || []) {
    tests.push(t);
    sectionOf[t.id] = s.id;
  }
}
const ids = new Set(tests.map((t) => t.id));
for (const a of ADD) {
  if (!a.test || !ids.has(a.test.id)) bad(`ADD entry was not inserted (unknown 'after' id?): ${a.after}`);
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
for (const k of Object.keys(PATCH)) if (!seen.has(k)) bad(`PATCH refers to unknown test id: ${k}`);

// --- every test is complete after PATCH ---
for (const t of tests) {
  const p = { ...t, ...(PATCH[t.id] || {}) };
  if (!text(p.title).trim()) bad(`${t.id}: empty title`);
  if (!Array.isArray(p.steps) || p.steps.length === 0 || p.steps.some((x) => !text(x).trim()))
    bad(`${t.id}: needs at least one non-empty step`);
  if (!text(p.expect).trim()) bad(`${t.id}: needs an expect text`);
}

// --- navigation help rows are complete ---
for (const row of NAV) {
  if (!Array.isArray(row) || row.length < 3 || row.slice(0, 3).some((c) => !text(c).trim()))
    bad(`NAV row incomplete: ${JSON.stringify(row).slice(0, 80)}`);
}

// --- the Experimental page no longer exists: no instructions may send people there ---
const stale = [
  /\bExperimental (page|item|tab)\b/i,
  /\bExperimental card\b/i,
  /\bcards? on the Experimental\b/i,
];
const everything = [];
for (const s of SECTIONS) everything.push([`section ${s.id} note`, text(s.note)]);
for (const t of tests) {
  const p = { ...t, ...(PATCH[t.id] || {}) };
  everything.push([`${t.id} title`, text(p.title)], [`${t.id} needs`, text(p.needs)], [`${t.id} expect`, text(p.expect)], [`${t.id} warn`, text(p.warn)]);
  for (const st of p.steps || []) everything.push([`${t.id} step`, text(st)]);
}
for (const row of NAV) everything.push([`NAV ${row[0]}`, row.join(" | ")]);
for (const [where, s] of everything) {
  for (const re of stale) if (re.test(s)) bad(`stale navigation (${re}) in ${where}: ${s.slice(0, 90)}`);
  if (/^Experimental, /.test(s)) bad(`stale section note start in ${where}: ${s.slice(0, 60)}`);
}
for (const row of NAV) {
  if (row[1] === "Experimental") bad(`NAV row still points at the removed Experimental tab: ${row[0]}`);
}
const validTabs = new Set(["General", "Models", "Advanced", "Agents", "Post Process", "History", "About", "Debug", "Tray", "Menu bar"]);
for (const row of NAV) if (!validTabs.has(row[1])) bad(`NAV row has an unknown tab '${row[1]}': ${row[0]}`);

// --- required coverage (new features that must have manual test cases) ---
const mustHaveIds = [
  // ACP agent and approvals
  "acp-setup", "acp-bad-path", "acp-no-double-paste", "acp-read-auto", "acp-write-asks",
  "acp-allow-once", "acp-deny", "acp-timeout", "acp-mode-ask-all", "acp-mode-deny",
  "acp-mode-persist", "acp-timeout-clamp", "acp-hidden-window", "acp-queue",
  "acp-failed-copy", "acp-contradiction", "acp-no-details", "acp-keyboard",
  // models tab and search
  "models-language", "models-translate", "models-unload", "models-vad-engine",
  "search-wake-action", "search-barge-in", "search-approval-rows", "search-gated-off",
  // voice features that were not in the guide
  "persona-pick", "persona-custom", "barge-in", "speaker-filter", "smart-turn", "phonon-voice", "natural-voice",
  // install and update scripts
  "update-script", "setup-voice-models-script",
  // generic assistant: spoken roles are built; hand-off, voice approval and voice cancel are planned
  "jarvis-role-spoken", "role-set", "role-continue-voice", "role-clear", "role-long-chunks",
  "role-local-first", "role-limits", "jarvis-agent-handoff", "jarvis-voice-approval", "jarvis-voice-cancel",
];
for (const id of mustHaveIds) if (!seen.has(id)) bad(`missing required test id: ${id}`);
const mustHaveSections = ["setup", "dictation", "acp-agent", "models-tab", "search", "install-update", "generic-assistant"];
const secIds = new Set(SECTIONS.map((s) => s.id));
for (const id of mustHaveSections) if (!secIds.has(id)) bad(`missing required section: ${id}`);

// planned tests must be clearly marked so nobody reports them as failures
const plannedIds = new Set(["jarvis-voice-approval"]);
for (const t of tests.filter((x) => plannedIds.has(x.id))) {
  const blob = JSON.stringify({ ...t, ...(PATCH[t.id] || {}) });
  if (!/not built yet|planned/i.test(blob)) bad(`${t.id}: planned test must say 'not built yet' or 'planned'`);
}

// --- no internal names in a personal open-source repo ---
const banned = /walmart|code[-_ ]puppy|wcnp|ram-ai|\/Users\/|s0v06yb/i;
if (banned.test(html)) bad("the page mentions an internal name or a home-directory path");

if (problems.length) {
  console.error(`${problems.length} problem(s) in the guide:`);
  for (const p of problems) console.error(" - " + p);
  process.exit(1);
}
console.log(`guide ok: ${SECTIONS.length} sections, ${tests.length} tests, ${NAV.length} setting rows`);
