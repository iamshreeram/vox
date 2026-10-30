import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import type { BrainFeature } from "@/bindings";
import { Button } from "../../ui/Button";
import { useSettings } from "../../../hooks/useSettings";
import { useBrainActions } from "../brain/useBrainActions";
import { useOpenBrainPage } from "../brain/brainNavigation";
import { useFeatureLabel } from "../brain/useFeatureLabel";
import { brainViewOf, featureView } from "../brain/brainModel";
import {
  setupBannerModel,
  type BannerFeatureState,
  type BannerInheritance,
  type BannerPage,
} from "./setupBanner";

/** Set to "1" once the user dismisses the banner's Brain prerequisite. Shared by every window. */
export const FINISH_SETUP_DISMISSED_KEY = "vox.finishSetup.dismissed";

function readDismissed(): boolean {
  try {
    return window.localStorage.getItem(FINISH_SETUP_DISMISSED_KEY) === "1";
  } catch {
    return false;
  }
}

const FEATURES_ON_GENERAL = (experimental: boolean): BrainFeature[] =>
  experimental ? ["composer", "realtime_stream"] : ["composer"];

/**
 * The one setup-status banner, shown identically on General and Models (UI enterprise S4a, U6).
 * It only navigates, stores a dismissal flag or calls the existing per-feature command: it never
 * makes a Brain request itself.
 */
export const SetupStatusBanner: React.FC<{ page: BannerPage }> = ({ page }) => {
  const { t } = useTranslation();
  const { settings } = useSettings();
  const actions = useBrainActions();
  const openBrainPage = useOpenBrainPage();
  const featureLabel = useFeatureLabel();
  const [dismissed, setDismissed] = useState(readDismissed);

  if (settings === null) return null;

  const view = brainViewOf(settings);
  const inherit: BannerInheritance[] = [];
  if (page === "general") {
    for (const feature of FEATURES_ON_GENERAL(
      settings.experimental_enabled === true,
    )) {
      const entry = featureView(view, feature);
      if (entry.source !== "inherit") continue;
      const state: BannerFeatureState =
        entry.state === "needs_setup" || entry.state === "needs_consent"
          ? entry.state
          : "ready";
      inherit.push({ feature, state });
    }
  }
  const brainUnset =
    (settings.brain_provider_id ?? "") === "" &&
    (settings.brain_model ?? "") === "";
  const model = setupBannerModel({ page, inherit, brainUnset });
  if (!model.visible) return null;
  // A dismissed Brain prerequisite stays hidden (persisted), as the old callout did.
  if (dismissed && model.secondary === "dismiss") return null;

  const handleDismiss = () => {
    try {
      window.localStorage.setItem(FINISH_SETUP_DISMISSED_KEY, "1");
    } catch {
      // Storage is unavailable: the banner still hides for this session.
    }
    setDismissed(true);
  };

  const textFor = (
    state: BannerFeatureState,
    feature: BrainFeature,
  ): string => {
    const name = featureLabel(feature);
    if (state === "needs_setup")
      return t("settings.brain.notice.needsSetup", { feature: name });
    if (state === "needs_consent")
      return t("settings.brain.notice.needsConsent", { feature: name });
    return t("settings.brain.notice.text", { feature: name });
  };

  return (
    <section
      role="note"
      data-testid="setup-status-banner"
      className="w-full rounded-md border border-mid-gray/30 bg-mid-gray/5 px-4 py-3 text-sm flex flex-col gap-3"
    >
      <ul className="space-y-1">
        {model.items.map((item) =>
          item.kind === "feature" ? (
            <li key={item.feature} data-testid={`brain-notice-${item.feature}`}>
              {textFor(item.state, item.feature)}
            </li>
          ) : (
            <li key="brain_unset" data-testid="brain-unset">
              <span className="font-medium">
                {t("settings.models.finishSetup.title")}
              </span>{" "}
              {t("settings.models.finishSetup.description")}
            </li>
          ),
        )}
      </ul>
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="primary" size="sm" onClick={openBrainPage}>
          {t("settings.brain.notice.open")}
        </Button>
        {model.secondary === "use_own" && (
          <Button
            variant="secondary"
            size="sm"
            onClick={() =>
              model.items.forEach((item) => {
                if (item.kind === "feature")
                  void actions.useForFeature(item.feature, false);
              })
            }
          >
            {t("settings.brain.notice.useOwn")}
          </Button>
        )}
        {model.secondary === "dismiss" && (
          <Button variant="secondary" size="sm" onClick={handleDismiss}>
            {t("settings.models.finishSetup.dismiss")}
          </Button>
        )}
      </div>
    </section>
  );
};
