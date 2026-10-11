import assert from "node:assert/strict";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

// Regression guard: every setting a UI component writes with
// `updateSetting("<key>", ...)` MUST have an entry in `settingUpdaters`
// (src/stores/settingsStore.ts). Without one, `updateSetting` only updates the
// in-memory store and logs "No handler for setting" -- the toggle looks like it
// works but the backend never hears about it and the change is lost on reload.
// That is exactly how Voice Commands, Memory and Agent Bridge silently failed
// to persist.

// Keys `updateSetting` deliberately handles without an updater (see the
// `key !== "bindings" && key !== "selected_model"` branch in the store).
const HANDLED_ELSEWHERE = new Set(["bindings", "selected_model"]);

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) walk(path, out);
    else if (path.endsWith(".tsx")) out.push(path);
  }
  return out;
}

const storeSource = readFileSync("src/stores/settingsStore.ts", "utf8");
const start = storeSource.indexOf("const settingUpdaters");
assert.ok(start >= 0, "could not find `const settingUpdaters` in settingsStore.ts");
const end = storeSource.indexOf("\n};", start);
assert.ok(end > start, "could not find the end of the settingUpdaters object");
const region = storeSource.slice(start, end);
const updaterKeys = new Set(
  [...region.matchAll(/^ {2}([a-z_0-9]+):/gm)].map((m) => m[1]),
);
assert.ok(updaterKeys.size > 30, `parsed suspiciously few updaters: ${updaterKeys.size}`);

const used = new Map<string, string[]>();
for (const file of walk("src/components")) {
  const source = readFileSync(file, "utf8");
  for (const match of source.matchAll(/updateSetting\(\s*"([a-z_0-9]+)"/g)) {
    const list = used.get(match[1]) ?? [];
    list.push(file);
    used.set(match[1], list);
  }
}
assert.ok(used.size > 30, `parsed suspiciously few UI settings: ${used.size}`);

const missing = [...used.entries()]
  .filter(([key]) => !updaterKeys.has(key) && !HANDLED_ELSEWHERE.has(key))
  .map(([key, files]) => `${key} (used in ${files.join(", ")})`);

assert.deepEqual(
  missing,
  [],
  `settings written by the UI with no entry in settingUpdaters (they will not persist):\n  ${missing.join("\n  ")}`,
);

// The guard itself must be able to fail: prove the parser sees real keys.
for (const key of ["theme", "experimental_enabled", "audio_feedback"]) {
  assert.ok(updaterKeys.has(key), `sanity: expected an updater for ${key}`);
}

console.log(`setting updaters coverage: ${used.size} UI settings all persist`);
