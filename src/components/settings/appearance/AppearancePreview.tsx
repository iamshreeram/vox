import React from "react";
import { useTranslation } from "react-i18next";
import { useSettings } from "@/hooks/useSettings";
import { CHAT_PALETTES } from "@/chat/chatTheme";
import { resolveAppearance } from "@/lib/appearance/palette";
import { resolveOrbAccent, orbRingColour } from "@/orb/orbAccentResolve";
import VoxAvatar from "@/chat/VoxAvatar";
import "@/chat/avatar.css";
import { SettingContainer } from "../../ui/SettingContainer";
import { effectiveAppearance, effectiveOrbAccent } from "./appearanceModel";
import { appearanceBucket, usePrefersDark } from "./appearanceBucket";

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
      orb_accent: effectiveOrbAccent(settings?.orb_accent),
    },
    prefersDark,
  );
  // The orb ring follows the orb colour, or the app accent under Follow app (DESIGN D3).
  const orbColor = effectiveOrbAccent(settings?.orb_accent);
  const orbRing = orbRingColour(
    "idle",
    resolveOrbAccent(choice.accent, orbColor, resolved.bucket),
    resolved.bucket,
  );
  // The same avatar the orb page and the conversation window draw, on the preview's own bucket.
  const avatarLook = {
    orbColor,
    appAccent: choice.accent,
    bucket: resolved.bucket,
  };
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
              data-ring={orbRing}
              className="relative h-12 w-12 shrink-0 overflow-hidden"
            >
              <span className="absolute left-1/2 top-1/2 block -translate-x-1/2 -translate-y-1/2 scale-[0.325]">
                <VoxAvatar face="idle" look={avatarLook} />
              </span>
            </span>
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
