// Escape hides the conversation window (DESIGN D1b). Pure helpers, no React, no console.
// ChatWindow owns the listener and the Clear state; this module only decides and guards.

export interface EscState {
  /** The inline Clear confirmation is open (Esc cancels it first). */
  confirming: boolean;
  /** A chat_hide_window call is in flight (repeated Esc must not send a second one). */
  hiding: boolean;
}

export interface EscEvent {
  key: string;
  altKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  isComposing: boolean;
  keyCode?: number;
}

export type EscAction = "ignore" | "cancelConfirm" | "hide";

/**
 * What a keydown does in the conversation window. Only a bare Escape is handled: an IME
 * composition, any modifier, or a hide already in flight leaves the key alone.
 */
export function escAction(state: EscState, event: EscEvent): EscAction {
  if (event.key !== "Escape") return "ignore";
  if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) {
    return "ignore";
  }
  if (event.isComposing || event.keyCode === 229) return "ignore";
  if (state.hiding) return "ignore";
  return state.confirming ? "cancelConfirm" : "hide";
}

export interface OnceGuard {
  /** True while the call started by the last successful trigger has not settled. */
  readonly inFlight: boolean;
  /** Starts the call unless one is already in flight. Returns whether it started. */
  trigger(): boolean;
}

/**
 * Runs `run` at most once at a time. A rejection (or a synchronous throw) is swallowed: the
 * conversation page is listen-only and has no console output, so the fallback is silence and
 * re-arming the guard.
 */
export function createOnceGuard(run: () => Promise<unknown>): OnceGuard {
  let inFlight = false;
  const attempt = async (): Promise<void> => {
    try {
      await run();
    } catch {
      // Swallowed on purpose (see createOnceGuard).
    } finally {
      inFlight = false;
    }
  };
  return {
    get inFlight() {
      return inFlight;
    },
    trigger(): boolean {
      if (inFlight) return false;
      inFlight = true;
      void attempt();
      return true;
    },
  };
}
