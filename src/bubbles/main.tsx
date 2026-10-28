// Entry for the bubble page (DESIGN.md R6-4, A7-2). A pure renderer: it shows the snapshots that
// Rust sends on `bubbles-state` and asks for nothing else. It needs no app settings and no i18n,
// because it shows no user-facing strings of its own.
import React from "react";
import ReactDOM from "react-dom/client";
import "./bubbles.css";
import BubblesApp from "./BubblesApp";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <BubblesApp />
  </React.StrictMode>,
);
