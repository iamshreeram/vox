import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { X } from "lucide-react";
import { useAutoUpdateStatus } from "../settings/useAutoUpdateStatus";
import {
  DISMISS_KEY,
  autoUpdateFailureNotice,
  parseDismissed,
} from "../update-checker/autoUpdateFailure";

function readDismissed(): number | null {
  try {
    return parseDismissed(localStorage.getItem(DISMISS_KEY));
  } catch {
    return null;
  }
}

/**
 * Footer alert for a failed automatic update. Hidden until the failure
 * happens, and again once the user dismisses that failure.
 */
const AutoUpdateFailureNotice: React.FC = () => {
  const { t, i18n } = useTranslation();
  const status = useAutoUpdateStatus();
  const [dismissedTs, setDismissedTs] = useState<number | null>(readDismissed);

  const notice = autoUpdateFailureNotice(status, dismissedTs);
  if (notice === null) return null;

  const reason = t(notice.reasonKey);
  const text =
    notice.targetShort === null
      ? t("footer.autoUpdate.failed", { reason })
      : t("footer.autoUpdate.failedTarget", {
          target: notice.targetShort,
          reason,
        });
  const date = new Intl.DateTimeFormat(i18n.language, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(notice.ts * 1000));

  const dismiss = (): void => {
    try {
      localStorage.setItem(DISMISS_KEY, String(notice.ts));
    } catch (error) {
      // Storage unavailable: the notice stays hidden for this session only.
      console.error("Failed to store the dismissed update failure:", error);
    }
    setDismissedTs(notice.ts);
  };

  return (
    <span
      role="alert"
      className="flex min-w-0 max-w-full items-center gap-1 text-danger"
    >
      <span title={`${text} (${date})`} className="min-w-0 truncate">
        {text}
      </span>
      <button
        type="button"
        aria-label={t("footer.autoUpdate.dismiss")}
        onClick={dismiss}
        className="shrink-0 cursor-pointer rounded-md border border-transparent p-1 text-muted transition-colors hover:border-mid-gray/20 hover:bg-mid-gray/10 hover:text-text focus-visible:ring-2 focus-visible:ring-(--focus-ring)"
      >
        <X className="h-3 w-3" aria-hidden="true" />
      </button>
    </span>
  );
};

export default AutoUpdateFailureNotice;
