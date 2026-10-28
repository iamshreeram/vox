import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands } from "@/bindings";
import { useSettings } from "../../hooks/useSettings";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { SettingContainer } from "../ui/SettingContainer";
import { Slider } from "../ui/Slider";
import { ToggleSwitch } from "../ui/ToggleSwitch";

// Bounds mirror the authoritative Rust validator in settings.rs
// (AMBIENT_WINDOW_MIN_MS / AMBIENT_WINDOW_MAX_MS).
const WINDOW_MIN_SECONDS = 10;
const WINDOW_MAX_SECONDS = 600;

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
    const ambientWakeName = getSetting("ambient_wake_name") ?? "Vox";
    const ambientWindowMs = getSetting("ambient_window_ms") ?? 90000;

    // Local draft for the name field. It re-syncs from the stored value, so a
    // backend rejection (the store reverts the setting) also reverts the field.
    const [draftName, setDraftName] = useState(ambientWakeName);
    useEffect(() => {
      setDraftName(ambientWakeName);
    }, [ambientWakeName]);

    const commitName = () => {
      const trimmed = draftName.trim();
      if (trimmed === ambientWakeName) {
        setDraftName(ambientWakeName);
        return;
      }
      void updateSetting("ambient_wake_name", trimmed);
    };

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
        <div
          title={
            wakeWordEnabled
              ? t("settings.advanced.ambientMode.disabledByWakeWord")
              : undefined
          }
        >
          <ToggleSwitch
            checked={ambientModeEnabled}
            onChange={(enabled) =>
              updateSetting("ambient_mode_enabled", enabled)
            }
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
          <div className="px-3 pb-2">
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
        <SettingContainer
          title={t("settings.advanced.ambientMode.wakeNameLabel")}
          description={t("settings.advanced.ambientMode.wakeNameDescription")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        >
          <Input
            type="text"
            value={draftName}
            onChange={(event) => setDraftName(event.target.value)}
            onBlur={commitName}
            onKeyDown={(event) => {
              if (event.key === "Enter") event.currentTarget.blur();
            }}
            aria-label={t("settings.advanced.ambientMode.wakeNameLabel")}
            variant="compact"
          />
        </SettingContainer>
        <Slider
          value={Math.round(ambientWindowMs / 1000)}
          onChange={(next) =>
            void updateSetting("ambient_window_ms", Math.round(next) * 1000)
          }
          min={WINDOW_MIN_SECONDS}
          max={WINDOW_MAX_SECONDS}
          step={5}
          label={t("settings.advanced.ambientMode.windowLabel")}
          description={t("settings.advanced.ambientMode.windowDescription")}
          descriptionMode={descriptionMode}
          grouped={grouped}
          formatValue={(v) =>
            t("settings.advanced.ambientMode.windowValue", {
              value: Math.round(v),
            })
          }
        />
      </>
    );
  },
);
