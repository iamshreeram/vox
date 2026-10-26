import { describe, expect, it } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import VoxAvatar from "./VoxAvatar";
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

function markup(face: AvatarFace): string {
  return renderToStaticMarkup(createElement(VoxAvatar, { face }));
}

describe("VoxAvatar", () => {
  it.each(FACES)("renders an aria-hidden inline svg with data-face=%s", (face) => {
    const html = markup(face);
    expect(html).toMatch(/^<svg\b/);
    expect(html).toMatch(/<svg\b[^>]*\baria-hidden="true"/);
    expect(html).toMatch(new RegExp(`<svg\\b[^>]*\\bdata-face="${face}"`));
  });

  it.each(FACES)("contains no external assets or forbidden elements for %s", (face) => {
    const html = markup(face);
    expect(html).not.toMatch(/<image\b/i);
    expect(html).not.toMatch(/<img\b/i);
    expect(html).not.toMatch(/<foreignObject\b/i);
    expect(html).not.toMatch(/<script\b/i);
    expect(html).not.toMatch(/https?:\/\//i);
    expect(html).not.toMatch(/\bxlink:href=/i);
  });

  it.each(FACES)("only uses fragment-id href values for %s", (face) => {
    const html = markup(face);
    const hrefs = [...html.matchAll(/\bhref="([^"]*)"/g)].map((m) => m[1] ?? "");
    for (const value of hrefs) {
      expect(value.startsWith("#")).toBe(true);
    }
  });
});
