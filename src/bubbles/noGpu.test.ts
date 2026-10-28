// G4a spec: no GPU primitives in the orb and bubble pages (DESIGN.md R6-1 and A29). Scans every
// css, ts, tsx and html file under src/orb and src/bubbles (test files skipped) and fails on
// canvas, WebGL, three.js, backdrop-filter, filter blur, or box-shadow inside a @keyframes block.
// Comments are stripped before scanning so that a comment about the rule does not trip it.
// Authoritative: do not weaken a pattern; report any hit in the existing code instead of editing it.
// Run with `bun run test:no-gpu`.

import assert from "node:assert/strict";
import { readdirSync, readFileSync, existsSync } from "node:fs";
import { join, extname, relative } from "node:path";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url)); // src/bubbles/
const repoSrc = join(here, "..");
const SCAN_DIRS = [join(repoSrc, "orb"), join(here)];
const EXTENSIONS = new Set([".css", ".ts", ".tsx", ".html"]);

type Violation = { label: string; index: number };

/** Removes block and line comments so that prose about a forbidden word is not flagged. */
export function stripComments(text: string): string {
  return text
    .replace(/\/\*[\s\S]*?\*\//g, " ")
    .replace(/(^|[^:\\])\/\/[^\n]*/g, "$1 ");
}

/** Body text of every @keyframes block (brace matched), comments already stripped. */
export function keyframesBodies(text: string): string[] {
  const bodies: string[] = [];
  let at = text.indexOf("@keyframes");
  while (at !== -1) {
    const open = text.indexOf("{", at);
    if (open === -1) break;
    let depth = 0;
    let close = -1;
    for (let i = open; i < text.length; i += 1) {
      if (text[i] === "{") depth += 1;
      if (text[i] === "}") {
        depth -= 1;
        if (depth === 0) {
          close = i;
          break;
        }
      }
    }
    if (close === -1) break;
    bodies.push(text.slice(open, close + 1));
    at = text.indexOf("@keyframes", close);
  }
  return bodies;
}

const FORBIDDEN: Array<[string, RegExp]> = [
  ["canvas element", /<canvas\b/i],
  ["canvas context", /getContext\s*\(/],
  ["WebGL", /webgl/i],
  [
    "three.js",
    /["'`]three(?:\/[^"'`]*)?["'`]|\bthree\.js\b|from\s+["']three|\bTHREE\./,
  ],
  ["backdrop-filter", /backdrop-filter/i],
  ["blur filter", /filter\s*:\s*[^;{}]*blur\s*\(/i],
];

/** Every violation in one source text. */
export function scanText(raw: string): Violation[] {
  const text = stripComments(raw);
  const found: Violation[] = [];
  for (const [label, pattern] of FORBIDDEN) {
    const match = pattern.exec(text);
    if (match) found.push({ label, index: match.index });
  }
  for (const body of keyframesBodies(text)) {
    if (/box-shadow/i.test(body))
      found.push({ label: "box-shadow inside @keyframes", index: 0 });
  }
  return found;
}

function walk(dir: string): string[] {
  if (!existsSync(dir)) return [];
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...walk(full));
    else out.push(full);
  }
  return out;
}

function scannedFiles(): string[] {
  return SCAN_DIRS.flatMap(walk).filter(
    (file) => EXTENSIONS.has(extname(file)) && !file.endsWith(".test.ts"),
  );
}

const scenarios: Record<string, () => void> = {
  "the scanner flags a canvas element": () => {
    assert.ok(scanText('<canvas id="orb"></canvas>').length > 0);
  },

  "the scanner flags a canvas context request": () => {
    assert.ok(scanText("const ctx = el.getContext('2d');").length > 0);
  },

  "the scanner flags WebGL and three.js": () => {
    assert.ok(scanText("const gl = c.getContext('webgl2');").length > 0);
    assert.ok(scanText("import * as THREE from 'three';").length > 0);
  },

  "the scanner flags backdrop-filter and blur filters": () => {
    assert.ok(scanText(".a { backdrop-filter: blur(4px); }").length > 0);
    assert.ok(scanText(".a { filter: blur(4px); }").length > 0);
  },

  "the scanner flags box-shadow only inside @keyframes": () => {
    assert.ok(
      scanText("@keyframes pulse { 50% { box-shadow: 0 0 9px red; } }").length >
        0,
    );
    assert.equal(
      scanText(".glow { box-shadow: 0 0 9px red; }").length,
      0,
      "a static box-shadow is not an animation",
    );
  },

  "the scanner accepts transform and opacity animation and comments about the rules":
    () => {
      assert.equal(
        scanText(
          "/* no WebGL, no canvas, no backdrop-filter here */\n" +
            "@keyframes enter { from { opacity: 0; transform: translateY(8px); } }\n" +
            "// filter: blur( would be forbidden in code\n",
        ).length,
        0,
      );
    },

  "no GPU primitives in src/orb or src/bubbles": () => {
    const files = scannedFiles();
    assert.ok(
      files.length > 0,
      "scan found no files under src/orb or src/bubbles",
    );
    const problems: string[] = [];
    for (const file of files) {
      const hits = scanText(readFileSync(file, "utf8"));
      for (const hit of hits) {
        problems.push(`${relative(repoSrc, file)}: ${hit.label}`);
      }
    }
    assert.deepEqual(
      problems,
      [],
      `GPU primitives found:\n${problems.join("\n")}`,
    );
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
console.log("noGpu ok");
