import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ask, open } from "@tauri-apps/plugin-dialog";
import { commands, type TtsEngine, type TtsEngineInfo } from "@/bindings";
import { Button } from "../ui/Button";
import { Dropdown } from "../ui/Dropdown";
import { Input } from "../ui/Input";
import { SettingContainer } from "../ui/SettingContainer";
import { Slider } from "../ui/Slider";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";
import {
  useNaturalVoice,
  type UseNaturalVoice,
} from "../../hooks/useNaturalVoice";
import { phononInstallState, usePhononVoice } from "../../hooks/usePhononVoice";
import {
  NATURAL_MODEL_SIZE_BYTES,
  downloadPercent,
  failureI18nKey,
  formatMegabytes,
  naturalRowState,
  selectedEngineNeedsModel,
} from "./naturalVoiceState";
import { buildEngineCards } from "./ttsEngineState";
import { PhononEngineBody, TtsEngineCard } from "./TtsEngineCard";
import { TamilVoiceRow } from "./TamilVoiceRow";
import {
  buildVoiceOptions,
  voiceValueToSetting,
  type TtsVoiceInfo,
} from "../../lib/ttsVoices";

interface TtsSettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

const DEFAULT_RATE_WPM = 180;

interface NaturalVoiceBodyProps {
  descriptionMode: "inline" | "tooltip";
  grouped: boolean;
  engine: TtsEngine;
  natural: UseNaturalVoice;
}

/**
 * Natural voice controls inside the Natural card: voice picker with preview, and the model
 * download row. Hidden until status loads and on unsupported platforms.
 */
export const NaturalVoiceBody: React.FC<NaturalVoiceBodyProps> = ({
  descriptionMode,
  grouped,
  engine,
  natural,
}) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const { status, voices, error, download, cancel, remove, retry } = natural;
  const [pending, setPending] = useState(false);

  const row = naturalRowState(status);
  if (row === "hidden" || status === null) return null;

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
            <span className="text-sm text-muted">
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
              <p className="text-xs text-muted">
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
          {selectedEngineNeedsModel(
            engine === "natural" ? "natural" : "system",
            status,
          ) && (
            <p className="text-xs text-muted">
              {t("settings.advanced.tts.natural.model.needed")}
            </p>
          )}
          {error !== null && (
            <p className="text-xs text-danger">
              {status.model === "not_installed"
                ? t("settings.advanced.tts.natural.model.downloadFailed", {
                    message: error,
                  })
                : error}
            </p>
          )}
          <p className="text-xs text-muted">
            {t("settings.advanced.tts.natural.notice.limitations")}
          </p>
        </div>
      </SettingContainer>
    </>
  );
};

/**
 * Spoken replies: an opt-in toggle, the engine picker (System, Natural, Phonon), the system
 * voice name, the speaking rate, and buttons to audition or stop speech. Spoken output is
 * audible to anyone nearby, so the toggle description says so explicitly.
 */
export const TtsSettings: React.FC<TtsSettingsProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const natural = useNaturalVoice();
    const phonon = usePhononVoice();
    const [catalog, setCatalog] = useState<TtsEngineInfo[]>([]);
    const [phononPending, setPhononPending] = useState(false);

    const ttsEnabled = getSetting("tts_enabled") ?? false;
    const ttsVoice = getSetting("tts_voice") ?? null;
    const ttsRate = getSetting("tts_rate_wpm") ?? DEFAULT_RATE_WPM;
    const engine: TtsEngine = getSetting("tts_engine") ?? "system";
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
          const result = await commands.listTtsVoices();
          list = Array.isArray(result) ? result : [];
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

    useEffect(() => {
      let active = true;
      commands
        .listTtsEngines()
        .then((list) => {
          if (active) setCatalog(Array.isArray(list) ? list : []);
        })
        .catch((error: unknown) => {
          console.warn("Listing speech engines failed:", error);
        });
      return () => {
        active = false;
      };
    }, []);

    const cards = buildEngineCards(catalog, engine, {
      phonon: phononInstallState(phonon.status, phonon.error),
    });
    const selectedCard = cards.find((card) => card.selected);
    // Before the catalog loads, keep the system voice and rate controls as they were.
    const showSystemVoice = selectedCard?.showVoiceDropdown ?? true;
    const showRateSlider = selectedCard?.showRateSlider ?? true;

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

    const handleSelectEngine = (id: TtsEngine) => {
      if (id === engine) return;
      updateSetting("tts_engine", id);
    };

    const withPhononPending = async (action: () => Promise<void>) => {
      setPhononPending(true);
      try {
        await action();
      } finally {
        setPhononPending(false);
      }
    };

    const handlePhononDelete = () =>
      withPhononPending(async () => {
        const confirmed = await ask(
          t("settings.advanced.tts.engines.confirmDelete"),
          {
            title: t("settings.advanced.tts.engines.phonon.name"),
            kind: "warning",
          },
        );
        if (confirmed) {
          await phonon.remove();
        }
      });

    const handlePhononImport = () =>
      withPhononPending(async () => {
        const picked = await open({ directory: true, multiple: false });
        if (typeof picked === "string") {
          await phonon.importFolder(picked);
        }
      });

    const handlePhononPreview = () =>
      withPhononPending(async () => {
        const result = await commands.ttsSpeakTest(
          t("settings.advanced.tts.testPhrase"),
        );
        if (result.status === "error") {
          console.warn("Phonon preview failed:", result.error);
        }
      });

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
        <SettingContainer
          title={t("settings.advanced.tts.natural.engine.label")}
          description={t("settings.advanced.tts.natural.engine.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
          layout="stacked"
        >
          <div className="flex flex-col gap-3" data-testid="tts-engine-picker">
            {cards.map((card) => (
              <TtsEngineCard
                key={card.id}
                card={card}
                onSelect={() => handleSelectEngine(card.id)}
              >
                {card.id === "phonon" && (
                  <PhononEngineBody
                    card={card}
                    descriptionMode={descriptionMode}
                    grouped={grouped}
                    pending={phononPending}
                    onInstall={() => withPhononPending(phonon.install)}
                    onCancel={() => withPhononPending(phonon.cancel)}
                    onDelete={handlePhononDelete}
                    onImportFolder={handlePhononImport}
                    onPreview={handlePhononPreview}
                  />
                )}
                {card.id === "natural" && (
                  <NaturalVoiceBody
                    descriptionMode={descriptionMode}
                    grouped={grouped}
                    engine={engine}
                    natural={natural}
                  />
                )}
              </TtsEngineCard>
            ))}
            <p className="text-xs text-muted">
              {t("settings.advanced.tts.engines.fixedNote")}
            </p>
          </div>
        </SettingContainer>
        <TamilVoiceRow descriptionMode={descriptionMode} grouped={grouped} />
        {showSystemVoice && (
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
        )}
        {showRateSlider && (
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
        )}
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
      </>
    );
  },
);
