// Entry for the standalone conversation window. Mirrors the overlay entry, except that it never
// reads settings: appearance comes from the last-applied copy in localStorage and follows
// `theme-changed` and cross-window `storage` events (DESIGN-CHAT A8, A14).
import React from "react";
import ReactDOM from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import { THEME_STORAGE_KEY, applyTheme, getStoredAppearance } from "@/lib/utils/theme";
import "./i18n";
import "./chat.css";
import ChatWindow from "./ChatWindow";

function applyStoredAppearance(): void {
  const stored = getStoredAppearance();
  applyTheme(stored.theme, stored.customThemeColors);
}

applyStoredAppearance();
window.addEventListener("storage", (event) => {
  if (event.key === THEME_STORAGE_KEY) applyStoredAppearance();
});
listen("theme-changed", () => applyStoredAppearance()).catch((error: unknown) => {
  console.warn("chat: failed to subscribe to theme-changed", error);
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ChatWindow />
  </React.StrictMode>,
);
