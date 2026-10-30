// UI enterprise S2 (A3/A4): the orb's face accent is derived from the accent tokens. Only the
// alarm colour of the `concerned` face stays a fixed hex (it is a state colour, not an accent).
import assert from "node:assert/strict";
import { ORB_ACCENT, orbAccentStyle } from "./orbModel";

const FACES = [
  "idle",
  "listening",
  "thinking",
  "acting",
  "speaking",
  "concerned",
];

for (const face of FACES) {
  assert.ok(ORB_ACCENT[face], `no accent for ${face}`);
}
assert.equal(orbAccentStyle("idle")["--chat-accent"], "var(--color-accent-a)");
assert.equal(
  orbAccentStyle("listening")["--chat-accent"],
  "var(--color-accent-b)",
);
assert.equal(
  orbAccentStyle("speaking")["--chat-accent"],
  "var(--color-accent-a)",
);
assert.equal(orbAccentStyle("concerned")["--chat-accent"], "#FF6B6B");
assert.equal(
  orbAccentStyle("unknown-face")["--chat-accent"],
  "var(--color-accent-a)",
);

for (const face of FACES.filter((f) => f !== "concerned")) {
  assert.match(
    ORB_ACCENT[face],
    /^var\(--color-accent-[ab]\)$/,
    `${face} must follow the accent`,
  );
}

console.log("orb accent: all assertions passed");
