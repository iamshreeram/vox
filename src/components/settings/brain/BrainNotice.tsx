import React from "react";
import { useTranslation } from "react-i18next";
import type { BrainFeature } from "@/bindings";
import { Button } from "../../ui/Button";
import { useSettings } from "../../../hooks/useSettings";
import { useBrainActions } from "./useBrainActions";
import { useOpenBrainPage } from "./brainNavigation";
import { brainViewOf, featureView } from "./brainModel";

interface BrainNoticeProps {
  feature: BrainFeature;
}

/**
 * Shown on a feature's own page while the feature inherits the Brain. Hidden when the
 * feature uses its own setup or the migration marker is absent.
 */
export const BrainNotice: React.FC<BrainNoticeProps> = ({ feature }) => {
  const { t } = useTranslation();
  const { settings } = useSettings();
  const actions = useBrainActions();
  const openBrainPage = useOpenBrainPage();

  if (settings === null) return null;
  const view = featureView(brainViewOf(settings), feature);
  if (view.source !== "inherit") return null;

  return (
    <div
      data-testid={`brain-notice-${feature}`}
      role="note"
      className="flex flex-wrap items-center gap-2 rounded-md border border-logo-primary/30 bg-logo-primary/5 px-4 py-3 text-sm"
    >
      <p className="flex-1 min-w-0">
        {t(
          view.state === "needs_setup"
            ? "settings.brain.notice.needsSetup"
            : view.state === "needs_consent"
              ? "settings.brain.notice.needsConsent"
              : "settings.brain.notice.text",
        )}
      </p>
      <Button variant="secondary" size="sm" onClick={openBrainPage}>
        {t("settings.brain.notice.open")}
      </Button>
      <Button
        variant="secondary"
        size="sm"
        onClick={() => void actions.useForFeature(feature, false)}
      >
        {t("settings.brain.notice.useOwn")}
      </Button>
    </div>
  );
};
