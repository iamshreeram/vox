import {
  commands,
  type CustomThemeColors,
  type Theme,
  type VisualStyle,
} from "@/bindings";
import {
  applyResolvedAppearance,
  resolveAppearance,
} from "@/lib/appearance/palette";
import { ACCENT_IDS, DEFAULT_ACCENT, type AccentId } from "@/styles/accents";

/**
 * Appearance theme handling (main windows: settings and overlay).
 *
 * Vox ships three fixed palettes (System/Light/Dark) plus a fixed "Aurora"
 * accent palette, and a 5th "custom" theme whose colors the user edits
 * directly (see `CustomThemeEditor.tsx`). The accent (UI S2) applies only under
 * System/Light/Dark; Aurora and Custom own their palette. The pure rules live in
 * src/lib/appearance/palette.ts (shared with the listen-only pages); this module
 * adds the main-window cache and the settings sync.
 *
 * localStorage is a FIRST-PAINT CACHE for the main window only. Other windows
 * never read it: they take the committed snapshot from the `appearance` event.
 *
 * Cross-window sync: writers emit the legacy `theme-changed` event, a plain
 * "something changed, go re-fetch" signal, and `syncAppearanceFromSettings()`
 * pulls the persisted settings (the source of truth). A request-generation guard
 * makes sure a slow, superseded fetch can never clobber a newer one.
 */

export {
  applyResolvedAppearance,
  derivedAccentProperties,
  neutralBucketFor,
  onAccentFor,
  relativeLuminance,
  resolveAppearance,
} from "@/lib/appearance/palette";
export type { ResolvedAppearance } from "@/lib/appearance/palette";

export const THEME_STORAGE_KEY = "handy.theme";

export const THEME_OPTIONS: Theme[] = [
  "system",
  "light",
  "dark",
  "aurora",
  "custom",
];

const isTheme = (value: unknown): value is Theme =>
  value === "system" ||
  value === "light" ||
  value === "dark" ||
  value === "aurora" ||
  value === "custom";

const isAccent = (value: unknown): value is AccentId =>
  typeof value === "string" &&
  (ACCENT_IDS as readonly string[]).includes(value);

const isVisualStyle = (value: unknown): value is VisualStyle =>
  value === "vibrant" || value === "restrained";

/**
 * Must stay in sync with the Rust defaults in `settings.rs`
 * (`default_custom_text`, etc.) -- both are the "Aurora" palette's values,
 * used both as the Rust-side default and as this module's placeholder before
 * settings load.
 */
export const DEFAULT_CUSTOM_THEME_COLORS: CustomThemeColors = {
  text: "#f3f5f7",
  background: "#161a23",
  accent: "#2dd4bf",
  accent_secondary: "#a78bfa",
  warning: "#fbbf24",
  error: "#f87171",
};

export interface Appearance {
  theme: Theme;
  customThemeColors: CustomThemeColors;
  accent: AccentId;
  visualStyle: VisualStyle;
}

const DEFAULT_APPEARANCE: Appearance = {
  theme: "system",
  customThemeColors: DEFAULT_CUSTOM_THEME_COLORS,
  accent: DEFAULT_ACCENT,
  visualStyle: "vibrant",
};

/**
 * Apply an appearance to the document root and remember it for next launch.
 * `accent` and `visualStyle` default to the cached values, so a caller that only
 * changes the theme (the theme picker) does not reset the accent in this window.
 */
export const applyTheme = (
  theme: Theme,
  customThemeColors: CustomThemeColors = DEFAULT_CUSTOM_THEME_COLORS,
  accent?: AccentId,
  visualStyle?: VisualStyle,
): void => {
  const cached = getStoredAppearance();
  const nextAccent = accent ?? cached.accent;
  const nextStyle = visualStyle ?? cached.visualStyle;
  const resolved = resolveAppearance(
    {
      rev: 0,
      theme,
      custom_colors: theme === "custom" ? customThemeColors : null,
      accent: nextAccent,
      chat_theme: "dark",
      visual_style: nextStyle,
      orb_accent: "auto",
    },
    false,
  );
  applyResolvedAppearance(document.documentElement, resolved);

  try {
    localStorage.setItem(
      THEME_STORAGE_KEY,
      JSON.stringify({
        theme,
        customThemeColors,
        accent: nextAccent,
        visualStyle: nextStyle,
      }),
    );
  } catch {
    // localStorage may be unavailable (e.g. private mode); the setting still
    // persists in AppSettings, so this only costs a one-frame flash on boot.
  }
};

/** Read the last-applied appearance for synchronous boot-time application. */
export const getStoredAppearance = (): Appearance => {
  try {
    const stored = localStorage.getItem(THEME_STORAGE_KEY);
    if (stored) {
      const parsed = JSON.parse(stored);
      if (isTheme(parsed?.theme)) {
        return {
          theme: parsed.theme,
          customThemeColors: {
            ...DEFAULT_CUSTOM_THEME_COLORS,
            ...(parsed.customThemeColors ?? {}),
          },
          accent: isAccent(parsed.accent) ? parsed.accent : DEFAULT_ACCENT,
          visualStyle: isVisualStyle(parsed.visualStyle)
            ? parsed.visualStyle
            : "vibrant",
        };
      }
    }
  } catch {
    // ignore -- malformed/legacy cache, fall through to the default below
  }
  return DEFAULT_APPEARANCE;
};

/** Back-compat: callers that only need the synchronous boot-time theme. */
export const getStoredTheme = (): Theme => getStoredAppearance().theme;

// Monotonically increasing id guarding against a slow, now-superseded fetch
// applying its (stale) result after a newer one already resolved. Not
// persisted -- reset to 0 on every page load, which is exactly the scope
// that matters (ordering only needs to hold within one running window).
let latestSyncId = 0;

/** Apply the persisted appearance from AppSettings (the source of truth). */
export const syncAppearanceFromSettings = async (): Promise<void> => {
  const mySyncId = ++latestSyncId;
  try {
    const result = await commands.getAppSettings();
    if (mySyncId !== latestSyncId) {
      // A newer sync was started (and may already have resolved) while this
      // one was in flight -- applying this stale result now would be able to
      // revert a newer, already-applied appearance. Drop it.
      return;
    }
    if (result.status === "ok") {
      applyTheme(
        result.data.theme ?? "system",
        result.data.custom_theme_colors ?? DEFAULT_CUSTOM_THEME_COLORS,
        result.data.accent,
        result.data.visual_style,
      );
    }
  } catch (e) {
    console.warn("Failed to sync appearance from settings:", e);
  }
};
