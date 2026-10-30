import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// UI enterprise R3-2: Tailwind v4 gradient utilities interpolate in oklab by default. The quality
// gate cannot sample an oklab gradient (unsupported paint), and colour differs across browsers.
// Every gradient must interpolate in sRGB: a gradient utility carries the `/srgb` modifier, and no
// emitted CSS (source or built) contains a gradient with `in oklab` / `in oklch`.

const here = path.dirname(fileURLToPath(import.meta.url));
const SRC = path.resolve(here, "..");
const REPO = path.resolve(here, "../..");

const walk = (dir: string): string[] =>
  readdirSync(dir).flatMap((name) => {
    const full = path.join(dir, name);
    if (statSync(full).isDirectory()) return walk(full);
    return /\.(tsx?|css)$/.test(name) && !/\.test\.tsx?$/.test(name)
      ? [full]
      : [];
  });

const sourceFiles = walk(SRC);
const rel = (file: string): string => path.relative(REPO, file);

// Gradient utilities as Tailwind v4 names them: bg-linear-*, bg-radial*, bg-conic*, and the v3
// spelling bg-gradient-to-*. Each must carry the /srgb modifier.
const GRADIENT_UTILITY =
  /(?<![\w-])bg-(?:linear|radial|conic|gradient-to)[\w-]*(?:\/[\w.[\]-]+)?/g;

const failures: string[] = [];

for (const file of sourceFiles) {
  const text = readFileSync(file, "utf8");
  for (const match of text.matchAll(GRADIENT_UTILITY)) {
    if (!match[0].endsWith("/srgb")) {
      failures.push(`${rel(file)}: gradient utility "${match[0]}" lacks /srgb`);
    }
  }
  if (/\bin\s+(oklab|oklch)\b/.test(text) && /gradient/.test(text)) {
    const line = text
      .split("\n")
      .find((l) => /gradient/.test(l) && /\bin\s+(oklab|oklch)\b/.test(l));
    if (line)
      failures.push(`${rel(file)}: gradient in oklab/oklch: ${line.trim()}`);
  }
}

// Built CSS, when a build exists: no gradient may interpolate in oklab or oklch.
const DIST = path.join(REPO, "dist", "assets");
if (existsSync(DIST)) {
  for (const name of readdirSync(DIST)) {
    if (!name.endsWith(".css")) continue;
    const css = readFileSync(path.join(DIST, name), "utf8");
    const bad = css.match(/-gradient\([^;{}]*\bin oklab\b[^;{}]*/g) ?? [];
    for (const hit of bad) {
      failures.push(`dist/assets/${name}: ${hit.slice(0, 120)}`);
    }
  }
}

const scenarios: Record<string, () => void> = {
  "every gradient utility in src carries the /srgb modifier": () => {
    const missing = failures.filter((f) => f.includes("lacks /srgb"));
    assert.deepEqual(missing, []);
  },

  "no gradient in src or built CSS interpolates in oklab or oklch": () => {
    const oklab = failures.filter((f) => !f.includes("lacks /srgb"));
    assert.deepEqual(oklab, []);
  },

  "the active sidebar item and the primary button use the sRGB gradient":
    () => {
      const button = readFileSync(
        path.join(SRC, "components/ui/Button.tsx"),
        "utf8",
      );
      const sidebar = readFileSync(
        path.join(SRC, "components/sidebarTheme.ts"),
        "utf8",
      );
      assert.match(button, /bg-linear-to-r\/srgb/);
      assert.match(sidebar, /bg-linear-to-r\/srgb/);
    },
};

let failed = 0;
for (const [name, run] of Object.entries(scenarios)) {
  try {
    run();
    console.log(`ok - ${name}`);
  } catch (error) {
    failed += 1;
    console.error(`FAIL - ${name}`);
    console.error(error);
  }
}
if (failed > 0) {
  console.error(`${failed} scenario(s) failed`);
  process.exit(1);
}
console.log("gradientColorSpace ok");
