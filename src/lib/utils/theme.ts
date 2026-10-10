import { commands, type CustomThemeColors, type Theme } from "@/bindings";

/**
 * Appearance theme handling.
 *
 * Vox ships three fixed palettes (System/Light/Dark) plus a fixed "Aurora"
 * accent palette, and a 5th "custom" theme whose colors the user edits
 * directly (see `CustomThemeEditor.tsx`). This module is the single place
 * that turns an appearance (theme + the current custom palette) into actual
 * CSS, for both the main Settings window and the separate recording-overlay
 * window (a distinct webview with its own `document`).
 *
 * `system` removes the `data-theme` override so the `prefers-color-scheme`
 * media query governs. `light`/`dark`/`aurora` set `data-theme` and rely on
 * the fixed blocks in theme.css/RecordingOverlay.css. `custom` ALSO sets
 * `data-theme="custom"` (whose block in theme.css holds placeholder/default
 * values, used only before the real colors below are applied) AND sets each
 * `--color-*` custom property directly as an inline style on `<html>` --
 * inline style has higher specificity than any attribute-selector CSS rule,
 * so it always wins once applied.
 *
 * Cross-window sync: `update_custom_theme_colors`/`change_theme_setting`
 * emit the EXISTING `theme-changed` event. Rather than trusting that event's
 * payload directly (which would require solving cross-window event-ordering
 * for two genuinely concurrent writers -- see the project history for why
 * that was rejected as disproportionate for a cosmetic settings feature),
 * every listener treats the event as a plain "something changed, go
 * re-fetch" signal and calls `syncAppearanceFromSettings()`, which pulls the
 * one current source of truth. A small request-generation guard inside that
 * function ensures a slow, now-superseded fetch can never clobber a newer
 * one that already resolved.
 */

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

/** Maps each `CustomThemeColors` field to the CSS custom property it drives. */
const CUSTOM_PROPERTY_MAP: Record<keyof CustomThemeColors, string> = {
  text: "--color-text",
  background: "--color-background",
  accent: "--color-logo-primary",
  accent_secondary: "--color-logo-stroke",
  warning: "--color-warning",
  error: "--color-error",
};

/** The recording-overlay's own neutral tokens that don't map 1:1 to a single
 * custom color -- for `custom`, these borrow whichever existing light/dark
 * pair (already defined in RecordingOverlay.css) is the closer visual match
 * for the chosen background, rather than requiring 5 more color pickers. */
const OVERLAY_NEUTRAL_PROPERTIES = [
  "--s-accent-soft",
  "--s-muted",
  "--s-faint",
  "--s-border",
  "--s-hair",
] as const;

/**
 * WCAG-style relative luminance of a `#rrggbb` hex color, in [0, 1]. This is
 * a practical brightness-matching heuristic (used to pick a light/dark
 * neutral bucket), NOT a contrast-ratio/accessibility claim. MUST stay in
 * sync with `relative_luminance` in `src-tauri/src/settings.rs` -- same
 * formula, duplicated here only because this one cosmetic CSS decision isn't
 * worth a backend round-trip. Returns `null` for an unparseable string.
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

/** Mirrors `native_chrome_for_hex_background` in settings.rs: Light above the
 * 0.5 midpoint, Dark otherwise (including for an unparseable hex). */
export const neutralBucketFor = (backgroundHex: string): "light" | "dark" => {
  const luminance = relativeLuminance(backgroundHex);
  return luminance !== null && luminance > 0.5 ? "light" : "dark";
};

export interface Appearance {
  theme: Theme;
  customThemeColors: CustomThemeColors;
}

/** Apply an appearance to the document root and remember it for next launch. */
export const applyTheme = (
  theme: Theme,
  customThemeColors: CustomThemeColors = DEFAULT_CUSTOM_THEME_COLORS,
): void => {
  const root = document.documentElement;

  // Always clear every custom-theme inline override first. Leaving a stale
  // inline property set would defeat the CSS of whatever preset is applied
  // next (inline style outranks the `data-theme="light"`/etc. selectors).
  for (const property of Object.values(CUSTOM_PROPERTY_MAP)) {
    root.style.removeProperty(property);
  }
  for (const property of OVERLAY_NEUTRAL_PROPERTIES) {
    root.style.removeProperty(property);
  }

  if (theme === "system") {
    delete root.dataset.theme;
  } else {
    root.dataset.theme = theme;
  }

  if (theme === "custom") {
    for (const [field, property] of Object.entries(CUSTOM_PROPERTY_MAP) as [
      keyof CustomThemeColors,
      string,
    ][]) {
      root.style.setProperty(property, customThemeColors[field]);
    }
    const bucket = neutralBucketFor(customThemeColors.background);
    for (const property of OVERLAY_NEUTRAL_PROPERTIES) {
      // Reuse the existing --light-s-*/--dark-s-* values already defined in
      // RecordingOverlay.css by referencing them, rather than duplicating
      // their actual color values here.
      root.style.setProperty(
        property,
        `var(--${bucket}-s${property.slice(2)})`,
      );
    }
  }

  try {
    localStorage.setItem(
      THEME_STORAGE_KEY,
      JSON.stringify({ theme, customThemeColors }),
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
        };
      }
    }
  } catch {
    // ignore -- malformed/legacy cache, fall through to the default below
  }
  return { theme: "system", customThemeColors: DEFAULT_CUSTOM_THEME_COLORS };
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
      );
    }
  } catch (e) {
    console.warn("Failed to sync appearance from settings:", e);
  }
};
