import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { ACCENT_IDS, ACCENTS, ACCENT_SURFACE } from "./accents";
import { contrastRatio } from "./wcag";
import {
  RESTRAINED_BUCKETS,
  RESTRAINED_CARD_HEX,
  RESTRAINED_RADIUS,
  RESTRAINED_TOKENS,
  VIBRANT_CARD_BORDER,
  type RestrainedBucket,
} from "./restrained";

// UI enterprise S4a (DESIGN Revision 3 A5 / U5). Restrained is a CSS-only visual style selected by
// `data-style="restrained"` on <html>. It must be a real alternative to Vibrant (not a re-skin of
// it) and every text pair it introduces must meet WCAG AA (4.5:1).

const css = readFileSync(
  fileURLToPath(new URL("./theme.css", import.meta.url)),
  "utf8",
);

// Prettier wraps long colour-mix() calls across lines; compare without whitespace.
const compact = (text: string): string => text.replace(/\s+/g, "");
const cssCompact = compact(css);

const TEXT: Record<RestrainedBucket, string> = {
  light: "#0f0f0f",
  dark: "#fbfbfb",
};

const scenarios: Record<string, () => void> = {
  "every restrained token value is written into theme.css": () => {
    assert.match(css, /:root\[data-style="restrained"\]/, "restrained block");
    for (const value of [
      RESTRAINED_TOKENS.cardBg,
      RESTRAINED_TOKENS.cardBorder,
      RESTRAINED_TOKENS.cardShadow,
      RESTRAINED_RADIUS,
    ]) {
      assert.ok(
        cssCompact.includes(compact(value)),
        `theme.css lacks ${value}`,
      );
    }
  },

  "restrained cards are neutral: no accent-coloured border, no gradient":
    () => {
      const { cardBorder, cardBg } = RESTRAINED_TOKENS;
      assert.ok(!cardBorder.includes("accent"), cardBorder);
      assert.ok(!cardBg.includes("gradient"), cardBg);
      for (const bucket of RESTRAINED_BUCKETS) {
        for (const accent of ACCENT_IDS) {
          assert.notEqual(
            RESTRAINED_CARD_HEX[bucket].toLowerCase(),
            ACCENTS[accent][bucket].fillA.toLowerCase(),
            `${bucket} card must not be the ${accent} accent`,
          );
        }
      }
    },

  "restrained differs from vibrant: neutral border and a lifted card surface":
    () => {
      assert.notEqual(RESTRAINED_TOKENS.cardBorder, VIBRANT_CARD_BORDER);
      // Vibrant cards sit on the bare page background; restrained cards are a distinct surface.
      assert.notEqual(RESTRAINED_TOKENS.cardBg, "var(--color-background)");
      assert.ok(RESTRAINED_TOKENS.cardShadow.length > 0, "elevation declared");
    },

  "restrained text on restrained cards meets 4.5:1 in both buckets": () => {
    for (const bucket of RESTRAINED_BUCKETS) {
      const ratio = contrastRatio(TEXT[bucket], RESTRAINED_CARD_HEX[bucket]);
      assert.ok(
        ratio >= 4.5,
        `${bucket} text on restrained card is ${ratio.toFixed(2)}:1`,
      );
    }
  },

  "selected item: accent text on the soft tint and surface meets 4.5:1 for every accent":
    () => {
      for (const bucket of RESTRAINED_BUCKETS) {
        for (const accent of ACCENT_IDS) {
          const roles = ACCENTS[accent][bucket];
          const onSoft = contrastRatio(roles.text, roles.soft);
          assert.ok(
            onSoft >= 4.5,
            `${accent}/${bucket}: accent text on soft tint is ${onSoft.toFixed(2)}:1`,
          );
          const onSurface = contrastRatio(roles.text, ACCENT_SURFACE[bucket]);
          assert.ok(
            onSurface >= 4.5,
            `${accent}/${bucket}: accent text on surface is ${onSurface.toFixed(2)}:1`,
          );
        }
      }
    },

  "restrained page background is flat: no gradient or image in its block":
    () => {
      assert.equal(RESTRAINED_TOKENS.pageBg, "var(--color-background)");
      const start = css.indexOf(':root[data-style="restrained"] {');
      assert.ok(start >= 0, "restrained token block");
      const block = css.slice(start, css.indexOf("}", start) + 1);
      assert.ok(!/background-image|gradient/.test(block), block);
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
if (failed > 0) throw new Error(`${failed} restrained style scenarios failed`);
console.log(
  `restrained style: ${Object.keys(scenarios).length} scenarios passed`,
);
