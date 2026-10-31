import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";
import { useTamilSttStatus } from "../../hooks/useTamilSttStatus";

interface TamilSttRowProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * Tamil speech recognition row in the speech-to-text settings: a toggle for
 * `stt_tamil_enabled`, a status line from the `tamil_stt_status` command, the memory note and
 * the credit. Status follows the TamilVoiceRow pattern: with no status read, no status line is
 * shown; the note and the credit are always shown.
 */
export const TamilSttRow: React.FC<TamilSttRowProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const enabled = getSetting("stt_tamil_enabled") ?? true;
  const { status } = useTamilSttStatus();

  let statusLine: React.ReactNode = null;
  let statusTone = "text-muted";
  if (status !== null) {
    if (!enabled) {
      statusLine = t("settings.modelSettings.tamilStt.statusOff");
    } else if (status.engine_error !== null) {
      statusTone = "text-danger";
      statusLine = t("settings.modelSettings.tamilStt.statusError", {
        message: status.engine_error,
      });
    } else if (status.ready) {
      statusLine = t("settings.modelSettings.tamilStt.statusReady");
    } else if (!status.installed && status.wrong_size !== null) {
      statusLine = t("settings.modelSettings.tamilStt.statusWrongSize");
    } else if (!status.installed) {
      statusLine = t("settings.modelSettings.tamilStt.statusMissing", {
        count: status.missing.length,
        folder: status.dir,
      });
    }
  }

  return (
    <>
      <ToggleSwitch
        checked={enabled}
        onChange={(value) => updateSetting("stt_tamil_enabled", value)}
        isUpdating={isUpdating("stt_tamil_enabled")}
        label={t("settings.modelSettings.tamilStt.title")}
        description={t("settings.modelSettings.tamilStt.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
        tooltipPosition="bottom"
      />
      <div
        className="flex flex-col gap-1 px-3 pb-2"
        data-testid="tamil-stt-status-block"
      >
        {statusLine !== null && (
          <p className={`text-xs ${statusTone}`} data-testid="tamil-stt-status">
            {statusLine}
          </p>
        )}
        <p className="text-xs text-muted">
          {t("settings.modelSettings.tamilStt.memoryNote")}
        </p>
        <p className="text-xs text-muted">
          {t("settings.modelSettings.tamilStt.credit")}
        </p>
      </div>
    </>
  );
};
