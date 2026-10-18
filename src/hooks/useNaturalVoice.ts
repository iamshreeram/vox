import { useCallback, useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  commands,
  type NaturalVoiceDownloadFinished,
  type NaturalVoiceOption,
  type NaturalVoiceStatus,
} from "@/bindings";
import { isBusyModelState } from "../components/settings/naturalVoiceState";

const POLL_INTERVAL_MS = 1000;

type CommandResult = { status: "ok" } | { status: "error"; error: string };

export interface UseNaturalVoice {
  status: NaturalVoiceStatus | null;
  voices: NaturalVoiceOption[];
  error: string | null;
  download: () => Promise<void>;
  cancel: () => Promise<void>;
  remove: () => Promise<void>;
  retry: () => Promise<void>;
}

/**
 * Status, voice list and actions for the natural voice model. Refreshes on the
 * natural voice events and polls once a second while the model is busy.
 */
export function useNaturalVoice(): UseNaturalVoice {
  const [status, setStatus] = useState<NaturalVoiceStatus | null>(null);
  const [voices, setVoices] = useState<NaturalVoiceOption[]>([]);
  const [error, setError] = useState<string | null>(null);
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const result = await commands.naturalVoiceStatus();
      if (!mounted.current) return;
      if (result.status === "ok") {
        setStatus(result.data);
      } else {
        setError(result.error);
      }
    } catch (err) {
      console.warn("Reading natural voice status failed:", err);
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    void refresh();
    commands
      .listNaturalVoices()
      .then((list) => {
        if (mounted.current) setVoices(list);
      })
      .catch((err: unknown) => {
        console.warn("Listing natural voices failed:", err);
      });

    const unlisteners: Promise<UnlistenFn>[] = [
      listen("natural-voice-download-progress", () => {
        void refresh();
      }),
      listen<NaturalVoiceDownloadFinished>(
        "natural-voice-download-finished",
        (event) => {
          if (!event.payload.ok && event.payload.message) {
            setError(event.payload.message);
          }
          void refresh();
        },
      ),
      listen<string>("natural-voice-error", () => {
        void refresh();
      }),
    ];

    return () => {
      mounted.current = false;
      for (const unlisten of unlisteners) {
        void unlisten.then((fn) => fn());
      }
    };
  }, [refresh]);

  const busy = status !== null && isBusyModelState(status.model);
  useEffect(() => {
    if (!busy) return;
    const timer = setInterval(() => {
      void refresh();
    }, POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [busy, refresh]);

  const run = useCallback(
    async (command: () => Promise<CommandResult>) => {
      setError(null);
      try {
        const result = await command();
        if (result.status === "error" && mounted.current) {
          setError(result.error);
        }
      } catch (err) {
        console.warn("Natural voice command failed:", err);
      }
      await refresh();
    },
    [refresh],
  );

  const download = useCallback(() => run(commands.naturalVoiceDownload), [run]);
  const cancel = useCallback(
    () => run(commands.naturalVoiceCancelDownload),
    [run],
  );
  const remove = useCallback(() => run(commands.naturalVoiceDelete), [run]);
  const retry = useCallback(() => run(commands.naturalVoiceRetry), [run]);

  return { status, voices, error, download, cancel, remove, retry };
}
