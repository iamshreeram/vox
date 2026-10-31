import React from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../ui/Button";
import { useSettings } from "../../hooks/useSettings";
import { shortcutLoadView } from "./shortcutLoadState";

/**
 * The shortcut list failed to load (T5, U9). Shows the actionable message with a real Retry that
 * re-runs the settings load. Only the announcing row carries role="alert" (see shortcutLoadState).
 */
export const ShortcutLoadError: React.FC<{ shortcutId: string }> = ({
  shortcutId,
}) => {
  const { t } = useTranslation();
  const { loadFailed, loadRetrying, retryLoad } = useSettings();
  const view = shortcutLoadView({
    loadFailed,
    retrying: loadRetrying,
    shortcutId,
  });
  if (!view.showError) return null;

  return (
    <div className="flex flex-col items-start gap-2">
      <p
        role={view.role}
        data-testid="shortcut-load-error"
        className="text-sm text-muted"
      >
        {t("settings.general.shortcut.loadFailed")}
      </p>
      <Button
        variant="secondary"
        size="md"
        className="min-h-[36px]"
        disabled={view.retryDisabled}
        aria-busy={view.busy}
        onClick={() => void retryLoad()}
      >
        {t(view.retryLabelKey)}
      </Button>
    </div>
  );
};
