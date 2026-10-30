import React, { useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check } from "lucide-react";
import { useSettings } from "@/hooks/useSettings";
import { useSettingsStore } from "@/stores/settingsStore";
import { SettingContainer } from "../../ui/SettingContainer";
import {
  effectiveOrbAccent,
  nextRadioIndex,
  ORB_CHOICES,
  orbChoiceId,
  orbChoiceNameKey,
  orbSelectedIndex,
  rovingTabStops,
  type OrbChoice,
} from "./appearanceModel";
import {
  orbSwatchSpec,
  sphereGradientId,
  type OrbSwatchSpec,
} from "./orbSwatchModel";

interface OrbColorSettingProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * One option's swatch: a small sphere painted from the globe's gradient stops (D4). Auto is four
 * state colours in quarters; Follow app is a ring in the app accent. Each instance has its own
 * gradient id (useId), because several swatches live in one document.
 */
const OrbSwatch: React.FC<{ spec: OrbSwatchSpec }> = ({ spec }) => {
  const gradientId = sphereGradientId(useId());
  const size = 28;
  const centre = size / 2;
  const radius = 13;
  const common = { width: size, height: size, viewBox: `0 0 ${size} ${size}` };

  if (spec.kind === "sphere") {
    return (
      <svg {...common} aria-hidden="true" focusable="false">
        <defs>
          <radialGradient id={gradientId} cx="35%" cy="30%" r="75%">
            <stop offset="0%" stopColor={spec.stops.hi} />
            <stop offset="55%" stopColor={spec.stops.mid} />
            <stop offset="100%" stopColor={spec.stops.lo} />
          </radialGradient>
        </defs>
        <circle
          cx={centre}
          cy={centre}
          r={radius}
          fill={`url(#${gradientId})`}
        />
      </svg>
    );
  }
  if (spec.kind === "auto") {
    // Four quarters, clockwise from the top: blue, emerald, violet, amber (the state colours).
    const [blue, emerald, violet, amber] = spec.quarters;
    const quarter = (startDeg: number, colour: string) => {
      const toRad = (deg: number) => ((deg - 90) * Math.PI) / 180;
      const x1 = centre + radius * Math.cos(toRad(startDeg));
      const y1 = centre + radius * Math.sin(toRad(startDeg));
      const x2 = centre + radius * Math.cos(toRad(startDeg + 90));
      const y2 = centre + radius * Math.sin(toRad(startDeg + 90));
      return (
        <path
          d={`M ${centre} ${centre} L ${x1} ${y1} A ${radius} ${radius} 0 0 1 ${x2} ${y2} Z`}
          fill={colour}
        />
      );
    };
    return (
      <svg {...common} aria-hidden="true" focusable="false">
        {quarter(0, blue)}
        {quarter(90, emerald)}
        {quarter(180, violet)}
        {quarter(270, amber)}
      </svg>
    );
  }
  return (
    <svg {...common} aria-hidden="true" focusable="false">
      <circle
        cx={centre}
        cy={centre}
        r={radius - 2}
        fill="none"
        strokeWidth={3}
        style={{ stroke: "var(--color-accent-a)" }}
      />
    </svg>
  );
};

/**
 * Orb colour (DESIGN D3, D4, A5): a radio group of Auto, Follow app colour and the six accents.
 * It sets the floating orb's own colour only, so the app palette is unchanged and the group is
 * never disabled. A save that fails is rolled back and says so.
 */
export const OrbColorSetting: React.FC<OrbColorSettingProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const { settings } = useSettings();
  const changeOrbAccent = useSettingsStore((state) => state.changeOrbAccent);
  const [failed, setFailed] = useState(false);
  const refs = useRef<(HTMLButtonElement | null)[]>([]);

  const orb = effectiveOrbAccent(settings?.orb_accent);
  const selected = orbSelectedIndex(orb);
  const stops = rovingTabStops(selected, ORB_CHOICES.length);
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
            const spec = orbSwatchSpec(choice);
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
                  "relative flex h-10 w-10 items-center justify-center rounded-full cursor-pointer motion-reduce:transition-none",
                  // Two-tone focus ring, as the accent swatches: the outer ring is the global
                  // --focus-ring, the inner ring the on-accent colour.
                  "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-(--focus-ring)",
                  "focus-visible:inset-ring-2 focus-visible:inset-ring-(--color-on-accent)",
                  isSelected ? "ring-2 ring-offset-2 ring-accent-a" : "",
                ].join(" ")}
              >
                <OrbSwatch spec={spec} />
                {isSelected ? (
                  <Check
                    aria-hidden="true"
                    size={18}
                    strokeWidth={3}
                    className="absolute"
                    style={{ color: spec.check }}
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
        <p className="text-xs text-muted">
          {t("settings.appearance.orb.helper")}
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
