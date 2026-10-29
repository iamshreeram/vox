import React from "react";
import { useTranslation } from "react-i18next";
import type { VisualStyle } from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { applyTheme } from "@/lib/utils/theme";
import { SettingContainer } from "../../ui/SettingContainer";
import { AppearanceRadioGroup, type RadioChoice } from "./AppearanceRadioGroup";
import { effectiveAppearance } from "./appearanceModel";

interface VisualStyleSettingProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * Vibrant (the default look) or Restrained (neutral cards, accent only on selection and primary
 * actions; DESIGN A5). The main window changes at once; the store saves and publishes the value.
 */
export const VisualStyleSetting: React.FC<VisualStyleSettingProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const { settings, updateSetting } = useSettings();
  const { theme, accent, visualStyle } = effectiveAppearance(settings ?? {});

  const choices: RadioChoice<VisualStyle>[] = [
    {
      value: "vibrant",
      label: t("settings.appearance.style.vibrant"),
      hint: t("settings.appearance.style.vibrantHint"),
    },
    {
      value: "restrained",
      label: t("settings.appearance.style.restrained"),
      hint: t("settings.appearance.style.restrainedHint"),
    },
  ];

  const change = (value: VisualStyle): void => {
    if (value === visualStyle) return;
    applyTheme(theme, settings?.custom_theme_colors, accent, value);
    void updateSetting("visual_style", value);
  };

  return (
    <SettingContainer
      title={t("settings.appearance.style.title")}
      description={t("settings.appearance.style.description")}
      descriptionMode={descriptionMode}
      grouped={grouped}
      layout="stacked"
    >
      <AppearanceRadioGroup<VisualStyle>
        label={t("settings.appearance.style.title")}
        choices={choices}
        value={visualStyle}
        testIdPrefix="visual-style-option"
        onChange={change}
      />
    </SettingContainer>
  );
};
