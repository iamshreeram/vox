/**
 * Pure state machine for the speaker enrollment wizard. No React, no Tauri.
 * Spec = enrollmentState.test.ts (the transition table is authoritative).
 *
 * Results from the backend carry the session id they belong to. Any result
 * whose id is not the live session is ignored, so a late answer can never move
 * a newer wizard.
 */

/** Three counted recordings, as in the backend (`steps`). */
export const ENROLL_STEPS = 3;
/** One recording window; matches the backend `window_ms` (6000). */
export const ENROLL_WINDOW_MS = 6000;

export type EnrollPhase =
  | "idle"
  | "starting"
  | "ready"
  | "stepping"
  | "finishing"
  | "done";

/** Message keys shown in the wizard. The UI maps each one to translated text. */
export type EnrollMessage =
  | "busy"
  | "too_little_speech"
  | "inconsistent"
  | "model_missing"
  | "session_ended"
  | "generic";

/** The backend error variants a wizard step or call can report. */
export type EnrollFailure =
  | "ModelMissing"
  | "Busy"
  | "StaleSession"
  | "NoActiveSession"
  | "BadStep"
  | "TooLittleSpeech"
  | "EmbedFailed"
  | "NotEnoughSteps"
  | "Inconsistent"
  | "UnsupportedPlatform"
  | "StoreFailed"
  | "Cancelled";

/** Structural copy of the backend `SpeakerCmdError` (kept local so this file stays pure). */
export type CmdErrorLike = "ModelMissing" | "StoreFailed" | { Enroll: string };

export interface EnrollState {
  phase: EnrollPhase;
  sessionId: number | null;
  /** Index of the step to record next (0..ENROLL_STEPS-1). */
  step: number;
  /** Number of counted (ok) steps so far (0..ENROLL_STEPS). */
  okSteps: number;
  message: EnrollMessage | null;
}

export type EnrollEvent =
  | { type: "BEGIN" }
  | { type: "BEGUN"; sessionId: number }
  | { type: "BEGIN_FAILED"; failure: EnrollFailure }
  | { type: "RECORD" }
  | { type: "STEP_OK"; sessionId: number }
  | { type: "STEP_FAILED"; sessionId: number; failure: EnrollFailure }
  | { type: "FINISH" }
  | { type: "FINISHED"; sessionId: number }
  | { type: "FINISH_FAILED"; sessionId: number; failure: EnrollFailure }
  | { type: "CANCEL" };

export function initialEnrollState(): EnrollState {
  return { phase: "idle", sessionId: null, step: 0, okSteps: 0, message: null };
}

const EMPTY_SESSION = {
  sessionId: null,
  step: 0,
  okSteps: 0,
  message: null,
} as const;

/** Reads the three shapes a command error can take. */
export function failureFromCmdError(error: CmdErrorLike): EnrollFailure {
  if (error === "ModelMissing") return "ModelMissing";
  if (error === "StoreFailed") return "StoreFailed";
  return error.Enroll as EnrollFailure;
}

function beginMessage(failure: EnrollFailure): EnrollMessage {
  if (failure === "Busy") return "busy";
  if (failure === "ModelMissing") return "model_missing";
  return "generic";
}

/** Session ended for good: the user cancelled, or the backend no longer knows the session. */
function endedMessage(failure: EnrollFailure): EnrollMessage | null {
  if (failure === "Cancelled") return null;
  if (failure === "StaleSession" || failure === "NoActiveSession") {
    return "session_ended";
  }
  return "generic";
}

export function enrollmentReducer(
  state: EnrollState,
  event: EnrollEvent,
): EnrollState {
  // Cancel is accepted from every state and always ends idle with no session.
  if (event.type === "CANCEL") {
    return { phase: "idle", ...EMPTY_SESSION };
  }

  const liveSession = (id: number): boolean =>
    state.sessionId !== null && state.sessionId === id;

  switch (state.phase) {
    case "idle":
    case "done":
      if (event.type === "BEGIN") {
        return { ...state, phase: "starting", ...EMPTY_SESSION };
      }
      return state;

    case "starting":
      if (event.type === "BEGUN") {
        return {
          phase: "ready",
          sessionId: event.sessionId,
          step: 0,
          okSteps: 0,
          message: null,
        };
      }
      if (event.type === "BEGIN_FAILED") {
        return {
          phase: "idle",
          ...EMPTY_SESSION,
          message: beginMessage(event.failure),
        };
      }
      return state;

    case "ready":
      if (event.type === "RECORD" && state.step < ENROLL_STEPS) {
        return { ...state, phase: "stepping" };
      }
      if (
        event.type === "FINISH" &&
        state.okSteps === ENROLL_STEPS &&
        state.sessionId !== null
      ) {
        return { ...state, phase: "finishing" };
      }
      return state;

    case "stepping":
      if (event.type === "STEP_OK" && liveSession(event.sessionId)) {
        const okSteps = Math.min(ENROLL_STEPS, state.okSteps + 1);
        return {
          ...state,
          phase: "ready",
          step: Math.min(ENROLL_STEPS - 1, state.step + 1),
          okSteps,
          message: null,
        };
      }
      if (event.type === "STEP_FAILED" && liveSession(event.sessionId)) {
        const ended =
          event.failure === "Cancelled" ||
          event.failure === "StaleSession" ||
          event.failure === "NoActiveSession";
        if (ended) {
          return {
            phase: "idle",
            ...EMPTY_SESSION,
            message: endedMessage(event.failure),
          };
        }
        // Not counted: the same step is retried. Step index and ok count stay.
        return {
          ...state,
          phase: "ready",
          message:
            event.failure === "TooLittleSpeech"
              ? "too_little_speech"
              : event.failure === "Busy"
                ? "busy"
                : "generic",
        };
      }
      return state;

    case "finishing":
      if (event.type === "FINISHED" && liveSession(event.sessionId)) {
        return { phase: "done", ...EMPTY_SESSION };
      }
      if (event.type === "FINISH_FAILED" && liveSession(event.sessionId)) {
        if (event.failure === "Inconsistent") {
          return {
            phase: "ready",
            sessionId: state.sessionId,
            step: 0,
            okSteps: 0,
            message: "inconsistent",
          };
        }
        if (
          event.failure === "StaleSession" ||
          event.failure === "NoActiveSession" ||
          event.failure === "Cancelled"
        ) {
          return {
            phase: "idle",
            ...EMPTY_SESSION,
            message: endedMessage(event.failure),
          };
        }
        // Busy, store and other failures keep the session so Finish can be retried.
        return {
          ...state,
          phase: "ready",
          message: event.failure === "Busy" ? "busy" : "generic",
        };
      }
      return state;
  }
}

/** The session to cancel on the backend, or null when there is none to cancel yet. */
export function cancelTargetOf(state: EnrollState): number | null {
  if (
    state.phase === "ready" ||
    state.phase === "stepping" ||
    state.phase === "finishing"
  ) {
    return state.sessionId;
  }
  return null;
}

/**
 * Whole seconds left in one recording window: 6 at the start, 0 at the end.
 * Non-finite input is treated as no time elapsed.
 */
export function countdownSeconds(elapsedMs: number): number {
  const elapsed = Number.isFinite(elapsedMs) ? Math.max(0, elapsedMs) : 0;
  const left = Math.max(0, ENROLL_WINDOW_MS - elapsed);
  return Math.min(ENROLL_WINDOW_MS / 1000, Math.ceil(left / 1000));
}
