import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands } from "@/bindings";
import { useSettings } from "../../hooks/useSettings";
import { Button } from "../ui/Button";
import { ToggleSwitch } from "../ui/ToggleSwitch";

interface AmbientModeToggleProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const AmbientModeToggle: React.FC<AmbientModeToggleProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const [isClearing, setIsClearing] = useState(false);

    const ambientModeEnabled = getSetting("ambient_mode_enabled") ?? false;
    const wakeWordEnabled = getSetting("wake_word_enabled") ?? false;

    const handleClear = async () => {
      setIsClearing(true);
      try {
        const result = await commands.ambientClear();
        if (result.status === "ok") {
          toast.success(t("settings.advanced.ambientMode.clearedToast"));
        } else {
          toast.error(result.error);
        }
      } catch (error) {
        toast.error(String(error));
      } finally {
        setIsClearing(false);
      }
    };

    return (
      <>
        <div title={wakeWordEnabled ? t("settings.advanced.ambientMode.disabledByWakeWord") : undefined}>
          <ToggleSwitch
            checked={ambientModeEnabled}
            onChange={(enabled) => updateSetting("ambient_mode_enabled", enabled)}
            isUpdating={isUpdating("ambient_mode_enabled")}
            disabled={wakeWordEnabled}
            label={t("settings.advanced.ambientMode.label")}
            description={t("settings.advanced.ambientMode.description")}
            descriptionMode={descriptionMode}
            grouped={grouped}
            tooltipPosition="bottom"
          />
        </div>
        {ambientModeEnabled && (
          <div className="px-4 pb-2">
            <Button
              variant="secondary"
              size="sm"
              onClick={handleClear}
              disabled={isClearing}
            >
              {t("settings.advanced.ambientMode.clearButton")}
            </Button>
          </div>
        )}
      </>
    );
  },
);
