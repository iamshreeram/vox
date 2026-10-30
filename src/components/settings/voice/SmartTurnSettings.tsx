import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../../ui/ToggleSwitch";
import { useSettings } from "../../../hooks/useSettings";
import { formatMegabytes } from "../naturalVoiceState";
import { AuxModelCard } from "./AuxModelCard";
import { useAuxModel } from "./useAuxModel";
import { voiceModelCard } from "./voiceModelCard";
import {
  smartTurnFallbackNote,
  smartTurnToggleEnabled,
  toggleDisabled,
} from "./voiceGates";

interface SmartTurnSettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * Smart turn: the on/off switch (enabled only while the model is Ready) and
 * the model card. When the switch is on but the model is missing, a note says
 * the fixed silence rule is used.
 */
export const SmartTurnSettings: React.FC<SmartTurnSettingsProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const model = useAuxModel("smart_turn");

  const loaded = model.status !== null;
  const ready = model.status?.state === "ready";
  const enabled = getSetting("converse_smart_turn") ?? false;
  const view = voiceModelCard(
    model.status?.state ?? "not_installed",
    model.downloaded,
    model.total,
    model.error,
  );

  return (
    <>
      <ToggleSwitch
        checked={enabled}
        onChange={(next) => void updateSetting("converse_smart_turn", next)}
        disabled={
          toggleDisabled(enabled, smartTurnToggleEnabled(ready)) ||
          isUpdating("converse_smart_turn")
        }
        isUpdating={isUpdating("converse_smart_turn")}
        label={t("converse.voice.smartTurn.label")}
        description={t("converse.voice.smartTurn.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      />
      {smartTurnFallbackNote(enabled, ready) && (
        <p className="px-3 text-xs text-muted">
          {t("converse.voice.smartTurn.fallbackNote")}
        </p>
      )}
      <AuxModelCard
        title={t("converse.voice.smartTurn.modelLabel")}
        description={
          model.status === null
            ? t("converse.voice.smartTurn.modelDescriptionPending")
            : t("converse.voice.smartTurn.modelDescription", {
                size: formatMegabytes(model.status.size_bytes),
              })
        }
        descriptionMode={descriptionMode}
        grouped={grouped}
        view={view}
        loaded={loaded}
        onDownload={() => void model.download()}
        onCancel={() => void model.cancel()}
        onDelete={() => void model.remove()}
      />
    </>
  );
};
