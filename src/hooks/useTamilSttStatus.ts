import { useCallback, useEffect, useState } from "react";
import { commands, type TamilSttStatus } from "@/bindings";
import { useSettings } from "./useSettings";

export interface TamilSttState {
  /** The last status read from the backend, or null before the first read or after a failed read. */
  status: TamilSttStatus | null;
  /**
   * Tamil speech recognition can be chosen: the pack files verify (`installed`) and the
   * `stt_tamil_enabled` setting is on. A transient engine error (`engine_error`) is shown by
   * the row and does not change this.
   */
  available: boolean;
  /** Reads the status again. */
  refresh: () => void;
}

/**
 * Reads the Tamil speech recognition status (`tamil_stt_status`, stat-only on the backend).
 * It is refetched on mount, whenever the `stt_tamil_enabled` setting changes and when the
 * window regains focus, so a pack installed in a terminal is picked up without a restart.
 */
export const useTamilSttStatus = (): TamilSttState => {
  const { getSetting } = useSettings();
  const enabled = getSetting("stt_tamil_enabled") ?? true;
  const [status, setStatus] = useState<TamilSttStatus | null>(null);
  const [revision, setRevision] = useState(0);

  const refresh = useCallback(() => {
    setRevision((value) => value + 1);
  }, []);

  useEffect(() => {
    let active = true;
    const read = () => {
      commands
        .tamilSttStatus()
        .then((next) => {
          if (active) setStatus(next);
        })
        .catch((error: unknown) => {
          console.warn("Reading the Tamil speech status failed:", error);
          if (active) setStatus(null);
        });
    };
    read();
    const onFocus = () => {
      read();
    };
    window.addEventListener("focus", onFocus);
    return () => {
      active = false;
      window.removeEventListener("focus", onFocus);
    };
  }, [enabled, revision]);

  return {
    status,
    available: status !== null && status.installed && enabled,
    refresh,
  };
};
