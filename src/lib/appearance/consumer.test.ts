// UI enterprise S2 (DESIGN Revision 3, A1/A2): the shared appearance consumer for the chat, orb,
// bubbles and overlay pages. Pure node: fake DOM root and fake matchMedia only. Listen-only: the
// consumer must never invoke a command.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import type { AppearanceSnapshot } from "@/bindings";
import {
  APPEARANCE_EVENT,
  MAX_SEED_LENGTH,
  ORB_COLOR_IDS,
  SEED_PARAM,
  createAppearanceConsumer,
  parseSeed,
  parseSnapshot,
  shouldApply,
} from "./consumer";
import { DEFAULT_CUSTOM_THEME_COLORS } from "../utils/theme";

const snap = (over: Partial<AppearanceSnapshot> = {}): AppearanceSnapshot => ({
  rev: 1,
  theme: "system",
  custom_colors: null,
  accent: "amber",
  chat_theme: "dark",
  visual_style: "vibrant",
  orb_accent: "auto",
  ...over,
});

/** The seed exactly as the Rust side writes it: percent-encoded JSON in `ap`. */
const seedFor = (s: AppearanceSnapshot) =>
  `?instance=2&${SEED_PARAM}=${encodeURIComponent(JSON.stringify(s))}`;

function fakeRoot() {
  const props = new Map<string, string>();
  const dataset: Record<string, string | undefined> = {};
  return {
    props,
    dataset,
    style: {
      setProperty: (name: string, value: string) => {
        props.set(name, value);
      },
      removeProperty: (name: string) => {
        props.delete(name);
        return "";
      },
    },
  };
}

function fakeMedia(initialDark: boolean) {
  let dark = initialDark;
  const listeners: Array<() => void> = [];
  return {
    setDark(value: boolean) {
      dark = value;
      for (const listener of listeners) listener();
    },
    matchMedia: (query: string) => ({
      get matches() {
        return query.includes("dark") ? dark : false;
      },
      addEventListener(_type: "change", listener: () => void) {
        listeners.push(listener);
      },
    }),
  };
}

const scenarios: Record<string, () => void> = {
  "the event name and seed parameter are the wire names": () => {
    assert.equal(APPEARANCE_EVENT, "appearance");
    assert.equal(SEED_PARAM, "ap");
    assert.equal(MAX_SEED_LENGTH, 4096);
  },

  "parseSeed reads a valid encoded seed among other parameters": () => {
    const s = snap({
      rev: 4,
      theme: "dark",
      accent: "blue",
      chat_theme: "follow_app",
    });
    assert.deepEqual(parseSeed(seedFor(s)), s);
  },

  "parseSeed returns null for garbage, truncated, malformed and missing input":
    () => {
      for (const search of [
        "",
        "?instance=2",
        `?${SEED_PARAM}=`,
        `?${SEED_PARAM}=%7B`,
        `?${SEED_PARAM}=%ZZ`,
        `?${SEED_PARAM}=not-json`,
        `?${SEED_PARAM}=${encodeURIComponent("[1,2,3]")}`,
        `?${SEED_PARAM}=${encodeURIComponent("null")}`,
      ]) {
        assert.equal(parseSeed(search), null, `search ${search}`);
      }
    },

  "parseSeed refuses an oversized seed before parsing it": () => {
    const oversized = `?${SEED_PARAM}=${"x".repeat(MAX_SEED_LENGTH + 1)}`;
    assert.equal(parseSeed(oversized), null);
  },

  "parseSeed refuses a well-formed seed with an unknown enum": () => {
    const bad = { ...snap(), accent: "red" };
    assert.equal(parseSeed(seedFor(bad as AppearanceSnapshot)), null);
  },

  "parseSnapshot accepts every valid theme, accent, chat theme and style":
    () => {
      for (const theme of ["system", "light", "dark", "aurora"] as const) {
        for (const accent of [
          "amber",
          "blue",
          "violet",
          "emerald",
          "rose",
          "slate",
        ] as const) {
          for (const chat_theme of ["dark", "follow_app", "light"] as const) {
            for (const visual_style of ["vibrant", "restrained"] as const) {
              const s = snap({
                theme,
                accent,
                chat_theme,
                visual_style,
                rev: 9,
              });
              assert.deepEqual(parseSnapshot(s), s);
            }
          }
        }
      }
      const custom = snap({
        theme: "custom",
        custom_colors: DEFAULT_CUSTOM_THEME_COLORS,
      });
      assert.deepEqual(parseSnapshot(custom), custom);
    },

  "parseSnapshot rejects bad revisions": () => {
    for (const rev of [
      0,
      -1,
      1.5,
      Number.NaN,
      Number.MAX_SAFE_INTEGER + 2,
      "3",
      null,
    ]) {
      assert.equal(
        parseSnapshot({ ...snap(), rev }),
        null,
        `rev ${String(rev)}`,
      );
    }
  },

  "parseSnapshot rejects unknown enums and wrong case": () => {
    for (const bad of [
      { theme: "Dark" },
      { theme: "neon" },
      { accent: "red" },
      { accent: "Blue" },
      { chat_theme: "auto" },
      { visual_style: "flat" },
    ]) {
      assert.equal(
        parseSnapshot({ ...snap(), ...bad }),
        null,
        JSON.stringify(bad),
      );
    }
  },

  "parseSnapshot requires custom colours exactly when the theme is custom":
    () => {
      assert.equal(
        parseSnapshot(snap({ theme: "custom", custom_colors: null })),
        null,
        "custom without colours",
      );
      assert.equal(
        parseSnapshot(
          snap({ theme: "dark", custom_colors: DEFAULT_CUSTOM_THEME_COLORS }),
        ),
        null,
        "dark with colours",
      );
      assert.equal(
        parseSnapshot(
          snap({
            theme: "custom",
            custom_colors: { ...DEFAULT_CUSTOM_THEME_COLORS, accent: "#fff" },
          }),
        ),
        null,
        "bad hex",
      );
    },

  "parseSnapshot rejects non-object payloads": () => {
    for (const payload of [null, undefined, "x", 3, [], true]) {
      assert.equal(parseSnapshot(payload), null, String(payload));
    }
  },

  "shouldApply is strictly greater than the applied revision": () => {
    assert.equal(shouldApply(0, 1), true);
    assert.equal(shouldApply(3, 3), false, "duplicate");
    assert.equal(shouldApply(3, 2), false, "older");
    assert.equal(shouldApply(3, 4), true, "newer");
  },

  "duplicate and reordered deliveries apply only the strictly newer revisions":
    () => {
      const root = fakeRoot();
      const consumer = createAppearanceConsumer({
        root,
        matchMedia: fakeMedia(false).matchMedia,
      });
      const applied: number[] = [];
      for (const rev of [1, 3, 2, 3, 5, 4, 5]) {
        if (consumer.accept(snap({ rev }))) applied.push(rev);
      }
      assert.deepEqual(applied, [1, 3, 5]);
      assert.equal(consumer.appliedRev(), 5);
    },

  "a seed seeds first paint and a late event with a lower rev is ignored":
    () => {
      const root = fakeRoot();
      const consumer = createAppearanceConsumer({
        root,
        matchMedia: fakeMedia(false).matchMedia,
      });
      const seed = parseSeed(seedFor(snap({ rev: 7, theme: "dark" })));
      assert.ok(seed);
      assert.equal(consumer.accept(seed), true);
      assert.equal(root.dataset.theme, "dark");
      assert.equal(consumer.accept(snap({ rev: 5, theme: "light" })), false);
      assert.equal(root.dataset.theme, "dark", "older event must not revert");
      assert.equal(consumer.accept(snap({ rev: 8, theme: "light" })), true);
      assert.equal(root.dataset.theme, "light");
    },

  "an invalid payload is ignored and leaves the applied state alone": () => {
    const root = fakeRoot();
    const consumer = createAppearanceConsumer({
      root,
      matchMedia: fakeMedia(false).matchMedia,
    });
    consumer.accept(snap({ rev: 2, accent: "blue" }));
    assert.equal(consumer.accept({ rev: 9, accent: "red" }), false);
    assert.equal(consumer.accept("garbage"), false);
    assert.equal(consumer.appliedRev(), 2);
    assert.equal(root.dataset.accent, "blue");
  },

  "a live prefers-color-scheme change re-resolves a follow-app chat palette":
    () => {
      const media = fakeMedia(false);
      const palettes: string[] = [];
      const consumer = createAppearanceConsumer({
        root: fakeRoot(),
        matchMedia: media.matchMedia,
        onApplied: (resolved) => palettes.push(resolved.chatPalette),
      });
      consumer.accept(
        snap({ rev: 1, theme: "system", chat_theme: "follow_app" }),
      );
      assert.deepEqual(palettes, ["light"]);
      media.setDark(true);
      assert.deepEqual(palettes, ["light", "dark"], "OS dark re-resolves live");
      media.setDark(false);
      assert.deepEqual(palettes, ["light", "dark", "light"]);
    },

  "a live colour-scheme change does nothing when the theme is fixed": () => {
    const media = fakeMedia(false);
    let calls = 0;
    const consumer = createAppearanceConsumer({
      root: fakeRoot(),
      matchMedia: media.matchMedia,
      onApplied: () => {
        calls += 1;
      },
    });
    consumer.accept(snap({ rev: 1, theme: "light", chat_theme: "dark" }));
    assert.equal(calls, 1);
    media.setDark(true);
    assert.equal(calls, 1, "a fixed theme ignores the OS scheme");
  },

  // RE-POINTED (A5): null/missing/garbage normalise to 'auto' (was null). Valid values are kept.
  "orb_accent: all eight orb colours are kept, anything else normalises to auto":
    () => {
      for (const orb of [
        "auto",
        "app",
        "amber",
        "blue",
        "violet",
        "emerald",
        "rose",
        "slate",
      ]) {
        assert.equal(
          parseSnapshot(snap({ orb_accent: orb as never }))?.orb_accent,
          orb,
        );
      }
      assert.equal(
        parseSnapshot(snap({ orb_accent: null as never }))?.orb_accent,
        "auto",
      );
      const missing: Record<string, unknown> = { ...snap() };
      delete missing.orb_accent;
      assert.equal(
        parseSnapshot(missing)?.orb_accent,
        "auto",
        "missing is auto",
      );
      for (const garbage of [
        "teal",
        "",
        "AMBER",
        "bogus",
        7,
        42,
        true,
        undefined,
        {},
        ["blue"],
      ]) {
        const parsed = parseSnapshot({ ...snap(), orb_accent: garbage });
        assert.ok(
          parsed !== null,
          `a bad orb_accent does not reject the snapshot: ${JSON.stringify(garbage)}`,
        );
        assert.equal(
          parsed.orb_accent,
          "auto",
          `garbage ${JSON.stringify(garbage)} -> auto`,
        );
      }
    },

  "orb_accent: ORB_COLOR_IDS lists auto, app, then the six accents": () => {
    assert.deepEqual(ORB_COLOR_IDS, [
      "auto",
      "app",
      "amber",
      "blue",
      "violet",
      "emerald",
      "rose",
      "slate",
    ]);
  },

  "orb_accent: normalisation of null, undefined, bogus, 42, {} and AMBER is auto":
    () => {
      for (const value of [null, undefined, "bogus", 42, {}, "AMBER"]) {
        assert.equal(
          parseSnapshot({ ...snap(), orb_accent: value })?.orb_accent,
          "auto",
          `${String(value)} -> auto`,
        );
      }
    },

  "orb_accent: a stale revision never changes the applied orb accent": () => {
    const consumer = createAppearanceConsumer({
      root: fakeRoot(),
      matchMedia: fakeMedia(false).matchMedia,
    });
    assert.equal(consumer.accept(snap({ rev: 3, orb_accent: "blue" })), true);
    assert.equal(consumer.accept(snap({ rev: 2, orb_accent: "rose" })), false);
    assert.equal(consumer.accept(snap({ rev: 3, orb_accent: "rose" })), false);
    assert.equal(consumer.snapshot()?.orb_accent, "blue");
    assert.equal(
      consumer.accept(snap({ rev: 4, orb_accent: "teal" as never })),
      true,
    );
    // RE-POINTED (A5): was null; an unknown value normalises to 'auto'.
    assert.equal(
      consumer.snapshot()?.orb_accent,
      "auto",
      "unknown normalises to auto",
    );
  },

  "the consumer and palette modules never invoke a command or import bindings at runtime":
    () => {
      const dir = fileURLToPath(new URL(".", import.meta.url));
      for (const name of ["consumer.ts", "palette.ts"]) {
        const source = readFileSync(`${dir}${name}`, "utf8");
        assert.doesNotMatch(source, /\binvoke\s*\(/, `${name} must not invoke`);
        assert.doesNotMatch(
          source,
          /\bcommands\./,
          `${name} must not call commands`,
        );
        assert.doesNotMatch(
          source,
          /^import\s+(?!type\b)[^;]*from\s+["']@\/bindings["']/m,
          `${name} must import bindings as types only`,
        );
        assert.doesNotMatch(
          source,
          /@tauri-apps/,
          `${name} must not use Tauri APIs`,
        );
      }
    },
};

let failed = 0;
for (const [name, run] of Object.entries(scenarios)) {
  try {
    run();
  } catch (error) {
    failed += 1;
    console.error(`FAIL: ${name}\n  ${(error as Error).message}`);
  }
}
if (failed > 0) {
  throw new Error(
    `${failed} of ${Object.keys(scenarios).length} consumer scenarios failed`,
  );
}
console.log(
  `appearance consumer: ${Object.keys(scenarios).length} scenarios passed`,
);
