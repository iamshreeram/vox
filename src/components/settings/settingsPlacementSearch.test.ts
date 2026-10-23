import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { SEARCH_ENTRIES } from "./search/settingsSearchIndex";
import { resolveTarget, search } from "./search/settingsSearch";
import type {
  Resolve,
  SearchContext,
  SearchEntry,
  SearchSection,
} from "./search/types";
import { PLACED_GROUPS } from "./settingsPlacement";

// Real English strings, so the test searches the way a user would.
const en = JSON.parse(
  readFileSync(
    new URL("../../i18n/locales/en/translation.json", import.meta.url),
    "utf8",
  ),
) as Record<string, unknown>;

const lookup = (key: string): string | undefined => {
  let node: unknown = en;
  for (const part of key.split(".")) {
    if (typeof node !== "object" || node === null) return undefined;
    node = (node as Record<string, unknown>)[part];
  }
  return typeof node === "string" ? node : undefined;
};

const resolve: Resolve = (key) => {
  const text = lookup(key) ?? key;
  return { local: text, en: text };
};

// Sections a user can see while the Experimental switch is off / on. The old
// "experimental" page and the new "agents" tab are both gated by the switch;
// postprocessing and debug are gated by their own switches and not under test.
const ALWAYS: readonly string[] = [
  "general",
  "history",
  "models",
  "advanced",
  "about",
];
const ctxOff: SearchContext = {
  isSectionEnabled: (s: SearchSection) => ALWAYS.includes(s),
  experimentalEnabled: false,
  platform: "macos",
};
const ctxOn: SearchContext = {
  isSectionEnabled: (s: SearchSection) =>
    ALWAYS.includes(s) || (s as string) === "agents",
  experimentalEnabled: true,
  platform: "macos",
};
// A context that predates the flag (field omitted) must keep the old behaviour.
const ctxLegacy: SearchContext = {
  isSectionEnabled: () => true,
  platform: "macos",
};

const moved = SEARCH_ENTRIES.filter((e) => e.id.startsWith("experimental."));
const requiresExperimental = (e: SearchEntry): boolean =>
  (e as { requiresExperimental?: boolean }).requiresExperimental === true;

const scenarios: Record<string, () => void> = {
  "moved rows exist and none still claims the experimental section": () => {
    assert.ok(moved.length >= 80, `found ${moved.length} moved rows`);
    for (const e of moved) {
      assert.notEqual(String(e.section), "experimental", e.id);
    }
  },
  "every moved row points at a real placed group in its own section": () => {
    for (const e of moved) {
      assert.ok(e.groupId, `${e.id} has a groupId`);
      const group = PLACED_GROUPS.find((g) => g.id === e.groupId);
      assert.ok(group, `${e.id}: unknown group ${e.groupId}`);
      assert.equal(String(e.section), group.section, e.id);
      assert.ok(requiresExperimental(e), `${e.id} must require experimental`);
      assert.equal(e.unlockedBy, "advanced.experimentalToggle", e.id);
    }
  },
  "switch off: every moved row is locked and resolves to the toggle": () => {
    let checked = 0;
    for (const e of moved) {
      const label = lookup(e.labelKey);
      if (label === undefined) continue; // dynamic labels are covered by the index tests
      const hits = search(SEARCH_ENTRIES, label, resolve, ctxOff, {
        limit: 1000,
      });
      const hit = hits.find((h) => h.entry.id === e.id);
      assert.ok(hit, `${e.id} ("${label}") is searchable with the switch off`);
      assert.equal(hit.locked, true, `${e.id} must be locked`);
      assert.equal(
        resolveTarget(hit, SEARCH_ENTRIES, ctxOff).id,
        "advanced.experimentalToggle",
        e.id,
      );
      checked += 1;
    }
    assert.ok(checked >= 60, `only ${checked} rows were checked`);
  },
  "switch off: ordinary rows in always-visible sections stay unlocked": () => {
    const hits = search(SEARCH_ENTRIES, "experimental", resolve, ctxOff, {
      limit: 1000,
    });
    const toggle = hits.find(
      (h) => h.entry.id === "advanced.experimentalToggle",
    );
    assert.ok(toggle);
    assert.equal(toggle.locked, false);
  },
  "switch on: moved rows are unlocked and reveal their own group": () => {
    let checked = 0;
    for (const e of moved) {
      const label = lookup(e.labelKey);
      if (label === undefined) continue;
      const hit = search(SEARCH_ENTRIES, label, resolve, ctxOn, {
        limit: 1000,
      }).find((h) => h.entry.id === e.id);
      assert.ok(hit, e.id);
      assert.equal(hit.locked, false, e.id);
      assert.equal(resolveTarget(hit, SEARCH_ENTRIES, ctxOn).id, e.id);
      checked += 1;
    }
    assert.ok(checked >= 60);
  },
  "a context without the flag keeps the old behaviour (treated as on)": () => {
    const e = moved.find((x) => lookup(x.labelKey) !== undefined);
    assert.ok(e);
    const hit = search(
      SEARCH_ENTRIES,
      lookup(e.labelKey) as string,
      resolve,
      ctxLegacy,
      { limit: 1000 },
    ).find((h) => h.entry.id === e.id);
    assert.ok(hit);
    assert.equal(hit.locked, false);
  },
};

let failed = 0;
for (const [name, fn] of Object.entries(scenarios)) {
  try {
    fn();
  } catch (error) {
    failed += 1;
    console.error(`FAIL: ${name}\n`, error);
  }
}
if (failed > 0) {
  console.error(`${failed} scenario(s) failed`);
  process.exit(1);
}
console.log(
  `settings placement search: ${Object.keys(scenarios).length} scenarios passed`,
);
