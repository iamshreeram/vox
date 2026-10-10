import React from "react";
import { useTranslation } from "react-i18next";
import { SettingContainer } from "../ui/SettingContainer";
import { useSettings } from "@/hooks/useSettings";
import { applyTheme, DEFAULT_CUSTOM_THEME_COLORS } from "@/lib/utils/theme";
import type { CustomThemeColors } from "@/bindings";

/**
 * Color pickers for the "custom" theme. Only rendered by `ThemeSelector`
 * when `theme === "custom"`.
 *
 * Reuses the app's existing generic settings-update pattern (see
 * `settingsStore.ts`'s `updateSetting`): each change computes the full
 * updated `CustomThemeColors` object from the CURRENT store value (never a
 * stale closure) and calls `applyTheme` (instant local visual feedback) +
 * `updateSetting("custom_theme_colors", merged)` (optimistic store update,
 * persists via `update_custom_theme_colors`, automatically rolls the store
 * back to the last-good value if that command errors) -- the exact same
 * side-by-side pattern `ThemeSelector.handleThemeChange` already uses for
 * the theme dropdown itself. No new synchronization machinery.
 */

const FIELDS: { key: keyof CustomThemeColors; labelKey: string }[] = [
  { key: "text", labelKey: "theme.custom.text" },
  { key: "background", labelKey: "theme.custom.background" },
  { key: "accent", labelKey: "theme.custom.accent" },
  { key: "accent_secondary", labelKey: "theme.custom.accentSecondary" },
  { key: "warning", labelKey: "theme.custom.warning" },
  { key: "error", labelKey: "theme.custom.error" },
];

export const CustomThemeEditor: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting } = useSettings();

  const colors: CustomThemeColors =
    getSetting("custom_theme_colors") ?? DEFAULT_CUSTOM_THEME_COLORS;

  const handleFieldChange = (field: keyof CustomThemeColors, value: string) => {
    const merged: CustomThemeColors = { ...colors, [field]: value };
    applyTheme("custom", merged);
    updateSetting("custom_theme_colors", merged);
  };

  return (
    <div className="pl-4 border-l-2 border-mid-gray/20 ml-2">
      {FIELDS.map(({ key, labelKey }) => (
        <SettingContainer
          key={key}
          title={t(labelKey)}
          description={t("theme.custom.description")}
          descriptionMode="tooltip"
          grouped={true}
        >
          <div className="flex items-center gap-2">
            <input
              type="color"
              value={colors[key]}
              onChange={(e) => handleFieldChange(key, e.target.value)}
              className="h-8 w-12 cursor-pointer rounded border border-mid-gray/20 bg-transparent p-0"
              aria-label={t(labelKey)}
            />
            <span className="text-sm font-mono text-mid-gray">
              {colors[key]}
            </span>
          </div>
        </SettingContainer>
      ))}
    </div>
  );
};

CustomThemeEditor.displayName = "CustomThemeEditor";
