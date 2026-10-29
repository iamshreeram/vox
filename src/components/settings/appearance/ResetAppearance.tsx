import React, { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { applyTheme } from "@/lib/utils/theme";
import { DEFAULT_ACCENT } from "@/styles/accents";
import { Button } from "../../ui/Button";
import { SettingContainer } from "../../ui/SettingContainer";
import { APPEARANCE_DEFAULTS, resetPlan } from "./appearanceModel";

/**
 * Reset to defaults, as an inline two-step confirm (not a native dialog). The first click only
 * asks; the second runs the plan: theme, accent, conversation theme, visual style, transparency.
 * The result is announced in a status region.
 */
export const ResetAppearance: React.FC = () => {
  const { t } = useTranslation();
  const { settings, updateSetting, refreshSettings } = useSettings();
  const [confirming, setConfirming] = useState(false);
  const [message, setMessage] = useState("");
  const confirmId = useId();

  const runReset = async (): Promise<void> => {
    // Apply the visible part at once, then save each setting in plan order.
    applyTheme(
      APPEARANCE_DEFAULTS.theme,
      settings?.custom_theme_colors,
      DEFAULT_ACCENT,
      APPEARANCE_DEFAULTS.visualStyle,
    );
    for (const step of resetPlan()) {
      if (step.kind === "theme") await updateSetting("theme", step.value);
      else if (step.kind === "accent")
        await updateSetting("accent", step.value);
      else if (step.kind === "chatTheme") {
        await updateSetting("chat_theme", step.value);
      } else if (step.kind === "visualStyle") {
        await updateSetting("visual_style", step.value);
      } else {
        await commands
          .changeChatOpacitySetting(step.value)
          .catch(() => undefined);
        await refreshSettings();
      }
    }
    setConfirming(false);
    setMessage(t("settings.appearance.reset.done"));
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
                setConfirming(true);
              }}
            >
              {t("settings.appearance.reset.title")}
            </Button>
          </div>
        )}
        <p role="status" aria-live="polite" className="text-xs text-mid-gray">
          {message}
        </p>
      </div>
    </SettingContainer>
  );
};
