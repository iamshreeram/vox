import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// UI enterprise R1b (policy: no rendered text below 12px). Static scan over src/**:
//  - every CSS `font-size` in px or rem is >= 12px (rem resolved against the 15px app root);
//  - every Tailwind arbitrary `text-[Npx]` / `text-[Nrem]` in TSX/TS is >= 12px;
//  - the Tailwind `text-xs` step (declared in App.css @theme) is >= 12px.
// The allow-list is deliberately empty: add an entry only with a written, user-visible reason.

const APP_ROOT_PX = 15; // :root font-size in src/App.css
const MIN_PX = 12;
const ALLOW_LIST: string[] = [];

const here = path.dirname(fileURLToPath(import.meta.url));
const srcRoot = path.resolve(here, "..");

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const full = path.join(dir, name);
    if (statSync(full).isDirectory()) return walk(full);
    if (/\.test\.(ts|tsx)$/.test(name)) return [];
    return /\.(ts|tsx|css)$/.test(name) ? [full] : [];
  });
}

const files = walk(srcRoot);
const rel = (file: string): string => path.relative(srcRoot, file);

function toPx(value: number, unit: string): number {
  return unit === "rem" ? value * APP_ROOT_PX : value;
}

function undersized(file: string): string[] {
  const offenders: string[] = [];
  const text = readFileSync(file, "utf8");
  const pattern = file.endsWith(".css")
    ? /font-size:\s*([\d.]+)(px|rem)\b/g
    : /text-\[([\d.]+)(px|rem)\]/g;
  for (const match of text.matchAll(pattern)) {
    const px = toPx(Number(match[1]), match[2]);
    const where = `${rel(file)}: ${match[0]}`;
    if (px < MIN_PX && !ALLOW_LIST.includes(where)) {
      offenders.push(`${where} resolves to ${px}px`);
    }
  }
  return offenders;
}

function textXsPx(): number {
  const app = readFileSync(path.join(srcRoot, "App.css"), "utf8");
  const match = app.match(/--text-xs:\s*([\d.]+)(px|rem)\s*;/);
  assert.ok(match, "App.css @theme must declare --text-xs");
  return toPx(Number(match[1]), match[2]);
}

const scenarios: Record<string, () => void> = {
  "no CSS font-size below 12px": () => {
    const offenders = files
      .filter((file) => file.endsWith(".css"))
      .flatMap(undersized);
    assert.deepEqual(offenders, []);
  },
  "no Tailwind arbitrary text size below 12px": () => {
    const offenders = files
      .filter((file) => /\.(ts|tsx)$/.test(file))
      .flatMap(undersized);
    assert.deepEqual(offenders, []);
  },
  "text-xs resolves to at least 12px at the app root size": () => {
    assert.ok(
      textXsPx() >= MIN_PX,
      `text-xs is ${textXsPx()}px, expected >= ${MIN_PX}px`,
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
console.log("typeFloor ok");
