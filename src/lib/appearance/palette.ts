// Pure appearance resolution and DOM application (UI enterprise S2, DESIGN Revision 3 A1-A3).
// Shared by the settings window (through theme.ts) and the listen-only pages (through consumer.ts).
// This module imports bindings as TYPES ONLY: it never invokes a command, so the chat, orb and
// bubble pages can import it without gaining any capability.
import type { AppearanceSnapshot, CustomThemeColors, Theme } from "@/bindings";
import type { AccentId } from "../../styles/accents";

/** The light or dark side of the palette: the bucket the page renders in. */
export type Bucket = "light" | "dark";

/** Everything a page needs to paint one appearance. */
export interface ResolvedAppearance {
  theme: Theme;
  /** The bucket the window renders in (system follows the OS; aurora is dark; custom by background). */
  bucket: Bucket;
  /** The conversation palette: fixed by `chat_theme`, or the bucket when following the app. */
  chatPalette: Bucket;
  /** The accent, or null when the theme owns its palette (aurora and custom, A3). */
  accent: AccentId | null;
  /** `vibrant` (the default look) or `restrained` (A5). */
  style: "vibrant" | "restrained";
  /** The custom palette, present only while the theme is custom. */
  customColors: CustomThemeColors | null;
}

/** The DOM surface the apply step writes to: the document element, or a test double. */
export interface AppearanceRoot {
  dataset: Record<string, string | undefined>;
  style: {
    setProperty(name: string, value: string): void;
    removeProperty(name: string): unknown;
  };
}

/** Maps each `CustomThemeColors` field to the CSS custom property it drives. */
export const CUSTOM_PROPERTY_MAP: Record<keyof CustomThemeColors, string> = {
  text: "--color-text",
  background: "--color-background",
  accent: "--color-logo-primary",
  accent_secondary: "--color-logo-stroke",
  warning: "--color-warning",
  error: "--color-error",
};

/** The recording-overlay's own neutral tokens that don't map 1:1 to a single custom colour. */
export const OVERLAY_NEUTRAL_PROPERTIES = [
  "--s-accent-soft",
  "--s-muted",
  "--s-faint",
  "--s-border",
  "--s-hair",
] as const;

/**
 * WCAG-style relative luminance of a `#rrggbb` hex colour, in [0, 1]. A practical
 * brightness-matching heuristic (used to pick a light/dark neutral bucket), NOT a
 * contrast-ratio claim. MUST stay in sync with `relative_luminance` in
 * `src-tauri/src/settings.rs`. Returns `null` for an unparseable string.
 */
export const relativeLuminance = (hex: string): number | null => {
  if (!/^#[0-9a-fA-F]{6}$/.test(hex)) return null;
  const channel = (start: number) => parseInt(hex.slice(start, start + 2), 16);
  const linearize = (byte: number) => {
    const s = byte / 255;
    return s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  };
  const r = linearize(channel(1));
  const g = linearize(channel(3));
  const b = linearize(channel(5));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};

/** Mirrors `native_chrome_for_hex_background` in settings.rs: Light above the 0.5 midpoint. */
export const neutralBucketFor = (backgroundHex: string): Bucket => {
  const luminance = relativeLuminance(backgroundHex);
  return luminance !== null && luminance > 0.5 ? "light" : "dark";
};

const ON_ACCENT_LIGHT = "#ffffff";
const ON_ACCENT_DARK = "#111111";

const contrastRatio = (a: string, b: string): number | null => {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  if (la === null || lb === null) return null;
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
};

/**
 * Text colour for content drawn on the custom accent gradient: whichever of white or near-black has
 * the better WORST-case contrast across both gradient ends. Falls back to dark text on bad input.
 */
export const onAccentFor = (accentA: string, accentB: string): string => {
  const worst = (fg: string): number | null => {
    const ra = contrastRatio(fg, accentA);
    const rb = contrastRatio(fg, accentB);
    return ra === null || rb === null ? null : Math.min(ra, rb);
  };
  const light = worst(ON_ACCENT_LIGHT);
  const dark = worst(ON_ACCENT_DARK);
  if (light === null || dark === null) return ON_ACCENT_DARK;
  return light > dark ? ON_ACCENT_LIGHT : ON_ACCENT_DARK;
};

/** The accent-gradient CSS properties derived from a custom palette. */
export const derivedAccentProperties = (
  colors: CustomThemeColors,
): Record<string, string> => ({
  "--color-accent-a": colors.accent,
  "--color-accent-b": colors.accent_secondary,
  "--color-on-accent": onAccentFor(colors.accent, colors.accent_secondary),
  "--color-background-ui": colors.accent,
});

/** Every custom property a custom palette can set inline (cleared on every apply). */
export const DERIVED_ACCENT_PROPERTIES = Object.keys(
  derivedAccentProperties({
    text: "#000000",
    background: "#000000",
    accent: "#000000",
    accent_secondary: "#000000",
    warning: "#000000",
    error: "#000000",
  }),
);

/**
 * Resolves a committed snapshot for one page. Pure: `prefersDark` is the OS colour scheme.
 * - bucket: light/dark as chosen; system follows `prefersDark`; aurora is dark; custom follows its
 *   background luminance.
 * - accent: only under system, light and dark (A3). Aurora and custom own their palette: null.
 */
export function resolveAppearance(
  snapshot: AppearanceSnapshot,
  prefersDark: boolean,
): ResolvedAppearance {
  const { theme } = snapshot;
  const bucket: Bucket =
    theme === "light"
      ? "light"
      : theme === "dark" || theme === "aurora"
        ? "dark"
        : theme === "custom"
          ? neutralBucketFor(snapshot.custom_colors?.background ?? "")
          : prefersDark
            ? "dark"
            : "light";
  const chatPalette: Bucket =
    snapshot.chat_theme === "follow_app"
      ? bucket
      : snapshot.chat_theme === "light"
        ? "light"
        : "dark";
  const ownsPalette = theme === "aurora" || theme === "custom";
  return {
    theme,
    bucket,
    chatPalette,
    accent: ownsPalette ? null : snapshot.accent,
    style: snapshot.visual_style,
    customColors: theme === "custom" ? snapshot.custom_colors : null,
  };
}

/**
 * Writes a resolved appearance onto a root element. Clears every inline override first, so a stale
 * custom palette can never outlive a switch to a preset (A3). The amber default sets no accent
 * attribute; `restrained` sets data-style, vibrant removes it.
 */
export function applyResolvedAppearance(
  root: AppearanceRoot,
  resolved: ResolvedAppearance,
): void {
  for (const property of Object.values(CUSTOM_PROPERTY_MAP)) {
    root.style.removeProperty(property);
  }
  for (const property of DERIVED_ACCENT_PROPERTIES) {
    root.style.removeProperty(property);
  }
  for (const property of OVERLAY_NEUTRAL_PROPERTIES) {
    root.style.removeProperty(property);
  }

  if (resolved.theme === "system") {
    delete root.dataset.theme;
  } else {
    root.dataset.theme = resolved.theme;
  }

  if (resolved.accent !== null && resolved.accent !== "amber") {
    root.dataset.accent = resolved.accent;
  } else {
    delete root.dataset.accent;
  }

  if (resolved.style === "restrained") {
    root.dataset.style = "restrained";
  } else {
    delete root.dataset.style;
  }

  if (resolved.theme === "custom" && resolved.customColors !== null) {
    const colors = resolved.customColors;
    for (const [field, property] of Object.entries(CUSTOM_PROPERTY_MAP) as [
      keyof CustomThemeColors,
      string,
    ][]) {
      root.style.setProperty(property, colors[field]);
    }
    for (const [property, value] of Object.entries(
      derivedAccentProperties(colors),
    )) {
      root.style.setProperty(property, value);
    }
    const bucket = neutralBucketFor(colors.background);
    for (const property of OVERLAY_NEUTRAL_PROPERTIES) {
      // Reuse the existing --light-s-*/--dark-s-* values defined in RecordingOverlay.css.
      root.style.setProperty(
        property,
        `var(--${bucket}-s${property.slice(2)})`,
      );
    }
  }
}
