// Inline two-step confirmation for Clear (UI enterprise S4b, U15a). Pure: ChatWindow owns the timer
// and the translated status text; this module only decides the next step.

/** How long the Confirm/Cancel pair stays up before it reverts to the Clear button. */
export const CLEAR_CONFIRM_MS = 4000;

export type ClearStep = "idle" | "confirming";
export type ClearEvent = "request" | "cancel" | "timeout" | "confirm";

export function nextClearStep(step: ClearStep, event: ClearEvent): ClearStep {
  if (step === "idle") {
    return event === "request" ? "confirming" : "idle";
  }
  // Confirming: a repeated request keeps the pair up; every other event closes it.
  return event === "request" ? "confirming" : "idle";
}
