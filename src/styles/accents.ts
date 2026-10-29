// Canonical accent table (UI enterprise S2, DESIGN Revision 3 A3). ONE table: every accent and every
// bucket (light, dark) defines the roles below. src/styles/theme.css mirrors each row in a
// `:root[data-accent=...]` block (amber is the :root default), and accents.test.ts pins the mirror and
// the WCAG floors. Amber equals today's exact colours, so the default look is unchanged.
//
// Roles:
//   fillA / fillB  gradient ends for toggles, sliders, primary buttons, the active item and the
//                  user bubble. onFill is the text on both ends (>= 4.5:1).
//   text           accent-coloured text on the soft tint and on the bucket surface (>= 4.5:1).
//   onFill         text colour placed on fillA/fillB.
//   soft           quiet tint for selected rows and the assistant bubble.
//   logo           the logo colour (--color-logo-primary); a graphic, not body text.
//   ui             the accent fill used by toggles and sliders (--color-background-ui).

export const ACCENT_IDS = [
  "amber",
  "blue",
  "violet",
  "emerald",
  "rose",
  "slate",
] as const;

export type AccentId = (typeof ACCENT_IDS)[number];
export type AccentBucket = "light" | "dark";
export type AccentRole =
  | "fillA"
  | "fillB"
  | "text"
  | "onFill"
  | "soft"
  | "logo"
  | "ui";

export const ACCENT_ROLES: readonly AccentRole[] = [
  "fillA",
  "fillB",
  "text",
  "onFill",
  "soft",
  "logo",
  "ui",
];

export const DEFAULT_ACCENT: AccentId = "amber";

/** The surface the accent text sits on, per bucket (the page background of each palette). */
export const ACCENT_SURFACE: Record<AccentBucket, string> = {
  light: "#fbfbfb",
  dark: "#2c2b29",
};

/** The theme.css custom-property suffix of each role: `--<bucket>-<suffix>`. */
export const ACCENT_CSS_PROPERTIES: Record<AccentRole, string> = {
  fillA: "color-accent-a",
  fillB: "color-accent-b",
  text: "color-accent-text",
  onFill: "color-on-accent",
  soft: "color-accent-soft",
  logo: "color-logo-primary",
  ui: "color-accent-ui",
};

export type AccentRoles = Record<AccentRole, string>;

export const ACCENTS: Record<AccentId, Record<AccentBucket, AccentRoles>> = {
  amber: {
    light: {
      fillA: "#f59e0b",
      fillB: "#f97316",
      text: "#7a4f00",
      onFill: "#1a1204",
      soft: "#fef3c7",
      logo: "#a6790a",
      ui: "#c9971e",
    },
    dark: {
      fillA: "#fbbf24",
      fillB: "#fb923c",
      text: "#f4c430",
      onFill: "#1a1204",
      soft: "#3d3418",
      logo: "#f4c430",
      ui: "#c9971e",
    },
  },
  blue: {
    light: {
      fillA: "#1d4ed8",
      fillB: "#1e40af",
      text: "#1d4ed8",
      onFill: "#ffffff",
      soft: "#dbeafe",
      logo: "#1d4ed8",
      ui: "#1d4ed8",
    },
    dark: {
      fillA: "#60a5fa",
      fillB: "#3b82f6",
      text: "#93c5fd",
      onFill: "#0b1220",
      soft: "#1e3a5f",
      logo: "#60a5fa",
      ui: "#60a5fa",
    },
  },
  violet: {
    light: {
      fillA: "#6d28d9",
      fillB: "#7c3aed",
      text: "#6d28d9",
      onFill: "#ffffff",
      soft: "#ede9fe",
      logo: "#6d28d9",
      ui: "#6d28d9",
    },
    dark: {
      fillA: "#a78bfa",
      fillB: "#8b5cf6",
      text: "#c4b5fd",
      onFill: "#100a1f",
      soft: "#2e2650",
      logo: "#a78bfa",
      ui: "#a78bfa",
    },
  },
  emerald: {
    light: {
      fillA: "#047857",
      fillB: "#065f46",
      text: "#065f46",
      onFill: "#ffffff",
      soft: "#d1fae5",
      logo: "#047857",
      ui: "#047857",
    },
    dark: {
      fillA: "#34d399",
      fillB: "#10b981",
      text: "#6ee7b7",
      onFill: "#04120c",
      soft: "#173d32",
      logo: "#34d399",
      ui: "#34d399",
    },
  },
  rose: {
    light: {
      fillA: "#be123c",
      fillB: "#9f1239",
      text: "#9f1239",
      onFill: "#ffffff",
      soft: "#ffe4e6",
      logo: "#be123c",
      ui: "#be123c",
    },
    dark: {
      fillA: "#fb7185",
      fillB: "#f43f5e",
      text: "#fda4af",
      onFill: "#1f0508",
      soft: "#4c1d28",
      logo: "#fb7185",
      ui: "#fb7185",
    },
  },
  slate: {
    light: {
      fillA: "#334155",
      fillB: "#1e293b",
      text: "#334155",
      onFill: "#ffffff",
      soft: "#e2e8f0",
      logo: "#334155",
      ui: "#334155",
    },
    dark: {
      fillA: "#cbd5e1",
      fillB: "#94a3b8",
      text: "#e2e8f0",
      onFill: "#0f172a",
      soft: "#334155",
      logo: "#cbd5e1",
      ui: "#cbd5e1",
    },
  },
};

/** The roles of one accent in one bucket. */
export function accentRoles(
  accent: AccentId,
  bucket: AccentBucket,
): AccentRoles {
  return ACCENTS[accent][bucket];
}
