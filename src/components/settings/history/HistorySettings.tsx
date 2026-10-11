import React, { useCallback, useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { readFile } from "@tauri-apps/plugin-fs";
import { Check, Copy, FolderOpen, RotateCcw, Star, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  commands,
  events,
  type HistoryEntry,
  type HistoryUpdatePayload,
} from "@/bindings";
import { useOsType } from "@/hooks/useOsType";
import {
  createHistorySearchController,
  type HistorySearchController,
  type HistorySearchState,
} from "@/lib/utils/historySearchController";
import { formatDateTime } from "@/utils/dateFormat";
import { AudioPlayer, AudioPlayerGroup } from "../../ui/AudioPlayer";
import { Button } from "../../ui/Button";
import { Input } from "../../ui/Input";
import { copyToClipboard } from "./clipboard";

const IconButton: React.FC<{
  onClick: () => void;
  title: string;
  disabled?: boolean;
  active?: boolean;
  children: React.ReactNode;
}> = ({ onClick, title, disabled, active, children }) => (
  <button
    onClick={onClick}
    disabled={disabled}
    className={`p-1.5 rounded-md flex items-center justify-center transition-colors cursor-pointer disabled:cursor-not-allowed disabled:text-text/20 ${
      active
        ? "text-logo-primary hover:text-logo-primary/80"
        : "text-text/50 hover:text-logo-primary"
    }`}
    title={title}
  >
    {children}
  </button>
);

const PAGE_SIZE = 30;

interface OpenRecordingsButtonProps {
  onClick: () => void;
  label: string;
}

const OpenRecordingsButton: React.FC<OpenRecordingsButtonProps> = ({
  onClick,
  label,
}) => (
  <Button
    onClick={onClick}
    variant="secondary"
    size="sm"
    className="flex items-center gap-2"
    title={label}
  >
    <FolderOpen className="w-4 h-4" />
    <span>{label}</span>
  </Button>
);

const INITIAL_STATE: HistorySearchState<HistoryEntry> = {
  mode: "list",
  entries: [],
  loading: true,
  error: null,
  hasMore: false,
  query: "",
};

export const HistorySettings: React.FC = () => {
  const { t } = useTranslation();
  const osType = useOsType();
  const [state, setState] =
    useState<HistorySearchState<HistoryEntry>>(INITIAL_STATE);
  // Local input text: the controller's `query` is trimmed, which would strip
  // the trailing space of a multi-word search while the user is typing it.
  const [searchInput, setSearchInput] = useState("");
  const sentinelRef = useRef<HTMLDivElement>(null);
  const controllerRef = useRef<HistorySearchController<HistoryEntry> | null>(
    null,
  );

  // The controller is created in the mount effect rather than lazily in
  // render: `dispose()` is permanent, and React StrictMode runs
  // mount -> unmount -> remount, so the remount needs a fresh controller.
  useEffect(() => {
    const controller = createHistorySearchController<HistoryEntry>({
      search: async (query) => {
        const result = await commands.searchHistoryEntries(query, null);
        if (result.status !== "ok") {
          throw new Error(String(result.error));
        }
        return result.data;
      },
      fetchPage: async (cursor) => {
        const result = await commands.getHistoryEntries(cursor, PAGE_SIZE);
        if (result.status !== "ok") {
          throw new Error(String(result.error));
        }
        return {
          entries: result.data.entries,
          hasMore: result.data.has_more,
        };
      },
      onState: setState,
      setTimer: (callback, ms) => window.setTimeout(callback, ms),
      clearTimer: (handle) => window.clearTimeout(handle as number),
    });
    controllerRef.current = controller;
    void controller.refresh();

    // Listen for entries added/updated from the transcription pipeline.
    const unlisten = events.historyUpdatePayload.listen((event) => {
      const payload: HistoryUpdatePayload = event.payload;
      if (controller.getState().mode === "search") {
        // Search results are not patched incrementally; re-run the search.
        controller.onHistoryUpdated();
      } else if (payload.action === "added") {
        controller.patchEntries((prev) => [payload.entry, ...prev]);
      } else if (payload.action === "updated") {
        controller.patchEntries((prev) =>
          prev.map((e) => (e.id === payload.entry.id ? payload.entry : e)),
        );
      }
      // "deleted" and "toggled" are handled by optimistic updates only,
      // so we intentionally ignore them here to avoid double-mutation.
    });

    return () => {
      controller.dispose();
      controllerRef.current = null;
      unlisten.then((fn) => fn());
    };
  }, []);

  const hasEntries = state.entries.length > 0;
  const canLoadMore = state.mode === "list" && state.hasMore && !state.loading;

  // Infinite scroll via IntersectionObserver
  useEffect(() => {
    const sentinel = sentinelRef.current;
    if (!canLoadMore || !hasEntries || !sentinel) return;

    const observer = new IntersectionObserver(
      (observerEntries) => {
        const first = observerEntries[0];
        if (first.isIntersecting) {
          void controllerRef.current?.loadMore();
        }
      },
      { threshold: 0 },
    );

    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [canLoadMore, hasEntries]);

  const reloadList = () => {
    setSearchInput("");
    void controllerRef.current?.refresh();
  };

  const handleSearchChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    setSearchInput(event.target.value);
    controllerRef.current?.setQuery(event.target.value);
  };

  const toggleSaved = async (id: number) => {
    const flipSaved = (prev: HistoryEntry[]) =>
      prev.map((e) => (e.id === id ? { ...e, saved: !e.saved } : e));

    // Optimistic update
    controllerRef.current?.patchEntries(flipSaved);
    try {
      const result = await commands.toggleHistoryEntrySaved(id);
      if (result.status !== "ok") {
        // Revert on failure
        controllerRef.current?.patchEntries(flipSaved);
      }
    } catch (error) {
      console.error("Failed to toggle saved status:", error);
      // Revert on failure
      controllerRef.current?.patchEntries(flipSaved);
    }
  };

  const getAudioUrl = useCallback(
    async (fileName: string) => {
      try {
        const result = await commands.getAudioFilePath(fileName);
        if (result.status === "ok") {
          if (osType === "linux") {
            const fileData = await readFile(result.data);
            const blob = new Blob([fileData], { type: "audio/wav" });
            return URL.createObjectURL(blob);
          }
          return convertFileSrc(result.data, "asset");
        }
        return null;
      } catch (error) {
        console.error("Failed to get audio file path:", error);
        return null;
      }
    },
    [osType],
  );

  const deleteAudioEntry = async (id: number) => {
    // Optimistically remove
    controllerRef.current?.patchEntries((prev) =>
      prev.filter((e) => e.id !== id),
    );
    try {
      const result = await commands.deleteHistoryEntry(id);
      if (result.status !== "ok") {
        // Reload on failure
        reloadList();
      }
    } catch (error) {
      console.error("Failed to delete entry:", error);
      reloadList();
    }
  };

  const retryHistoryEntry = async (id: number) => {
    const result = await commands.retryHistoryEntryTranscription(id);
    if (result.status !== "ok") {
      throw new Error(String(result.error));
    }
  };

  const openRecordingsFolder = async () => {
    try {
      const result = await commands.openRecordingsFolder();
      if (result.status !== "ok") {
        throw new Error(String(result.error));
      }
    } catch (error) {
      console.error("Failed to open recordings folder:", error);
    }
  };

  let content: React.ReactNode;

  if (state.loading) {
    content = (
      <div className="px-4 py-3 text-center text-text/60">
        {t("settings.history.loading")}
      </div>
    );
  } else if (state.error && !hasEntries) {
    content = (
      <div role="alert" className="px-4 py-3 text-center text-red-500">
        {t("settings.history.searchError")}
      </div>
    );
  } else if (!hasEntries) {
    content = (
      <div className="px-4 py-3 text-center text-text/60">
        {state.mode === "search"
          ? t("settings.history.noResults")
          : t("settings.history.empty")}
      </div>
    );
  } else {
    content = (
      <>
        {state.error && (
          <div
            role="alert"
            className="px-4 py-3 text-center text-red-500 border-b border-mid-gray/20"
          >
            {t("settings.history.searchError")}
          </div>
        )}
        <AudioPlayerGroup>
          <div className="divide-y divide-mid-gray/20">
            {state.entries.map((entry) => (
              <HistoryEntryComponent
                key={entry.id}
                entry={entry}
                onToggleSaved={() => toggleSaved(entry.id)}
                onCopyText={() => copyToClipboard(entry.transcription_text)}
                getAudioUrl={getAudioUrl}
                deleteAudio={deleteAudioEntry}
                retryTranscription={retryHistoryEntry}
              />
            ))}
          </div>
        </AudioPlayerGroup>
        {/* Sentinel for infinite scroll */}
        <div ref={sentinelRef} className="h-1" />
      </>
    );
  }

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <div className="space-y-2">
        <div className="px-4 flex items-center justify-between">
          <div>
            <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
              {t("settings.history.title")}
            </h2>
          </div>
          <OpenRecordingsButton
            onClick={openRecordingsFolder}
            label={t("settings.history.openFolder")}
          />
        </div>
        <div className="px-4">
          <Input
            type="search"
            value={searchInput}
            onChange={handleSearchChange}
            placeholder={t("settings.history.searchPlaceholder")}
            aria-label={t("settings.history.searchLabel")}
            className="w-full"
          />
        </div>
        <div className="bg-background border border-mid-gray/20 rounded-lg overflow-visible">
          {content}
        </div>
      </div>
    </div>
  );
};

interface HistoryEntryProps {
  entry: HistoryEntry;
  onToggleSaved: () => void;
  onCopyText: () => Promise<boolean>;
  getAudioUrl: (fileName: string) => Promise<string | null>;
  deleteAudio: (id: number) => Promise<void>;
  retryTranscription: (id: number) => Promise<void>;
}

const HistoryEntryComponent: React.FC<HistoryEntryProps> = ({
  entry,
  onToggleSaved,
  onCopyText,
  getAudioUrl,
  deleteAudio,
  retryTranscription,
}) => {
  const { t, i18n } = useTranslation();
  const [showCopied, setShowCopied] = useState(false);
  const [retrying, setRetrying] = useState(false);

  const hasTranscription = entry.transcription_text.trim().length > 0;

  const handleLoadAudio = useCallback(
    () => getAudioUrl(entry.file_name),
    [getAudioUrl, entry.file_name],
  );

  const handleCopyText = async () => {
    if (!hasTranscription) {
      return;
    }

    const copied = await onCopyText();
    if (!copied) {
      toast.error(t("settings.history.copyError"));
      return;
    }

    setShowCopied(true);
    setTimeout(() => setShowCopied(false), 2000);
  };

  const handleDeleteEntry = async () => {
    try {
      await deleteAudio(entry.id);
    } catch (error) {
      console.error("Failed to delete entry:", error);
      toast.error(t("settings.history.deleteError"));
    }
  };

  const handleRetranscribe = async () => {
    try {
      setRetrying(true);
      await retryTranscription(entry.id);
    } catch (error) {
      console.error("Failed to re-transcribe:", error);
      toast.error(t("settings.history.retranscribeError"));
    } finally {
      setRetrying(false);
    }
  };

  const formattedDate = formatDateTime(String(entry.timestamp), i18n.language);

  return (
    <div className="px-4 py-2 pb-5 flex flex-col gap-3">
      <div className="flex justify-between items-center">
        <p className="text-sm font-medium">{formattedDate}</p>
        <div className="flex items-center">
          <IconButton
            onClick={handleCopyText}
            disabled={!hasTranscription || retrying}
            title={t("settings.history.copyToClipboard")}
          >
            {showCopied ? (
              <Check width={16} height={16} />
            ) : (
              <Copy width={16} height={16} />
            )}
          </IconButton>
          <IconButton
            onClick={onToggleSaved}
            disabled={retrying}
            active={entry.saved}
            title={
              entry.saved
                ? t("settings.history.unsave")
                : t("settings.history.save")
            }
          >
            <Star
              width={16}
              height={16}
              fill={entry.saved ? "currentColor" : "none"}
            />
          </IconButton>
          <IconButton
            onClick={handleRetranscribe}
            disabled={retrying}
            title={t("settings.history.retranscribe")}
          >
            <RotateCcw
              width={16}
              height={16}
              style={
                retrying
                  ? { animation: "spin 1s linear infinite reverse" }
                  : undefined
              }
            />
          </IconButton>
          <IconButton
            onClick={handleDeleteEntry}
            disabled={retrying}
            title={t("settings.history.delete")}
          >
            <Trash2 width={16} height={16} />
          </IconButton>
        </div>
      </div>

      <p
        className={`italic text-sm pb-2 ${
          retrying
            ? ""
            : hasTranscription
              ? "text-text/90 select-text cursor-text whitespace-pre-wrap break-words"
              : "text-text/40"
        }`}
        style={
          retrying
            ? { animation: "transcribe-pulse 3s ease-in-out infinite" }
            : undefined
        }
      >
        {retrying && (
          <style>{`
            @keyframes transcribe-pulse {
              0%, 100% { color: color-mix(in srgb, var(--color-text) 40%, transparent); }
              50% { color: color-mix(in srgb, var(--color-text) 90%, transparent); }
            }
          `}</style>
        )}
        {retrying
          ? t("settings.history.transcribing")
          : hasTranscription
            ? entry.transcription_text
            : t("settings.history.transcriptionFailed")}
      </p>

      <AudioPlayer onLoadRequest={handleLoadAudio} className="w-full" />
    </div>
  );
};
