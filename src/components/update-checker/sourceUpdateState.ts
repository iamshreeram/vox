import type { SourceUpdateInfo } from "../../bindings";

export type Phase =
  | "idle"
  | "checking"
  | "available"
  | "upToDate"
  | "untracked"
  | "starting"
  | "started"
  | "error";

export interface UpdateState {
  phase: Phase;
  info: SourceUpdateInfo | null;
  error: string | null;
  /** True while the check in flight was started by the user. */
  manual: boolean;
}

export type Action =
  | { type: "checkStarted"; manual: boolean }
  | { type: "checkSucceeded"; info: SourceUpdateInfo }
  | { type: "checkFailed"; message: string }
  | { type: "statusPushed"; info: SourceUpdateInfo | null }
  | { type: "hydrated"; info: SourceUpdateInfo | null }
  | { type: "installStarted" }
  | { type: "installLaunched" }
  | { type: "installFailed"; message: string }
  | { type: "checksDisabled" }
  | { type: "notificationExpired" };

export type FooterAction = "none" | "check" | "openDialog";

const MAX_CHANGE_LINES = 5;

export const initialState: UpdateState = {
  phase: "idle",
  info: null,
  error: null,
  manual: false,
};

/** Settled state for a status that was pushed or hydrated (not user-driven). */
function settledFromInfo(info: SourceUpdateInfo | null): UpdateState {
  if (info === null) {
    return { ...initialState };
  }
  return {
    phase: info.state === "available" ? "available" : "idle",
    info,
    error: null,
    manual: false,
  };
}

export function reducer(state: UpdateState, action: Action): UpdateState {
  switch (action.type) {
    case "checkStarted": {
      if (state.phase === "starting") return state;
      if (state.phase === "started" && !action.manual) return state;
      return {
        ...state,
        phase: "checking",
        error: null,
        manual: action.manual,
      };
    }

    case "checkSucceeded": {
      if (state.phase === "starting" || state.phase === "started") {
        return state;
      }
      const { info } = action;
      let phase: Phase;
      if (info.state === "available") {
        phase = "available";
      } else if (info.state === "up_to_date") {
        phase = state.manual ? "upToDate" : "idle";
      } else {
        phase = state.manual ? "untracked" : "idle";
      }
      return { phase, info, error: null, manual: false };
    }

    case "checkFailed": {
      if (state.phase === "starting" || state.phase === "started") {
        return state;
      }
      if (state.manual) {
        return {
          ...state,
          phase: "error",
          error: action.message,
          manual: false,
        };
      }
      return {
        ...state,
        phase: state.info?.state === "available" ? "available" : "idle",
        error: null,
        manual: false,
      };
    }

    case "statusPushed": {
      if (
        state.phase === "checking" ||
        state.phase === "starting" ||
        state.phase === "started"
      ) {
        return state;
      }
      return settledFromInfo(action.info);
    }

    case "hydrated": {
      if (state.phase !== "idle") return state;
      return settledFromInfo(action.info);
    }

    case "installStarted": {
      if (state.phase !== "available" || !canConfirmInstall(state.info)) {
        return state;
      }
      return { ...state, phase: "starting", error: null };
    }

    case "installLaunched": {
      if (state.phase !== "starting") return state;
      return { ...state, phase: "started" };
    }

    case "installFailed": {
      if (state.phase !== "starting") return state;
      return { ...state, phase: "error", error: action.message };
    }

    case "checksDisabled": {
      return { ...initialState };
    }

    case "notificationExpired": {
      if (state.phase !== "upToDate" && state.phase !== "untracked") {
        return state;
      }
      return { ...state, phase: "idle" };
    }
  }
}

export function footerLabelKey(
  state: UpdateState,
  checksEnabled: boolean,
): string {
  if (!checksEnabled) return "footer.updateCheckingDisabled";
  switch (state.phase) {
    case "idle":
      return "footer.checkForUpdates";
    case "checking":
      return "footer.checkingUpdates";
    case "available":
      return "footer.updateAvailableShort";
    case "upToDate":
      return "footer.upToDate";
    case "untracked":
      return "footer.sourceUpdate.untracked";
    case "starting":
      return "footer.sourceUpdate.starting";
    case "started":
      return "footer.sourceUpdate.started";
    case "error":
      return "footer.sourceUpdate.error";
  }
}

export function footerAction(
  state: UpdateState,
  checksEnabled: boolean,
): FooterAction {
  if (!checksEnabled) return "none";
  switch (state.phase) {
    case "idle":
    case "started":
    case "error":
    case "untracked":
      return "check";
    case "available":
      return "openDialog";
    case "checking":
    case "upToDate":
    case "starting":
      return "none";
  }
}

export function canConfirmInstall(info: SourceUpdateInfo | null): boolean {
  return (
    info !== null &&
    info.state === "available" &&
    info.can_install &&
    info.install_blocker === null
  );
}

export function formatChangeList(info: SourceUpdateInfo): {
  lines: string[];
  moreCount: number;
  countLabel: string;
} {
  const lines = info.changes
    .map((change) => change.trim())
    .filter((change) => change.length > 0)
    .slice(0, MAX_CHANGE_LINES);
  return {
    lines,
    moreCount: Math.max(0, info.behind_by - lines.length),
    countLabel: String(info.behind_by),
  };
}
