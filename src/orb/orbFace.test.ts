// G3d spec (DESIGN.md 'Dim-orb fix'): a resting orb must stay visible on dark backgrounds, so the
// orb page never shows the dim 'sleep' face. Pure checks for orbFace(face) in ./orbModel.ts and
// the wiring of OrbApp.tsx. Run with `bun run test:orb:face`. Authoritative: do not weaken an
// assertion; if a test is wrong, report it.
//
// Contract (src/orb/orbModel.ts):
//   orbFace(face: string): string
//     'sleep' -> 'idle'; idle, concerned, listening, thinking, speaking, acting unchanged;
//     any other string (including '', 'SLEEP', 'Idle', 'dozing') -> 'idle'.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { orbFace } from "./orbModel";

const read = (relative: string): string =>
  readFileSync(new URL(relative, import.meta.url), "utf8");

// ---------- the mapping ----------
assert.equal(orbFace("sleep"), "idle", "the dim sleep face is shown as idle");

for (const face of [
  "idle",
  "concerned",
  "listening",
  "thinking",
  "speaking",
  "acting",
]) {
  assert.equal(orbFace(face), face, `${face} is unchanged`);
}

for (const unknown of ["", "nap", "SLEEP", "Idle", "dozing", "unknown"]) {
  assert.equal(orbFace(unknown), "idle", `unknown face "${unknown}" is idle`);
}

// The mapping is a function of the face only: repeated calls agree.
assert.equal(orbFace("sleep"), orbFace("sleep"));

// ---------- wiring: the page applies the mapping on every path that sets its face ----------
const orbModelSource = read("./orbModel.ts");
assert.match(
  orbModelSource,
  /export function orbFace\(face: string\): string/,
  "orbModel.ts exports orbFace(face: string): string",
);

const pageSource = read("./OrbApp.tsx");
assert.match(
  pageSource,
  /import \{[^}]*\borbFace\b[^}]*\} from "\.\/orbModel"/,
  "OrbApp.tsx imports orbFace from ./orbModel",
);
assert.match(
  pageSource,
  /\borbFace\(/,
  "OrbApp.tsx applies orbFace to the face it renders",
);

console.log("orbFace.test.ts: all assertions passed");
