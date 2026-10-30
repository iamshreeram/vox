import { describe, expect, it } from "vitest";
import { resolveAppearance } from "@/lib/appearance/palette";
import type { AppearanceSnapshot } from "@/bindings";
import {
  chatOrbLookFor,
  getChatOrbLook,
  publishChatOrbLook,
  subscribeChatOrbLook,
  type ChatOrbLook,
} from "./chatOrbLook";

// Keep the module-level store isolated: each test publishes from rev 0 upward in order.
const look = (over: Partial<ChatOrbLook>): ChatOrbLook => ({
  rev: 0,
  orbColor: "auto",
  appAccent: "amber",
  bucket: "dark",
  ...over,
});

describe("chatOrbLook store", () => {
  it("starts on auto, the default accent and the dark bucket", () => {
    expect(getChatOrbLook()).toEqual({
      rev: 0,
      orbColor: "auto",
      appAccent: "amber",
      bucket: "dark",
    });
  });

  it("ignores a stale revision and keeps the newest", () => {
    publishChatOrbLook(look({ rev: 5, orbColor: "blue" }));
    expect(publishChatOrbLook(look({ rev: 4, orbColor: "rose" }))).toBe(false);
    expect(getChatOrbLook().orbColor).toBe("blue");
    expect(publishChatOrbLook(look({ rev: 6, orbColor: "rose" }))).toBe(true);
    expect(getChatOrbLook()).toMatchObject({ rev: 6, orbColor: "rose" });
  });

  it("lets an equal revision change the bucket (OS scheme change)", () => {
    publishChatOrbLook(look({ rev: 9, bucket: "dark" }));
    expect(publishChatOrbLook(look({ rev: 9, bucket: "light" }))).toBe(true);
    expect(getChatOrbLook().bucket).toBe("light");
  });

  it("notifies subscribers on change and stops after unsubscribe", () => {
    let calls = 0;
    const unsubscribe = subscribeChatOrbLook(() => {
      calls += 1;
    });
    publishChatOrbLook(look({ rev: 20, orbColor: "emerald" }));
    publishChatOrbLook(look({ rev: 20, orbColor: "emerald" }));
    expect(calls).toBe(1);
    unsubscribe();
    publishChatOrbLook(look({ rev: 21, orbColor: "slate" }));
    expect(calls).toBe(1);
  });
});

describe("chatOrbLookFor (chat surface bucket)", () => {
  const snapshot = (over: Partial<AppearanceSnapshot>): AppearanceSnapshot => ({
    rev: 7,
    theme: "light",
    custom_colors: null,
    accent: "blue",
    orb_accent: "rose",
    chat_theme: "dark",
    visual_style: "vibrant",
    ...over,
  });

  it("takes the chat palette, not the app bucket", () => {
    const snap = snapshot({});
    const resolved = resolveAppearance(snap, false);
    expect(resolved.bucket).toBe("light");
    expect(resolved.chatPalette).toBe("dark");
    expect(chatOrbLookFor(resolved, snap)).toEqual({
      rev: 7,
      orbColor: "rose",
      appAccent: "blue",
      bucket: "dark",
    });
  });

  it("reads a missing orb colour as auto", () => {
    const snap = snapshot({ orb_accent: undefined, chat_theme: "follow_app" });
    const resolved = resolveAppearance(snap, false);
    expect(chatOrbLookFor(resolved, snap).orbColor).toBe("auto");
    expect(chatOrbLookFor(resolved, snap).bucket).toBe("light");
  });
});
