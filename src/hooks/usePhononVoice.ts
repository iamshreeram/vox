import { useCallback, useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { commands, type NaturalVoiceStatus } from "@/bindings";
import { isBusyModelState } from "../components/settings/naturalVoiceState";
import type { EngineInstall } from "../components/settings/ttsEngineState";

const POLL_INTERVAL_MS = 1000;
const STATE_EVENT = "phonon-voice-state";
const ERROR_EVENT = "phonon-voice-error";

type CommandResult = { status: "ok" } | { status: "error"; error: string };

/** Error kinds of an install, as the engine picker shows them. */
export type InstallErrorKind = Extract<
  EngineInstall,
  { phase: "error" }
>["kind"];

export interface UsePhononVoice {
  status: NaturalVoiceStatus | null;
  /** Last failure text from the backend (UI-safe), or null. */
  error: string | null;
  install: () => Promise<void>;
  cancel: () => Promise<void>;
  remove: () => Promise<void>;
  importFolder: (path: string) => Promise<void>;
}

/**
 * Maps a UI-safe backend message to an install error kind. The backend texts come from
 * `error_message` in commands/natural_voice.rs and the Phonon import path.
 */
export function phononErrorKind(message: string): InstallErrorKind {
  const text = message.toLowerCase();
  if (text.includes("proxy")) return "proxy";
  if (text.includes("written to disk")) return "disk";
  if (
    text.includes("failed verification") ||
    text.includes("download the model again")
  ) {
    return "corrupt";
  }
  if (text.includes("not allowed")) return "blocked";
  return "generic";
}

/**
 * Install state of the Phonon model for the engine picker. A load failure on an installed
 * model is reported as a generic error so the card offers a reinstall.
 */
export function phononInstallState(
  status: NaturalVoiceStatus | null,
  error: string | null,
): EngineInstall {
  if (status === null) return { phase: "not_installed" };
  switch (status.model) {
    case "downloading":
      return {
        phase: "downloading",
        done: status.downloaded,
        total: status.total,
      };
    case "verifying":
    case "installing":
    case "deleting":
    case "loading":
      return { phase: "verifying" };
    case "ready":
      return status.health === "failed"
        ? { phase: "error", kind: "generic" }
        : { phase: "ready" };
    default:
      return error !== null
        ? { phase: "error", kind: phononErrorKind(error) }
        : { phase: "not_installed" };
  }
}

/**
 * Status, actions and the last error for the Phonon model. Refreshes on the Phonon events
 * and polls once a second while the model is busy. Mirrors useNaturalVoice.
 */
export function usePhononVoice(): UsePhononVoice {
  const [status, setStatus] = useState<NaturalVoiceStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const result = await commands.phononVoiceStatus();
      if (!mounted.current) return;
      if (result.status === "ok") {
        setStatus(result.data);
      } else {
        setError(result.error);
      }
    } catch (err) {
      console.warn("Reading Phonon voice status failed:", err);
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    void refresh();

    const unlisteners: Promise<UnlistenFn>[] = [
      listen(STATE_EVENT, () => {
        void refresh();
      }),
      listen<{ ok: boolean; message: string | null }>(ERROR_EVENT, (event) => {
        if (event.payload.message) {
          setError(event.payload.message);
        }
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
        console.warn("Phonon voice command failed:", err);
      }
      await refresh();
    },
    [refresh],
  );

  const install = useCallback(() => run(commands.phononVoiceInstall), [run]);
  const cancel = useCallback(() => run(commands.phononVoiceCancel), [run]);
  const remove = useCallback(() => run(commands.phononVoiceDelete), [run]);
  const importFolder = useCallback(
    (path: string) => run(() => commands.phononVoiceImportFolder(path)),
    [run],
  );

  return { status, error, install, cancel, remove, importFolder };
}
