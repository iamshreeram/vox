import { useEffect, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { commands, type AutoUpdateStatus } from "../../bindings";

/** Event the backend emits with the current AutoUpdateStatus. */
const AUTO_UPDATE_EVENT = "auto-update-state-changed";

/**
 * Current automatic-update status: live pushes from the backend, plus one
 * fetch on mount. A pushed status is newer than any fetch still in flight.
 * Null until the first value arrives.
 */
export function useAutoUpdateStatus(): AutoUpdateStatus | null {
  const [status, setStatus] = useState<AutoUpdateStatus | null>(null);

  useEffect(() => {
    let disposed = false;
    let pushed = false;
    let unlisten: UnlistenFn | null = null;

    listen<AutoUpdateStatus>(AUTO_UPDATE_EVENT, (event) => {
      if (disposed) return;
      pushed = true;
      setStatus(event.payload);
    })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch((error) => {
        console.error("Failed to subscribe to auto-update events:", error);
      });

    commands
      .getAutoUpdateStatus()
      .then((value) => {
        if (!disposed && !pushed) setStatus(value);
      })
      .catch((error) => {
        console.error("Failed to read auto-update status:", error);
      });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  return status;
}
