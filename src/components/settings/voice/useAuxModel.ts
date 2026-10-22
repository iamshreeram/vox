import { useCallback, useEffect, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  commands,
  type AuxFinishedEvent,
  type AuxKind,
  type AuxProgressEvent,
  type AuxStatus,
} from "@/bindings";

export interface AuxModelState {
  /** Null until the first status read. */
  status: AuxStatus | null;
  downloaded: number;
  total: number;
  /** Backend AuxError code of the last failed attempt, or null. */
  error: string | null;
}

export interface UseAuxModel extends AuxModelState {
  /** Only the Download button may call this. */
  download: () => Promise<void>;
  cancel: () => Promise<void>;
  remove: () => Promise<void>;
}

const EMPTY: AuxModelState = {
  status: null,
  downloaded: 0,
  total: 0,
  error: null,
};

/**
 * Status, progress and actions for one optional model. Progress and the
 * result arrive as `aux-model-progress` and `aux-model-finished`, filtered
 * to this kind. Payloads carry numbers and enums only.
 */
export function useAuxModel(kind: AuxKind): UseAuxModel {
  const [model, setModel] = useState<AuxModelState>(EMPTY);

  const refresh = useCallback(async () => {
    try {
      const status = await commands.auxModelStatus(kind);
      setModel((prev) => ({ ...prev, status }));
    } catch (err) {
      console.warn("Reading model status failed:", err);
    }
  }, [kind]);

  useEffect(() => {
    let active = true;
    const unlisteners: Promise<UnlistenFn>[] = [
      listen<AuxProgressEvent>("aux-model-progress", (event) => {
        if (!active || event.payload.kind !== kind) return;
        setModel((prev) => ({
          ...prev,
          downloaded: event.payload.downloaded,
          total: event.payload.total,
        }));
      }),
      listen<AuxFinishedEvent>("aux-model-finished", (event) => {
        if (!active || event.payload.kind !== kind) return;
        setModel((prev) => ({
          ...prev,
          downloaded: 0,
          total: 0,
          error: event.payload.error,
        }));
        void refresh();
      }),
    ];
    void refresh();
    return () => {
      active = false;
      for (const unlisten of unlisteners) {
        void unlisten.then((fn) => fn());
      }
    };
  }, [kind, refresh]);

  const run = useCallback(
    async (action: () => Promise<void>) => {
      setModel((prev) => ({ ...prev, error: null }));
      try {
        await action();
      } catch (err) {
        console.warn("Model command failed:", err);
      }
      await refresh();
    },
    [refresh],
  );

  const download = useCallback(
    () =>
      run(async () => {
        const result = await commands.auxModelDownload(kind);
        if (result.status === "error") {
          setModel((prev) => ({ ...prev, error: result.error }));
        }
      }),
    [kind, run],
  );

  const cancel = useCallback(
    () =>
      run(async () => {
        const result = await commands.auxModelCancel(kind);
        if (result.status === "error") {
          setModel((prev) => ({ ...prev, error: result.error }));
        }
      }),
    [kind, run],
  );

  const remove = useCallback(
    () =>
      run(async () => {
        const result = await commands.auxModelDelete(kind);
        if (result.status === "error") {
          setModel((prev) => ({ ...prev, error: result.error }));
        }
      }),
    [kind, run],
  );

  return { ...model, download, cancel, remove };
}
