import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import type { BrainFeature } from "@/bindings";
import { Button } from "../../ui/Button";
import { Dialog } from "../../ui/Dialog";
import { useSettings } from "../../../hooks/useSettings";
import { useBrainActions } from "./useBrainActions";
import { brainViewOf, destinationLabel, featureView } from "./brainModel";
import { useFeatureLabel } from "./useFeatureLabel";

const FEATURES: readonly BrainFeature[] = [
  "composer",
  "realtime_stream",
  "post_process",
];

/**
 * One card per feature: whether it uses the Brain or its own setup. Using the Brain asks
 * for explicit consent that names the destination; using its own setup needs none.
 */
export const BrainFeatureGroup: React.FC = () => {
  const { t } = useTranslation();
  const { settings } = useSettings();
  const actions = useBrainActions();
  const featureLabel = useFeatureLabel();
  const [consentFor, setConsentFor] = useState<BrainFeature | null>(null);

  if (settings === null) return null;

  const view = brainViewOf(settings);
  const destination = destinationLabel(view.brain_base_url ?? "");

  const confirmConsent = async () => {
    const feature = consentFor;
    setConsentFor(null);
    if (feature !== null) await actions.useForFeature(feature, true);
  };

  return (
    <>
      {FEATURES.map((feature) => {
        const state = featureView(view, feature);
        const alreadyInherits =
          state.source === "inherit" && state.state === "inheriting";
        return (
          <div
            key={feature}
            data-testid={`brain-feature-${feature}`}
            className="px-4 py-3 space-y-2"
          >
            <div className="flex flex-wrap items-center justify-between gap-2">
              <p className="text-sm font-medium">
                {t(`settings.brain.feature.${feature}`)}
              </p>
              <p
                data-testid={`brain-feature-${feature}-state`}
                className="text-xs text-mid-gray"
              >
                {t(`settings.brain.state.${state.state}`)}
              </p>
            </div>
            <div className="flex items-center gap-2">
              <Button
                data-testid={`brain-feature-${feature}-use-brain`}
                variant="secondary"
                size="sm"
                disabled={!state.canUseBrain || alreadyInherits}
                onClick={() => setConsentFor(feature)}
              >
                {t("settings.brain.feature.useBrain")}
              </Button>
              <Button
                data-testid={`brain-feature-${feature}-use-own`}
                variant="secondary"
                size="sm"
                disabled={state.source === "own"}
                onClick={() => void actions.useForFeature(feature, false)}
              >
                {t("settings.brain.feature.useOwn")}
              </Button>
            </div>
          </div>
        );
      })}
      <Dialog
        open={consentFor !== null}
        title={
          consentFor === null
            ? ""
            : t("settings.brain.consent.title", {
                feature: featureLabel(consentFor),
              })
        }
        onOpenChange={(open) => {
          if (!open) setConsentFor(null);
        }}
        closeLabel={t("settings.brain.consent.cancel")}
      >
        <div data-testid="brain-consent-dialog" className="space-y-4">
          <p className="text-sm">{t("settings.brain.consent.body")}</p>
          <p
            data-testid="brain-consent-destination"
            className="text-sm font-medium"
          >
            {t("settings.brain.consent.destination", { destination })}
          </p>
          <div className="flex justify-end gap-2">
            <Button
              data-testid="brain-consent-cancel"
              variant="secondary"
              size="md"
              onClick={() => setConsentFor(null)}
            >
              {t("settings.brain.consent.cancel")}
            </Button>
            <Button
              data-testid="brain-consent-confirm"
              variant="primary"
              size="md"
              onClick={() => void confirmConsent()}
            >
              {t("settings.brain.consent.confirm")}
            </Button>
          </div>
        </div>
      </Dialog>
    </>
  );
};
