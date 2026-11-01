import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import Badge from "../ui/Badge";
import { useSettings } from "../../hooks/useSettings";
import { commands, type AutoLanguageStatus } from "@/bindings";

interface AutoLanguageRowProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * Automatic language detection row in the speech-to-text settings (opt-in, experimental): a toggle
 * for `stt_auto_language_enabled` with an Experimental badge, and a status line under it while the
 * toggle is on. The status comes from `auto_language_status`; a failed read shows no status line.
 */
export const AutoLanguageRow: React.FC<AutoLanguageRowProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const enabled = getSetting("stt_auto_language_enabled") ?? false;
  const [status, setStatus] = useState<AutoLanguageStatus | null>(null);

  // Read on mount and again whenever the toggle changes.
  useEffect(() => {
    let active = true;
    commands
      .autoLanguageStatus()
      .then((next) => {
        if (active) setStatus(next);
      })
      .catch((error: unknown) => {
        console.warn("Reading the automatic language status failed:", error);
        if (active) setStatus(null);
      });
    return () => {
      active = false;
    };
  }, [enabled]);

  let statusLine: string | null = null;
  if (enabled && status !== null) {
    if (status.lid_model_id === null) {
      statusLine = t("settings.modelSettings.autoLanguage.statusNeedsModel");
    } else {
      const name = status.lid_model_name ?? status.lid_model_id;
      statusLine = status.lid_loaded
        ? t("settings.modelSettings.autoLanguage.statusModelLoaded", { name })
        : t("settings.modelSettings.autoLanguage.statusModel", { name });
    }
  }

  return (
    <>
      <ToggleSwitch
        checked={enabled}
        onChange={(value) => updateSetting("stt_auto_language_enabled", value)}
        isUpdating={isUpdating("stt_auto_language_enabled")}
        label={t("settings.modelSettings.autoLanguage.title")}
        description={t("settings.modelSettings.autoLanguage.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
        tooltipPosition="bottom"
        badge={
          <Badge variant="secondary">
            {t("settings.modelSettings.autoLanguage.experimentalBadge")}
          </Badge>
        }
      />
      {statusLine !== null && (
        <div
          className="flex flex-col gap-1 px-3 pb-2"
          data-testid="auto-language-status-block"
        >
          <p className="text-xs text-muted" data-testid="auto-language-status">
            {statusLine}
          </p>
        </div>
      )}
    </>
  );
};
