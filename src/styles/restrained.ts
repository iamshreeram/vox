// Restrained visual style tokens (UI enterprise S4a, DESIGN Revision 3 A5 / U5). Pure data, no
// React, so Bun can unit-test it. src/styles/theme.css writes the CSS forms of these values inside
// `:root[data-style="restrained"]`; restrained.test.ts pins that mirror and the WCAG floors.
// Vibrant (the default) keeps its own look: nothing here applies unless data-style is restrained.
//
// The CSS values are bucket-agnostic (colour-mix over the active --color-text and
// --color-background), so they follow light, dark, custom and the OS scheme without a second set.

export const RESTRAINED_BUCKETS = ["light", "dark"] as const;
export type RestrainedBucket = (typeof RESTRAINED_BUCKETS)[number];

/** One consistent card radius for every restrained card and group. */
export const RESTRAINED_RADIUS = "0.5rem";

/** The vibrant card border (an accent tint), which restrained replaces with a neutral one. */
export const VIBRANT_CARD_BORDER =
  "color-mix(in srgb, var(--color-accent-a) 28%, transparent)";

export interface RestrainedTokens {
  /** A faint neutral lift of the page background: the card surface. */
  cardBg: string;
  /** A neutral 1px border: never the accent. */
  cardBorder: string;
  /** Subtle elevation: no glow and no accent tint. */
  cardShadow: string;
  /** The page background: flat, the theme's own background colour. */
  pageBg: string;
}

export const RESTRAINED_TOKENS: RestrainedTokens = {
  cardBg: "color-mix(in srgb, var(--color-text) 3%, var(--color-background))",
  cardBorder:
    "color-mix(in srgb, var(--color-text) 14%, var(--color-background))",
  cardShadow: "0 1px 2px color-mix(in srgb, var(--color-text) 8%, transparent)",
  pageBg: "var(--color-background)",
};

/**
 * The card surface as a literal hex per bucket, for the WCAG checks only (the CSS above is a
 * colour-mix). Each value is the mix of the bucket's text and background at 3%.
 */
export const RESTRAINED_CARD_HEX: Record<RestrainedBucket, string> = {
  light: "#f4f4f4",
  dark: "#32312f",
};
