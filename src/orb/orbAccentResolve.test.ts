// Orb accent resolution (DESIGN Revision 3, D3 / A-C2 era). Authoritative: do not weaken an
// assertion; if a test is wrong, report it. Run with `bun run test:orb`.
//
// Contract (src/orb/orbAccentResolve.ts):
// Contract (src/orb/orbAccentResolve.ts), OrbColor-aware (A5):
//   resolveOrbAccent(snapshotAccent, orbColor, bucket) -> {orbColor, id, roles}
//     orbColor 'auto' | 'app' | accent; null/unknown -> 'auto'; id = the accent for 'app'/explicit,
//     AUTO_FACE_ACCENT.idle placeholder for 'auto' (ring colours resolve per face)
//   orbRingColour(face, pick, bucket) -> hex; concerned stays #FF6B6B
import assert from "node:assert/strict";
import {
  ACCENTS,
  ACCENT_IDS,
  ACCENT_SURFACE,
  DEFAULT_ACCENT,
  type AccentBucket,
  type AccentId,
} from "../styles/accents";
import { contrastRatio } from "../styles/wcag";
import { orbRingColour, resolveOrbAccent } from "./orbAccentResolve";
import { AUTO_FACE_ACCENT } from "./globePalette";

const BUCKETS: AccentBucket[] = ["light", "dark"];
const FACES = [
  "idle",
  "listening",
  "thinking",
  "acting",
  "speaking",
  "concerned",
];

// RE-POINTED (A5): the old null (follow the app accent) is now the orb colour 'app'. The same
// assertions hold for 'app'.
for (const app of ACCENT_IDS) {
  for (const bucket of BUCKETS) {
    const picked = resolveOrbAccent(app, "app", bucket);
    assert.equal(picked.id, app, `${app}/${bucket} follows the app accent`);
    assert.deepEqual(picked.roles, ACCENTS[app][bucket]);
  }
}

// RE-POINTED (A5): null -> 'auto'. Auto is per face, so the pick names the idle placeholder and
// the ring colour of each face is checked in the auto block at the end of this file.
for (const app of ACCENT_IDS) {
  for (const bucket of BUCKETS) {
    const picked = resolveOrbAccent(app, "auto", bucket);
    assert.equal(picked.orbColor, "auto", `${app}/${bucket} is auto`);
    assert.equal(picked.id, AUTO_FACE_ACCENT.idle);
  }
}

// Some(accent) wins over the app accent, for all 6 x 6 pairs and both buckets.
for (const app of ACCENT_IDS) {
  for (const orb of ACCENT_IDS) {
    for (const bucket of BUCKETS) {
      const picked = resolveOrbAccent(app, orb, bucket);
      assert.equal(picked.id, orb, `${orb} wins over ${app}/${bucket}`);
      assert.deepEqual(picked.roles, ACCENTS[orb][bucket]);
    }
  }
}

// RE-POINTED (A5): an unknown orb colour (a stale or garbage value) now reads as 'auto', not as
// the app accent.
const GARBAGE = [
  "teal",
  "",
  "AMBER",
  "__proto__",
  "constructor",
  null,
] as const;
for (const garbage of GARBAGE) {
  for (const app of ACCENT_IDS) {
    for (const bucket of BUCKETS) {
      const picked = resolveOrbAccent(app, garbage as never, bucket);
      assert.equal(picked.orbColor, "auto", `garbage ${garbage} -> auto`);
    }
  }
}

// An unknown app accent falls back to the default accent. RE-POINTED (A5): the probe uses 'app'
// (follow the app accent), because 'auto' names no app accent.
assert.equal(
  resolveOrbAccent("nope" as AccentId, "app", "dark").id,
  DEFAULT_ACCENT,
);

// Ring colour: the state slot picks fillA (accent-a faces) or fillB (accent-b faces) on dark;
// on light the accent text role is used for both slots (fillA is 2.08:1 on the light surface).
for (const accent of ACCENT_IDS) {
  for (const bucket of BUCKETS) {
    const pick = resolveOrbAccent(DEFAULT_ACCENT, accent, bucket);
    const roles = ACCENTS[accent][bucket];
    if (bucket === "dark") {
      assert.equal(orbRingColour("idle", pick, bucket), roles.fillA);
      assert.equal(orbRingColour("thinking", pick, bucket), roles.fillA);
      assert.equal(orbRingColour("speaking", pick, bucket), roles.fillA);
      assert.equal(orbRingColour("listening", pick, bucket), roles.fillB);
      assert.equal(orbRingColour("acting", pick, bucket), roles.fillB);
    } else {
      for (const face of FACES.filter((f) => f !== "concerned")) {
        assert.equal(orbRingColour(face, pick, bucket), roles.text);
      }
    }
    assert.equal(orbRingColour("concerned", pick, bucket), "#FF6B6B");
  }
}

// Ring contrast: every non-concerned ring colour is >= 3:1 on its bucket surface, all accents.
for (const accent of ACCENT_IDS) {
  for (const bucket of BUCKETS) {
    const pick = resolveOrbAccent(DEFAULT_ACCENT, accent, bucket);
    for (const face of FACES.filter((f) => f !== "concerned")) {
      const colour = orbRingColour(face, pick, bucket);
      const ratio = contrastRatio(colour, ACCENT_SURFACE[bucket]);
      assert.ok(
        ratio >= 3,
        `${accent}/${bucket} ${face} ring ${colour}: ${ratio.toFixed(2)}:1`,
      );
    }
  }
}

// Auto (D1, A4): each face's ring uses its state accent (AUTO_FACE_ACCENT); concerned stays red.
// Light uses the accent text role; dark uses fillB for the accent-b faces (listening, acting).
const AUTO_FACES = [
  "idle",
  "listening",
  "thinking",
  "acting",
  "speaking",
  "sleep",
  "concerned",
] as const;
const AUTO_ACCENT: Record<string, AccentId> = {
  idle: "blue",
  listening: "emerald",
  thinking: "violet",
  acting: "rose",
  speaking: "amber",
  sleep: "slate",
};
const ACCENT_B_FACES = ["listening", "acting"];
for (const bucket of BUCKETS) {
  const pick = resolveOrbAccent(DEFAULT_ACCENT, "auto", bucket);
  for (const face of AUTO_FACES) {
    const colour = orbRingColour(face, pick, bucket);
    if (face === "concerned") {
      assert.equal(colour, "#FF6B6B", `concerned ring ${bucket}`);
      continue;
    }
    const roles = ACCENTS[AUTO_ACCENT[face] as AccentId][bucket];
    const expected =
      bucket === "light"
        ? roles.text
        : ACCENT_B_FACES.includes(face)
          ? roles.fillB
          : roles.fillA;
    assert.equal(colour, expected, `auto ${face}/${bucket}`);
  }
}

// Auto ring contrast: >= 3:1 on both bucket surfaces for every face except concerned (A5).
for (const bucket of BUCKETS) {
  const pick = resolveOrbAccent(DEFAULT_ACCENT, "auto", bucket);
  for (const face of AUTO_FACES.filter((f) => f !== "concerned")) {
    const colour = orbRingColour(face, pick, bucket);
    const ratio = contrastRatio(colour, ACCENT_SURFACE[bucket]);
    assert.ok(
      ratio >= 3,
      `auto ${face}/${bucket} ring ${colour}: ${ratio.toFixed(2)}:1`,
    );
  }
}

console.log("orb accent resolve: all assertions passed");
