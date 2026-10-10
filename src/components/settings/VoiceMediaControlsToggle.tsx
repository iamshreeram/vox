import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";

interface VoiceMediaControlsToggleProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const VoiceMediaControlsToggle: React.FC<VoiceMediaControlsToggleProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const mediaControlsEnabled =
      getSetting("voice_media_controls_enabled") ?? false;
    // Media controls only act while Voice Commands is on, so the toggle is
    // greyed out (not hidden) until the master switch is enabled.
    const voiceCommandsEnabled = getSetting("voice_commands_enabled") ?? false;

    return (
      <ToggleSwitch
        checked={mediaControlsEnabled}
        onChange={(enabled) =>
          updateSetting("voice_media_controls_enabled", enabled)
        }
        isUpdating={isUpdating("voice_media_controls_enabled")}
        disabled={!voiceCommandsEnabled}
        label={t("settings.advanced.voiceMediaControls.label")}
        description={t("settings.advanced.voiceMediaControls.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
        tooltipPosition="bottom"
      />
    );
  });
