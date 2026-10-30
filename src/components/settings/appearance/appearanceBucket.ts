import { useEffect, useState } from "react";
import { resolveAppearance, type Bucket } from "@/lib/appearance/palette";
import { effectiveAppearance } from "./appearanceModel";

const DARK_QUERY = "(prefers-color-scheme: dark)";

/** The OS colour scheme, kept current so System follows a change while the page is open. */
export function usePrefersDark(): boolean {
  const [prefersDark, setPrefersDark] = useState<boolean>(() =>
    typeof window.matchMedia === "function"
      ? window.matchMedia(DARK_QUERY).matches
      : false,
  );
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const query = window.matchMedia(DARK_QUERY);
    const onChange = (event: MediaQueryListEvent) =>
      setPrefersDark(event.matches);
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, []);
  return prefersDark;
}

/**
 * The light or dark bucket the app renders in for these settings (the same rule the preview
 * uses). The orb colour controls take their swatch colours from this bucket.
 */
export function appearanceBucket(
  settings: {
    theme?: unknown;
    accent?: unknown;
    chat_theme?: unknown;
    visual_style?: unknown;
    custom_theme_colors?: unknown;
  } | null,
  prefersDark: boolean,
): Bucket {
  const choice = effectiveAppearance(settings ?? {});
  return resolveAppearance(
    {
      rev: 0,
      theme: choice.theme,
      custom_colors:
        choice.theme === "custom"
          ? ((settings?.custom_theme_colors as never) ?? null)
          : null,
      accent: choice.accent,
      chat_theme: choice.chatTheme,
      visual_style: choice.visualStyle,
      orb_accent: null,
    },
    prefersDark,
  ).bucket;
}
