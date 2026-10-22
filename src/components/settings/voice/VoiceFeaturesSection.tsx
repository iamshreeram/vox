import React from "react";
import { useTranslation } from "react-i18next";
import { LatencyPanel } from "./LatencyPanel";
import { PersonaSettings } from "./PersonaSettings";
import { SmartTurnSettings } from "./SmartTurnSettings";
import { SpeakerFilterSettings } from "./SpeakerFilterSettings";

interface VoiceFeaturesSectionProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * The optional voice features, grouped under one heading inside the
 * conversation settings. Each part has its own sub-heading; all are off by
 * default and each one downloads nothing until its Download button is pressed.
 */
export const VoiceFeaturesSection: React.FC<VoiceFeaturesSectionProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    return (
      <section className="flex flex-col gap-3">
        <h3 className="px-4 pt-2 text-sm font-semibold">
          {t("converse.voice.title")}
        </h3>
        <p className="px-4 text-xs text-mid-gray">
          {t("converse.voice.intro")}
        </p>

        <h4 className="px-4 text-xs font-medium uppercase tracking-wide text-mid-gray">
          {t("converse.voice.smartTurn.heading")}
        </h4>
        <SmartTurnSettings
          descriptionMode={descriptionMode}
          grouped={grouped}
        />

        <h4 className="px-4 text-xs font-medium uppercase tracking-wide text-mid-gray">
          {t("converse.voice.latency.heading")}
        </h4>
        <LatencyPanel descriptionMode={descriptionMode} grouped={grouped} />

        <h4 className="px-4 text-xs font-medium uppercase tracking-wide text-mid-gray">
          {t("converse.voice.persona.heading")}
        </h4>
        <PersonaSettings descriptionMode={descriptionMode} grouped={grouped} />

        <h4 className="px-4 text-xs font-medium uppercase tracking-wide text-mid-gray">
          {t("converse.voice.speaker.heading")}
        </h4>
        <SpeakerFilterSettings
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
      </section>
    );
  });

VoiceFeaturesSection.displayName = "VoiceFeaturesSection";
