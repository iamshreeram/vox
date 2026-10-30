import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSettings } from "@/hooks/useSettings";
import { CHAT_PALETTES } from "@/chat/chatTheme";
import { resolveAppearance } from "@/lib/appearance/palette";
import { SettingContainer } from "../../ui/SettingContainer";
import { effectiveAppearance } from "./appearanceModel";

const DARK_QUERY = "(prefers-color-scheme: dark)";

/** The OS colour scheme, kept current so System follows a change while the page is open. */
function usePrefersDark(): boolean {
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
 * A live sample of the chosen look: an orb, bubbles and a primary button drawn with the shared
 * tokens (so the accent follows accents.ts), plus the conversation palette the chosen setting
 * resolves to. CSS only: no canvas, no blur.
 */
export const AppearancePreview: React.FC = () => {
  const { t } = useTranslation();
  const { settings } = useSettings();
  const prefersDark = usePrefersDark();
  const choice = effectiveAppearance(settings ?? {});
  const resolved = resolveAppearance(
    {
      rev: 0,
      theme: choice.theme,
      custom_colors:
        choice.theme === "custom"
          ? (settings?.custom_theme_colors ?? null)
          : null,
      accent: choice.accent,
      chat_theme: choice.chatTheme,
      visual_style: choice.visualStyle,
    },
    prefersDark,
  );
  const palette = CHAT_PALETTES[resolved.chatPalette];
  const paletteLabel =
    resolved.chatPalette === "light"
      ? t("settings.appearance.chatTheme.light")
      : t("settings.appearance.chatTheme.dark");

  return (
    <SettingContainer
      title={t("settings.appearance.preview.title")}
      description={t("settings.appearance.preview.description")}
      descriptionMode="inline"
      layout="stacked"
    >
      <div className="grid gap-3 sm:grid-cols-2">
        <div
          className="flex flex-col gap-3 rounded-lg border border-mid-gray/30 p-3"
          style={{ color: "var(--color-text)" }}
        >
          <div className="flex items-center gap-3">
            <span
              aria-hidden="true"
              data-testid="appearance-preview-orb"
              className="h-12 w-12 shrink-0 rounded-full"
              style={{
                background:
                  "radial-gradient(circle at 35% 30%, var(--color-accent-a), var(--color-accent-b))",
              }}
            />
            <span className="text-sm">
              {t("settings.appearance.preview.orb")}
            </span>
          </div>
          <span
            data-testid="appearance-preview-assistant"
            className="max-w-[85%] rounded-lg px-3 py-1.5 text-sm"
            style={{
              backgroundColor: "var(--color-accent-soft)",
              color: "var(--color-text)",
            }}
          >
            {t("settings.appearance.preview.assistant")}
          </span>
          <span
            data-testid="appearance-preview-user"
            className="max-w-[85%] self-end rounded-lg px-3 py-1.5 text-sm"
            style={{
              backgroundColor: "var(--color-accent-a)",
              color: "var(--color-on-accent)",
            }}
          >
            {t("settings.appearance.preview.user")}
          </span>
          <span
            aria-hidden="true"
            data-testid="appearance-preview-primary"
            className="inline-flex w-fit items-center rounded-lg px-3 py-1.5 text-sm font-medium"
            style={{
              backgroundColor: "var(--color-accent-a)",
              color: "var(--color-on-accent)",
            }}
          >
            {t("settings.appearance.preview.action")}
          </span>
        </div>
        <div
          data-testid="appearance-preview-chat"
          data-palette={resolved.chatPalette}
          className="flex flex-col gap-2 rounded-lg border p-3"
          style={{
            backgroundColor: palette.surface,
            color: palette.text,
            borderColor: palette.border,
          }}
        >
          <p className="text-xs" style={{ color: palette.muted }}>
            {t("settings.appearance.preview.chat", { palette: paletteLabel })}
          </p>
          <span
            className="max-w-[85%] self-end rounded-lg px-3 py-1.5 text-sm"
            style={{
              backgroundColor: palette.userBubble,
              color: palette.userText,
            }}
          >
            {t("settings.appearance.preview.user")}
          </span>
          <span className="max-w-[85%] text-sm" style={{ color: palette.text }}>
            {t("settings.appearance.preview.assistant")}
          </span>
        </div>
      </div>
    </SettingContainer>
  );
};
