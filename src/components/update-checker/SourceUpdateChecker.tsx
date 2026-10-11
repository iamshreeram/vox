import React, { useEffect, useId, useRef } from "react";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useSettings } from "../../hooks/useSettings";
import { useSourceUpdate } from "./useSourceUpdate";
import {
  canConfirmInstall,
  footerAction,
  footerLabelKey,
  formatChangeList,
} from "./sourceUpdateState";

interface SourceUpdateCheckerProps {
  className?: string;
}

/** Only the project's own compare pages are ever opened from the dialog. */
const TRUSTED_COMPARE_PREFIX = "https://github.com/iamshreeram/vox/";

const SourceUpdateChecker: React.FC<SourceUpdateCheckerProps> = ({
  className = "",
}) => {
  const { t } = useTranslation();
  const { settings, isLoading, updateChecksLocked } = useSettings();
  // Same gating as UpdateChecker: wait for the lock state (null = not loaded
  // yet) and honor the forced-off system configuration.
  const settingsLoaded =
    !isLoading && settings !== null && updateChecksLocked !== null;
  const checksEnabled =
    settingsLoaded &&
    (settings?.update_checks_enabled ?? false) &&
    updateChecksLocked === false;

  const {
    state,
    dialogOpen,
    blocker,
    blockerChecking,
    openDialog,
    closeDialog,
    runCheck,
    confirmInstall,
  } = useSourceUpdate({ checksEnabled });

  const titleId = useId();
  const primaryButtonRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!dialogOpen) return;
    primaryButtonRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") closeDialog();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [dialogOpen, closeDialog]);

  // Before settings load, checksEnabled is false, so this shows the same
  // "checking disabled" label the existing widget shows while loading.
  const label = t(footerLabelKey(state, checksEnabled));
  const action = footerAction(state, checksEnabled);
  const isAvailable = state.phase === "available";

  const handleFooterClick = () => {
    if (action === "openDialog") {
      openDialog();
    } else if (action === "check") {
      void runCheck(true);
    }
  };

  const info = state.info;
  const changeList = info ? formatChangeList(info) : null;
  const canOpenCompare =
    info !== null && info.compare_url.startsWith(TRUSTED_COMPARE_PREFIX);
  const isStarting = state.phase === "starting";
  const installDisabled =
    state.phase !== "available" ||
    !canConfirmInstall(info) ||
    blockerChecking ||
    blocker !== null;
  const showInstallError = state.phase === "error" && info !== null;

  const footerTitle =
    state.phase === "untracked" && info?.note ? info.note : undefined;

  return (
    <>
      {dialogOpen && info && changeList && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
          <div
            role="dialog"
            aria-modal="true"
            aria-labelledby={titleId}
            className="bg-background border border-mid-gray/20 rounded-lg p-6 max-w-md w-full mx-4 space-y-4"
          >
            <h2 id={titleId} className="text-base font-semibold">
              {t("footer.sourceUpdate.dialog.title")}
            </h2>
            <div className="space-y-2 text-sm text-text/70">
              <p>
                {t("footer.sourceUpdate.dialog.summary", {
                  count: info.behind_by,
                })}
              </p>
              <p className="text-xs">
                {t("footer.sourceUpdate.dialog.versions", {
                  current: info.current_commit,
                  latest: info.latest_commit,
                })}
              </p>
              {changeList.lines.length > 0 && (
                <ul className="list-disc pl-5 space-y-1">
                  {changeList.lines.map((line, index) => (
                    <li key={`${index}-${line}`}>{line}</li>
                  ))}
                  {changeList.moreCount > 0 && (
                    <li className="list-none pl-0">
                      {t("footer.sourceUpdate.dialog.more", {
                        count: changeList.moreCount,
                      })}
                    </li>
                  )}
                </ul>
              )}
              {canOpenCompare && (
                <button
                  type="button"
                  onClick={() => {
                    void openUrl(info.compare_url);
                  }}
                  className="text-logo-primary hover:text-logo-primary/80 underline transition-colors text-left"
                >
                  {t("footer.sourceUpdate.dialog.viewChanges")}
                </button>
              )}
              <p>{t("footer.sourceUpdate.dialog.explanation")}</p>
              {blocker !== null && (
                <p
                  role="alert"
                  className="rounded border border-yellow-500/40 bg-yellow-500/10 px-3 py-2 text-xs text-yellow-700 dark:text-yellow-300"
                >
                  {blocker}
                </p>
              )}
              {showInstallError && state.error && (
                <p role="alert" className="text-xs text-red-500">
                  {state.error}
                </p>
              )}
            </div>
            <div className="flex gap-2 justify-end">
              <button
                type="button"
                className="px-3 py-1.5 text-sm rounded border border-mid-gray/20 hover:bg-mid-gray/10 transition-colors"
                onClick={closeDialog}
              >
                {t("footer.sourceUpdate.dialog.notNow")}
              </button>
              <button
                ref={primaryButtonRef}
                type="button"
                disabled={installDisabled}
                className="px-3 py-1.5 text-sm rounded bg-logo-primary text-white hover:bg-logo-primary/80 transition-colors disabled:opacity-50"
                onClick={() => {
                  void confirmInstall();
                }}
              >
                {isStarting
                  ? t("footer.sourceUpdate.dialog.updating")
                  : t("footer.sourceUpdate.dialog.updateNow")}
              </button>
            </div>
          </div>
        </div>
      )}
      <div className={`flex items-center gap-3 ${className}`}>
        {action !== "none" ? (
          <button
            type="button"
            onClick={handleFooterClick}
            title={footerTitle}
            className={`transition-colors disabled:opacity-50 tabular-nums ${
              isAvailable
                ? "text-logo-primary hover:text-logo-primary/80 font-medium"
                : "text-text/60 hover:text-text/80"
            }`}
          >
            {label}
          </button>
        ) : (
          <span
            title={footerTitle}
            className={`tabular-nums ${
              isAvailable ? "text-logo-primary font-medium" : "text-text/60"
            }`}
          >
            {label}
          </span>
        )}
      </div>
    </>
  );
};

export default SourceUpdateChecker;
