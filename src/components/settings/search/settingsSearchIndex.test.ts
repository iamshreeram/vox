import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { CORE_ENTRIES } from "./index/core";
import { EXPERIMENTAL_ENTRIES } from "./index/experimental";
import {
  DYNAMIC_FAMILIES,
  EXCLUDED_KEYS,
  SEARCH_ENTRIES,
} from "./settingsSearchIndex";
import type { SearchEntry, SearchSection } from "./types";

/**
 * Index integrity + coverage.
 *   SEARCH_SCOPE=core|experimental : LOCAL checks (4a) on that file's entries only.
 *   (unset / all)                   : local checks + cross-file unlock chains (4b) + coverage (1-3).
 *   SEARCH_DROP=id1,id2             : mutation switch: drop these entries before checking, to
 *                                     prove the test fails and names the missing control.
 *   SEARCH_VERBOSE=1                : print every problem instead of the first 80.
 */
const HERE = path.dirname(fileURLToPath(import.meta.url));
const SRC = path.resolve(HERE, "../../..");
const REPO = path.resolve(SRC, "..");
const SCOPE = process.env.SEARCH_SCOPE ?? "all";
assert.ok(
  ["all", "core", "experimental"].includes(SCOPE),
  `bad SEARCH_SCOPE ${SCOPE}`,
);
const DROP = new Set(
  (process.env.SEARCH_DROP ?? "").split(",").filter(Boolean),
);

const EN = JSON.parse(
  readFileSync(path.join(SRC, "i18n/locales/en/translation.json"), "utf8"),
) as Record<string, unknown>;
function en(key: string): unknown {
  let cur: unknown = EN;
  for (const part of key.split(".")) {
    if (cur === null || typeof cur !== "object") return undefined;
    cur = (cur as Record<string, unknown>)[part];
  }
  return cur;
}
const hasText = (key: string): boolean => typeof en(key) === "string";

const CORE_SECTIONS: SearchSection[] = [
  "general",
  "history",
  "models",
  "advanced",
  "postprocessing",
  "debug",
  "about",
];
const ALL_SECTIONS: SearchSection[] = [...CORE_SECTIONS, "agents"];
const GATED: SearchSection[] = ["postprocessing", "agents"];
const ENABLED_BY_DEFAULT: SearchSection[] = [
  "general",
  "history",
  "models",
  "advanced",
  "about",
];
const VALID_GROUP_IDS = new Set([
  "engine",
  "vadEngine",
  "voiceCommands",
  "listening",
  "memory",
  "agentBridge",
  "acpAgent",
  "spokenReplies",
  "realtime",
  "conversationMemory",
  "agentTools",
  "codingJobs",
  "smartTurn",
  "speakerFilter",
  "replyStyle",
  "conversation",
]);
const PLATFORMS = new Set(["macos", "windows", "linux"]);

const problems: string[] = [];
const fail = (msg: string): void => {
  problems.push(msg);
};

const scoped = (entries: readonly SearchEntry[]): SearchEntry[] =>
  entries.filter((e) => !DROP.has(e.id));
const own: SearchEntry[] =
  SCOPE === "core"
    ? scoped(CORE_ENTRIES)
    : SCOPE === "experimental"
      ? scoped(EXPERIMENTAL_ENTRIES)
      : scoped(SEARCH_ENTRIES);
const scopeSections = SCOPE === "core" ? CORE_SECTIONS : ALL_SECTIONS;

// ---------- 4a: local integrity ----------
const seen = new Set<string>();
const dupKey = new Map<string, number[]>();
for (const e of own) {
  const who = `entry ${e.id}`;
  if (seen.has(e.id)) fail(`${who}: duplicate id`);
  seen.add(e.id);
  // Moved rows keep their "experimental.<slug>" id although their page is a real section.
  const idPrefix = e.requiresExperimental ? "experimental" : e.section;
  if (!e.id.startsWith(`${idPrefix}.`) || e.id.length <= idPrefix.length + 1)
    fail(
      `${who}: id must be "<section>.<slug>" (moved rows: "experimental.<slug>")`,
    );
  if (!ALL_SECTIONS.includes(e.section))
    fail(`${who}: unknown section ${e.section}`);
  else if (!scopeSections.includes(e.section))
    fail(`${who}: section ${e.section} does not belong to scope ${SCOPE}`);
  if (!hasText(e.labelKey))
    fail(`${who}: labelKey ${e.labelKey} is not a string in en translation`);
  if (e.descriptionKey !== undefined && !hasText(e.descriptionKey))
    fail(`${who}: descriptionKey ${e.descriptionKey} missing in en`);
  if (e.groupKey !== undefined && !hasText(e.groupKey))
    fail(`${who}: groupKey ${e.groupKey} missing in en`);
  if (e.requiresExperimental) {
    if (!e.groupId || !e.groupKey)
      fail(`${who}: moved rows need groupId and groupKey`);
    else if (!VALID_GROUP_IDS.has(e.groupId))
      fail(`${who}: unknown groupId ${e.groupId}`);
  } else if (e.groupId !== undefined)
    fail(`${who}: groupId only applies to moved rows (requiresExperimental)`);
  if (GATED.includes(e.section) && !e.unlockedBy)
    fail(`${who}: section ${e.section} is gated, unlockedBy is required`);
  if (
    e.occurrence !== undefined &&
    (!Number.isInteger(e.occurrence) || e.occurrence < 0)
  )
    fail(`${who}: occurrence must be an integer >= 0`);
  for (const kw of e.keywords ?? []) {
    if (kw !== kw.toLowerCase() || kw.trim() !== kw || kw === "")
      fail(
        `${who}: keyword ${JSON.stringify(kw)} must be non-empty, trimmed, lowercase`,
      );
  }
  for (const p of e.platforms ?? [])
    if (!PLATFORMS.has(p)) fail(`${who}: unknown platform ${p}`);
  if (e.pageOnly && e.section !== "history" && e.section !== "models")
    fail(
      `${who}: pageOnly is only for the history and models pages (they have no setting rows)`,
    );
  const english =
    typeof en(e.labelKey) === "string"
      ? (en(e.labelKey) as string)
      : e.labelKey;
  const k = `${e.section}|${e.groupId ?? ""}|${english}`;
  dupKey.set(k, [...(dupKey.get(k) ?? []), e.occurrence ?? 0]);
}
for (const [k, occ] of dupKey) {
  if (new Set(occ).size !== occ.length)
    fail(
      `same-title rows must have distinct occurrence: ${k} -> ${occ.join(",")}`,
    );
}

// ---------- combined gate only ----------
if (SCOPE === "all") {
  const byId = new Map(own.map((e) => [e.id, e]));
  // 4b: unlock chains resolve across files and end in a section that is on by default.
  for (const e of own) {
    if (!e.unlockedBy) continue;
    let cur: SearchEntry | undefined = e;
    let depth = 0;
    const visited = new Set<string>([e.id]);
    while (cur && !ENABLED_BY_DEFAULT.includes(cur.section) && depth < 4) {
      const next: SearchEntry | undefined = cur.unlockedBy
        ? byId.get(cur.unlockedBy)
        : undefined;
      if (!next) {
        fail(
          `entry ${e.id}: unlockedBy chain breaks at ${cur.id} -> ${cur.unlockedBy ?? "(none)"} (enabler missing from the index)`,
        );
        cur = undefined;
        break;
      }
      if (visited.has(next.id)) {
        fail(`entry ${e.id}: unlockedBy cycle at ${next.id}`);
        cur = undefined;
        break;
      }
      visited.add(next.id);
      cur = next;
      depth++;
    }
    if (cur && !ENABLED_BY_DEFAULT.includes(cur.section))
      fail(
        `entry ${e.id}: unlockedBy chain deeper than 3 or never reaches a default-visible section`,
      );
  }
  for (const s of ALL_SECTIONS) {
    if (!own.some((e) => e.section === s)) fail(`section ${s} has no entries`);
  }

  // ---------- coverage 1-3 ----------
  const labelKeys = new Set(own.map((e) => e.labelKey));
  const groupKeys = new Set(
    own.map((e) => e.groupKey).filter((k): k is string => !!k),
  );

  for (const [key, reason] of Object.entries(EXCLUDED_KEYS)) {
    if (!hasText(key))
      fail(`EXCLUDED_KEYS ${key}: not a string in en translation`);
    if (reason.trim().length < 10 || /^dynamic/i.test(reason.trim()))
      fail(
        `EXCLUDED_KEYS ${key}: needs a real reason, got ${JSON.stringify(reason)}`,
      );
  }

  const walk = (dir: string): string[] =>
    readdirSync(dir).flatMap((name) => {
      const p = path.join(dir, name);
      if (statSync(p).isDirectory()) return name === "search" ? [] : walk(p);
      return /\.tsx$/.test(name) && !/\.test\.tsx$/.test(name) ? [p] : [];
    });
  const files = walk(path.join(SRC, "components/settings"));
  const lineOf = (text: string, index: number): number =>
    text.slice(0, index).split("\n").length;
  const rel = (p: string): string =>
    path.relative(REPO, p).split(path.sep).join("/");

  const dynamicFiles = new Map<string, number[]>();
  const GROUP_ELEMENTS = new Set(["SettingsGroup", "CollapsibleSettingsGroup"]);
  for (const file of files) {
    const text = readFileSync(file, "utf8");
    const attrRe = /\b(title|label)=\{\s*t\(\s*(["'])([^"']+)\2/g;
    for (let m = attrRe.exec(text); m; m = attrRe.exec(text)) {
      const before = text.slice(0, m.index);
      const elements = [...before.matchAll(/<([A-Z][A-Za-z0-9_.]*)\b/g)];
      const element = elements.length ? elements[elements.length - 1][1] : "?";
      const key = m[3];
      const where = `${element} at ${rel(file)}:${lineOf(text, m.index)}`;
      if (GROUP_ELEMENTS.has(element)) {
        if (!groupKeys.has(key) && !(key in EXCLUDED_KEYS))
          fail(
            `MISSING group title ${key} (${where}) is neither the groupKey of an entry nor in EXCLUDED_KEYS`,
          );
      } else if (!labelKeys.has(key) && !(key in EXCLUDED_KEYS)) {
        fail(
          `MISSING row ${key} (${where}) is neither an indexed labelKey nor in EXCLUDED_KEYS`,
        );
      }
    }
    const callRe = /(?<![\w.$])t\(\s*([^)]{0,200})/g;
    for (let m = callRe.exec(text); m; m = callRe.exec(text)) {
      const arg = m[1].trim();
      const isLiteral =
        /^(["'])/.test(arg) || (arg.startsWith("`") && !arg.includes("${"));
      if (arg === "" || isLiteral) continue;
      dynamicFiles.set(rel(file), [
        ...(dynamicFiles.get(rel(file)) ?? []),
        lineOf(text, m.index),
      ]);
    }
  }
  const reviewed = new Set(DYNAMIC_FAMILIES.map((f) => f.source));
  for (const [file, lines] of dynamicFiles) {
    if (!reviewed.has(file))
      fail(
        `UNREVIEWED dynamic t() in ${file} (lines ${lines.slice(0, 5).join(",")}): add a DynamicFamily for it (ids, or ids: [] with a note)`,
      );
  }
  for (const fam of DYNAMIC_FAMILIES) {
    const where = `DynamicFamily ${fam.source} (${fam.prefix})`;
    if (!existsSync(path.join(REPO, fam.source)))
      fail(`${where}: source file does not exist`);
    if (fam.ids.length === 0) {
      if ((fam.note ?? "").trim().length < 20)
        fail(
          `${where}: ids is empty, a note of at least 20 characters is required`,
        );
      continue;
    }
    for (const id of fam.ids) {
      const key = `${fam.prefix}${id}${fam.suffix ?? ""}`;
      if (!hasText(key))
        fail(`${where}: key ${key} is not a string in en translation`);
      else if (!labelKeys.has(key) && !(key in EXCLUDED_KEYS))
        fail(
          `MISSING dynamic row ${key} (${where}) is neither indexed nor excluded`,
        );
    }
  }
  // ---------- reverse check 5: every indexed key is really used by the UI ----------
  // Guards against typos and invented entries: an entry whose label/group key never appears
  // as a literal in component source (and is not part of a declared dynamic family) is bogus.
  const sourceFiles = (dir: string): string[] =>
    readdirSync(dir).flatMap((name) => {
      const p = path.join(dir, name);
      if (statSync(p).isDirectory())
        return name === "search" ? [] : sourceFiles(p);
      return /\.tsx?$/.test(name) && !/\.test\.tsx?$/.test(name) ? [p] : [];
    });
  const allSource = sourceFiles(path.join(SRC, "components"))
    .map((f) => readFileSync(f, "utf8"))
    .join("\n");
  const usedLiterally = (key: string): boolean =>
    allSource.includes(`"${key}"`) ||
    allSource.includes(`'${key}'`) ||
    allSource.includes(`\`${key}\``);
  const familyKeys = new Set(
    DYNAMIC_FAMILIES.flatMap((f) =>
      f.ids.map((id) => `${f.prefix}${id}${f.suffix ?? ""}`),
    ),
  );
  for (const e of own) {
    if (
      !e.pageOnly &&
      !usedLiterally(e.labelKey) &&
      !familyKeys.has(e.labelKey)
    ) {
      fail(
        `entry ${e.id}: labelKey ${e.labelKey} is never used by any component (no literal and no DynamicFamily key): typo or invented row`,
      );
    }
    if (
      e.groupKey &&
      !usedLiterally(e.groupKey) &&
      !familyKeys.has(e.groupKey)
    ) {
      fail(
        `entry ${e.id}: groupKey ${e.groupKey} is never used by any component`,
      );
    }
  }
}

if (problems.length) {
  console.error(
    `settingsSearchIndex: ${problems.length} problem(s) in scope ${SCOPE}${DROP.size ? ` (dropped: ${[...DROP].join(",")})` : ""}`,
  );
  const cap = process.env.SEARCH_VERBOSE ? problems.length : 80;
  for (const p of problems.slice(0, cap)) console.error(" - " + p);
  if (problems.length > cap)
    console.error(
      ` ... and ${problems.length - cap} more (SEARCH_VERBOSE=1 shows all)`,
    );
  process.exit(1);
}
console.log(`settingsSearchIndex: ok (scope ${SCOPE}, ${own.length} entries)`);
