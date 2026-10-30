import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { Button } from "../ui/Button";
import { Textarea } from "../ui/Textarea";
import { useSettings } from "../../hooks/useSettings";

// Bounds mirror the authoritative Rust validator in settings.rs
// (CUSTOM_FILLER_MAX_WORDS / CUSTOM_FILLER_WORD_MAX_CHARS).
const MAX_WORDS = 100;
const MAX_WORD_CHARS = 32;

type FillerChoice = "builtIn" | "custom" | "off";

/** Maps the stored value to the choice it represents. */
function choiceFromStored(words: string[] | null): FillerChoice {
  if (words === null) return "builtIn";
  return words.length === 0 ? "off" : "custom";
}

/** Splits the draft on commas and new lines, dropping blank entries. */
export function parseFillerDraft(draft: string): string[] {
  return draft
    .split(/[,\n]/)
    .map((word) => word.trim())
    .filter((word) => word.length > 0);
}

interface FillerWordRemovalProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const FillerWordRemoval: React.FC<FillerWordRemovalProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const enabled = getSetting("filler_word_removal_enabled") ?? true;
    const storedWords = getSetting("custom_filler_words") ?? null;
    const storedKey = JSON.stringify(storedWords);

    const [choice, setChoice] = useState<FillerChoice>(
      choiceFromStored(storedWords),
    );
    const [draft, setDraft] = useState((storedWords ?? []).join("\n"));

    // Re-sync from the stored value. A backend rejection makes the store revert
    // the setting, which also reverts the choice and the draft here.
    useEffect(() => {
      const words: string[] | null = JSON.parse(storedKey);
      setChoice(choiceFromStored(words));
      setDraft((words ?? []).join("\n"));
    }, [storedKey]);

    const parsed = parseFillerDraft(draft);
    const tooManyWords = parsed.length > MAX_WORDS;
    const wordTooLong = parsed.some(
      (word) => Array.from(word).length > MAX_WORD_CHARS,
    );
    const canSave = parsed.length > 0 && !tooManyWords && !wordTooLong;
    const busy = isUpdating("custom_filler_words");

    const chooseBuiltIn = () => {
      setChoice("builtIn");
      void updateSetting("custom_filler_words", null);
    };
    const chooseOff = () => {
      setChoice("off");
      void updateSetting("custom_filler_words", []);
    };
    const saveCustom = () => {
      if (!canSave) return;
      void updateSetting("custom_filler_words", parsed);
    };

    const choiceOptions: { value: FillerChoice; label: string }[] = [
      {
        value: "builtIn",
        label: t("settings.advanced.fillerWordRemoval.builtIn"),
      },
      {
        value: "custom",
        label: t("settings.advanced.fillerWordRemoval.custom"),
      },
      { value: "off", label: t("settings.advanced.fillerWordRemoval.off") },
    ];

    const handleChoice = (next: FillerChoice) => {
      if (next === "builtIn") chooseBuiltIn();
      else if (next === "off") chooseOff();
      else setChoice("custom");
    };

    return (
      <>
        <ToggleSwitch
          checked={enabled}
          onChange={(nextEnabled) =>
            updateSetting("filler_word_removal_enabled", nextEnabled)
          }
          isUpdating={isUpdating("filler_word_removal_enabled")}
          label={t("settings.advanced.fillerWordRemoval.title")}
          description={t("settings.advanced.fillerWordRemoval.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
        <div className="px-4 pb-3 flex flex-col gap-2">
          <p
            id="custom-filler-words-mode-label"
            className="text-sm font-medium"
          >
            {t("settings.advanced.fillerWordRemoval.modeLabel")}
          </p>
          <div
            role="radiogroup"
            aria-labelledby="custom-filler-words-mode-label"
            className="flex flex-wrap gap-2"
          >
            {choiceOptions.map((option) => (
              <label
                key={option.value}
                className="flex items-center gap-2 text-sm cursor-pointer"
              >
                <input
                  type="radio"
                  aria-label={option.label}
                  name="custom-filler-words-mode"
                  value={option.value}
                  checked={choice === option.value}
                  disabled={busy}
                  onChange={() => handleChoice(option.value)}
                />
                {option.label}
              </label>
            ))}
          </div>

          {choice === "custom" && (
            <div className="flex flex-col gap-2">
              <label className="text-sm font-medium">
                {t("settings.advanced.fillerWordRemoval.customLabel")}
              </label>
              <Textarea
                variant="compact"
                value={draft}
                onChange={(event) => setDraft(event.target.value)}
                disabled={busy}
                aria-label={t(
                  "settings.advanced.fillerWordRemoval.customLabel",
                )}
              />
              <p className="text-xs text-muted">
                {t("settings.advanced.fillerWordRemoval.customHelp")}
              </p>
              <p className="text-xs text-muted">
                {t("settings.advanced.fillerWordRemoval.customCount", {
                  count: parsed.length,
                  max: MAX_WORDS,
                })}
              </p>
              {tooManyWords && (
                <p className="text-xs text-danger">
                  {t("settings.advanced.fillerWordRemoval.customTooMany", {
                    max: MAX_WORDS,
                  })}
                </p>
              )}
              {wordTooLong && (
                <p className="text-xs text-danger">
                  {t("settings.advanced.fillerWordRemoval.customTooLong", {
                    max: MAX_WORD_CHARS,
                  })}
                </p>
              )}
              {parsed.length === 0 && (
                <p className="text-xs text-muted">
                  {t("settings.advanced.fillerWordRemoval.customEmpty")}
                </p>
              )}
              <div>
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={saveCustom}
                  disabled={!canSave || busy}
                >
                  {t("settings.advanced.fillerWordRemoval.saveCustom")}
                </Button>
              </div>
            </div>
          )}
        </div>
      </>
    );
  },
);
