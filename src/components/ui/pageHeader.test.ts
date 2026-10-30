import assert from "node:assert/strict";
import { readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// UI enterprise S4a (U9): one shared PageHeader (title + optional description) on every settings
// page. The page title is the page's only h1; sections use h2.

const here = path.dirname(fileURLToPath(import.meta.url));
const src = path.join(here, "../..");
const read = (file: string): string =>
  readFileSync(path.join(src, file), "utf8");

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const full = path.join(dir, name);
    if (statSync(full).isDirectory()) return walk(full);
    return /\.tsx$/.test(name) && !/\.test\.tsx$/.test(name) ? [full] : [];
  });
}

const scenarios: Record<string, () => void> = {
  "a shared PageHeader component exists and renders the page h1": () => {
    const header = read("components/ui/PageHeader.tsx");
    assert.match(header, /export const PageHeader/);
    assert.match(header, /<h1\b/);
    assert.match(header, /description/);
  },

  "App renders the PageHeader for every settings section": () => {
    const app = read("App.tsx");
    assert.match(app, /import \{ PageHeader \}/);
    assert.match(app, /<PageHeader/);
  },

  "no settings component renders its own h1 (the header is the only h1)":
    () => {
      const offenders = walk(path.join(src, "components/settings"))
        .filter((file) => /<h1\b/.test(readFileSync(file, "utf8")))
        .map((file) => path.relative(src, file));
      assert.deepEqual(offenders, [], `own h1 in: ${offenders.join(", ")}`);
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
if (failed > 0) throw new Error(`${failed} page header scenarios failed`);
console.log(`page header: ${Object.keys(scenarios).length} scenarios passed`);
