// Entry for the bubble page (DESIGN.md R6-4, A7-2). A pure renderer: it shows the snapshots that
// Rust sends on `bubbles-state`. Appearance (UI enterprise S2, A1/A2) is listen-only too: the seed
// in the page URL plus `appearance` events, through the shared consumer. It needs no app settings
// and no i18n, because it shows no user-facing strings of its own.
import React from "react";
import ReactDOM from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import "../styles/theme.css";
import "./bubbles.css";
import BubblesApp from "./BubblesApp";
import {
  APPEARANCE_EVENT,
  createAppearanceConsumer,
  parseSeed,
} from "../lib/appearance/consumer";

const consumer = createAppearanceConsumer({
  root: document.documentElement,
  matchMedia: (query) => window.matchMedia(query),
});
const seed = parseSeed(window.location.search);
if (seed !== null) consumer.accept(seed);
// Tauri v2 defines __TAURI_INTERNALS__ in every webview. The browser preview has none, so it
// skips the subscription instead of logging a second warning (BubblesApp warns once for it).
if ("__TAURI_INTERNALS__" in window) {
  listen(APPEARANCE_EVENT, (event) => {
    consumer.accept(event.payload);
  }).catch((error: unknown) => {
    console.warn("bubbles: failed to subscribe to appearance", error);
  });
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <BubblesApp />
  </React.StrictMode>,
);
