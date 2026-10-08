import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";

interface WakeWordToggleProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const WakeWordToggle: React.FC<WakeWordToggleProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const wakeWordEnabled = getSetting("wake_word_enabled") ?? false;
    const ambientModeEnabled = getSetting("ambient_mode_enabled") ?? false;

    return (
      <div title={ambientModeEnabled ? t("settings.advanced.wakeWord.disabledByAmbientMode") : undefined}>
        <ToggleSwitch
          checked={wakeWordEnabled}
          onChange={(enabled) => updateSetting("wake_word_enabled", enabled)}
          isUpdating={isUpdating("wake_word_enabled")}
          disabled={ambientModeEnabled}
          label={t("settings.advanced.wakeWord.label")}
          description={t("settings.advanced.wakeWord.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
          tooltipPosition="bottom"
        />
      </div>
    );
  },
);
