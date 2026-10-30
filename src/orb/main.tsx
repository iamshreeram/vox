// Entry for the orb page. Listen-only (like the conversation window): it never reads settings or
// calls no Tauri command. Appearance comes from the seed in the page URL and from `appearance` events,
// through the shared consumer (UI enterprise S2, A1/A2). The orb takes its face accent from the
// accent tokens the consumer sets on the document root.
import React from "react";
import ReactDOM from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import "../styles/theme.css";
import "../chat/i18n";
import "../chat/avatar.css";
import "./orb.css";
import OrbApp from "./OrbApp";
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
listen(APPEARANCE_EVENT, (event) => {
  consumer.accept(event.payload);
}).catch((error: unknown) => {
  console.warn("orb: failed to subscribe to appearance", error);
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <OrbApp />
  </React.StrictMode>,
);
