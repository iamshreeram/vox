// T3 (DESIGN Rev 4, B5): pure helpers for the orb's Hide badge and Esc dismissal. Node/bun only.

/** Whether a keydown dismisses the orb: Escape, but not an auto-repeat of it. */
export function isDismissKey(event: {
  key: string;
  repeat?: boolean;
}): boolean {
  return event.key === "Escape" && event.repeat !== true;
}

/**
 * Wraps `action` so that a call made while an earlier call is still pending is ignored. One press
 * (click, Enter or Esc) therefore issues one command even when two handlers see it. The guard
 * clears once the call settles, so a later press (after the orb is shown again) works.
 */
export function createSingleFlight(action: () => Promise<unknown>): () => void {
  let pending = false;
  return () => {
    if (pending) return;
    pending = true;
    action()
      .catch(() => undefined)
      .finally(() => {
        pending = false;
      });
  };
}
