import { useTranslation } from "react-i18next";
import type { BrainFeature } from "@/bindings";

/**
 * The translated name of a feature, as shown on the Brain page cards. Each key is written
 * out literally so every translation key stays statically visible to the audits.
 */
export function useFeatureLabel(): (feature: BrainFeature) => string {
  const { t } = useTranslation();
  return (feature) => {
    switch (feature) {
      case "composer":
        return t("settings.brain.feature.composer");
      case "realtime_stream":
        return t("settings.brain.feature.realtime_stream");
      case "post_process":
        return t("settings.brain.feature.post_process");
    }
  };
}
