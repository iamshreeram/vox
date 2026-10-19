import { describe, expect, it } from "vitest";
import {
  allowsDeleteVoiceprint,
  smartTurnFallbackNote,
  smartTurnToggleEnabled,
  speakerFilterInactiveNote,
  speakerFilterToggleEnabled,
  toggleDisabled,
} from "./voiceGates";

describe("smart turn gating", () => {
  it("the toggle needs a Ready model", () => {
    expect(smartTurnToggleEnabled(true)).toBe(true);
    expect(smartTurnToggleEnabled(false)).toBe(false);
  });

  it("the fallback note shows only when on and the model is missing", () => {
    expect(smartTurnFallbackNote(true, false)).toBe(true);
    expect(smartTurnFallbackNote(true, true)).toBe(false);
    expect(smartTurnFallbackNote(false, false)).toBe(false);
    expect(smartTurnFallbackNote(false, true)).toBe(false);
  });
});

describe("speaker filter gating", () => {
  it("the toggle needs a Ready model AND an enrolled voice", () => {
    expect(speakerFilterToggleEnabled(true, true)).toBe(true);
    expect(speakerFilterToggleEnabled(true, false)).toBe(false);
    expect(speakerFilterToggleEnabled(false, true)).toBe(false);
    expect(speakerFilterToggleEnabled(false, false)).toBe(false);
  });

  it("the inactive note shows when on but not both ready and enrolled", () => {
    expect(speakerFilterInactiveNote(true, true, true)).toBe(false);
    expect(speakerFilterInactiveNote(true, true, false)).toBe(true);
    expect(speakerFilterInactiveNote(true, false, true)).toBe(true);
    expect(speakerFilterInactiveNote(false, false, false)).toBe(false);
  });

  it("delete is offered only while a voiceprint exists and no enrollment runs", () => {
    expect(allowsDeleteVoiceprint(true, false)).toBe(true);
    expect(allowsDeleteVoiceprint(false, false)).toBe(false);
    expect(allowsDeleteVoiceprint(true, true)).toBe(false);
  });
});

describe("toggleDisabled", () => {
  it("a switch that is on can always be turned off", () => {
    expect(toggleDisabled(true, false)).toBe(false);
  });

  it("a switch that is off stays disabled until its gate opens", () => {
    expect(toggleDisabled(false, false)).toBe(true);
    expect(toggleDisabled(false, true)).toBe(false);
  });
});
