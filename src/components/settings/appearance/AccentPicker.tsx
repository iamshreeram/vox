import React, { useRef } from "react";
import { useTranslation } from "react-i18next";
import { Check } from "lucide-react";
import { useSettings } from "@/hooks/useSettings";
import { applyTheme } from "@/lib/utils/theme";
import { ACCENTS, ACCENT_IDS, type AccentId } from "@/styles/accents";
import { SettingContainer } from "../../ui/SettingContainer";
import {
  ACCENT_NAME_KEYS,
  accentEnabledFor,
  effectiveAppearance,
  nextRadioIndex,
  rovingTabStops,
} from "./appearanceModel";

interface AccentPickerProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * Accent swatches: a radio group of six named colours. The group is disabled, with a note, while
 * the theme owns its palette (Aurora and Custom, DESIGN A3). Choosing a swatch applies it to this
 * window at once and saves it through the store (which publishes it to the other windows).
 */
export const AccentPicker: React.FC<AccentPickerProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const { settings, updateSetting } = useSettings();
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const { theme, accent, visualStyle } = effectiveAppearance(settings ?? {});
  const locked = !accentEnabledFor(theme);
  const selected = ACCENT_IDS.indexOf(accent);
  const stops = rovingTabStops(selected, ACCENT_IDS.length);

  const choose = (id: AccentId): void => {
    if (locked || id === accent) return;
    applyTheme(theme, settings?.custom_theme_colors, id, visualStyle);
    void updateSetting("accent", id);
  };

  const onKeyDown = (event: React.KeyboardEvent, index: number): void => {
    const next = nextRadioIndex(event.key, index, ACCENT_IDS.length);
    if (next === null) return;
    event.preventDefault();
    refs.current[next]?.focus();
    choose(ACCENT_IDS[next]);
  };

  return (
    <SettingContainer
      title={t("settings.appearance.accent.title")}
      description={t("settings.appearance.accent.description")}
      descriptionMode={descriptionMode}
      grouped={grouped}
      layout="stacked"
      disabled={locked}
    >
      <div className="flex flex-col gap-2">
        <div
          role="radiogroup"
          aria-label={t("settings.appearance.accent.title")}
          aria-disabled={locked}
          className="flex flex-wrap gap-3"
        >
          {ACCENT_IDS.map((id, index) => {
            const isSelected = id === accent;
            const name = t(ACCENT_NAME_KEYS[id]);
            return (
              <button
                key={id}
                ref={(node) => {
                  refs.current[index] = node;
                }}
                type="button"
                role="radio"
                aria-checked={isSelected}
                aria-label={name}
                tabIndex={locked ? -1 : stops[index]}
                disabled={locked}
                data-testid={`accent-swatch-${id}`}
                data-accent-id={id}
                onClick={() => choose(id)}
                onKeyDown={(event) => onKeyDown(event, index)}
                className={[
                  "flex h-10 w-10 items-center justify-center rounded-full border-2 cursor-pointer motion-reduce:transition-none",
                  "focus:outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent-a",
                  "disabled:cursor-not-allowed disabled:opacity-50",
                  isSelected
                    ? "border-accent-a ring-2 ring-offset-2 ring-accent-a"
                    : "border-transparent",
                ].join(" ")}
                style={{
                  backgroundImage: `linear-gradient(135deg, ${ACCENTS[id].light.fillA}, ${ACCENTS[id].light.fillB})`,
                }}
              >
                {isSelected ? (
                  <Check
                    aria-hidden="true"
                    size={18}
                    strokeWidth={3}
                    style={{ color: ACCENTS[id].light.onFill }}
                  />
                ) : null}
              </button>
            );
          })}
        </div>
        <p className="text-sm text-mid-gray">
          {/* Names are shown under the swatch row so colour is never the only cue. */}
          {t(ACCENT_NAME_KEYS[accent])}
        </p>
        {locked ? (
          <p data-testid="accent-locked-note" className="text-sm text-mid-gray">
            {t("settings.appearance.accent.locked")}
          </p>
        ) : null}
      </div>
    </SettingContainer>
  );
};
