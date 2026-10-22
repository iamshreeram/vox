import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type TtsEngine } from "@/bindings";
import { Button } from "../ui/Button";
import { Dropdown } from "../ui/Dropdown";
import { Input } from "../ui/Input";
import { SettingContainer } from "../ui/SettingContainer";
import { Slider } from "../ui/Slider";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";
import { useNaturalVoice } from "../../hooks/useNaturalVoice";
import {
  NATURAL_MODEL_SIZE_BYTES,
  downloadPercent,
  failureI18nKey,
  formatMegabytes,
  naturalRowState,
  selectedEngineNeedsModel,
} from "./naturalVoiceState";
import {
  buildVoiceOptions,
  voiceValueToSetting,
  type TtsVoiceInfo,
} from "../../lib/ttsVoices";
import { SpokenReplyStyle } from "./SpokenReplyStyle";
import { ConversationSection } from "./ConversationSection";
import { VoiceFeaturesSection } from "./voice/VoiceFeaturesSection";

interface TtsSettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

const DEFAULT_RATE_WPM = 180;

interface NaturalVoiceSectionProps {
  descriptionMode: "inline" | "tooltip";
  grouped: boolean;
}

/**
 * Natural voice controls: engine choice, voice picker with preview, and the
 * model download row. Hidden until status loads and on unsupported platforms.
 */
const NaturalVoiceSection: React.FC<NaturalVoiceSectionProps> = ({
  descriptionMode,
  grouped,
}) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const { status, voices, error, download, cancel, remove, retry } =
    useNaturalVoice();
  const [pending, setPending] = useState(false);

  const row = naturalRowState(status);
  if (row === "hidden" || status === null) return null;

  const engine: TtsEngine =
    getSetting("tts_engine") === "natural" ? "natural" : "system";
  const naturalVoice = getSetting("tts_natural_voice") ?? "";
  const selectedVoice = voices.some((voice) => voice.id === naturalVoice)
    ? naturalVoice
    : (voices[0]?.id ?? "");

  const withPending = async (action: () => Promise<void>) => {
    setPending(true);
    try {
      await action();
    } finally {
      setPending(false);
    }
  };

  const handlePreview = () =>
    withPending(async () => {
      const result = await commands.ttsSpeakTest(
        t("settings.advanced.tts.natural.previewText"),
      );
      if (result.status === "error") {
        console.warn("Natural voice preview failed:", result.error);
      }
    });

  const percent = downloadPercent(status.downloaded, status.total);

  return (
    <>
      <SettingContainer
        title={t("settings.advanced.tts.natural.engine.label")}
        description={t("settings.advanced.tts.natural.engine.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      >
        <Dropdown
          options={[
            {
              value: "system",
              label: t("settings.advanced.tts.natural.engine.system"),
            },
            {
              value: "natural",
              label: t("settings.advanced.tts.natural.engine.natural"),
            },
          ]}
          selectedValue={engine}
          onSelect={(value) => {
            if (value === engine) return;
            updateSetting("tts_engine", value as TtsEngine);
          }}
          disabled={isUpdating("tts_engine")}
          className="flex-1 min-w-[360px]"
        />
      </SettingContainer>
      {engine === "natural" && (
        <SettingContainer
          title={t("settings.advanced.tts.natural.voice.label")}
          description={t("settings.advanced.tts.natural.voice.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        >
          <div className="flex items-center gap-2">
            <Dropdown
              options={voices.map((voice) => ({
                value: voice.id,
                label: t("settings.advanced.tts.natural.voice.option", {
                  name: voice.name,
                  accent: t(
                    `settings.advanced.tts.natural.accent.${voice.accent}`,
                  ),
                  gender: t(
                    `settings.advanced.tts.natural.gender.${voice.gender}`,
                  ),
                }),
              }))}
              selectedValue={selectedVoice}
              onSelect={(value) => {
                if (value === naturalVoice) return;
                updateSetting("tts_natural_voice", value);
              }}
              disabled={voices.length === 0 || isUpdating("tts_natural_voice")}
              className="flex-1 min-w-[280px]"
            />
            <Button
              variant="secondary"
              size="md"
              onClick={handlePreview}
              disabled={pending}
            >
              {t("settings.advanced.tts.natural.preview")}
            </Button>
          </div>
        </SettingContainer>
      )}
      <SettingContainer
        title={t("settings.advanced.tts.natural.model.label")}
        description={t("settings.advanced.tts.natural.notice.firstSpeech")}
        descriptionMode={descriptionMode}
        grouped={grouped}
        layout="stacked"
      >
        <div className="flex flex-col gap-2">
          {row === "download" && (
            <div className="flex items-center justify-between gap-3">
              <p className="text-sm">
                {t("settings.advanced.tts.natural.model.notInstalled", {
                  size: formatMegabytes(NATURAL_MODEL_SIZE_BYTES),
                })}
              </p>
              <Button
                variant="primary"
                size="md"
                onClick={() => withPending(download)}
                disabled={pending}
              >
                {t("settings.advanced.tts.natural.model.download")}
              </Button>
            </div>
          )}
          {row === "downloading" && (
            <div className="flex items-center gap-3">
              <div className="flex flex-1 flex-col gap-1">
                <span className="text-sm">
                  {t("settings.advanced.tts.natural.model.downloading", {
                    percent,
                  })}
                </span>
                <div className="h-1.5 w-full overflow-hidden rounded-full bg-mid-gray/20">
                  <div
                    className="h-full rounded-full bg-logo-primary transition-all"
                    style={{ width: `${percent}%` }}
                  />
                </div>
              </div>
              <Button
                variant="secondary"
                size="md"
                onClick={() => withPending(cancel)}
                disabled={pending}
              >
                {t("settings.advanced.tts.natural.model.cancel")}
              </Button>
            </div>
          )}
          {row === "working" && (
            <span className="text-sm text-mid-gray">
              {t("settings.advanced.tts.natural.model.working")}
            </span>
          )}
          {row === "ready" && (
            <div className="flex items-center justify-between gap-3">
              <span className="text-sm">
                {t("settings.advanced.tts.natural.model.ready")}
              </span>
              <Button
                variant="danger-ghost"
                size="md"
                onClick={() => withPending(remove)}
                disabled={pending}
              >
                {t("settings.advanced.tts.natural.model.delete")}
              </Button>
            </div>
          )}
          {row === "failed" && (
            <div className="flex flex-col gap-2 rounded-lg border border-warning/40 bg-warning/10 p-3">
              <p className="text-sm">
                {t("settings.advanced.tts.natural.failure.banner")}
              </p>
              <p className="text-xs text-mid-gray">
                {t(failureI18nKey(status.failure))}
              </p>
              <div className="flex items-center gap-2">
                <Button
                  variant="warning"
                  size="md"
                  onClick={() => withPending(retry)}
                  disabled={pending}
                >
                  {t("settings.advanced.tts.natural.failure.retry")}
                </Button>
                <Button
                  variant="danger-ghost"
                  size="md"
                  onClick={() => withPending(remove)}
                  disabled={pending}
                >
                  {t("settings.advanced.tts.natural.model.delete")}
                </Button>
              </div>
            </div>
          )}
          {selectedEngineNeedsModel(engine, status) && (
            <p className="text-xs text-mid-gray">
              {t("settings.advanced.tts.natural.model.needed")}
            </p>
          )}
          {error !== null && (
            <p className="text-xs text-red-400">
              {status.model === "not_installed"
                ? t("settings.advanced.tts.natural.model.downloadFailed", {
                    message: error,
                  })
                : error}
            </p>
          )}
          <p className="text-xs text-mid-gray">
            {t("settings.advanced.tts.natural.notice.limitations")}
          </p>
        </div>
      </SettingContainer>
    </>
  );
};

/**
 * Spoken replies: an opt-in toggle, the voice name, the speaking rate, and
 * buttons to audition or stop speech. Spoken output is audible to anyone
 * nearby, so the toggle description says so explicitly.
 */
export const TtsSettings: React.FC<TtsSettingsProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const ttsEnabled = getSetting("tts_enabled") ?? false;
    const ttsVoice = getSetting("tts_voice") ?? null;
    const ttsRate = getSetting("tts_rate_wpm") ?? DEFAULT_RATE_WPM;
    const [localVoice, setLocalVoice] = useState(ttsVoice ?? "");
    const [voices, setVoices] = useState<TtsVoiceInfo[]>([]);
    const [loading, setLoading] = useState(true);

    useEffect(() => {
      setLocalVoice(ttsVoice ?? "");
    }, [ttsVoice]);

    useEffect(() => {
      let active = true;
      const loadVoices = async () => {
        let list: TtsVoiceInfo[] = [];
        try {
          list = await commands.listTtsVoices();
        } catch (error) {
          console.warn("Listing system voices failed:", error);
        }
        if (active) {
          setVoices(list);
          setLoading(false);
        }
      };
      void loadVoices();
      return () => {
        active = false;
      };
    }, []);

    const voiceResult = buildVoiceOptions(voices, ttsVoice, {
      defaultLabel: t("settings.advanced.tts.voiceDefaultOption"),
      notInstalled: (name: string) =>
        t("settings.advanced.tts.voiceNotInstalled", { name }),
    });
    const showTextInput = !loading && voiceResult.useTextFallback;

    const handleVoiceSelect = (value: string) => {
      const next = voiceValueToSetting(value);
      if (next === ttsVoice) return;
      updateSetting("tts_voice", next);
    };

    const handleTest = async () => {
      const result = await commands.ttsSpeakTest(
        t("settings.advanced.tts.testPhrase"),
      );
      if (result.status === "error") {
        console.warn("Spoken reply test failed:", result.error);
      }
    };

    const handleStop = async () => {
      const result = await commands.ttsStop();
      if (result.status === "error") {
        console.warn("Stopping speech failed:", result.error);
      }
    };

    return (
      <>
        <ToggleSwitch
          checked={ttsEnabled}
          onChange={(enabled) => updateSetting("tts_enabled", enabled)}
          isUpdating={isUpdating("tts_enabled")}
          label={t("settings.advanced.tts.label")}
          description={t("settings.advanced.tts.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
          tooltipPosition="bottom"
        />
        <NaturalVoiceSection
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
        <SettingContainer
          title={t("settings.advanced.tts.voiceLabel")}
          description={t(
            loading
              ? "settings.advanced.tts.voiceListDescription"
              : voiceResult.descriptionKey,
          )}
          descriptionMode={descriptionMode}
          grouped={grouped}
        >
          {showTextInput ? (
            <Input
              type="text"
              value={localVoice}
              onChange={(event) => setLocalVoice(event.target.value)}
              onBlur={() => {
                const next = localVoice.trim();
                if (next === (ttsVoice ?? "")) return;
                updateSetting("tts_voice", next === "" ? null : next);
              }}
              placeholder={t("settings.advanced.tts.voicePlaceholder")}
              variant="compact"
              disabled={isUpdating("tts_voice")}
              className="flex-1 min-w-[360px]"
            />
          ) : (
            <Dropdown
              options={
                loading
                  ? [
                      {
                        value: "",
                        label: t("settings.advanced.tts.voiceDefaultOption"),
                      },
                      ...(voiceResult.selected !== ""
                        ? [
                            {
                              value: voiceResult.selected,
                              label: voiceResult.selected,
                            },
                          ]
                        : []),
                    ]
                  : voiceResult.options
              }
              selectedValue={voiceResult.selected}
              onSelect={handleVoiceSelect}
              placeholder={t("settings.advanced.tts.voiceDefaultOption")}
              disabled={loading || isUpdating("tts_voice")}
              className="flex-1 min-w-[360px]"
            />
          )}
        </SettingContainer>
        <Slider
          value={ttsRate}
          onChange={(value: number) => updateSetting("tts_rate_wpm", value)}
          min={80}
          max={400}
          step={10}
          label={t("settings.advanced.tts.rateLabel")}
          description={t("settings.advanced.tts.rateDescription")}
          descriptionMode={descriptionMode}
          grouped={grouped}
          formatValue={(value) =>
            t("settings.advanced.tts.rateValue", { value: Math.round(value) })
          }
          disabled={isUpdating("tts_rate_wpm")}
        />
        <SettingContainer
          title={t("settings.advanced.tts.testLabel")}
          description={t("settings.advanced.tts.testDescription")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        >
          <div className="flex items-center gap-2">
            <Button variant="secondary" size="md" onClick={handleTest}>
              {t("settings.advanced.tts.testButton")}
            </Button>
            <Button variant="secondary" size="md" onClick={handleStop}>
              {t("settings.advanced.tts.stopButton")}
            </Button>
          </div>
        </SettingContainer>
        <SpokenReplyStyle descriptionMode={descriptionMode} grouped={grouped} />
        <ConversationSection
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
        <VoiceFeaturesSection
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
      </>
    );
  },
);
