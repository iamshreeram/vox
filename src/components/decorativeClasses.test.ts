import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// UI enterprise S4a (item 3): no decorative palette classes in src/components/** outside the
// sidebar's vibrant map. Status and accent colour come from the shared tokens instead.

const here = path.dirname(fileURLToPath(import.meta.url));
const DECORATIVE =
  /(?<![\w-])(?:text|bg|border|ring|from|to|via|fill|stroke)-(?:sky|blue|violet|orange|rose|indigo|pink|fuchsia|emerald|amber)-\d{2,3}(?:\/\d+)?(?![\w-])/g;
const EXEMPT = new Set([path.join(here, "sidebarTheme.ts")]);

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const full = path.join(dir, name);
    if (statSync(full).isDirectory()) return walk(full);
    if (!/\.tsx?$/.test(name) || /\.test\.tsx?$/.test(name)) return [];
    return EXEMPT.has(full) ? [] : [full];
  });
}

const offenders: string[] = [];
for (const file of walk(here)) {
  const lines = readFileSync(file, "utf8").split("\n");
  lines.forEach((line, index) => {
    for (const match of line.matchAll(DECORATIVE)) {
      offenders.push(`${path.relative(here, file)}:${index + 1} ${match[0]}`);
    }
  });
}

assert.deepEqual(
  offenders,
  [],
  `decorative palette classes outside sidebarTheme.ts:\n  ${offenders.join("\n  ")}`,
);
console.log("decorative classes: 1 scenario passed (0 offenders)");
