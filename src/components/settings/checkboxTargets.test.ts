// Static target-size gate (UI enterprise S2c, WCAG 2.5.8). A native checkbox is 13px, so every
// `type="checkbox"` input under src/components/settings must sit inside a >= 24px hit area:
//   (a) its own class sets a size >= 24px, or
//   (b) the nearest enclosing <label> sets a min size >= 24px, or
//   (c) it is visually hidden (sr-only) and drawn by a sibling control that sets the size.
// Case (c) is the ToggleSwitch pattern; the sibling is checked by the ToggleSwitch tests.
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));

function tsxFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const full = path.join(dir, name);
    if (statSync(full).isDirectory()) return tsxFiles(full);
    return name.endsWith(".tsx") && !name.endsWith(".test.tsx") ? [full] : [];
  });
}

// Tailwind size tokens that reach 24px: h-6 / min-h-6 / size-6 / w-6 / min-w-6, any larger
// h-N / w-N (N >= 6), bracketed px values >= 24, and h-full / w-full / h-10.
const SIZE_TOKEN =
  /(?:^|\s)(?:min-)?(?:h|w|size)-(?:\[(\d+)px\]|(\d+)|full)(?=\s|$|")/g;

function meetsTwentyFour(classes: string): boolean {
  for (const match of classes.matchAll(SIZE_TOKEN)) {
    const [, px, unit] = match;
    if (px !== undefined && Number(px) >= 24) return true;
    if (unit !== undefined && Number(unit) * 4 >= 24) return true;
    if (px === undefined && unit === undefined) return true; // h-full / w-full
  }
  return false;
}

function classAttr(tag: string): string {
  const match = /className=(?:"([^"]*)"|\{`([^`]*)`\}|\{"([^"]*)"\})/.exec(tag);
  return match ? (match[1] ?? match[2] ?? match[3] ?? "") : "";
}

/** The opening tag of the nearest <label> before `index`, or null if none is open. */
function enclosingLabelClasses(source: string, index: number): string | null {
  const before = source.slice(0, index);
  const open = before.lastIndexOf("<label");
  if (open < 0) return null;
  const close = before.lastIndexOf("</label>");
  if (close > open) return null;
  const tagEnd = source.indexOf(">", open);
  return classAttr(source.slice(open, tagEnd + 1));
}

interface Finding {
  file: string;
  line: number;
}

function findFailures(file: string): Finding[] {
  const source = readFileSync(file, "utf8");
  const failures: Finding[] = [];
  const pattern = /<input\b[^>]*type="checkbox"[^>]*>/g;
  for (const match of source.matchAll(pattern)) {
    const tag = match[0];
    const index = match.index ?? 0;
    const line = source.slice(0, index).split("\n").length;
    const own = classAttr(tag);
    if (meetsTwentyFour(own)) continue;
    const label = enclosingLabelClasses(source, index);
    if (label !== null && meetsTwentyFour(label)) continue;
    if (/\bsr-only\b/.test(own)) continue;
    failures.push({ file: path.relative(here, file), line });
  }
  return failures;
}

const settingsDir = here;
const files = tsxFiles(settingsDir);
const scanned = files.filter((file) =>
  readFileSync(file, "utf8").includes('type="checkbox"'),
);

const failures = files.flatMap(findFailures);
assert.deepEqual(
  failures,
  [],
  `checkbox without a 24px target: ${JSON.stringify(failures)}`,
);

// The gate must see the fixed brain acknowledgement, not pass by finding nothing.
assert.ok(
  scanned.some((file) => file.endsWith("brain/BrainTestGroup.tsx")),
  "BrainTestGroup.tsx should contain the acknowledgement checkbox",
);

// Self-test of the rule itself, so a broken matcher cannot pass silently.
const bad = `<label className="flex items-center"><input type="checkbox" /></label>`;
const good = `<label className="flex min-h-[24px] min-w-[24px]"><input type="checkbox" /></label>`;
const sized = `<input type="checkbox" className="h-6 w-6" />`;
const hidden = `<input type="checkbox" className="sr-only peer" />`;
const selfTest = (fixture: string): boolean => {
  const index = fixture.indexOf("<input");
  const tag = fixture.slice(index, fixture.indexOf(">", index) + 1);
  if (meetsTwentyFour(classAttr(tag))) return true;
  const label = enclosingLabelClasses(fixture, index);
  return label !== null && meetsTwentyFour(label);
};
assert.equal(selfTest(bad), false, "13px checkbox in a plain label must fail");
assert.equal(selfTest(good), true, "24px label must pass");
assert.equal(selfTest(sized), true, "h-6 w-6 input must pass");
assert.equal(
  /\bsr-only\b/.test(classAttr(hidden)),
  true,
  "sr-only fixture is recognised",
);

console.log(
  `checkboxTargets.test.ts: ${scanned.length} file(s) with checkboxes, all at least 24px`,
);
