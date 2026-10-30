import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// UI enterprise S4b (DESIGN D4 U10, U12, U13). Static scans over src/**:
//  - U10: no `outline-none` / `outline: none` without a visible focus replacement on the same
//    element (TSX line) or a :focus-visible rule in the same stylesheet (CSS).
//  - U12: every stylesheet with animation/transition/@keyframes is covered by a reduced-motion
//    override, either in its own file or in the stylesheet graph of an entry that imports the
//    shared theme.css (which carries the global override).
//  - U13: every icon-only <button>/<Button>/<IconButton> has an accessible name (aria-label,
//    aria-labelledby or title) or visible text.

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
const read = (file: string): string => readFileSync(file, "utf8");
const rel = (file: string): string => path.relative(srcRoot, file);

// ---- U10: outline removal needs a focus replacement -------------------------------------
function outlineOffenders(): string[] {
  const offenders: string[] = [];
  for (const file of files) {
    const text = read(file);
    if (file.endsWith(".css")) {
      if (
        /outline:\s*(none|0)\b/.test(text) &&
        !text.includes(":focus-visible")
      ) {
        offenders.push(
          `${rel(file)}: outline none without a :focus-visible rule`,
        );
      }
      continue;
    }
    text.split("\n").forEach((line, index) => {
      if (
        /outline-none|outline-0\b/.test(line) &&
        !line.includes("focus-visible:")
      ) {
        offenders.push(
          `${rel(file)}:${index + 1} outline removed without focus-visible replacement`,
        );
      }
    });
  }
  return offenders;
}

// ---- U12: reduced-motion coverage ----------------------------------------------------------
const MOTION = /@keyframes|animation\s*:|transition\s*:/;
const IMPORT_CSS =
  /(?:import\s+["']([^"']+\.css)["']|@import\s+["']([^"']+\.css)["'])/g;

// `file` is an absolute stylesheet path; every relative import/@import inside it is followed.
function cssGraph(file: string, seen = new Set<string>()): Set<string> {
  if (seen.has(file)) return seen;
  seen.add(file);
  for (const match of read(file).matchAll(IMPORT_CSS)) {
    const target = match[1] ?? match[2];
    if (target.startsWith(".")) {
      cssGraph(path.resolve(path.dirname(file), target), seen);
    }
  }
  return seen;
}

function globallyCovered(): Set<string> {
  const covered = new Set<string>();
  for (const file of files.filter((f) => f.endsWith(".css"))) {
    if (/@import\s+["'][^"']*(theme|App)\.css["']/.test(read(file))) {
      cssGraph(file).forEach((item) => covered.add(item));
    }
  }
  for (const file of files) {
    if (file.endsWith(".css")) continue;
    const text = read(file);
    if (!/theme\.css["']|App\.css["']/.test(text)) continue;
    for (const match of text.matchAll(/import\s+["']([^"']+\.css)["']/g)) {
      if (match[1].startsWith(".")) {
        cssGraph(path.resolve(path.dirname(file), match[1])).forEach((item) =>
          covered.add(item),
        );
      }
    }
  }
  return covered;
}

function motionOffenders(): string[] {
  const covered = globallyCovered();
  const offenders: string[] = [];
  for (const file of files.filter((f) => f.endsWith(".css"))) {
    const text = read(file);
    if (!MOTION.test(text)) continue;
    if (text.includes("prefers-reduced-motion")) continue;
    if (covered.has(file)) continue;
    offenders.push(`${rel(file)} animates without a reduced-motion override`);
  }
  return offenders;
}

// ---- U13: icon-only buttons need an accessible name ----------------------------------------
const BUTTON_OPEN = /<(button|Button|IconButton)(?=[\s>/])/g;

function openTagEnd(text: string, start: number): number {
  let depth = 0;
  let quote: string | null = null;
  for (let i = start; i < text.length; i += 1) {
    const ch = text[i];
    if (quote) {
      if (ch === quote) quote = null;
      continue;
    }
    if (depth === 0 && (ch === '"' || ch === "'")) quote = ch;
    else if (ch === "{") depth += 1;
    else if (ch === "}") depth -= 1;
    else if (ch === ">" && depth === 0) return i;
  }
  return -1;
}

function iconButtonOffenders(): string[] {
  const offenders: string[] = [];
  for (const file of files.filter((f) => f.endsWith(".tsx"))) {
    const text = read(file);
    for (const match of text.matchAll(BUTTON_OPEN)) {
      const start = match.index ?? 0;
      const end = openTagEnd(text, start + match[0].length);
      if (end < 0) continue;
      const openTag = text.slice(start, end + 1);
      if (openTag.endsWith("/>")) continue;
      const name = match[1];
      const close = text.indexOf(`</${name}>`, end);
      if (close < 0) continue;
      const body = text.slice(end + 1, close);
      if (!body.includes("<")) continue;
      const textOnly = body.replace(/<[^>]*\/>/g, "").replace(/<[^>]*>/g, "");
      const named = /aria-label|aria-labelledby|\btitle=/.test(openTag);
      if (!named && textOnly.trim() === "" && !body.includes("{")) {
        const line = text.slice(0, start).split("\n").length;
        offenders.push(
          `${rel(file)}:${line} icon-only <${name}> without aria-label/title`,
        );
      }
    }
  }
  return offenders;
}

const scenarios: Record<string, () => void> = {
  "no outline removal without a focus-visible replacement": () => {
    assert.deepEqual(outlineOffenders(), []);
  },
  "every animated stylesheet has a reduced-motion override": () => {
    assert.deepEqual(motionOffenders(), []);
  },
  "every icon-only button has an accessible name": () => {
    assert.deepEqual(iconButtonOffenders(), []);
  },
  // UI enterprise R1b: controls forward their row label as the accessible name.
  "ToggleSwitch names its checkbox from the row label": () => {
    const text = read(path.join(srcRoot, "components/ui/ToggleSwitch.tsx"));
    assert.match(text, /aria-label=\{label\}/);
  },
  "Slider names its range input from the row label": () => {
    const text = read(path.join(srcRoot, "components/ui/Slider.tsx"));
    assert.match(text, /aria-label=\{label\}/);
  },
  "Select forwards an accessible name to react-select": () => {
    const text = read(path.join(srcRoot, "components/ui/Select.tsx"));
    assert.match(text, /"aria-label": ariaLabel \?\? placeholder/);
  },
  "ResetButton has a translated default accessible name": () => {
    const text = read(path.join(srcRoot, "components/ui/ResetButton.tsx"));
    assert.match(text, /aria-label=\{ariaLabel \?\? t\("common\.reset"\)\}/);
  },
  "every raw <input> has an accessible name source": () => {
    const offenders: string[] = [];
    for (const file of files.filter((f) => f.endsWith(".tsx"))) {
      const text = read(file);
      // Prop-forwarding primitives (`{...props}`) take their name from the caller.
      if (/\{\.\.\.props\}/.test(text)) continue;
      for (const match of text.matchAll(/<input(?=[\s>/])/g)) {
        const start = match.index ?? 0;
        const end = openTagEnd(text, start + match[0].length);
        if (end < 0) continue;
        const openTag = text.slice(start, end + 1);
        if (/type=["'](hidden)["']/.test(openTag)) continue;
        if (/aria-label|aria-labelledby|\btitle=|\bid=/.test(openTag)) continue;
        const line = text.slice(0, start).split("\n").length;
        offenders.push(`${rel(file)}:${line} <input> without a name source`);
      }
    }
    assert.deepEqual(offenders, []);
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
console.log("a11yStatic ok");
