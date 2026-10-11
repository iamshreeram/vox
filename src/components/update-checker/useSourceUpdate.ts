import { useCallback, useEffect, useReducer, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { commands, type SourceUpdateInfo } from "../../bindings";
import {
  canConfirmInstall,
  initialState,
  reducer,
  type UpdateState,
} from "./sourceUpdateState";

/** How long the transient "up to date" / "untracked" notices stay visible. */
const NOTICE_TIMEOUT_MS = 3000;

export interface UseSourceUpdateOptions {
  checksEnabled: boolean;
}

export interface UseSourceUpdateReturn {
  state: UpdateState;
  dialogOpen: boolean;
  /** Install blocker reported by fresh local git facts; null = installable. */
  blocker: string | null;
  /** True while the blocker is still being fetched for the open dialog. */
  blockerChecking: boolean;
  openDialog: () => void;
  closeDialog: () => void;
  runCheck: (manual: boolean) => Promise<void>;
  confirmInstall: () => Promise<void>;
}

export function useSourceUpdate({
  checksEnabled,
}: UseSourceUpdateOptions): UseSourceUpdateReturn {
  const { t } = useTranslation();
  const [state, dispatch] = useReducer(reducer, initialState);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [blocker, setBlocker] = useState<string | null>(null);
  const [blockerChecking, setBlockerChecking] = useState(false);

  const tRef = useRef(t);
  const mountedRef = useRef(false);
  const activeRef = useRef(false);
  // Bumped whenever the enabled session changes or the component unmounts,
  // so async results from an earlier session are dropped.
  const sessionRef = useRef(0);
  // Bumped on every blocker fetch so only the latest fetch can write state.
  const blockerRequestRef = useRef(0);

  useEffect(() => {
    tRef.current = t;
  }, [t]);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      sessionRef.current += 1;
    };
  }, []);

  const closeDialog = useCallback(() => {
    blockerRequestRef.current += 1;
    setDialogOpen(false);
    setBlockerChecking(false);
  }, []);

  const openDialog = useCallback(() => {
    const session = sessionRef.current;
    const request = (blockerRequestRef.current += 1);
    setDialogOpen(true);
    setBlocker(null);
    setBlockerChecking(true);

    const isCurrent = () =>
      mountedRef.current &&
      session === sessionRef.current &&
      request === blockerRequestRef.current;

    void (async () => {
      try {
        const value = await commands.getSourceUpdateInstallBlocker();
        if (!isCurrent()) return;
        setBlocker(value);
      } catch (error) {
        console.error("Failed to read source update blocker:", error);
        if (!isCurrent()) return;
        setBlocker(tRef.current("footer.sourceUpdate.dialog.blockerUnknown"));
      } finally {
        if (isCurrent()) setBlockerChecking(false);
      }
    })();
  }, []);

  const runCheck = useCallback(
    async (manual: boolean) => {
      if (!activeRef.current) return;
      const session = sessionRef.current;
      const isCurrent = () =>
        mountedRef.current && session === sessionRef.current;

      dispatch({ type: "checkStarted", manual });
      try {
        const res = await commands.checkSourceUpdate();
        if (!isCurrent()) return;
        if (res.status === "ok") {
          dispatch({ type: "checkSucceeded", info: res.data });
          if (manual && res.data.state === "available") {
            openDialog();
          }
        } else {
          dispatch({ type: "checkFailed", message: res.error });
        }
      } catch (error) {
        console.error("Failed to check for source updates:", error);
        if (isCurrent()) {
          dispatch({
            type: "checkFailed",
            message: tRef.current("footer.sourceUpdate.error"),
          });
        }
      }
    },
    [openDialog],
  );

  const confirmInstall = useCallback(async () => {
    if (state.phase !== "available" || blockerChecking) return;
    if (blocker !== null || !canConfirmInstall(state.info)) return;

    const session = sessionRef.current;
    const isCurrent = () =>
      mountedRef.current && session === sessionRef.current;

    dispatch({ type: "installStarted" });
    try {
      const res = await commands.startSourceUpdate();
      if (!isCurrent()) return;
      if (res.status === "ok") {
        dispatch({ type: "installLaunched" });
        closeDialog();
      } else {
        dispatch({ type: "installFailed", message: res.error });
      }
    } catch (error) {
      console.error("Failed to start source update:", error);
      if (isCurrent()) {
        dispatch({
          type: "installFailed",
          message: tRef.current("footer.sourceUpdate.error"),
        });
      }
    }
  }, [state.phase, state.info, blocker, blockerChecking, closeDialog]);

  // Transient notices fall back to idle after a short delay. Changing phase
  // (or unmounting) clears the pending timer.
  useEffect(() => {
    if (state.phase !== "upToDate" && state.phase !== "untracked") return;
    const timer = setTimeout(() => {
      if (mountedRef.current) dispatch({ type: "notificationExpired" });
    }, NOTICE_TIMEOUT_MS);
    return () => clearTimeout(timer);
  }, [state.phase]);

  useEffect(() => {
    activeRef.current = checksEnabled;

    if (!checksEnabled) {
      dispatch({ type: "checksDisabled" });
      closeDialog();
      return;
    }

    let disposed = false;
    const unlisteners: UnlistenFn[] = [];

    // Keep the unlisten handle even if cleanup already ran while listen()
    // was still pending, so it is released immediately.
    const keep = async (pending: Promise<UnlistenFn>) => {
      const unlisten = await pending;
      if (disposed) {
        unlisten();
      } else {
        unlisteners.push(unlisten);
      }
    };

    void (async () => {
      // 1. Subscribe first so no pushed status is missed.
      try {
        await keep(
          listen<SourceUpdateInfo | null>("source-update-status", (event) => {
            if (!disposed) {
              dispatch({ type: "statusPushed", info: event.payload });
            }
          }),
        );
        await keep(
          listen("check-for-updates", () => {
            if (!disposed) void runCheck(true);
          }),
        );
      } catch (error) {
        console.error("Failed to subscribe to update events:", error);
      }
      if (disposed) return;

      // 2. Hydrate from the backend's cached status.
      try {
        const cached = await commands.getCachedSourceUpdate();
        if (!disposed) dispatch({ type: "hydrated", info: cached });
      } catch (error) {
        console.error("Failed to load cached source update:", error);
      }
      if (disposed) return;

      // 3. One automatic check.
      void runCheck(false);
    })();

    return () => {
      disposed = true;
      activeRef.current = false;
      sessionRef.current += 1;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [checksEnabled, runCheck, closeDialog]);

  return {
    state,
    dialogOpen,
    blocker,
    blockerChecking,
    openDialog,
    closeDialog,
    runCheck,
    confirmInstall,
  };
}
