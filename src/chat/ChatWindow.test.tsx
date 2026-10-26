// Static-markup checks for ChatWindow. renderToStaticMarkup runs no effects, so
// hydration, events, clicks and scrolling are NOT covered here; see
// ChatWindow.dom.test.tsx.pending (needs a DOM test environment, not installed).
import { describe, expect, it } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import i18next from "i18next";
import { initReactI18next } from "react-i18next";
import translationEn from "@/i18n/locales/en/translation.json";
import ChatWindow from "./ChatWindow";

// Mirrors the app's i18n init (src/i18n/index.ts): English only, React escapes.
const testI18n = i18next.createInstance();
await testI18n.use(initReactI18next).init({
  resources: { en: { translation: translationEn } },
  lng: "en",
  fallbackLng: "en",
  interpolation: { escapeValue: false },
});

function render(): string {
  return renderToStaticMarkup(createElement(ChatWindow));
}

describe("ChatWindow static markup", () => {
  it("renders the transcript as a polite live log", () => {
    const html = render();
    expect(html).toMatch(/<[a-z]+\b[^>]*\brole="log"[^>]*>/);
    expect(html).toMatch(/<[a-z]+\b[^>]*\brole="log"[^>]*\baria-live="polite"/);
  });

  it("shows the status caption for the default idle state", () => {
    expect(render()).toMatch(/chat\.status\.idle|Idle/);
  });

  it("offers the Clear button with its i18n key", () => {
    expect(render()).toMatch(/chat\.clear|Clear/);
  });

  it("renders the avatar as an aria-hidden svg", () => {
    expect(render()).toMatch(/<svg\b[^>]*\baria-hidden="true"/);
  });

  it("does not render the Jump to latest button before any scrolling", () => {
    expect(render()).not.toContain("chat.jumpToLatest");
  });
});
