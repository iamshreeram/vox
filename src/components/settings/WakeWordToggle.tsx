import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { Slider } from "../ui/Slider";
import { Dropdown } from "../ui/Dropdown";
import { SettingContainer } from "../ui/SettingContainer";
import { useSettings } from "../../hooks/useSettings";
import {
  WAKE_WORD_ACTIONS,
  normalizeWakeWordAction,
  wakeWordActionLabelKey,
  type WakeWordAction,
} from "./wakeWordAction";

interface WakeWordToggleProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

// Bounds mirror the authoritative Rust validators in settings.rs
// (WAKE_WORD_CONFIDENCE_*, WAKE_WORD_COOLDOWN_MAX_MS, WAKE_WORD_SILENCE_TIMEOUT_MAX_MS).
const CONFIDENCE_MIN = 0.1;
const CONFIDENCE_MAX = 0.95;
const COOLDOWN_MAX_MS = 10000;
const SILENCE_TIMEOUT_MAX_MS = 30000;

export const WakeWordToggle: React.FC<WakeWordToggleProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const wakeWordEnabled = getSetting("wake_word_enabled") ?? false;
    const ambientModeEnabled = getSetting("ambient_mode_enabled") ?? false;
    const confidenceThreshold =
      getSetting("wake_word_confidence_threshold") ?? 0.5;
    const cooldownMs = getSetting("wake_word_cooldown_ms") ?? 500;
    const silenceTimeoutMs = getSetting("wake_word_silence_timeout_ms") ?? 1500;
    const wakeWordAction = normalizeWakeWordAction(
      getSetting("wake_word_action"),
    );
    const wakeWordActionOptions = WAKE_WORD_ACTIONS.map((action) => ({
      value: action,
      label: t(wakeWordActionLabelKey(action)),
    }));

    return (
      <>
        <div
          title={
            ambientModeEnabled
              ? t("settings.advanced.wakeWord.disabledByAmbientMode")
              : undefined
          }
        >
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

        {wakeWordEnabled && (
          <SettingContainer
            title={t("settings.advanced.wakeWord.action.label")}
            description={t("settings.advanced.wakeWord.action.description")}
            descriptionMode={descriptionMode}
            grouped={grouped}
          >
            <Dropdown
              options={wakeWordActionOptions}
              selectedValue={wakeWordAction}
              onSelect={(value) =>
                updateSetting("wake_word_action", value as WakeWordAction)
              }
              disabled={isUpdating("wake_word_action")}
            />
          </SettingContainer>
        )}

        <Slider
          value={confidenceThreshold}
          onChange={(next) =>
            void updateSetting(
              "wake_word_confidence_threshold",
              Math.round(next * 100) / 100,
            )
          }
          min={CONFIDENCE_MIN}
          max={CONFIDENCE_MAX}
          step={0.01}
          label={t("settings.advanced.wakeWord.confidenceLabel")}
          description={t("settings.advanced.wakeWord.confidenceDescription")}
          descriptionMode={descriptionMode}
          grouped={grouped}
          formatValue={(v) =>
            t("settings.advanced.wakeWord.percentValue", {
              value: Math.round(v * 100),
            })
          }
        />

        <Slider
          value={cooldownMs}
          onChange={(next) =>
            void updateSetting("wake_word_cooldown_ms", Math.round(next))
          }
          min={0}
          max={COOLDOWN_MAX_MS}
          step={50}
          label={t("settings.advanced.wakeWord.cooldownLabel")}
          description={t("settings.advanced.wakeWord.cooldownDescription")}
          descriptionMode={descriptionMode}
          grouped={grouped}
          formatValue={(v) =>
            t("settings.advanced.wakeWord.msValue", { value: Math.round(v) })
          }
        />

        <Slider
          value={silenceTimeoutMs}
          onChange={(next) =>
            void updateSetting("wake_word_silence_timeout_ms", Math.round(next))
          }
          min={0}
          max={SILENCE_TIMEOUT_MAX_MS}
          step={100}
          label={t("settings.advanced.wakeWord.silenceTimeoutLabel")}
          description={t(
            "settings.advanced.wakeWord.silenceTimeoutDescription",
          )}
          descriptionMode={descriptionMode}
          grouped={grouped}
          formatValue={(v) =>
            Math.round(v) === 0
              ? t("settings.advanced.wakeWord.offValue")
              : t("settings.advanced.wakeWord.msValue", {
                  value: Math.round(v),
                })
          }
        />
      </>
    );
  },
);
