import { describe, expect, it, vi, beforeEach } from "vitest";

// T5 (U9): the settings load failure is real state. A failed load sets loadFailed; Retry re-runs
// the load, shows busy while it is in flight, clears loadFailed on success and keeps it on failure.

const mock = vi.hoisted(() => ({
  outcomes: [] as Array<"ok" | "fail">,
  calls: 0,
  gate: null as null | Promise<void>,
}));

vi.mock("@/bindings", () => ({
  commands: {
    getAppSettings: vi.fn(async () => {
      mock.calls += 1;
      if (mock.gate) await mock.gate;
      const outcome = mock.outcomes.shift() ?? "ok";
      if (outcome === "fail") {
        return { status: "error", error: "settings unavailable" };
      }
      return {
        status: "ok",
        data: { always_on_microphone: true, bindings: {} },
      };
    }),
    getDefaultSettings: vi.fn(async () => ({ status: "ok", data: {} })),
  },
}));

vi.mock("sonner", () => ({ toast: { error: vi.fn(), success: vi.fn() } }));

import { useSettingsStore } from "./settingsStore";

const state = () => useSettingsStore.getState();

describe("settings load failure and Retry (T5)", () => {
  beforeEach(() => {
    mock.outcomes = [];
    mock.calls = 0;
    mock.gate = null;
    useSettingsStore.setState({
      settings: null,
      isLoading: true,
      loadFailed: false,
      loadRetrying: false,
    });
  });

  it("a failed load sets loadFailed and ends loading", async () => {
    mock.outcomes = ["fail"];
    await state().refreshSettings();
    expect(state().loadFailed).toBe(true);
    expect(state().isLoading).toBe(false);
    expect(state().settings).toBeNull();
  });

  it("fail -> Retry -> success clears loadFailed and loads settings", async () => {
    mock.outcomes = ["fail", "ok"];
    await state().refreshSettings();
    await state().retryLoad();
    expect(state().loadFailed).toBe(false);
    expect(state().loadRetrying).toBe(false);
    expect(state().settings).not.toBeNull();
    expect(mock.calls).toBe(2);
  });

  it("fail -> Retry -> fail keeps loadFailed set", async () => {
    mock.outcomes = ["fail", "fail"];
    await state().refreshSettings();
    await state().retryLoad();
    expect(state().loadFailed).toBe(true);
    expect(state().loadRetrying).toBe(false);
    expect(mock.calls).toBe(2);
  });

  it("is busy while a Retry is in flight and ignores a second press", async () => {
    let release: () => void = () => undefined;
    mock.outcomes = ["fail"];
    await state().refreshSettings();
    mock.gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const first = state().retryLoad();
    expect(state().loadRetrying).toBe(true);
    const second = state().retryLoad();
    release();
    await Promise.all([first, second]);
    expect(state().loadRetrying).toBe(false);
    expect(mock.calls).toBe(2);
  });
});
