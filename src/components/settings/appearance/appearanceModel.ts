// Pure rules of the Appearance settings page (UI enterprise S3, DESIGN Revision 3 A3, A5, D3).
// No React and no commands here: the components apply these results.
import type { Theme } from "@/bindings";
import { ACCENT_IDS, DEFAULT_ACCENT, type AccentId } from "@/styles/accents";

/** The settings the page owns, with the values Reset to defaults restores. */
export const APPEARANCE_DEFAULTS = {
  theme: "system",
  accent: "amber",
  chatTheme: "dark",
  visualStyle: "vibrant",
  chatOpacityPct: 70,
} as const;

export type ResetStep =
  | { kind: "theme"; value: "system" }
  | { kind: "accent"; value: "amber" }
  | { kind: "chatTheme"; value: "dark" }
  | { kind: "visualStyle"; value: "vibrant" }
  | { kind: "chatOpacity"; value: number };

/** The commands Reset to defaults issues, in order. */
export function resetPlan(): ResetStep[] {
  return [
    { kind: "theme", value: APPEARANCE_DEFAULTS.theme },
    { kind: "accent", value: APPEARANCE_DEFAULTS.accent },
    { kind: "chatTheme", value: APPEARANCE_DEFAULTS.chatTheme },
    { kind: "visualStyle", value: APPEARANCE_DEFAULTS.visualStyle },
    { kind: "chatOpacity", value: APPEARANCE_DEFAULTS.chatOpacityPct },
  ];
}

/**
 * The accent applies only while the theme is System, Light or Dark (A3). Aurora and Custom own
 * their palette, so the accent group is disabled there and explains why.
 */
export function accentEnabledFor(theme: Theme): boolean {
  return theme === "system" || theme === "light" || theme === "dark";
}

/** Translation key of each swatch's name. */
export const ACCENT_NAME_KEYS: Readonly<Record<AccentId, string>> = {
  amber: "settings.appearance.accent.swatch.amber",
  blue: "settings.appearance.accent.swatch.blue",
  violet: "settings.appearance.accent.swatch.violet",
  emerald: "settings.appearance.accent.swatch.emerald",
  rose: "settings.appearance.accent.swatch.rose",
  slate: "settings.appearance.accent.swatch.slate",
};

/**
 * The orb colour (DESIGN D3). `null` means follow the app accent and is the default; the six accents
 * pick the orb's own colour. The first option is always Follow app.
 */
export type OrbChoice = AccentId | null;

/** The id of the Follow app option. No accent is called "follow", so ids stay unique. */
export const ORB_FOLLOW_ID = "follow";

/** Follow app first, then the six accents in accent order. */
export const ORB_CHOICES: readonly OrbChoice[] = [null, ...ACCENT_IDS];

/** A stable id for one orb option: the accent id, or the follow id for null. */
export function orbChoiceId(choice: OrbChoice): string {
  return choice ?? ORB_FOLLOW_ID;
}

/** Translation key of an orb option's name: Follow app, or the accent's swatch name. */
export function orbChoiceNameKey(choice: OrbChoice): string {
  return choice === null
    ? "settings.appearance.orb.followApp"
    : ACCENT_NAME_KEYS[choice];
}

/**
 * The stored orb accent as the page shows it. Absent or unknown values read as null (follow the app
 * accent), so an older or garbled setting never leaves the group without a selection.
 */
export function effectiveOrbAccent(value: unknown): OrbChoice {
  return (ACCENT_IDS as readonly string[]).includes(value as string)
    ? (value as AccentId)
    : null;
}

/** The index of an orb option in ORB_CHOICES (Follow app is 0). */
export function orbSelectedIndex(choice: OrbChoice): number {
  const index = ORB_CHOICES.indexOf(effectiveOrbAccent(choice));
  return index < 0 ? 0 : index;
}

const isChatTheme = (
  value: unknown,
): value is "dark" | "light" | "follow_app" =>
  value === "dark" || value === "light" || value === "follow_app";

const isVisualStyle = (value: unknown): value is "vibrant" | "restrained" =>
  value === "vibrant" || value === "restrained";

const THEMES: readonly Theme[] = [
  "system",
  "light",
  "dark",
  "aurora",
  "custom",
];

const isTheme = (value: unknown): value is Theme =>
  typeof value === "string" && (THEMES as readonly string[]).includes(value);

/** The values the page shows: absent or unknown stored values read as the defaults. */
export interface EffectiveAppearance {
  theme: Theme;
  accent: AccentId;
  chatTheme: "dark" | "light" | "follow_app";
  visualStyle: "vibrant" | "restrained";
}

export function effectiveAppearance(settings: {
  theme?: unknown;
  accent?: unknown;
  chat_theme?: unknown;
  visual_style?: unknown;
}): EffectiveAppearance {
  return {
    theme: isTheme(settings.theme) ? settings.theme : "system",
    accent: (ACCENT_IDS as readonly string[]).includes(
      settings.accent as string,
    )
      ? (settings.accent as AccentId)
      : DEFAULT_ACCENT,
    chatTheme: isChatTheme(settings.chat_theme)
      ? settings.chat_theme
      : APPEARANCE_DEFAULTS.chatTheme,
    visualStyle: isVisualStyle(settings.visual_style)
      ? settings.visual_style
      : APPEARANCE_DEFAULTS.visualStyle,
  };
}

/**
 * Next index of a radio group for an arrow, Home or End key (roving focus). Returns null for any
 * other key. Arrow keys wrap around; Home and End jump to the ends.
 */
export function nextRadioIndex(
  key: string,
  current: number,
  count: number,
): number | null {
  if (count <= 0) return null;
  switch (key) {
    case "ArrowRight":
    case "ArrowDown":
      return (current + 1) % count;
    case "ArrowLeft":
    case "ArrowUp":
      return (current - 1 + count) % count;
    case "Home":
      return 0;
    case "End":
      return count - 1;
    default:
      return null;
  }
}

/**
 * The tabindex of each radio of a roving group: only the selected radio is a tab stop. When
 * nothing is selected the first radio is the stop, so the group stays reachable.
 */
export function rovingTabStops(selected: number, count: number): number[] {
  const stop = selected >= 0 && selected < count ? selected : 0;
  return Array.from({ length: count }, (_, index) => (index === stop ? 0 : -1));
}

/**
 * Search rows that moved or were renamed when the Appearance page was added. A saved query or
 * deep link that still names an old id resolves to the new one. Other ids pass through.
 */
const LEGACY_SEARCH_IDS: Readonly<Record<string, string>> = {
  "general.theme": "appearance.theme",
  "general.customThemeText": "appearance.customThemeText",
  "general.customThemeBackground": "appearance.customThemeBackground",
  "general.customThemeAccent": "appearance.customThemeAccent",
  "general.customThemeAccentSecondary": "appearance.customThemeAccentSecondary",
  "general.customThemeWarning": "appearance.customThemeWarning",
  "general.customThemeError": "appearance.customThemeError",
  "general.chatOpacity": "appearance.chatOpacity",
};

export function resolveSearchEntryId(id: string): string {
  return LEGACY_SEARCH_IDS[id] ?? id;
}
