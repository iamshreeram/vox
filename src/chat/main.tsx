// Entry for the standalone conversation window. Mirrors the overlay entry, except that it never
// reads settings or invokes commands: appearance comes from the last-applied copy in localStorage
// and follows `theme-changed` and cross-window `storage` events (DESIGN-CHAT A8, A14). The window
// opacity comes from the page URL and from `chat-opacity` events (Track T, DESIGN Revision 3).
import React from "react";
import ReactDOM from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import {
  THEME_STORAGE_KEY,
  applyTheme,
  getStoredAppearance,
} from "@/lib/utils/theme";
import "./i18n";
import "./chat.css";
import ChatWindow from "./ChatWindow";
import { CHAT_RGB } from "./chatTheme";
import {
  alphaForPct,
  applyOpacityEvent,
  initialOpacity,
  type OpacityState,
} from "./chatOpacity";

function applyStoredAppearance(): void {
  const stored = getStoredAppearance();
  applyTheme(stored.theme, stored.customThemeColors);
}

/** The palette triplets chat.css paints from (single source: chatTheme.ts). */
function applyPaletteVariables(): void {
  const style = document.documentElement.style;
  style.setProperty("--chat-bg-rgb", CHAT_RGB.bg);
  style.setProperty("--chat-surface-rgb", CHAT_RGB.surface);
}

function applyOpacity(state: OpacityState): void {
  document.documentElement.style.setProperty(
    "--chat-alpha",
    alphaForPct(state.pct),
  );
}

applyStoredAppearance();
applyPaletteVariables();

// The first value is the one the window was built with (?opacity=&rev=). Later values apply only
// when their rev is strictly higher, so a late subscriber converges on the latest commit.
let opacity: OpacityState = initialOpacity(window.location.search);
applyOpacity(opacity);
listen("chat-opacity", (event) => {
  const next = applyOpacityEvent(opacity, event.payload);
  if (next !== opacity) {
    opacity = next;
    applyOpacity(opacity);
  }
}).catch((error: unknown) => {
  console.warn("chat: failed to subscribe to chat-opacity", error);
});

window.addEventListener("storage", (event) => {
  if (event.key === THEME_STORAGE_KEY) applyStoredAppearance();
});
listen("theme-changed", () => applyStoredAppearance()).catch(
  (error: unknown) => {
    console.warn("chat: failed to subscribe to theme-changed", error);
  },
);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ChatWindow />
  </React.StrictMode>,
);
