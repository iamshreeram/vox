// TYPING SPEC (DESIGN-TYPE Rev 1, written by ram-ai; workers must not edit this file).
// Contract for src/chat/composer.ts (pure helpers, no React, no imports):
//   export const MAX_CHARS = 2000; export const COUNTER_FROM = 1800;
//   prepareSend(raw): { ok: true; text: string } | { ok: false; reason: "empty" | "too_long" }
//   errorKey(code: string): string
//   shouldSubmitOnKey(e: { key: string; shiftKey: boolean; altKey: boolean; isComposing: boolean; keyCode?: number }): boolean
//   counterVisible(length: number): boolean
import { describe, expect, it } from "vitest";
import {
  COUNTER_FROM,
  MAX_CHARS,
  counterVisible,
  errorKey,
  prepareSend,
  shouldSubmitOnKey,
} from "./composer";

describe("composer constants", () => {
  it("match the backend limit and the counter threshold", () => {
    expect(MAX_CHARS).toBe(2000);
    expect(COUNTER_FROM).toBe(1800);
  });
});

describe("prepareSend", () => {
  it("trims the ends and keeps inner newlines", () => {
    expect(prepareSend("  hi there \n")).toEqual({
      ok: true,
      text: "hi there",
    });
    expect(prepareSend("a\nb")).toEqual({ ok: true, text: "a\nb" });
  });
  it("rejects empty and whitespace-only text", () => {
    expect(prepareSend("")).toEqual({ ok: false, reason: "empty" });
    expect(prepareSend(" \n\t ")).toEqual({ ok: false, reason: "empty" });
  });
  it("rejects text over the limit (after trimming)", () => {
    expect(prepareSend("x".repeat(MAX_CHARS))).toEqual({
      ok: true,
      text: "x".repeat(MAX_CHARS),
    });
    expect(prepareSend("x".repeat(MAX_CHARS + 1))).toEqual({
      ok: false,
      reason: "too_long",
    });
  });
  it("keeps markup as plain text", () => {
    expect(prepareSend("<b>x</b>")).toEqual({ ok: true, text: "<b>x</b>" });
  });
});

describe("errorKey", () => {
  it("maps the backend codes to translation keys and everything else to failed", () => {
    expect(errorKey("not_configured")).toBe("chat.sendError.not_configured");
    expect(errorKey("busy")).toBe("chat.sendError.busy");
    expect(errorKey("too_long")).toBe("chat.sendError.too_long");
    expect(errorKey("failed")).toBe("chat.sendError.failed");
    expect(errorKey("empty")).toBe("chat.sendError.failed");
    expect(errorKey("")).toBe("chat.sendError.failed");
    expect(errorKey("<script>")).toBe("chat.sendError.failed");
  });
});

describe("shouldSubmitOnKey", () => {
  const base = {
    key: "Enter",
    shiftKey: false,
    altKey: false,
    isComposing: false,
  };
  it("Enter submits", () => {
    expect(shouldSubmitOnKey(base)).toBe(true);
  });
  it("Shift+Enter and Alt+Enter do not", () => {
    expect(shouldSubmitOnKey({ ...base, shiftKey: true })).toBe(false);
    expect(shouldSubmitOnKey({ ...base, altKey: true })).toBe(false);
  });
  it("Enter during IME composition does not (flag or keyCode 229)", () => {
    expect(shouldSubmitOnKey({ ...base, isComposing: true })).toBe(false);
    expect(shouldSubmitOnKey({ ...base, keyCode: 229 })).toBe(false);
  });
  it("other keys do not", () => {
    expect(shouldSubmitOnKey({ ...base, key: "a" })).toBe(false);
    expect(shouldSubmitOnKey({ ...base, key: "Tab" })).toBe(false);
  });
});

describe("counterVisible", () => {
  it("shows from 1800 characters", () => {
    expect(counterVisible(0)).toBe(false);
    expect(counterVisible(1799)).toBe(false);
    expect(counterVisible(1800)).toBe(true);
    expect(counterVisible(2000)).toBe(true);
  });
});
