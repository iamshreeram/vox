// DICTATION SWITCH SPEC (DESIGN-A Rev 1, written by ram-ai; workers must not edit this file).
// Source-level contract: the Agents settings page has its own "Send dictation to the agent" switch,
// default off, independent of the agent switch; it is wired through the store, bindings, the
// settings search index and every locale. Run: bun src/components/settings/acpDictationSwitch.test.ts
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const read = (rel: string) => readFileSync(join(root, rel), "utf8");

const page = read("src/components/settings/AcpAgentSettings.tsx");
assert.match(page, /acp_dictation_enabled/, "page reads acp_dictation_enabled");
assert.match(
  page,
  /settings\.advanced\.acpAgent\.dictation\.label/,
  "page uses the dictation label key",
);
assert.match(
  page,
  /settings\.advanced\.acpAgent\.dictation\.description/,
  "page uses the dictation description key",
);
assert.match(
  page,
  /getSetting\("acp_dictation_enabled"\)\s*\?\?\s*false/,
  "missing value means off",
);

const store = read("src/stores/settingsStore.ts");
assert.match(store, /acp_dictation_enabled/, "store has an updater");
assert.match(
  store,
  /change_acp_dictation_enabled_setting|changeAcpDictationEnabledSetting/,
);

const bindings = read("src/bindings.ts");
assert.match(bindings, /changeAcpDictationEnabledSetting/);
assert.match(
  bindings,
  /acp_dictation_enabled/,
  "AppSettings type has the field",
);

const index = read("src/components/settings/search/index/experimental.ts");
assert.match(index, /acpDictationToAgent/, "search index entry exists");

const locales = join(root, "src/i18n/locales");
const want = [
  "settings.advanced.acpAgent.dictation.label",
  "settings.advanced.acpAgent.dictation.description",
];
for (const dir of readdirSync(locales)) {
  const json = JSON.parse(read(`src/i18n/locales/${dir}/translation.json`));
  for (const key of want) {
    const value = key
      .split(".")
      .reduce<unknown>(
        (o, k) => (o as Record<string, unknown> | undefined)?.[k],
        json,
      );
    assert.equal(typeof value, "string", `${dir} has ${key}`);
    assert.ok((value as string).length > 0);
  }
}
const en = JSON.parse(read("src/i18n/locales/en/translation.json"));
assert.equal(
  en.settings.advanced.acpAgent.dictation.label,
  "Send dictation to the agent",
);
assert.match(
  en.settings.advanced.acpAgent.dictation.description,
  /pastes/i,
  "description says that dictation otherwise pastes",
);
console.log("acpDictationSwitch ok");
