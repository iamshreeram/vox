import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type TamilVoiceStatus } from "@/bindings";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";

interface TamilVoiceRowProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * Tamil voice row in the TTS settings: a toggle for `tts_tamil_enabled` and a status line
 * from the `tamil_voice_status` command. The status is refetched on mount and whenever the
 * toggle changes. If the command fails, no status line is shown.
 */
export const TamilVoiceRow: React.FC<TamilVoiceRowProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const enabled = getSetting("tts_tamil_enabled") ?? true;
  const [status, setStatus] = useState<TamilVoiceStatus | null>(null);

  useEffect(() => {
    let active = true;
    commands
      .tamilVoiceStatus()
      .then((next) => {
        if (active) setStatus(next);
      })
      .catch((error: unknown) => {
        console.warn("Reading the Tamil voice status failed:", error);
        if (active) setStatus(null);
      });
    return () => {
      active = false;
    };
  }, [enabled]);

  let statusLine: React.ReactNode = null;
  let statusTone = "text-muted";
  if (status !== null) {
    if (status.engine_error !== null) {
      statusTone = "text-danger";
      statusLine = t("settings.advanced.tts.tamil.statusError", {
        message: status.engine_error,
      });
    } else if (status.installed && status.ready) {
      statusLine = t("settings.advanced.tts.tamil.statusInstalled");
    } else if (status.wrong_size.length > 0 && status.missing.length === 0) {
      statusLine = t("settings.advanced.tts.tamil.statusIncomplete");
    } else if (!status.installed) {
      statusLine = t("settings.advanced.tts.tamil.statusMissing", {
        count: status.missing.length,
      });
    }
  }

  return (
    <>
      <ToggleSwitch
        checked={enabled}
        onChange={(value) => updateSetting("tts_tamil_enabled", value)}
        isUpdating={isUpdating("tts_tamil_enabled")}
        label={t("settings.advanced.tts.tamil.title")}
        description={t("settings.advanced.tts.tamil.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
        tooltipPosition="bottom"
      />
      <div
        className="flex flex-col gap-1 px-3 pb-2"
        data-testid="tamil-voice-status-block"
      >
        {statusLine !== null && (
          <p
            className={`text-xs ${statusTone}`}
            data-testid="tamil-voice-status"
          >
            {statusLine}
          </p>
        )}
        <p className="text-xs text-muted">
          {t("settings.advanced.tts.tamil.credit")}
        </p>
      </div>
    </>
  );
};
