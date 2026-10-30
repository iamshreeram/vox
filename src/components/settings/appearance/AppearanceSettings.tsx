import React from "react";
import { useTranslation } from "react-i18next";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { PlacedGroups } from "../PlacedGroups";
import { AccentPicker } from "./AccentPicker";
import { AppearancePreview } from "./AppearancePreview";
import { ChatThemeSetting } from "./ChatThemeSetting";
import { ResetAppearance } from "./ResetAppearance";
import { ThemeSelector } from "./ThemeSelector";
import { VisualStyleSetting } from "./VisualStyleSetting";

/**
 * The Appearance page: theme, accent, visual style, conversation window theme and transparency,
 * a live preview, and Reset to defaults. Every control writes through the settings store, and the
 * backend publishes the committed snapshot to the other windows.
 */
export const AppearanceSettings: React.FC = () => {
  const { t } = useTranslation();
  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <SettingsGroup title={t("settings.appearance.title")}>
        <ThemeSelector descriptionMode="tooltip" grouped={true} />
        <AccentPicker descriptionMode="tooltip" grouped={true} />
        <VisualStyleSetting descriptionMode="tooltip" grouped={true} />
        <ChatThemeSetting descriptionMode="tooltip" grouped={true} />
        <AppearancePreview />
        <ResetAppearance />
      </SettingsGroup>
      <PlacedGroups section="appearance" />
    </div>
  );
};
