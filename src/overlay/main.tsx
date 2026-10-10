import React from "react";
import ReactDOM from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import RecordingOverlay from "./RecordingOverlay";
import {
  applyTheme,
  getStoredAppearance,
  syncAppearanceFromSettings,
} from "@/lib/utils/theme";
import "@/i18n";

// A separate webview from the settings window, so the overlay has to set
// `data-theme` on its own document: last-known appearance before render
// (shared localStorage) to avoid a flash, reconcile with the persisted
// setting in case the overlay booted first, then follow live changes. The
// event payload is intentionally ignored -- it's only a "something changed,
// go re-fetch" signal, see theme.ts's module doc comment for why.
const storedAppearance = getStoredAppearance();
applyTheme(storedAppearance.theme, storedAppearance.customThemeColors);
syncAppearanceFromSettings();
listen("theme-changed", () => syncAppearanceFromSettings());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <RecordingOverlay />
  </React.StrictMode>,
);
