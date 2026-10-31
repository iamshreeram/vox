import { describe, expect, it } from "vitest";
import {
  CLEAR_CONFIRM_MS,
  clearSecondsLeft,
  nextClearStep,
  type ClearStep,
} from "./clearConfirm";

// UI enterprise S4b (U15a): Clear asks for an inline two-step confirmation. The state machine is
// pure so the 4-second window and the cancel paths are pinned without a DOM.
describe("inline Clear confirmation", () => {
  it("the confirmation window is 4 seconds", () => {
    expect(CLEAR_CONFIRM_MS).toBe(4000);
  });

  it("a first Clear request only asks for confirmation", () => {
    expect(nextClearStep("idle", "request")).toBe("confirming");
  });

  it("a second request while confirming does not clear again by itself", () => {
    expect(nextClearStep("confirming", "request")).toBe("confirming");
  });

  it("Cancel, the 4-second timeout and a confirmed clear all return to idle", () => {
    for (const event of ["cancel", "timeout", "confirm"] as const) {
      expect(nextClearStep("confirming", event)).toBe("idle");
    }
  });

  it("stray events in idle keep the idle step", () => {
    for (const event of ["cancel", "timeout", "confirm"] as const) {
      expect(nextClearStep("idle", event)).toBe("idle");
    }
  });

  it("every step is one of the two named steps", () => {
    const steps: ClearStep[] = ["idle", "confirming"];
    expect(steps).toHaveLength(2);
  });
});

// T1b: the confirm bar shows the seconds left (text countdown; reduced motion keeps only the text).
describe("clear confirmation countdown", () => {
  it("rounds the time left up to whole seconds", () => {
    expect(clearSecondsLeft(4000)).toBe(4);
    expect(clearSecondsLeft(3001)).toBe(4);
    expect(clearSecondsLeft(3000)).toBe(3);
    expect(clearSecondsLeft(1)).toBe(1);
  });

  it("never goes below zero", () => {
    expect(clearSecondsLeft(0)).toBe(0);
    expect(clearSecondsLeft(-250)).toBe(0);
  });
});
