import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import type { VaultIndexStatusDto } from "@/bindings";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { SettingContainer } from "../ui/SettingContainer";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";
import { useSettingsStore } from "../../stores/settingsStore";
import { VaultRagPreview } from "./VaultRagPreview";
import {
  VAULT_RAG_POLL_MS,
  canEnableVaultRag,
  canReindex,
  createStatusPoller,
  foldersOutcome,
  normalizeFolders,
  probeMessage,
  shouldPollStatus,
  statusView,
} from "./vaultRagForm";

interface VaultRagSettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

type RagSettingsPatch =
  | { vault_rag_enabled: boolean }
  | { vault_rag_folders: string };

/** Mirrors a saved value into the settings store (the backend already has it). */
function patchRagSettings(patch: RagSettingsPatch): void {
  useSettingsStore.setState((state) => ({
    settings: state.settings && { ...state.settings, ...patch },
  }));
}

export const VaultRagSettings: React.FC<VaultRagSettingsProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const { getSetting } = useSettings();
  const enabled = getSetting("vault_rag_enabled") ?? false;
  const vaultPath = getSetting("vault_path") ?? "";
  const folders = getSetting("vault_rag_folders") ?? "";

  const [localFolders, setLocalFolders] = useState(folders);
  const [foldersError, setFoldersError] = useState<string | null>(null);
  const [dto, setDto] = useState<VaultIndexStatusDto | null>(null);
  // One poller per component: its in-flight state survives effect re-runs.
  const [poller] = useState(() =>
    createStatusPoller({
      run: () => commands.vaultIndexStatus().then(setDto, () => setDto(null)),
      schedule: (fn, ms) => window.setTimeout(fn, ms),
      cancel: (handle) => window.clearTimeout(handle as number),
      intervalMs: VAULT_RAG_POLL_MS,
    }),
  );
  const [probeLine, setProbeLine] = useState<string | null>(null);
  const [reindexing, setReindexing] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [toggling, setToggling] = useState(false);

  useEffect(() => setLocalFolders(folders), [folders]);

  // Polls only while visible and enabled; the poller never overlaps requests.
  useEffect(() => {
    const sync = () => {
      const pageVisible = document.visibilityState === "visible";
      if (shouldPollStatus({ pageVisible, enabled, requestInFlight: false })) {
        poller.start();
      } else {
        poller.stop();
      }
    };
    sync();
    document.addEventListener("visibilitychange", sync);
    return () => {
      document.removeEventListener("visibilitychange", sync);
      poller.stop();
    };
  }, [enabled, poller]);

  // Literal keys only: the i18n scanner rejects dynamic t() calls.
  const statusLabel = {
    off: t("settings.experimental.vaultRag.status.off"),
    indexing: t("settings.experimental.vaultRag.status.indexing"),
    ready: t("settings.experimental.vaultRag.status.ready"),
    unavailable: t("settings.experimental.vaultRag.status.unavailable"),
    busy: t("settings.experimental.vaultRag.status.busy"),
  };
  const probeLabel: Record<string, string> = {
    supported: t("settings.experimental.vaultRag.probe.results.supported"),
    unsupported: t("settings.experimental.vaultRag.probe.results.unsupported"),
    unknown: t("settings.experimental.vaultRag.probe.results.unknown"),
  };

  // Direct call so a refusal shows inline; a refused toggle keeps its stored (OFF) value.
  const setEnabled = async (value: boolean): Promise<void> => {
    setToggling(true);
    const result = await commands.changeVaultRagSetting(value);
    setToggling(false);
    setActionError(result.status === "error" ? result.error : null);
    if (result.status === "ok") patchRagSettings({ vault_rag_enabled: value });
  };

  const saveFolders = async (): Promise<void> => {
    const attempted = localFolders;
    // Called directly (not via updateSetting) so the backend error can be shown inline.
    const result = await commands.changeVaultRagFoldersSetting(attempted);
    const saved = normalizeFolders(attempted);
    const outcome = foldersOutcome(folders, saved, result);
    setLocalFolders(outcome.value);
    setFoldersError(outcome.error);
    if (outcome.error === null)
      patchRagSettings({ vault_rag_folders: outcome.value });
  };

  const reindex = async (): Promise<void> => {
    setReindexing(true);
    const result = await commands.vaultReindex();
    setActionError(result.status === "error" ? result.error : null);
    setReindexing(false);
  };

  const runProbe = async (): Promise<void> => {
    const message = probeMessage(await commands.vaultRagProbe());
    setProbeLine(
      message.kind === "error" ? message.text : probeLabel[message.kind],
    );
  };

  const view = statusView(enabled, dto);
  const canEnable = canEnableVaultRag(vaultPath);

  return (
    <>
      <ToggleSwitch
        checked={enabled}
        onChange={(value) => setEnabled(value)}
        isUpdating={toggling}
        disabled={!enabled && !canEnable}
        label={t("settings.experimental.vaultRag.enable.label")}
        description={t("settings.experimental.vaultRag.enable.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      />
      {!canEnable && (
        <p className="px-3 text-xs text-mid-gray">
          {t("settings.experimental.vaultRag.needsVaultPath")}
        </p>
      )}
      <SettingContainer
        title={t("settings.experimental.vaultRag.folders.label")}
        description={t("settings.experimental.vaultRag.folders.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      >
        <div className="flex flex-col gap-1">
          <Input
            type="text"
            value={localFolders}
            onChange={(event) => setLocalFolders(event.target.value)}
            onBlur={() => saveFolders()}
            placeholder={t(
              "settings.experimental.vaultRag.folders.placeholder",
            )}
            variant="compact"
            className="flex-1 min-w-[360px]"
          />
          {foldersError !== null && (
            <p role="alert" className="text-xs text-red-400">
              {foldersError}
            </p>
          )}
        </div>
      </SettingContainer>
      <SettingContainer
        title={t("settings.experimental.vaultRag.title")}
        description={t("settings.experimental.vaultRag.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      >
        <div className="flex flex-col gap-2">
          <p className="text-sm">{statusLabel[view.key]}</p>
          {view.key !== "off" && (
            <p className="text-xs text-mid-gray">
              {t("settings.experimental.vaultRag.status.counts", {
                notes: view.notes,
                chunks: view.chunks,
                skipped: view.skipped,
              })}
            </p>
          )}
          {view.toolSupport !== "untested" && (
            <p className="text-xs">{probeLabel[view.toolSupport]}</p>
          )}
          <div className="flex flex-wrap gap-2">
            <Button
              variant="secondary"
              size="sm"
              disabled={!canReindex(enabled, dto) || reindexing}
              onClick={() => reindex()}
            >
              {t("settings.experimental.vaultRag.reindex.button")}
            </Button>
            <Button
              variant="secondary"
              size="sm"
              disabled={!enabled}
              onClick={() => runProbe()}
            >
              {t("settings.experimental.vaultRag.probe.button")}
            </Button>
          </div>
          {probeLine !== null && <p className="text-xs">{probeLine}</p>}
          {actionError !== null && (
            <p role="alert" className="text-xs text-red-400">
              {actionError}
            </p>
          )}
        </div>
      </SettingContainer>
      <VaultRagPreview enabled={enabled} onError={setActionError} />
      <ul className="flex flex-col gap-2 px-3 pb-3 text-xs text-mid-gray">
        <li>{t("settings.experimental.vaultRag.disclosure.folders")}</li>
        <li>{t("settings.experimental.vaultRag.disclosure.sentToProvider")}</li>
        <li>
          {t("settings.experimental.vaultRag.disclosure.bestEffortRedaction")}
        </li>
        <li>
          {t("settings.experimental.vaultRag.disclosure.notesMayInstruct")}
        </li>
        <li>{t("settings.experimental.vaultRag.disclosure.spokenAloud")}</li>
        <li>
          {t("settings.experimental.vaultRag.disclosure.toolsRequirement")}
        </li>
        <li>
          {t("settings.experimental.vaultRag.disclosure.unsupportedFallback")}
        </li>
        <li>{t("settings.experimental.vaultRag.disclosure.localCache")}</li>
      </ul>
    </>
  );
};
