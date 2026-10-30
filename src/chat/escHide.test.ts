// Escape hides the conversation window (DESIGN D1b). Pure rules: escAction decides, the once-guard
// keeps one hide call in flight. Written first; workers must not weaken these.
import { describe, expect, it } from "vitest";
import { createOnceGuard, escAction, type EscEvent } from "./escHide";

const ESC: EscEvent = {
  key: "Escape",
  altKey: false,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
  isComposing: false,
  keyCode: 27,
};

const IDLE = { confirming: false, hiding: false };

describe("escAction", () => {
  it("plain Escape hides when nothing else needs it", () => {
    expect(escAction(IDLE, ESC)).toBe("hide");
  });

  it("Escape with the inline Clear confirmation open cancels the confirm only", () => {
    expect(escAction({ confirming: true, hiding: false }, ESC)).toBe(
      "cancelConfirm",
    );
  });

  it("Escape during an IME composition does nothing", () => {
    expect(escAction(IDLE, { ...ESC, isComposing: true })).toBe("ignore");
    expect(escAction(IDLE, { ...ESC, keyCode: 229 })).toBe("ignore");
    expect(
      escAction({ confirming: true, hiding: false }, { ...ESC, keyCode: 229 }),
    ).toBe("ignore");
  });

  it("Escape with Cmd, Ctrl, Alt or Shift does nothing", () => {
    for (const mod of ["altKey", "ctrlKey", "metaKey", "shiftKey"] as const) {
      expect(escAction(IDLE, { ...ESC, [mod]: true }), mod).toBe("ignore");
    }
  });

  it("other keys do nothing, even with the confirm open", () => {
    for (const key of ["Enter", "a", "Tab", "Esc ", "Space"]) {
      expect(escAction(IDLE, { ...ESC, key }), key).toBe("ignore");
      expect(
        escAction({ confirming: true, hiding: false }, { ...ESC, key }),
        key,
      ).toBe("ignore");
    }
  });

  it("Escape while a hide is already in flight is ignored (no second call)", () => {
    expect(escAction({ confirming: false, hiding: true }, ESC)).toBe("ignore");
  });
});

describe("createOnceGuard", () => {
  it("sends one call for repeated triggers while the first is in flight", async () => {
    let calls = 0;
    let release: () => void = () => undefined;
    const guard = createOnceGuard(
      () =>
        new Promise<void>((resolve) => {
          calls += 1;
          release = resolve;
        }),
    );
    expect(guard.trigger()).toBe(true);
    expect(guard.trigger()).toBe(false);
    expect(guard.trigger()).toBe(false);
    expect(guard.inFlight).toBe(true);
    expect(calls).toBe(1);
    release();
    await Promise.resolve();
    await Promise.resolve();
    expect(guard.inFlight).toBe(false);
    expect(guard.trigger()).toBe(true);
    expect(calls).toBe(2);
  });

  it("swallows a rejected call and re-arms without throwing", async () => {
    const guard = createOnceGuard(() => Promise.reject(new Error("nope")));
    expect(() => guard.trigger()).not.toThrow();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(guard.inFlight).toBe(false);
  });

  it("swallows a synchronous throw as well", async () => {
    const guard = createOnceGuard(() => {
      throw new Error("sync");
    });
    expect(() => guard.trigger()).not.toThrow();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(guard.inFlight).toBe(false);
  });
});
