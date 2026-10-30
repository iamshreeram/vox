import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  shortcutEmptyModel,
  type ShortcutEmptyModel,
} from "./shortcutEmptyState";

// UI enterprise S4a (U7): the General page showed two identical "Vox Shortcuts / No shortcuts
// configured" rows (one per shortcut) when the binding list was empty. Each row now names its own
// shortcut and explains the empty state.

const here = path.dirname(fileURLToPath(import.meta.url));
const en = JSON.parse(
  readFileSync(
    path.join(here, "../../../src/i18n/locales/en/translation.json"),
    "utf8",
  ).replace(/^/, ""),
) as Record<string, unknown>;

const has = (key: string): boolean =>
  key
    .split(".")
    .reduce<unknown>(
      (node, part) =>
        node && typeof node === "object"
          ? (node as Record<string, unknown>)[part]
          : undefined,
      en,
    ) !== undefined;

const scenarios: Record<string, () => void> = {
  "each shortcut gets its own title key (no shared generic title)": () => {
    const transcribe: ShortcutEmptyModel = shortcutEmptyModel("transcribe");
    const cancel: ShortcutEmptyModel = shortcutEmptyModel("cancel");
    assert.notEqual(transcribe.titleKey, cancel.titleKey);
    assert.equal(
      transcribe.titleKey,
      "settings.general.shortcut.bindings.transcribe.name",
    );
    assert.equal(
      cancel.titleKey,
      "settings.general.shortcut.bindings.cancel.name",
    );
  },

  "the empty state explains what to do (a body key distinct from the title)":
    () => {
      const model = shortcutEmptyModel("transcribe");
      assert.equal(model.bodyKey, "settings.general.shortcut.noneHint");
      assert.notEqual(model.bodyKey, model.titleKey);
    },

  "every key the empty state uses exists in the English translation": () => {
    for (const id of ["transcribe", "cancel"]) {
      const model = shortcutEmptyModel(id);
      assert.ok(has(model.titleKey), `${model.titleKey} in en`);
      assert.ok(has(model.bodyKey), `${model.bodyKey} in en`);
    }
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
if (failed > 0)
  throw new Error(`${failed} shortcut empty-state scenarios failed`);
console.log(
  `shortcut empty state: ${Object.keys(scenarios).length} scenarios passed`,
);
