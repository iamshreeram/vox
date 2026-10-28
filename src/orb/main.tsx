// Entry for the orb page. Like the conversation window it never reads settings: appearance
// comes from the last-applied copy in localStorage and follows `theme-changed` and `storage`.
// It reuses the chat i18n bootstrap (the lightest one that does not call app commands).
import React from "react";
import ReactDOM from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import {
  THEME_STORAGE_KEY,
  applyTheme,
  getStoredAppearance,
} from "@/lib/utils/theme";
import "../chat/i18n";
import "../chat/avatar.css";
import "./orb.css";
import OrbApp from "./OrbApp";

function applyStoredAppearance(): void {
  const stored = getStoredAppearance();
  applyTheme(stored.theme, stored.customThemeColors);
}

applyStoredAppearance();
window.addEventListener("storage", (event) => {
  if (event.key === THEME_STORAGE_KEY) applyStoredAppearance();
});
listen("theme-changed", () => applyStoredAppearance()).catch(
  (error: unknown) => {
    console.warn("orb: failed to subscribe to theme-changed", error);
  },
);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <OrbApp />
  </React.StrictMode>,
);
