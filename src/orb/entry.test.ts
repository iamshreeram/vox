// UI enterprise S2 (A2, A3): the orb and bubble pages stay listen-only, seed their appearance from
// the URL and follow `appearance` events through the shared consumer. Node fs only.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const read = (file: string) => readFileSync(`${here}${file}`, "utf8");

const BANNED = [
  "getAppSettings",
  "get_app_settings",
  "syncAppearanceFromSettings",
  "applyTheme(",
  "getStoredAppearance",
  "localStorage",
  "THEME_STORAGE_KEY",
  "invoke(",
  "commands.",
  "@/bindings",
  "@tauri-apps/plugin-",
];

const scenarios: Record<string, () => void> = {
  "orb entry: listen-only, no settings, no command, no storage": () => {
    const src = read("main.tsx");
    for (const banned of BANNED) {
      assert.ok(!src.includes(banned), `orb main.tsx must not use ${banned}`);
    }
    assert.ok(
      !/addEventListener\(\s*["']storage["']/.test(src),
      "no storage listener",
    );
  },
  "orb entry: seeds from the URL and follows appearance events via the consumer":
    () => {
      const src = read("main.tsx");
      assert.match(src, /from\s+["']\.\.\/lib\/appearance\/consumer["']/);
      assert.match(src, /parseSeed\(window\.location\.search\)/);
      assert.match(src, /listen\(\s*APPEARANCE_EVENT/);
      assert.match(src, /consumer\.accept\(/);
    },
  "orb entry: imports the shared tokens and the orb stylesheet explicitly":
    () => {
      const src = read("main.tsx");
      assert.match(src, /import\s+["']\.\.\/styles\/theme\.css["']/);
      assert.match(src, /import\s+["']\.\/orb\.css["']/);
    },
  "bubbles entry: listen-only, no settings, no command, no storage": () => {
    const src = read("../bubbles/main.tsx");
    for (const banned of BANNED) {
      assert.ok(
        !src.includes(banned),
        `bubbles main.tsx must not use ${banned}`,
      );
    }
  },
  "bubbles entry: seeds from the URL and follows appearance events via the consumer":
    () => {
      const src = read("../bubbles/main.tsx");
      assert.match(src, /from\s+["']\.\.\/lib\/appearance\/consumer["']/);
      assert.match(src, /parseSeed\(window\.location\.search\)/);
      assert.match(src, /listen\(\s*APPEARANCE_EVENT/);
      assert.match(src, /import\s+["']\.\.\/styles\/theme\.css["']/);
      assert.match(src, /import\s+["']\.\/bubbles\.css["']/);
    },
  "bubble colours come from the accent tokens, not literals": () => {
    const css = read("../bubbles/bubbles.css");
    const user = css.match(/\.bubble-user\s*\{([^}]*)\}/)?.[1] ?? "";
    const assistant = css.match(/\.bubble-assistant\s*\{([^}]*)\}/)?.[1] ?? "";
    assert.match(user, /background:\s*var\(--color-accent-a\)/);
    assert.match(user, /color:\s*var\(--color-on-accent\)/);
    assert.match(assistant, /background:\s*var\(--color-accent-soft\)/);
    assert.match(assistant, /color:\s*var\(--color-accent-text\)/);
    assert.doesNotMatch(
      css,
      /rgba\(\s*\d+\s*,\s*\d+\s*,\s*\d+\s*,\s*0?\.\d+\s*\)/,
    );
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
if (failed > 0) {
  throw new Error(
    `${failed} of ${Object.keys(scenarios).length} orb/bubble entry scenarios failed`,
  );
}
console.log(
  `orb and bubble entries: ${Object.keys(scenarios).length} scenarios passed`,
);
