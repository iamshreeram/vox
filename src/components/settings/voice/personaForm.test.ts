import { describe, expect, it } from "vitest";
import {
  PERSONA_LIMITS,
  isBuiltinId,
  personaErrorIssue,
  validatePersonaDraft,
  type PersonaDraft,
  type PersonaFormContext,
} from "./personaForm";

const draft = (over: Partial<PersonaDraft> = {}): PersonaDraft => ({
  name: "Mentor",
  style: "Be calm and brief.",
  voice: null,
  ...over,
});

const ctx = (over: Partial<PersonaFormContext> = {}): PersonaFormContext => ({
  customCount: 0,
  editingId: null,
  isBuiltin: false,
  knownVoices: ["af_sky", "am_adam"],
  ...over,
});

describe("persona limits match the backend", () => {
  it("is 12 custom personas, 40 name characters, 300 style characters", () => {
    expect(PERSONA_LIMITS).toEqual({
      maxCustom: 12,
      nameMaxChars: 40,
      styleMaxChars: 300,
    });
  });
});

describe("validatePersonaDraft", () => {
  it("accepts a valid draft and returns it trimmed", () => {
    const result = validatePersonaDraft(
      draft({ name: "  Mentor  ", style: " Be brief. ", voice: "af_sky" }),
      ctx(),
    );
    expect(result).toEqual({
      ok: true,
      persona: { name: "Mentor", style: "Be brief.", voice: "af_sky" },
    });
  });

  it("an empty voice becomes no voice", () => {
    const result = validatePersonaDraft(draft({ voice: "" }), ctx());
    expect(result.ok && result.persona.voice).toBeNull();
  });

  it("empty name and empty style are both reported", () => {
    const result = validatePersonaDraft(
      draft({ name: "   ", style: "" }),
      ctx(),
    );
    expect(result).toEqual({
      ok: false,
      issues: ["name_empty", "style_empty"],
    });
  });

  it("the name limit is inclusive at 40 and rejected at 41", () => {
    const atLimit = validatePersonaDraft(
      draft({ name: "n".repeat(40) }),
      ctx(),
    );
    expect(atLimit.ok).toBe(true);
    const over = validatePersonaDraft(draft({ name: "n".repeat(41) }), ctx());
    expect(over).toEqual({ ok: false, issues: ["name_too_long"] });
  });

  it("the name length counts characters, not UTF-16 units", () => {
    const emoji = "\u{1F600}".repeat(40);
    expect(validatePersonaDraft(draft({ name: emoji }), ctx()).ok).toBe(true);
    expect(validatePersonaDraft(draft({ name: `${emoji}x` }), ctx())).toEqual({
      ok: false,
      issues: ["name_too_long"],
    });
  });

  it("the style limit is inclusive at 300 and rejected at 301", () => {
    expect(
      validatePersonaDraft(draft({ style: "s".repeat(300) }), ctx()).ok,
    ).toBe(true);
    expect(
      validatePersonaDraft(draft({ style: "s".repeat(301) }), ctx()),
    ).toEqual({ ok: false, issues: ["style_too_long"] });
  });

  it("control characters are rejected in name and style", () => {
    expect(
      validatePersonaDraft(draft({ name: "Men\u0007tor" }), ctx()),
    ).toEqual({ ok: false, issues: ["control_chars"] });
    expect(
      validatePersonaDraft(draft({ style: "Be\u0000 brief" }), ctx()),
    ).toEqual({ ok: false, issues: ["control_chars"] });
  });

  it("an unknown voice is rejected", () => {
    expect(validatePersonaDraft(draft({ voice: "zz_unknown" }), ctx())).toEqual(
      { ok: false, issues: ["unknown_voice"] },
    );
  });

  it("a new persona is refused once 12 custom personas exist", () => {
    expect(validatePersonaDraft(draft(), ctx({ customCount: 11 })).ok).toBe(
      true,
    );
    expect(validatePersonaDraft(draft(), ctx({ customCount: 12 }))).toEqual({
      ok: false,
      issues: ["limit_reached"],
    });
  });

  it("editing an existing custom persona is not blocked by the limit", () => {
    expect(
      validatePersonaDraft(
        draft(),
        ctx({ customCount: 12, editingId: "custom-3" }),
      ).ok,
    ).toBe(true);
  });

  it("a built-in persona cannot be changed", () => {
    expect(
      validatePersonaDraft(
        draft(),
        ctx({ editingId: "builtin-coach", isBuiltin: true }),
      ),
    ).toEqual({ ok: false, issues: ["builtin_immutable"] });
  });

  it("with no known voices list, any chosen voice is refused, not guessed", () => {
    expect(
      validatePersonaDraft(
        draft({ voice: "af_sky" }),
        ctx({ knownVoices: [] }),
      ),
    ).toEqual({ ok: false, issues: ["unknown_voice"] });
  });
});

describe("isBuiltinId", () => {
  it("recognises the reserved built-in prefix only", () => {
    expect(isBuiltinId("builtin-dry")).toBe(true);
    expect(isBuiltinId("custom-1")).toBe(false);
    expect(isBuiltinId("")).toBe(false);
  });
});

describe("personaErrorIssue maps every backend PersonaError", () => {
  const backend: [string, string][] = [
    ["NameEmpty", "name_empty"],
    ["NameTooLong", "name_too_long"],
    ["StyleEmpty", "style_empty"],
    ["StyleTooLong", "style_too_long"],
    ["ControlChars", "control_chars"],
    ["UnknownVoice", "unknown_voice"],
    ["BuiltinImmutable", "builtin_immutable"],
    ["LimitReached", "limit_reached"],
  ];
  for (const [error, issue] of backend) {
    it(`${error} -> ${issue}`, () => {
      expect(personaErrorIssue(error)).toBe(issue);
    });
  }

  it("ReservedId and NotFound and unknown values are generic", () => {
    expect(personaErrorIssue("ReservedId")).toBe("generic");
    expect(personaErrorIssue("NotFound")).toBe("generic");
    expect(personaErrorIssue("Whatever")).toBe("generic");
  });
});
