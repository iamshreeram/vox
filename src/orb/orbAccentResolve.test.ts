// Orb accent resolution (DESIGN Revision 3, D3 / A-C2 era). Authoritative: do not weaken an
// assertion; if a test is wrong, report it. Run with `bun run test:orb`.
//
// Contract (src/orb/orbAccentResolve.ts):
//   resolveOrbAccent(snapshotAccent, orbAccent, bucket) -> {id, roles}
//     orbAccent ?? snapshotAccent (unknown orbAccent -> snapshotAccent; unknown snapshotAccent -> amber)
//     roles = ACCENTS[id][bucket]
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

const BUCKETS: AccentBucket[] = ["light", "dark"];
const FACES = [
  "idle",
  "listening",
  "thinking",
  "acting",
  "speaking",
  "concerned",
];

// None (null) follows the app accent, for every app accent and bucket.
for (const app of ACCENT_IDS) {
  for (const bucket of BUCKETS) {
    const picked = resolveOrbAccent(app, null, bucket);
    assert.equal(picked.id, app, `${app}/${bucket} follows the app accent`);
    assert.deepEqual(picked.roles, ACCENTS[app][bucket]);
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

// An unknown orb accent (a stale or garbage value) falls back to the app accent.
const GARBAGE = ["teal", "", "AMBER", "__proto__", "constructor"] as const;
for (const garbage of GARBAGE) {
  for (const app of ACCENT_IDS) {
    for (const bucket of BUCKETS) {
      const picked = resolveOrbAccent(app, garbage as AccentId, bucket);
      assert.equal(picked.id, app, `garbage ${garbage} -> ${app}`);
    }
  }
}

// An unknown app accent falls back to the default accent.
assert.equal(
  resolveOrbAccent("nope" as AccentId, null, "dark").id,
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

console.log("orb accent resolve: all assertions passed");
