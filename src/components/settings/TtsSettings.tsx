import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import { Button } from "../ui/Button";
import { Dropdown } from "../ui/Dropdown";
import { Input } from "../ui/Input";
import { SettingContainer } from "../ui/SettingContainer";
import { Slider } from "../ui/Slider";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";
import {
  buildVoiceOptions,
  voiceValueToSetting,
  type TtsVoiceInfo,
} from "../../lib/ttsVoices";
import { SpokenReplyStyle } from "./SpokenReplyStyle";
import { ConversationSection } from "./ConversationSection";

interface TtsSettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

const DEFAULT_RATE_WPM = 180;

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
      </>
    );
  },
);
