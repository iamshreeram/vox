// STUB (test-first scaffold): replaced by the real implementation.
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

export const initialState: UpdateState = {
  phase: "idle",
  info: null,
  error: null,
  manual: false,
};

export function reducer(_state: UpdateState, _action: Action): UpdateState {
  throw new Error("not implemented");
}

export function footerLabelKey(
  _state: UpdateState,
  _checksEnabled: boolean,
): string {
  throw new Error("not implemented");
}

export function footerAction(
  _state: UpdateState,
  _checksEnabled: boolean,
): FooterAction {
  throw new Error("not implemented");
}

export function canConfirmInstall(_info: SourceUpdateInfo | null): boolean {
  throw new Error("not implemented");
}

export function formatChangeList(_info: SourceUpdateInfo): {
  lines: string[];
  moreCount: number;
  countLabel: string;
} {
  throw new Error("not implemented");
}
