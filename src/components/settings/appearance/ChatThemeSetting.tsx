import React from "react";
import { useTranslation } from "react-i18next";
import type { ChatTheme } from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { SettingContainer } from "../../ui/SettingContainer";
import { AppearanceRadioGroup, type RadioChoice } from "./AppearanceRadioGroup";
import { effectiveAppearance } from "./appearanceModel";

interface ChatThemeSettingProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * Colours of the conversation window: follow the app, or fix Dark or Light. The window has no
 * title bar buttons (macOS hides them); a note says how to drag it and hide it (DESIGN D1, D1b).
 */
export const ChatThemeSetting: React.FC<ChatThemeSettingProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const { settings, updateSetting } = useSettings();
  const { chatTheme } = effectiveAppearance(settings ?? {});

  const choices: RadioChoice<ChatTheme>[] = [
    {
      value: "follow_app",
      label: t("settings.appearance.chatTheme.followApp"),
    },
    { value: "dark", label: t("settings.appearance.chatTheme.dark") },
    { value: "light", label: t("settings.appearance.chatTheme.light") },
  ];

  return (
    <SettingContainer
      title={t("settings.appearance.chatTheme.title")}
      description={t("settings.appearance.chatTheme.description")}
      descriptionMode={descriptionMode}
      grouped={grouped}
      layout="stacked"
    >
      <div className="flex flex-col gap-2">
        <AppearanceRadioGroup<ChatTheme>
          label={t("settings.appearance.chatTheme.title")}
          choices={choices}
          value={chatTheme}
          testIdPrefix="chat-theme-option"
          onChange={(value) => {
            if (value !== chatTheme) void updateSetting("chat_theme", value);
          }}
        />
        <p className="text-xs text-muted">
          {t("settings.appearance.chatTheme.nativeNote")}
        </p>
      </div>
    </SettingContainer>
  );
};
