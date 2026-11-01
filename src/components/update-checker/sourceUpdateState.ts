import type {
  RunPhase,
  SourceUpdateInfo,
  UpdateProgress,
} from "../../bindings";

export type Phase =
  | "idle"
  | "checking"
  | "available"
  | "upToDate"
  | "untracked"
  | "starting"
  | "updating"
  | "updated"
  | "failed"
  | "error";

/** Backend phases that are still running (done and failed are terminal). */
export type ActiveRunPhase = Exclude<RunPhase, "done" | "failed">;

export interface RunState {
  runId: string;
  /** Never decreases for the same run. */
  percent: number;
  /** Null once the run has reached a terminal phase. */
  backendPhase: ActiveRunPhase | null;
  target: string;
  estimated: boolean;
  /** True after done or failed: later progress of this run is ignored. */
  terminal: boolean;
}

export interface UpdateState {
  phase: Phase;
  info: SourceUpdateInfo | null;
  /** Check/start error, or the sanitised message of a failed run. */
  error: string | null;
  /** True while the check in flight was started by the user. */
  manual: boolean;
  run: RunState | null;
  /** Recently seen run ids; a progress event for one of these (other than the current run) is stale. */
  seenRuns: string[];
}

export type Action =
  | { type: "checkStarted"; manual: boolean }
  | { type: "checkSucceeded"; info: SourceUpdateInfo }
  | { type: "checkFailed"; message: string }
  | { type: "statusPushed"; info: SourceUpdateInfo | null }
  | { type: "hydrated"; info: SourceUpdateInfo | null }
  | { type: "installStarted" }
  | { type: "installFailed"; message: string }
  | { type: "progress"; progress: UpdateProgress }
  | { type: "updatedExpired" }
  | { type: "checksDisabled" }
  | { type: "notificationExpired" };

export type FooterAction = "none" | "check" | "openDialog";

const MAX_CHANGE_LINES = 5;
const MAX_SEEN_RUNS = 8;
/** Percent buckets the footer announces to screen readers. */
const MILESTONES = [0, 25, 50, 75, 100] as const;

export const initialState: UpdateState = {
  phase: "idle",
  info: null,
  error: null,
  manual: false,
  run: null,
  seenRuns: [],
};

/**
 * True when the footer should offer the update. An automatic attempt that is
 * healthy for the same target (`auto_pending`) keeps the footer idle.
 */
function isFooterAvailable(info: SourceUpdateInfo | null): boolean {
  return info?.state === "available" && info.auto_pending !== true;
}

/** Settled state for a status that was pushed or hydrated (not user-driven). */
function settledFromInfo(
  seenRuns: string[],
  info: SourceUpdateInfo | null,
): UpdateState {
  if (info === null) {
    return { ...initialState, seenRuns };
  }
  return {
    phase: isFooterAvailable(info) ? "available" : "idle",
    info,
    error: null,
    manual: false,
    run: null,
    seenRuns,
  };
}

function clampPercent(value: number): number | null {
  if (!Number.isFinite(value)) return null;
  return Math.min(100, Math.max(0, Math.round(value)));
}

function activePhaseOf(phase: RunPhase): ActiveRunPhase | null {
  return phase === "done" || phase === "failed" ? null : phase;
}

function applyProgress(state: UpdateState, p: UpdateProgress): UpdateState {
  // Automatic runs are silent: no footer, progress or failure UI for them.
  if (p.silent === true) return state;
  const reported = clampPercent(p.percent);
  if (reported === null) return state;
  // Only done reaches 100; any other phase is held at 99.
  const percent = p.phase === "done" ? reported : Math.min(reported, 99);

  const current = state.run;
  const sameRun = current !== null && current.runId === p.run_id;
  if (current !== null && sameRun) {
    // Terminal for this run: duplicates and late events change nothing.
    if (current.terminal) return state;
    // Percent never goes backwards within a run.
    if (
      p.phase !== "done" &&
      p.phase !== "failed" &&
      percent < current.percent
    ) {
      return state;
    }
  } else if (state.seenRuns.includes(p.run_id)) {
    // An older run that has already been superseded.
    return state;
  }

  const terminal = p.phase === "done" || p.phase === "failed";
  let nextPercent: number;
  if (p.phase === "done") {
    nextPercent = 100;
  } else if (sameRun && current !== null) {
    nextPercent = Math.max(current.percent, percent);
  } else {
    nextPercent = percent;
  }

  const run: RunState = {
    runId: p.run_id,
    percent: nextPercent,
    backendPhase: activePhaseOf(p.phase),
    target: p.target,
    estimated: p.estimated,
    terminal,
  };

  let phase: Phase = "updating";
  if (p.phase === "done") phase = "updated";
  if (p.phase === "failed") phase = "failed";

  return {
    phase,
    info: state.info,
    error: p.phase === "failed" ? p.message : null,
    manual: false,
    run,
    seenRuns: sameRun
      ? state.seenRuns
      : [...state.seenRuns, p.run_id].slice(-MAX_SEEN_RUNS),
  };
}

export function reducer(state: UpdateState, action: Action): UpdateState {
  switch (action.type) {
    case "checkStarted": {
      if (
        state.phase === "starting" ||
        state.phase === "updating" ||
        state.phase === "updated"
      ) {
        return state;
      }
      return {
        ...state,
        phase: "checking",
        error: null,
        manual: action.manual,
        run: null,
      };
    }

    case "checkSucceeded": {
      if (
        state.phase === "starting" ||
        state.phase === "updating" ||
        state.phase === "updated"
      ) {
        return state;
      }
      const { info } = action;
      let phase: Phase;
      if (info.state === "available") {
        phase = isFooterAvailable(info) ? "available" : "idle";
      } else if (info.state === "up_to_date") {
        phase = state.manual ? "upToDate" : "idle";
      } else {
        phase = state.manual ? "untracked" : "idle";
      }
      return {
        phase,
        info,
        error: null,
        manual: false,
        run: null,
        seenRuns: state.seenRuns,
      };
    }

    case "checkFailed": {
      if (
        state.phase === "starting" ||
        state.phase === "updating" ||
        state.phase === "updated"
      ) {
        return state;
      }
      if (state.manual) {
        return {
          ...state,
          phase: "error",
          error: action.message,
          manual: false,
          run: null,
        };
      }
      return {
        ...state,
        phase: isFooterAvailable(state.info) ? "available" : "idle",
        error: null,
        manual: false,
        run: null,
      };
    }

    case "statusPushed": {
      if (
        state.phase === "checking" ||
        state.phase === "starting" ||
        state.phase === "updating" ||
        state.phase === "updated"
      ) {
        return state;
      }
      return settledFromInfo(state.seenRuns, action.info);
    }

    case "hydrated": {
      if (state.phase !== "idle") return state;
      return settledFromInfo(state.seenRuns, action.info);
    }

    case "installStarted": {
      if (state.phase !== "available" || !canConfirmInstall(state.info)) {
        return state;
      }
      return { ...state, phase: "starting", error: null, run: null };
    }

    case "installFailed": {
      if (state.phase !== "starting") return state;
      return {
        ...state,
        phase: "error",
        error: action.message,
        run: null,
      };
    }

    case "progress": {
      return applyProgress(state, action.progress);
    }

    case "updatedExpired": {
      if (state.phase !== "updated") return state;
      // The run stays attached (terminal) so the up-to-date label does not
      // time out on its own and late duplicates are still ignored.
      return { ...state, phase: "upToDate" };
    }

    case "checksDisabled": {
      return { ...initialState };
    }

    case "notificationExpired": {
      if (state.phase !== "upToDate" && state.phase !== "untracked") {
        return state;
      }
      // A notice that follows a finished run stays until the next check.
      if (state.run !== null) return state;
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
    case "updating":
      return "footer.sourceUpdate.updating";
    case "updated":
      return "footer.sourceUpdate.updated";
    case "failed":
      return "footer.sourceUpdate.failed";
    case "error":
      return "footer.sourceUpdate.error";
  }
}

/** The percentage text key; the estimated variant carries a leading tilde. */
export function progressTextKey(run: RunState | null): string {
  return run?.estimated
    ? "footer.sourceUpdate.updatingEstimated"
    : "footer.sourceUpdate.updating";
}

/** Localised stage word for a running backend phase. */
export function stageLabelKey(phase: ActiveRunPhase): string {
  return `footer.sourceUpdate.stage.${phase}`;
}

/** Milestones (0/25/50/75/100) passed between `lastAnnounced` (exclusive) and `percent` (inclusive). */
export function milestonesCrossed(
  lastAnnounced: number,
  percent: number,
): number[] {
  return MILESTONES.filter((m) => m > lastAnnounced && m <= percent);
}

export function footerAction(
  state: UpdateState,
  checksEnabled: boolean,
): FooterAction {
  if (!checksEnabled) return "none";
  switch (state.phase) {
    case "idle":
    case "error":
    case "untracked":
      return "check";
    case "available":
      return "openDialog";
    case "checking":
    case "upToDate":
    case "starting":
    case "updating":
    case "updated":
    case "failed":
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
