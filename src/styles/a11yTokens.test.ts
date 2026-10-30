import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { ACCENT_IDS, ACCENTS, type AccentBucket } from "./accents";
import { RESTRAINED_CARD_HEX } from "./restrained";
import { blendOver, contrastRatio, relativeLuminance } from "./wcag";

// UI enterprise S4b (DESIGN D4 U10 focus, U11 contrast, U12 motion). The CSS token pairs are
// evaluated by replaying the same colour maths the stylesheet uses (color-mix in sRGB equals a
// linear blend), over the palette base values read from theme.css.

const css = readFileSync(new URL("./theme.css", import.meta.url), "utf8");
const base = (name: string): string => {
  const match = css.match(new RegExp(`--${name}:\\s*(#[0-9a-fA-F]{6})`));
  assert.ok(match, `theme.css defines --${name}`);
  return match[1].toLowerCase();
};

const MUTED_MIX = 0.78; // --color-muted: color-mix(in srgb, var(--color-text) 78%, background)
const DISABLED_ACCENT_MIX = 0.2; // .btn-primary:disabled fill: color-mix(text 20%, background)
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

  "muted text is a dedicated token that blends the text colour 78% over the background":
    () => {
      assert.match(
        css,
        /--color-muted:\s*color-mix\(\s*in srgb,\s*var\(--color-text\)\s*78%/,
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

  "selected surfaces, accent fills and their text reach 4.5:1 for every palette, style and accent (table, R1a)":
    () => {
      // Rows: light and dark for each of the six accents (vibrant accent tokens), plus aurora once
      // (aurora owns its palette and ignores the accent). Restrained uses the same selected and fill
      // tokens and adds its neutral card surface.
      const rows: {
        label: string;
        bucket: AccentBucket | null;
        page: Palette;
        soft: string;
        text: string;
        ui: string;
        onAccent: string;
        fillA: string;
        fillB: string;
      }[] = [];
      for (const palette of palettes) {
        if (palette.bucket === null) {
          rows.push({
            label: "aurora",
            bucket: null,
            page: palette,
            soft: base("aurora-color-accent-soft"),
            text: base("aurora-color-accent-text"),
            ui: base("aurora-color-accent-ui"),
            onAccent: base("aurora-color-on-accent"),
            fillA: base("aurora-color-accent-a"),
            fillB: base("aurora-color-accent-b"),
          });
          continue;
        }
        for (const id of ACCENT_IDS) {
          const roles = ACCENTS[id][palette.bucket];
          rows.push({
            label: `${palette.name}/${id}`,
            bucket: palette.bucket,
            page: palette,
            soft: roles.soft,
            text: roles.text,
            ui: roles.ui,
            onAccent: roles.onFill,
            fillA: roles.fillA,
            fillB: roles.fillB,
          });
        }
      }
      for (const style of ["vibrant", "restrained"]) {
        for (const row of rows) {
          const label = `${style}/${row.label}`;
          const pageText = row.page.text;
          // Selected surface: accent text (the hint captions) and body text sit on it.
          assertAtLeast(
            contrastRatio(row.text, row.soft),
            4.5,
            `${label} accent text on selected surface`,
          );
          assertAtLeast(
            contrastRatio(pageText, row.soft),
            4.5,
            `${label} body text on selected surface`,
          );
          // Filled controls: on-accent text on both gradient ends and on the solid accent fill.
          for (const [fill, name] of [
            [row.fillA, "fillA"],
            [row.fillB, "fillB"],
            [row.ui, "ui"],
          ] as const) {
            assertAtLeast(
              contrastRatio(row.onAccent, fill),
              4.5,
              `${label} on-accent on ${name}`,
            );
          }
          // The selected surface is a dark tint in the dark bucket (never the light cream).
          if (row.bucket === "dark") {
            assert.ok(
              relativeLuminance(row.soft) < 0.1,
              `${label} dark selected surface ${row.soft} must be dark`,
            );
          }
          if (style === "restrained") {
            const card =
              row.bucket === null
                ? blendOver(pageText, 0.03, row.page.background)
                : RESTRAINED_CARD_HEX[row.bucket];
            assertAtLeast(
              contrastRatio(pageText, card),
              4.5,
              `${label} body text on restrained card`,
            );
            assertAtLeast(
              contrastRatio(
                blendOver(pageText, MUTED_MIX, row.page.background),
                card,
              ),
              4.5,
              `${label} muted text on restrained card`,
            );
          }
        }
      }
    },

  "muted text also reaches 4.5:1 on the selected accent tint, for every accent and bucket (R2-9/10)":
    () => {
      for (const palette of palettes) {
        if (palette.bucket === null) continue;
        const muted = blendOver(palette.text, MUTED_MIX, palette.background);
        for (const id of ACCENT_IDS) {
          const soft = ACCENTS[id][palette.bucket].soft;
          assertAtLeast(
            contrastRatio(muted, soft),
            4.5,
            `${palette.name}/${id} muted on selected tint ${soft}`,
          );
        }
      }
    },

  "disabled accent-filled buttons keep their label at 4.5:1 on the neutral disabled fill (R2-1..3)":
    () => {
      const app = readFileSync(new URL("../App.css", import.meta.url), "utf8");
      for (const palette of palettes) {
        const disabledFill = blendOver(
          palette.text,
          DISABLED_ACCENT_MIX,
          palette.background,
        );
        assertAtLeast(
          contrastRatio(palette.text, disabledFill),
          4.5,
          `${palette.name} disabled primary label on its fill`,
        );
      }
      assert.match(
        css,
        /\.btn-primary:disabled\s*\{[^}]*opacity:\s*1[^}]*background-color:\s*color-mix\(\s*in srgb,\s*var\(--color-text\)\s*20%/,
      );
      assert.match(
        app,
        /:root\[data-style="restrained"\] \.btn-primary:disabled\s*\{[^}]*background-color:\s*color-mix\(\s*in srgb,\s*var\(--color-text\)\s*20%/,
      );
    },

  "restrained solid primary: the label is --color-on-accent on the accent-ui fill, >= 4.5:1 for every accent and palette (R2-1..3 exact pairs)":
    () => {
      const app = readFileSync(new URL("../App.css", import.meta.url), "utf8");
      assert.match(
        app,
        /:root\[data-style="restrained"\] \.btn-primary\s*\{[^}]*background-color:\s*var\(--color-accent-ui\)/,
      );
      for (const palette of palettes) {
        if (palette.bucket === null) {
          assertAtLeast(
            contrastRatio(
              base("aurora-color-on-accent"),
              base("aurora-color-accent-ui"),
            ),
            4.5,
            "aurora restrained primary label on accent-ui",
          );
          continue;
        }
        for (const id of ACCENT_IDS) {
          const roles = ACCENTS[id][palette.bucket];
          assertAtLeast(
            contrastRatio(roles.onFill, roles.ui),
            4.5,
            `${palette.name}/${id} restrained primary label ${roles.onFill} on ${roles.ui}`,
          );
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
