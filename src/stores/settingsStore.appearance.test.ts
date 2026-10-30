import { describe, expect, it, vi, beforeEach } from "vitest";

// The three appearance fields follow the same store pattern as theme (UI enterprise S3):
// an optimistic write, the generated command, and a rollback to the previous value when the
// command fails. The bindings are replaced so no Tauri runtime is needed.
const calls = vi.hoisted(() => ({
  accent: [] as unknown[],
  chatTheme: [] as unknown[],
  visualStyle: [] as unknown[],
  theme: [] as unknown[],
  orbAccent: [] as unknown[],
}));
const failing = vi.hoisted(() => ({
  accent: false,
  chatTheme: false,
  visualStyle: false,
  orbAccent: false,
}));
// When set, the next orb write waits on this gate before it resolves (stale-response test).
const gate = vi.hoisted(() => ({ pending: null as null | Promise<void> }));

vi.mock("@/bindings", () => ({
  commands: {
    changeAccentSetting: vi.fn(async (accent: unknown) => {
      calls.accent.push(accent);
      if (failing.accent) throw new Error("accent rejected");
    }),
    changeChatThemeSetting: vi.fn(async (chatTheme: unknown) => {
      calls.chatTheme.push(chatTheme);
      if (failing.chatTheme) throw new Error("chat theme rejected");
    }),
    changeVisualStyleSetting: vi.fn(async (visualStyle: unknown) => {
      calls.visualStyle.push(visualStyle);
      if (failing.visualStyle) throw new Error("style rejected");
    }),
    changeThemeSetting: vi.fn(async (theme: unknown) => {
      calls.theme.push(theme);
      return { status: "ok", data: null };
    }),
    changeOrbAccentSetting: vi.fn(async (accent: unknown) => {
      calls.orbAccent.push(accent);
      // The outcome is decided when the call is made, so a later change of `failing` does not
      // rewrite an earlier request.
      const shouldFail = failing.orbAccent;
      if (gate.pending) await gate.pending;
      if (shouldFail) throw new Error("orb accent rejected");
    }),
  },
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));
vi.mock("sonner", () => ({ toast: { error: vi.fn(), success: vi.fn() } }));

import { useSettingsStore } from "./settingsStore";

const BASE = {
  accent: "amber",
  chat_theme: "dark",
  visual_style: "vibrant",
  theme: "system",
  orb_accent: null,
} as const;

beforeEach(() => {
  calls.accent.length = 0;
  calls.chatTheme.length = 0;
  calls.visualStyle.length = 0;
  calls.theme.length = 0;
  calls.orbAccent.length = 0;
  failing.accent = false;
  failing.chatTheme = false;
  failing.visualStyle = false;
  failing.orbAccent = false;
  gate.pending = null;
  useSettingsStore.setState({
    settings: { ...BASE } as never,
    isUpdating: {},
  });
});

describe("appearance fields follow the theme pattern", () => {
  it("accent: writes optimistically, calls changeAccentSetting once with the value", async () => {
    await useSettingsStore.getState().updateSetting("accent", "blue");
    expect(calls.accent).toEqual(["blue"]);
    expect(useSettingsStore.getState().settings?.accent).toBe("blue");
  });

  it("accent: a rejected command rolls the store back to the previous value", async () => {
    failing.accent = true;
    await useSettingsStore.getState().updateSetting("accent", "rose");
    expect(calls.accent).toEqual(["rose"]);
    expect(useSettingsStore.getState().settings?.accent).toBe("amber");
  });

  it("chat theme: calls changeChatThemeSetting and rolls back on failure", async () => {
    await useSettingsStore.getState().updateSetting("chat_theme", "light");
    expect(calls.chatTheme).toEqual(["light"]);
    expect(useSettingsStore.getState().settings?.chat_theme).toBe("light");

    failing.chatTheme = true;
    await useSettingsStore.getState().updateSetting("chat_theme", "follow_app");
    expect(useSettingsStore.getState().settings?.chat_theme).toBe("light");
  });

  it("visual style: calls changeVisualStyleSetting and rolls back on failure", async () => {
    await useSettingsStore
      .getState()
      .updateSetting("visual_style", "restrained");
    expect(calls.visualStyle).toEqual(["restrained"]);
    expect(useSettingsStore.getState().settings?.visual_style).toBe(
      "restrained",
    );

    failing.visualStyle = true;
    await useSettingsStore.getState().updateSetting("visual_style", "vibrant");
    expect(useSettingsStore.getState().settings?.visual_style).toBe(
      "restrained",
    );
  });

  it("theme keeps its own command", async () => {
    await useSettingsStore.getState().updateSetting("theme", "dark");
    expect(calls.theme).toEqual(["dark"]);
  });
});

describe("orb colour (D3)", () => {
  it("changeOrbAccent writes optimistically and calls changeOrbAccentSetting with the value", async () => {
    const ok = await useSettingsStore.getState().changeOrbAccent("violet");
    expect(ok).toBe(true);
    expect(calls.orbAccent).toEqual(["violet"]);
    expect(useSettingsStore.getState().settings?.orb_accent).toBe("violet");
  });

  it("changeOrbAccent(null) writes follow-the-app and never sends an accent", async () => {
    useSettingsStore.setState({
      settings: { ...BASE, orb_accent: "rose" } as never,
    });
    const ok = await useSettingsStore.getState().changeOrbAccent(null);
    expect(ok).toBe(true);
    expect(calls.orbAccent).toEqual([null]);
    expect(useSettingsStore.getState().settings?.orb_accent).toBeNull();
  });

  it("a rejected write rolls back to the previous value and reports failure", async () => {
    useSettingsStore.setState({
      settings: { ...BASE, orb_accent: "blue" } as never,
    });
    failing.orbAccent = true;
    const ok = await useSettingsStore.getState().changeOrbAccent("emerald");
    expect(ok).toBe(false);
    expect(calls.orbAccent).toEqual(["emerald"]);
    expect(useSettingsStore.getState().settings?.orb_accent).toBe("blue");
  });

  it("a stale failure does not undo a newer choice", async () => {
    let release: () => void = () => undefined;
    gate.pending = new Promise<void>((resolve) => {
      release = resolve;
    });
    failing.orbAccent = true;
    const first = useSettingsStore.getState().changeOrbAccent("rose");
    // The second write starts before the first one settles and succeeds.
    failing.orbAccent = false;
    gate.pending = null;
    const second = useSettingsStore.getState().changeOrbAccent("slate");
    release();
    expect(await second).toBe(true);
    expect(await first).toBe(false);
    expect(useSettingsStore.getState().settings?.orb_accent).toBe("slate");
  });

  it("updateSetting('orb_accent') uses the same command (coverage and reset path)", async () => {
    await useSettingsStore.getState().updateSetting("orb_accent", "blue");
    expect(calls.orbAccent).toEqual(["blue"]);
  });
});
