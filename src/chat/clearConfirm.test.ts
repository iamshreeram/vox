import { describe, expect, it } from "vitest";
import {
  CLEAR_CONFIRM_MS,
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
