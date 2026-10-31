import React, { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import { getVersion } from "@tauri-apps/api/app";
import { useSettings } from "../../hooks/useSettings";
import { versionLabel } from "../footer/footerLabels";
import { useSourceUpdate } from "./useSourceUpdate";
import {
  canConfirmInstall,
  footerAction,
  footerLabelKey,
  formatChangeList,
  milestonesCrossed,
  progressTextKey,
  stageLabelKey,
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

  // The app version is shown in the footer already; the "updated" label reuses it.
  const [appVersion, setAppVersion] = useState("");
  useEffect(() => {
    let active = true;
    getVersion()
      .then((value) => {
        if (active) setAppVersion(value);
      })
      .catch(() => {
        // Without a version the label falls back to the commit alone.
      });
    return () => {
      active = false;
    };
  }, []);

  // Screen-reader announcements: only at 0/25/50/75/100 and at the end.
  const [announcement, setAnnouncement] = useState("");
  const announcedRef = useRef<{
    runId: string | null;
    percent: number;
    outcome: string | null;
  }>({ runId: null, percent: -1, outcome: null });
  const runId = state.run?.runId ?? null;
  const runPercent = state.run?.percent ?? -1;
  useEffect(() => {
    const last = announcedRef.current;
    if (last.runId !== runId) {
      last.runId = runId;
      last.percent = -1;
      last.outcome = null;
    }
    const run = state.run;
    if (state.phase === "updating" && run !== null) {
      const crossed = milestonesCrossed(last.percent, runPercent);
      const top = crossed[crossed.length - 1];
      if (top !== undefined) {
        last.percent = top;
        const stage = t(stageLabelKey(run.backendPhase ?? "preparing"));
        setAnnouncement(
          t("footer.sourceUpdate.announce", { percent: top, stage }),
        );
      }
    } else if (
      (state.phase === "updated" || state.phase === "failed") &&
      last.outcome !== state.phase
    ) {
      last.outcome = state.phase;
      setAnnouncement(
        state.phase === "updated"
          ? t("footer.sourceUpdate.announceDone", {
              sha: run?.target ?? "",
            })
          : t("footer.sourceUpdate.failed"),
      );
    }
  }, [state.phase, runId, runPercent, t]);

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
  const run = state.run;
  const isUpdating = state.phase === "updating" || isStarting;
  const percent = run?.percent ?? 0;
  const stageText = t(stageLabelKey(run?.backendPhase ?? "preparing"));
  const updatedVersion = versionLabel(appVersion) ?? "Vox";
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
                  className="rounded border border-warning bg-warning-soft px-3 py-2 text-xs text-warning dark:text-warning"
                >
                  {blocker}
                </p>
              )}
              {showInstallError && state.error && (
                <p role="alert" className="text-xs text-danger">
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
        {/* Always mounted so screen readers catch every announcement. */}
        <span role="status" className="sr-only">
          {announcement}
        </span>
        {isUpdating ? (
          <span className="flex items-center gap-2 whitespace-nowrap text-muted">
            <span className="tabular-nums min-w-[9.5rem]">
              {isStarting
                ? t("footer.sourceUpdate.starting")
                : t(progressTextKey(run), { percent })}
            </span>
            <span
              role="progressbar"
              aria-label={t("footer.sourceUpdate.a11y.progress")}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={percent}
              aria-valuetext={t("footer.sourceUpdate.progressValueText", {
                percent,
                stage: stageText,
              })}
              className="block h-1 w-20 overflow-hidden rounded-full bg-mid-gray/20"
            >
              <span
                className="block h-full rounded-full bg-logo-primary transition-[width] duration-300 ease-linear motion-reduce:transition-none motion-reduce:transition-none"
                style={{ width: `${percent}%` }}
              />
            </span>
          </span>
        ) : state.phase === "updated" ? (
          <span
            title={footerTitle}
            className="tabular-nums whitespace-nowrap text-muted"
          >
            {t("footer.sourceUpdate.updated", {
              version: updatedVersion,
              sha: run?.target ?? "",
            })}
          </span>
        ) : state.phase === "failed" ? (
          <span className="flex min-w-0 items-center gap-2 whitespace-nowrap">
            <span className="text-danger">
              {t("footer.sourceUpdate.failed")}
            </span>
            {state.error !== null && (
              <span
                role="alert"
                title={state.error}
                className="inline-block max-w-[12rem] truncate align-bottom text-xs text-muted"
              >
                {state.error}
              </span>
            )}
            <button
              type="button"
              onClick={() => {
                void runCheck(true);
              }}
              className="underline text-logo-primary hover:text-logo-primary/80 transition-colors"
            >
              {t("footer.sourceUpdate.retry")}
            </button>
          </span>
        ) : action !== "none" ? (
          <button
            type="button"
            onClick={handleFooterClick}
            title={footerTitle}
            className={`transition-colors disabled:opacity-50 tabular-nums ${
              isAvailable
                ? "text-logo-primary hover:text-logo-primary/80 font-medium"
                : "text-muted hover:text-text/80"
            }`}
          >
            {label}
          </button>
        ) : (
          <span
            title={footerTitle}
            className={`tabular-nums ${
              isAvailable ? "text-logo-primary font-medium" : "text-muted"
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
