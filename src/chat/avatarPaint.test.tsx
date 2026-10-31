import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { ACCENT_IDS } from "../styles/accents";
import {
  GLOBE_STOPS,
  CONCERNED_STOPS,
  appModeStops,
  appRingColour,
} from "../orb/globePalette";
import { orbRingColour, resolveOrbAccent } from "../orb/orbAccentResolve";
import { ORB_COLOR_IDS } from "../lib/appearance/consumer";
import { ACCENTS } from "../styles/accents";
import type { OrbColor } from "../bindings";
import VoxAvatar from "./VoxAvatar";
import { avatarStyle, type AvatarLook } from "./avatarStyle";
import type { AvatarFace } from "./chatModel";

const FACES: AvatarFace[] = [
  "idle",
  "listening",
  "thinking",
  "acting",
  "speaking",
  "sleep",
  "concerned",
];
const BUCKETS = ["light", "dark"] as const;
const MODES: readonly OrbColor[] = ORB_COLOR_IDS;

describe("avatarStyle", () => {
  it.each(
    FACES.flatMap((face): [AvatarFace, OrbColor][] =>
      MODES.map((mode): [AvatarFace, OrbColor] => [face, mode]),
    ),
  )("paints %s in mode %s", (face, mode) => {
    const look: AvatarLook = {
      orbColor: mode,
      appAccent: "violet",
      bucket: "dark",
    };
    const style = avatarStyle(face, look);
    if (face === "concerned") {
      expect(style["--vox-orb-hi"]).toBe(CONCERNED_STOPS.hi);
      expect(style["--vox-orb-mid"]).toBe(CONCERNED_STOPS.mid);
      expect(style["--vox-orb-lo"]).toBe(CONCERNED_STOPS.lo);
      expect(style["--chat-accent"]).toBe("#FF6B6B");
      return;
    }
    if (mode === "app") {
      expect(style["--vox-app-accent"]).toBe("var(--color-accent-a)");
      expect(style["--vox-orb-hi"]).toBe(appModeStops.hi);
      expect(style["--vox-orb-mid"]).toBe("var(--vox-app-accent)");
      expect(style["--vox-orb-lo"]).toBe(appModeStops.lo);
      expect(style["--chat-accent"]).toBe(appRingColour(look.bucket));
      return;
    }
    const pick = resolveOrbAccent("violet", mode, "dark");
    expect(style["--vox-app-accent"]).toBeUndefined();
    expect(style["--chat-accent"]).toBe(orbRingColour(face, pick, "dark"));
    if (mode !== "auto") {
      expect(style["--vox-orb-hi"]).toBe(GLOBE_STOPS[mode].hi);
      expect(style["--vox-orb-mid"]).toBe(GLOBE_STOPS[mode].mid);
      expect(style["--vox-orb-lo"]).toBe(GLOBE_STOPS[mode].lo);
    }
  });

  it("app mode rings are the accent mixed into the bucket surface (A7), never the text role", () => {
    for (const bucket of BUCKETS) {
      const style = avatarStyle("idle", {
        orbColor: "app",
        appAccent: "amber",
        bucket,
      });
      expect(style["--chat-accent"]).toBe(appRingColour(bucket));
      expect(style["--chat-accent"]).not.toContain("color-accent-text");
    }
  });

  it("every table stop is a hex colour (no var() in table modes)", () => {
    for (const face of FACES) {
      for (const mode of MODES) {
        if (mode === "app") continue;
        const style = avatarStyle(face, {
          orbColor: mode,
          appAccent: "blue",
          bucket: "light",
        });
        for (const key of ["--vox-orb-hi", "--vox-orb-mid", "--vox-orb-lo"]) {
          expect(style[key]).toMatch(/^#[0-9A-Fa-f]{6}$/);
        }
      }
    }
  });

  it("covers every accent for explicit modes on both buckets", () => {
    for (const accent of ACCENT_IDS) {
      for (const bucket of BUCKETS) {
        const style = avatarStyle("idle", {
          orbColor: accent,
          appAccent: "amber",
          bucket,
        });
        expect(style["--vox-orb-mid"]).toBe(GLOBE_STOPS[accent].mid);
        expect(style["--chat-accent"]).toBe(
          orbRingColour(
            "idle",
            resolveOrbAccent("amber", accent, bucket),
            bucket,
          ),
        );
        expect(ACCENTS[accent][bucket].text).toBeTruthy();
      }
    }
  });
});

describe("VoxAvatar paint", () => {
  const render = (props: { face: AvatarFace; look?: AvatarLook }): string =>
    renderToStaticMarkup(createElement(VoxAvatar, props));

  it("gives every instance its own gradient ids with no ':'", () => {
    // Two avatars in one tree: React's useId is unique per tree, so they must not share ids.
    const both = renderToStaticMarkup(
      createElement(
        "div",
        null,
        createElement(VoxAvatar, { face: "idle" }),
        createElement(VoxAvatar, { face: "idle" }),
      ),
    );
    const ids = [...both.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1] ?? "");
    expect(ids.length).toBe(4);
    expect(new Set(ids).size).toBe(4);
    for (const id of ids) {
      expect(id).not.toContain(":");
      expect(both).toContain(`url(#${id})`);
    }
  });

  it("stamps data-orb-color and the inline custom properties", () => {
    const html = render({
      face: "speaking",
      look: { orbColor: "rose", appAccent: "amber", bucket: "dark" },
    });
    expect(html).toMatch(/data-orb-color="rose"/);
    expect(html).toMatch(/--vox-orb-mid:#F43F5E/);
    expect(html).toMatch(/data-face="speaking"/);
  });

  it("defaults an absent look to auto", () => {
    const html = render({ face: "idle" });
    expect(html).toMatch(/data-orb-color="auto"/);
    expect(html).toMatch(new RegExp(`--vox-orb-mid:${GLOBE_STOPS.blue.mid}`));
  });

  it("keeps the legacy gold literals out of the avatar source", () => {
    const tsx = readFileSync(
      new URL("./VoxAvatar.tsx", import.meta.url),
      "utf8",
    );
    const css = readFileSync(new URL("./avatar.css", import.meta.url), "utf8");
    for (const literal of ["#FFE27A", "#F5B921", "#fff8dc", "#fff8DC"]) {
      expect(tsx.toUpperCase()).not.toContain(literal.toUpperCase());
      expect(css.toUpperCase()).not.toContain(literal.toUpperCase());
    }
    expect(css).not.toMatch(/vox-glow-fill/);
  });
});
