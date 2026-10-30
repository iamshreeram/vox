import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { SEARCH_ENTRIES } from "./settingsSearchIndex";
import { entryForId, resolveTarget, search } from "./settingsSearch";
import { resolveSearchEntryId } from "../appearance/appearanceModel";
import type { SearchContext } from "./types";

// UI enterprise S4a housekeeping: old search ids (moved to the Appearance page) must resolve
// THROUGH the search path to the new control, not only in a lookup table.

const here = path.dirname(fileURLToPath(import.meta.url));
const en = JSON.parse(
  readFileSync(
    path.join(here, "../../../i18n/locales/en/translation.json"),
    "utf8",
  ),
) as Record<string, unknown>;
const lookup = (key: string): unknown =>
  key
    .split(".")
    .reduce<unknown>(
      (node, part) =>
        node && typeof node === "object"
          ? (node as Record<string, unknown>)[part]
          : undefined,
      en,
    );

const ctx: SearchContext = {
  isSectionEnabled: () => true,
  platform: "macos",
  settings: {},
} as unknown as SearchContext;

const LEGACY_IDS = [
  "general.theme",
  "general.customThemeText",
  "general.customThemeBackground",
  "general.customThemeAccent",
  "general.customThemeAccentSecondary",
  "general.customThemeWarning",
  "general.customThemeError",
  "general.chatOpacity",
];

const scenarios: Record<string, () => void> = {
  "every legacy id resolves to a live index entry through entryForId": () => {
    for (const id of LEGACY_IDS) {
      const entry = entryForId(SEARCH_ENTRIES, id);
      assert.ok(entry, `${id} resolves to an entry`);
      assert.equal(entry.id, resolveSearchEntryId(id));
      assert.equal(entry.section, "appearance", `${id} lands on Appearance`);
    }
  },

  "an unknown id resolves to nothing rather than a wrong row": () => {
    assert.equal(entryForId(SEARCH_ENTRIES, "general.doesNotExist"), undefined);
  },

  "the resolved legacy entry has a translated label the new control renders":
    () => {
      for (const id of LEGACY_IDS) {
        const entry = entryForId(SEARCH_ENTRIES, id);
        assert.ok(entry, id);
        assert.equal(typeof lookup(entry.labelKey), "string", `${id} label`);
      }
    },

  "the search path wires the alias: a reveal target for an old id is the new control":
    () => {
      const entry = entryForId(SEARCH_ENTRIES, "general.theme");
      assert.ok(entry);
      const hit = search(
        SEARCH_ENTRIES,
        String(lookup(entry.labelKey)),
        (key) => ({ local: String(lookup(key)), en: String(lookup(key)) }),
        ctx,
      ).find((candidate) => candidate.entry.id === entry.id);
      assert.ok(hit, "searching the Theme label finds the Appearance entry");
      const target = resolveTarget(hit, SEARCH_ENTRIES, ctx);
      assert.equal(target.section, "appearance");
      assert.equal(target.labelKey, entry.labelKey);
    },
};

let failed = 0;
for (const [name, run] of Object.entries(scenarios)) {
  try {
    run();
  } catch (error) {
    failed += 1;
    console.error(`FAIL: ${name}\n  ${(error as Error).message}`);
  }
}
if (failed > 0) throw new Error(`${failed} search alias scenarios failed`);
console.log(`search alias: ${Object.keys(scenarios).length} scenarios passed`);
