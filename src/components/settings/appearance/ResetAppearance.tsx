import React, { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { useSettingsStore } from "@/stores/settingsStore";
import { applyTheme } from "@/lib/utils/theme";
import { DEFAULT_ACCENT } from "@/styles/accents";
import { Button } from "../../ui/Button";
import { SettingContainer } from "../../ui/SettingContainer";
import { APPEARANCE_DEFAULTS, resetPlan } from "./appearanceModel";

/**
 * Reset to defaults, as an inline two-step confirm (not a native dialog). The first click only
 * asks; the second runs the plan: theme, accent, conversation theme, visual style, transparency, then the orb colour.
 * The result is announced in a status region.
 */
export const ResetAppearance: React.FC = () => {
  const { t } = useTranslation();
  const { settings, refreshSettings } = useSettings();
  // The store action reports whether the write was saved (the hook's copy does not).
  const updateSetting = useSettingsStore((state) => state.updateSetting);
  const changeOrbAccent = useSettingsStore((state) => state.changeOrbAccent);
  const [confirming, setConfirming] = useState(false);
  const [message, setMessage] = useState("");
  const [failed, setFailed] = useState(false);
  const confirmId = useId();

  const runReset = async (): Promise<void> => {
    // Apply the visible part at once, then save each setting in plan order.
    // Any failed write marks the whole reset as failed: no success message is shown.
    let allSaved = true;
    applyTheme(
      APPEARANCE_DEFAULTS.theme,
      settings?.custom_theme_colors,
      DEFAULT_ACCENT,
      APPEARANCE_DEFAULTS.visualStyle,
    );
    for (const step of resetPlan()) {
      if (step.kind === "theme") {
        allSaved = (await updateSetting("theme", step.value)) && allSaved;
      } else if (step.kind === "accent") {
        allSaved = (await updateSetting("accent", step.value)) && allSaved;
      } else if (step.kind === "chatTheme") {
        allSaved = (await updateSetting("chat_theme", step.value)) && allSaved;
      } else if (step.kind === "visualStyle") {
        allSaved =
          (await updateSetting("visual_style", step.value)) && allSaved;
      } else {
        try {
          await commands.changeChatOpacitySetting(step.value);
        } catch (error) {
          console.error("Failed to reset chat opacity:", error);
          allSaved = false;
        }
        await refreshSettings();
      }
    }
    // The orb colour is not in the plan (its pinned order is unchanged): it returns to Auto.
    allSaved = (await changeOrbAccent("auto")) && allSaved;
    setConfirming(false);
    setFailed(!allSaved);
    setMessage(allSaved ? t("settings.appearance.reset.done") : "");
  };

  return (
    <SettingContainer
      title={t("settings.appearance.reset.title")}
      description={t("settings.appearance.reset.description")}
      descriptionMode="inline"
      layout="stacked"
    >
      <div className="flex flex-col gap-2">
        {confirming ? (
          <div
            role="group"
            aria-labelledby={confirmId}
            className="flex flex-wrap items-center gap-2"
          >
            <p id={confirmId} className="text-sm">
              {t("settings.appearance.reset.confirm")}
            </p>
            <Button variant="danger" size="sm" onClick={() => void runReset()}>
              {t("settings.appearance.reset.confirmAction")}
            </Button>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => setConfirming(false)}
            >
              {t("settings.appearance.reset.cancel")}
            </Button>
          </div>
        ) : (
          <div>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => {
                setMessage("");
                setFailed(false);
                setConfirming(true);
              }}
            >
              {t("settings.appearance.reset.title")}
            </Button>
          </div>
        )}
        {failed ? (
          <p role="alert" className="text-xs text-danger">
            {t("settings.appearance.reset.failed")}
          </p>
        ) : (
          <p role="status" aria-live="polite" className="text-xs text-muted">
            {message}
          </p>
        )}
      </div>
    </SettingContainer>
  );
};
