import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../ui/Button";
import { useSettings } from "../../../hooks/useSettings";
import { useOpenBrainPage } from "../brain/brainNavigation";

/** Set to "1" once the user dismisses the callout. Shared by every window of this app. */
export const FINISH_SETUP_DISMISSED_KEY = "vox.finishSetup.dismissed";

function readDismissed(): boolean {
  try {
    return window.localStorage.getItem(FINISH_SETUP_DISMISSED_KEY) === "1";
  } catch {
    return false;
  }
}

/**
 * Shown on Models to users who never connected the Brain. It only navigates or stores a
 * flag: it never makes a request.
 */
export const FinishSetupCallout: React.FC = () => {
  const { t } = useTranslation();
  const { settings } = useSettings();
  const openBrainPage = useOpenBrainPage();
  const [dismissed, setDismissed] = useState(readDismissed);

  if (settings === null || dismissed) return null;
  const brainUnset =
    (settings.brain_provider_id ?? "") === "" &&
    (settings.brain_model ?? "") === "";
  if (!brainUnset) return null;

  const handleDismiss = () => {
    try {
      window.localStorage.setItem(FINISH_SETUP_DISMISSED_KEY, "1");
    } catch {
      // Storage is unavailable: the callout still hides for this session.
    }
    setDismissed(true);
  };

  return (
    <section
      data-testid="finish-setup-callout"
      className="rounded-lg border border-logo-primary/40 bg-logo-primary/10 p-4 flex flex-col gap-3"
    >
      <div className="space-y-1">
        <h2 className="text-sm font-semibold">
          {t("settings.models.finishSetup.title")}
        </h2>
        <p className="text-sm text-text/70">
          {t("settings.models.finishSetup.description")}
        </p>
      </div>
      <div className="flex items-center gap-2">
        <Button variant="primary" size="sm" onClick={openBrainPage}>
          {t("settings.models.finishSetup.action")}
        </Button>
        <Button variant="secondary" size="sm" onClick={handleDismiss}>
          {t("settings.models.finishSetup.dismiss")}
        </Button>
      </div>
    </section>
  );
};
