// Globe palette (DESIGN Revision 3, G1, D1, D3, A4, A2b). Authoritative: do not weaken an
// assertion; if a test is wrong, report it. Run with `bun run test:orb`.
//
// Contract (src/orb/globePalette.ts):
//   GLOBE_STOPS: Record<AccentId, {hi, mid, lo}>   CONCERNED_STOPS (red)   SHINE
//   AUTO_FACE_ACCENT: Record<AvatarFace, AccentId>
//   resolveGlobe(face, orbColor, appAccent) -> table | app | concerned (A4 precedence)
//   appModeStops: CSS strings derived from --vox-app-accent (A2b)
import assert from "node:assert/strict";
import { ACCENT_IDS, type AccentId } from "../styles/accents";
import type { AvatarFace } from "../chat/chatModel";
import type { OrbColor } from "../bindings";
import {
  AUTO_FACE_ACCENT,
  CONCERNED_STOPS,
  GLOBE_STOPS,
  SHINE,
  appModeStops,
  resolveGlobe,
} from "./globePalette";

const FACES: AvatarFace[] = [
  "idle",
  "listening",
  "thinking",
  "acting",
  "speaking",
  "sleep",
  "concerned",
];

const MODES: OrbColor[] = ["auto", "app", ...ACCENT_IDS];

const HEX = /^#[0-9A-Fa-f]{6}$/;

function rgb(hex: string): [number, number, number] {
  return [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16)) as [
    number,
    number,
    number,
  ];
}

function luminance(hex: string): number {
  const [r, g, b] = rgb(hex).map((c) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  }) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function hueOf(hex: string): number {
  const [r, g, b] = rgb(hex).map((c) => c / 255) as [number, number, number];
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const d = max - min;
  assert.ok(d > 0, `${hex} has no hue (grey)`);
  let h: number;
  if (max === r) h = (((g - b) / d) % 6) as number;
  else if (max === g) h = (b - r) / d + 2;
  else h = (r - g) / d + 4;
  return (h * 60 + 360) % 360;
}

function hueDistance(a: number, b: number): number {
  const d = Math.abs(a - b) % 360;
  return Math.min(d, 360 - d);
}

// ---- A4 precedence: exhaustive expected values, 7 faces x 8 modes, app accent fixed. ----
// Auto state map (D1): idle blue, listening emerald, thinking violet, acting rose, speaking
// amber, sleep slate. Concerned is handled by precedence and never reaches this map.
const AUTO_EXPECT: Record<Exclude<AvatarFace, "concerned">, AccentId> = {
  idle: "blue",
  listening: "emerald",
  thinking: "violet",
  acting: "rose",
  speaking: "amber",
  sleep: "slate",
};

const APP_ACCENT: AccentId = "violet";

function expected(face: AvatarFace, mode: OrbColor) {
  if (face === "concerned") {
    return { kind: "concerned", stops: CONCERNED_STOPS };
  }
  if (mode === "auto") {
    const accent = AUTO_EXPECT[face];
    return { kind: "table", accent, stops: GLOBE_STOPS[accent] };
  }
  if (mode === "app") {
    return { kind: "app", accent: APP_ACCENT };
  }
  return { kind: "table", accent: mode, stops: GLOBE_STOPS[mode] };
}

for (const face of FACES) {
  for (const mode of MODES) {
    const label = `${face} x ${mode}`;
    check(`A4 ${label}`, () => {
      assert.deepEqual(
        resolveGlobe(face, mode, APP_ACCENT),
        expected(face, mode),
        label,
      );
    });
  }
}

check("A4 explicit blue keeps sleep blue, auto sleep is slate", () => {
  assert.equal(resolveGlobe("sleep", "blue", APP_ACCENT).kind, "table");
  assert.equal(
    (resolveGlobe("sleep", "blue", APP_ACCENT) as { accent: AccentId }).accent,
    "blue",
  );
  assert.equal(
    (resolveGlobe("sleep", "auto", APP_ACCENT) as { accent: AccentId }).accent,
    "slate",
  );
});

check("A4 concerned is red in every mode", () => {
  for (const mode of MODES) {
    assert.equal(resolveGlobe("concerned", mode, APP_ACCENT).kind, "concerned");
  }
});

check("A4 app mode returns kind app for every non-concerned face", () => {
  for (const face of FACES.filter((f) => f !== "concerned")) {
    assert.equal(resolveGlobe(face, "app", APP_ACCENT).kind, "app");
  }
});

// ---- Table sanity (D3, G1) ----
check(
  "GLOBE_STOPS: every stop is #RRGGBB for all accents and concerned",
  () => {
    for (const id of ACCENT_IDS) {
      for (const stop of Object.values(GLOBE_STOPS[id])) {
        assert.match(stop, HEX, `${id} stop ${stop}`);
      }
    }
    for (const stop of Object.values(CONCERNED_STOPS)) {
      assert.match(stop, HEX, `concerned stop ${stop}`);
    }
  },
);

check("GLOBE_STOPS: relative luminance is strictly hi > mid > lo", () => {
  const triples: [string, { hi: string; mid: string; lo: string }][] = [
    ...ACCENT_IDS.map(
      (id): [string, { hi: string; mid: string; lo: string }] => [
        id,
        GLOBE_STOPS[id],
      ],
    ),
    ["concerned", CONCERNED_STOPS],
  ];
  for (const [name, stops] of triples) {
    assert.ok(
      luminance(stops.hi) > luminance(stops.mid),
      `${name}: hi must be lighter than mid`,
    );
    assert.ok(
      luminance(stops.mid) > luminance(stops.lo),
      `${name}: mid must be lighter than lo`,
    );
  }
});

check("GLOBE_STOPS: amber is exactly the legacy gold", () => {
  assert.deepEqual(GLOBE_STOPS.amber, {
    hi: "#FFE27A",
    mid: "#F5B921",
    lo: "#B97A00",
  });
});

check(
  "GLOBE_STOPS: the five non-amber mids are >= 25 deg apart, and from amber",
  () => {
    const others: AccentId[] = ["blue", "violet", "emerald", "rose", "slate"];
    const hues = Object.fromEntries(
      ACCENT_IDS.map((id) => [id, hueOf(GLOBE_STOPS[id].mid)]),
    ) as Record<AccentId, number>;
    for (const id of others) {
      assert.ok(
        hueDistance(hues[id], hues.amber) >= 25,
        `${id} mid hue ${hues[id].toFixed(1)} too close to amber`,
      );
    }
    for (let i = 0; i < others.length; i += 1) {
      for (let j = i + 1; j < others.length; j += 1) {
        const a = others[i] as AccentId;
        const b = others[j] as AccentId;
        const distance = hueDistance(hues[a], hues[b]);
        assert.ok(
          distance >= 25,
          `${a}/${b} mid hues ${distance.toFixed(1)} apart`,
        );
      }
    }
  },
);

check("CONCERNED_STOPS: red (mid is red-dominant)", () => {
  const [r, g, b] = rgb(CONCERNED_STOPS.mid);
  assert.ok(r > 200 && g < 100 && b < 100, CONCERNED_STOPS.mid);
});

check("SHINE: white at 55% opacity", () => {
  assert.deepEqual(SHINE, { colour: "#FFFFFF", opacity: 0.55 });
});

check("AUTO_FACE_ACCENT: the D1 state map", () => {
  for (const face of Object.keys(AUTO_EXPECT) as (keyof typeof AUTO_EXPECT)[]) {
    assert.equal(AUTO_FACE_ACCENT[face], AUTO_EXPECT[face], face);
  }
});

check("appModeStops: derived in CSS from --vox-app-accent (A2b)", () => {
  assert.deepEqual(appModeStops, {
    hi: "color-mix(in srgb, var(--vox-app-accent) 55%, white)",
    mid: "var(--vox-app-accent)",
    lo: "color-mix(in srgb, var(--vox-app-accent) 55%, black)",
  });
});

console.log("globe palette: all assertions passed");

// Runs one named check and names it on failure.
function check(name: string, body: () => void): void {
  try {
    body();
  } catch (error) {
    console.error(`FAIL ${name}`);
    throw error;
  }
}
