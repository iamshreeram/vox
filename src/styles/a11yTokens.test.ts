import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { ACCENT_IDS, ACCENTS, type AccentBucket } from "./accents";
import { blendOver, contrastRatio } from "./wcag";

// UI enterprise S4b (DESIGN D4 U10 focus, U11 contrast, U12 motion). The CSS token pairs are
// evaluated by replaying the same colour maths the stylesheet uses (color-mix in sRGB equals a
// linear blend), over the palette base values read from theme.css.

const css = readFileSync(new URL("./theme.css", import.meta.url), "utf8");
const base = (name: string): string => {
  const match = css.match(new RegExp(`--${name}:\\s*(#[0-9a-fA-F]{6})`));
  assert.ok(match, `theme.css defines --${name}`);
  return match[1].toLowerCase();
};

const MUTED_MIX = 0.68; // --color-muted: color-mix(in srgb, var(--color-text) 68%, background)
const DISABLED_OPACITY = 0.5; // disabled:opacity-50
const MID_GRAY = "#808080";

type Palette = {
  name: string;
  text: string;
  background: string;
  bucket: AccentBucket | null;
};

const palettes: Palette[] = [
  {
    name: "light",
    text: base("light-color-text"),
    background: base("light-color-background"),
    bucket: "light",
  },
  {
    name: "dark",
    text: base("dark-color-text"),
    background: base("dark-color-background"),
    bucket: "dark",
  },
  {
    name: "aurora",
    text: base("aurora-color-text"),
    background: base("aurora-color-background"),
    bucket: null,
  },
];

const assertAtLeast = (ratio: number, floor: number, label: string): void => {
  assert.ok(
    ratio >= floor,
    `${label}: ${ratio.toFixed(2)}:1 is below ${floor}:1`,
  );
};

const scenarios: Record<string, () => void> = {
  "theme.css defines one global focus ring: text colour, 2px outline, 2px offset on :focus-visible":
    () => {
      assert.match(css, /--focus-ring:\s*var\(--color-text\)/);
      assert.match(
        css,
        /:focus-visible\s*\{[^}]*outline:\s*2px solid var\(--focus-ring\)[^}]*\}/,
      );
      assert.match(css, /:focus-visible\s*\{[^}]*outline-offset:\s*2px/);
    },

  "muted text is a dedicated token that blends the text colour 68% over the background":
    () => {
      assert.match(
        css,
        /--color-muted:\s*color-mix\(\s*in srgb,\s*var\(--color-text\)\s*68%/,
      );
    },

  "muted text reaches 4.5:1 on the page, the restrained card and every mid-gray surface, per palette":
    () => {
      for (const palette of palettes) {
        const muted = blendOver(palette.text, MUTED_MIX, palette.background);
        const card = blendOver(palette.text, 0.03, palette.background);
        const surfaces = [
          palette.background,
          card,
          blendOver(MID_GRAY, 0.1, palette.background),
          blendOver(MID_GRAY, 0.2, palette.background),
        ];
        surfaces.forEach((surface, index) =>
          assertAtLeast(
            contrastRatio(muted, surface),
            4.5,
            `${palette.name} muted on surface ${index} (${surface})`,
          ),
        );
      }
    },

  "muted text reaches 4.5:1 over the worst stop of every Vibrant page gradient, for every accent":
    () => {
      for (const palette of palettes) {
        if (palette.bucket === null) continue;
        const muted = blendOver(palette.text, MUTED_MIX, palette.background);
        for (const id of ACCENT_IDS) {
          const roles = ACCENTS[id][palette.bucket];
          // App.css: accent-b at 14% (top right) and accent-a at 12% (bottom left).
          const stops = [
            blendOver(roles.fillB, 0.14, palette.background),
            blendOver(roles.fillA, 0.12, palette.background),
            blendOver(roles.soft, 1, palette.background),
          ];
          stops.forEach((stop, index) =>
            assertAtLeast(
              contrastRatio(muted, stop),
              4.5,
              `${palette.name}/${id} muted over gradient stop ${index}`,
            ),
          );
        }
      }
    },

  "disabled controls keep their text at 3:1 against their own fill, per palette":
    () => {
      for (const palette of palettes) {
        const disabledText = blendOver(
          palette.text,
          DISABLED_OPACITY,
          palette.background,
        );
        const disabledFill = blendOver(
          MID_GRAY,
          0.1 * DISABLED_OPACITY,
          palette.background,
        );
        assertAtLeast(
          contrastRatio(disabledText, disabledFill),
          3,
          `${palette.name} disabled control text`,
        );
        assertAtLeast(
          contrastRatio(disabledText, palette.background),
          3,
          `${palette.name} disabled text on page`,
        );
      }
    },

  "the focus ring is 3:1 against the page background in every palette": () => {
    for (const palette of palettes) {
      assertAtLeast(
        contrastRatio(palette.text, palette.background),
        3,
        `${palette.name} focus ring vs page`,
      );
    }
  },

  "a focus ring on an accent-filled control is a two-tone ring: the on-accent inner ring is 4.5:1 on both fill ends for every accent":
    () => {
      for (const palette of palettes) {
        if (palette.bucket === null) continue;
        for (const id of ACCENT_IDS) {
          const roles = ACCENTS[id][palette.bucket];
          for (const fill of [roles.fillA, roles.fillB]) {
            assertAtLeast(
              contrastRatio(roles.onFill, fill),
              4.5,
              `${palette.name}/${id} inner focus ring on fill ${fill}`,
            );
          }
        }
      }
    },

  "reduced motion: one global override zeroes animation and transition durations":
    () => {
      assert.match(
        css,
        /@media \(prefers-reduced-motion: reduce\)\s*\{\s*\*,\s*\*::before,\s*\*::after\s*\{[^}]*animation-duration:\s*0\.01ms\s*!important[^}]*transition-duration:\s*0\.01ms\s*!important/,
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
console.log("a11yTokens ok");
