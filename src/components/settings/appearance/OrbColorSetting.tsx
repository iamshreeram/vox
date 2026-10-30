import React, { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check } from "lucide-react";
import { useSettings } from "@/hooks/useSettings";
import { useSettingsStore } from "@/stores/settingsStore";
import { ACCENTS } from "@/styles/accents";
import { SettingContainer } from "../../ui/SettingContainer";
import {
  effectiveAppearance,
  effectiveOrbAccent,
  nextRadioIndex,
  ORB_CHOICES,
  orbChoiceId,
  orbChoiceNameKey,
  orbSelectedIndex,
  rovingTabStops,
  type OrbChoice,
} from "./appearanceModel";
import { appearanceBucket, usePrefersDark } from "./appearanceBucket";

interface OrbColorSettingProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * Orb colour (DESIGN D3): a radio group of "Follow app" and the six accents. It sets the floating
 * orb's own colour only, so the app palette is unchanged and the group is never disabled (the orb
 * has its own face under every theme). A save that fails is rolled back and says so.
 */
export const OrbColorSetting: React.FC<OrbColorSettingProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const { settings } = useSettings();
  const changeOrbAccent = useSettingsStore((state) => state.changeOrbAccent);
  const prefersDark = usePrefersDark();
  const [failed, setFailed] = useState(false);
  const refs = useRef<(HTMLButtonElement | null)[]>([]);

  const app = effectiveAppearance(settings ?? {});
  const orb = effectiveOrbAccent(settings?.orb_accent);
  const selected = orbSelectedIndex(orb);
  const stops = rovingTabStops(selected, ORB_CHOICES.length);
  const bucket = appearanceBucket(settings, prefersDark);
  const groupName = t("settings.appearance.orb.title");

  const choose = async (choice: OrbChoice): Promise<void> => {
    if (choice === orb) return;
    setFailed(false);
    const saved = await changeOrbAccent(choice);
    if (!saved) setFailed(true);
  };

  const onKeyDown = (event: React.KeyboardEvent, index: number): void => {
    const next = nextRadioIndex(event.key, index, ORB_CHOICES.length);
    if (next === null) return;
    event.preventDefault();
    refs.current[next]?.focus();
    void choose(ORB_CHOICES[next]);
  };

  return (
    <SettingContainer
      title={groupName}
      description={t("settings.appearance.orb.description")}
      descriptionMode={descriptionMode}
      grouped={grouped}
      layout="stacked"
    >
      <div className="flex flex-col gap-2">
        <div
          role="radiogroup"
          aria-label={groupName}
          className="flex flex-wrap gap-3"
        >
          {ORB_CHOICES.map((choice, index) => {
            const isSelected = index === selected;
            const name = t(orbChoiceNameKey(choice));
            // Follow app previews the app accent; an accent shows its own colours.
            const swatch = ACCENTS[choice ?? app.accent][bucket];
            // The boundary is the accent text role on light and the first fill on dark: both are
            // at least 3:1 against their surface (checked by the UI quality gate).
            const boundary = bucket === "light" ? swatch.text : swatch.fillA;
            return (
              <button
                key={orbChoiceId(choice)}
                ref={(node) => {
                  refs.current[index] = node;
                }}
                type="button"
                role="radio"
                aria-checked={isSelected}
                aria-label={name}
                tabIndex={stops[index]}
                data-testid={`orb-swatch-${orbChoiceId(choice)}`}
                data-orb-id={orbChoiceId(choice)}
                onClick={() => void choose(choice)}
                onKeyDown={(event) => onKeyDown(event, index)}
                className={[
                  "flex h-10 w-10 items-center justify-center rounded-full border-2 cursor-pointer motion-reduce:transition-none",
                  // Two-tone focus ring, as the accent swatches: the outer ring is the global
                  // --focus-ring, the inner ring the on-accent colour.
                  "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-(--focus-ring)",
                  "focus-visible:inset-ring-2 focus-visible:inset-ring-(--color-on-accent)",
                  choice === null ? "border-dashed" : "",
                  isSelected ? "ring-2 ring-offset-2 ring-accent-a" : "",
                ].join(" ")}
                style={{
                  backgroundImage: `linear-gradient(135deg, ${swatch.fillA}, ${swatch.fillB})`,
                  borderColor: boundary,
                }}
              >
                {isSelected ? (
                  <Check
                    aria-hidden="true"
                    size={18}
                    strokeWidth={3}
                    style={{ color: swatch.onFill }}
                  />
                ) : null}
              </button>
            );
          })}
        </div>
        <p className="text-sm text-muted">
          {/* The name is shown under the swatches so colour is never the only cue. */}
          {t(orbChoiceNameKey(orb))}
        </p>
        {failed ? (
          <p role="alert" className="text-xs text-danger">
            {t("settings.appearance.orb.failed")}
          </p>
        ) : null}
      </div>
    </SettingContainer>
  );
};
