import React, { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  type SpeakerSensitivity,
  type SpeakerStatus,
} from "@/bindings";
import { Button } from "../../ui/Button";
import { Dropdown } from "../../ui/Dropdown";
import { SettingContainer } from "../../ui/SettingContainer";
import { ToggleSwitch } from "../../ui/ToggleSwitch";
import { useSettings } from "../../../hooks/useSettings";
import { formatMegabytes } from "../naturalVoiceState";
import { AuxModelCard } from "./AuxModelCard";
import { SpeakerEnrollWizard } from "./SpeakerEnrollWizard";
import { useAuxModel } from "./useAuxModel";
import { voiceModelCard } from "./voiceModelCard";
import {
  allowsDeleteVoiceprint,
  speakerFilterInactiveNote,
  speakerFilterToggleEnabled,
  toggleDisabled,
} from "./voiceGates";

interface SpeakerFilterSettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

const SENSITIVITIES: SpeakerSensitivity[] = ["lenient", "balanced", "strict"];

/** Size of the speaker model download (docs/voice-features.md). Shown until the status loads. */
const SPEAKER_MODEL_SIZE_BYTES = 26_534_365;

/**
 * The speaker filter. It is a convenience filter, not security, and the
 * disclosure stays visible next to the switch. The switch needs a Ready model
 * and an enrolled voice (a switch that is already on can still be turned off).
 */
export const SpeakerFilterSettings: React.FC<SpeakerFilterSettingsProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const model = useAuxModel("speaker_resnet_34");
  const [status, setStatus] = useState<SpeakerStatus | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [deleteFailed, setDeleteFailed] = useState(false);
  const [busy, setBusy] = useState(false);

  const refreshStatus = useCallback(async () => {
    try {
      setStatus(await commands.speakerStatus());
    } catch (err) {
      console.warn("Reading speaker status failed:", err);
    }
  }, []);

  useEffect(() => {
    void refreshStatus();
  }, [refreshStatus]);

  const loaded = model.status !== null;
  const ready = model.status?.state === "ready";
  const enrolled = status?.enrolled ?? false;
  const enrollmentActive = status?.enrollment_active ?? false;
  const enabled = getSetting("speaker_filter_enabled") ?? false;
  const sensitivity = getSetting("speaker_filter_sensitivity") ?? "balanced";
  const view = voiceModelCard(
    model.status?.state ?? "not_installed",
    model.downloaded,
    model.total,
    model.error,
  );
  const gateOpen = speakerFilterToggleEnabled(ready, enrolled);

  const sensitivityLabel = (value: SpeakerSensitivity): string => {
    switch (value) {
      case "lenient":
        return t("converse.voice.speaker.sensitivity.lenient");
      case "strict":
        return t("converse.voice.speaker.sensitivity.strict");
      default:
        return t("converse.voice.speaker.sensitivity.balanced");
    }
  };

  const sensitivityHint = (value: SpeakerSensitivity): string => {
    switch (value) {
      case "lenient":
        return t("converse.voice.speaker.sensitivity.lenientHint");
      case "strict":
        return t("converse.voice.speaker.sensitivity.strictHint");
      default:
        return t("converse.voice.speaker.sensitivity.balancedHint");
    }
  };

  const deleteVoiceprint = async () => {
    setBusy(true);
    setDeleteFailed(false);
    try {
      const result = await commands.speakerDeleteVoiceprint();
      if (result.status === "ok") {
        setConfirming(false);
      } else {
        setDeleteFailed(true);
      }
    } catch (err) {
      console.warn("Deleting the voiceprint failed:", err);
      setDeleteFailed(true);
    } finally {
      setBusy(false);
      await refreshStatus();
    }
  };

  return (
    <>
      <div className="mx-3 flex flex-col gap-1 rounded-lg border border-warning/40 bg-warning/10 p-3">
        <p className="text-sm font-medium">
          {t("converse.voice.speaker.disclosure.lead")}
        </p>
        <p className="text-xs text-muted">
          {t("converse.voice.speaker.disclosure.detail")}
        </p>
      </div>
      <ToggleSwitch
        checked={enabled}
        onChange={(next) => void updateSetting("speaker_filter_enabled", next)}
        disabled={
          toggleDisabled(enabled, gateOpen) ||
          isUpdating("speaker_filter_enabled")
        }
        isUpdating={isUpdating("speaker_filter_enabled")}
        label={t("converse.voice.speaker.label")}
        description={t("converse.voice.speaker.description")}
        descriptionMode="inline"
        grouped={grouped}
      />
      {speakerFilterInactiveNote(enabled, ready, enrolled) && (
        <p className="px-3 text-xs text-muted">
          {t("converse.voice.speaker.inactiveNote")}
        </p>
      )}
      <SettingContainer
        title={t("converse.voice.speaker.sensitivityLabel")}
        description={t("converse.voice.speaker.sensitivityDescription")}
        descriptionMode={descriptionMode}
        grouped={grouped}
        layout="stacked"
      >
        <div className="flex flex-col gap-1">
          <Dropdown
            options={SENSITIVITIES.map((value) => ({
              value,
              label: sensitivityLabel(value),
              description: sensitivityHint(value),
            }))}
            selectedValue={sensitivity}
            onSelect={(value) =>
              void updateSetting(
                "speaker_filter_sensitivity",
                value as SpeakerSensitivity,
              )
            }
            disabled={isUpdating("speaker_filter_sensitivity")}
            className="min-w-[280px]"
          />
        </div>
      </SettingContainer>
      <AuxModelCard
        title={t("converse.voice.speaker.modelLabel")}
        description={t("converse.voice.speaker.modelDescription", {
          size: formatMegabytes(
            model.status?.size_bytes ?? SPEAKER_MODEL_SIZE_BYTES,
          ),
        })}
        descriptionMode={descriptionMode}
        grouped={grouped}
        view={view}
        loaded={loaded}
        onDownload={() => void model.download()}
        onCancel={() => void model.cancel()}
        onDelete={() => void model.remove()}
      />
      <SettingContainer
        title={t("converse.voice.speaker.enroll.label")}
        description={t("converse.voice.speaker.enroll.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
        layout="stacked"
      >
        <div className="flex flex-col gap-3">
          <span className="text-sm">
            {enrolled
              ? t("converse.voice.speaker.enrolled")
              : t("converse.voice.speaker.notEnrolled")}
          </span>
          <SpeakerEnrollWizard
            modelReady={ready}
            onFinished={() => void refreshStatus()}
          />
        </div>
      </SettingContainer>
      <SettingContainer
        title={t("converse.voice.speaker.deleteLabel")}
        description={t("converse.voice.speaker.deleteDescription")}
        descriptionMode={descriptionMode}
        grouped={grouped}
        layout="stacked"
      >
        <div className="flex flex-col gap-2">
          {!confirming ? (
            <Button
              variant="danger-ghost"
              size="md"
              onClick={() => setConfirming(true)}
              disabled={!allowsDeleteVoiceprint(enrolled, enrollmentActive)}
              className="self-start"
            >
              {t("converse.voice.speaker.deleteButton")}
            </Button>
          ) : (
            <div className="flex flex-col gap-2">
              <p className="text-sm">
                {t("converse.voice.speaker.deleteConfirm")}
              </p>
              <div className="flex items-center gap-2">
                <Button
                  variant="danger"
                  size="md"
                  onClick={() => void deleteVoiceprint()}
                  disabled={busy}
                >
                  {t("converse.voice.speaker.deleteConfirmButton")}
                </Button>
                <Button
                  variant="secondary"
                  size="md"
                  onClick={() => setConfirming(false)}
                  disabled={busy}
                >
                  {t("converse.voice.speaker.deleteKeep")}
                </Button>
              </div>
            </div>
          )}
          {deleteFailed && (
            <p role="alert" className="text-xs text-danger">
              {t("converse.voice.speaker.deleteError")}
            </p>
          )}
        </div>
      </SettingContainer>
    </>
  );
};
