// Entry for the standalone conversation window. It never reads settings or invokes commands
// (listen-only, DESIGN-CHAT A8). Appearance comes from the seed in the page URL (`ap`) and from
// `appearance` events, through the shared consumer, which applies only strictly newer revisions
// (UI enterprise S2, A1/A2). The window opacity comes from the URL (?opacity=&rev=) and from
// `chat-opacity` events (Track T). The two revisions are independent counters.
import React from "react";
import ReactDOM from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import {
  APPEARANCE_EVENT,
  createAppearanceConsumer,
  parseSeed,
} from "../lib/appearance/consumer";
import { DEFAULT_ACCENT, type AccentId } from "../styles/accents";
import "../styles/theme.css";
import "./i18n";
import "./chat.css";
import ChatWindow from "./ChatWindow";
import { chatPaletteVariables, type ChatPaletteName } from "./chatTheme";
import {
  alphaForPct,
  applyOpacityEvent,
  initialOpacity,
  type OpacityState,
} from "./chatOpacity";

/** Sets every palette variable on the document root (the one place chat.css reads colours from). */
function applyPalette(palette: ChatPaletteName, accent: AccentId): void {
  const style = document.documentElement.style;
  for (const [name, value] of Object.entries(
    chatPaletteVariables(palette, accent),
  )) {
    style.setProperty(name, value);
  }
}

function applyOpacity(state: OpacityState): void {
  document.documentElement.style.setProperty(
    "--chat-alpha",
    alphaForPct(state.pct),
  );
}

// The dark palette until a snapshot says otherwise, so the first paint is never unstyled.
applyPalette("dark", DEFAULT_ACCENT);

const consumer = createAppearanceConsumer({
  root: document.documentElement,
  matchMedia: (query) => window.matchMedia(query),
  onApplied: (resolved) =>
    applyPalette(resolved.chatPalette, resolved.accent ?? DEFAULT_ACCENT),
});
const seed = parseSeed(window.location.search);
if (seed !== null) consumer.accept(seed);
listen(APPEARANCE_EVENT, (event) => {
  consumer.accept(event.payload);
}).catch((error: unknown) => {
  console.warn("chat: failed to subscribe to appearance", error);
});

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

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ChatWindow />
  </React.StrictMode>,
);
