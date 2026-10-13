import React, { useEffect, useState } from "react";
import { commands, type SourceUpdateMode } from "../../bindings";
import PluginUpdateChecker from "./UpdateChecker";
import SourceUpdateChecker from "./SourceUpdateChecker";

interface UpdateCheckerProps {
  className?: string;
}

type ModeState = "loading" | SourceUpdateMode;

/**
 * Picks the update flow for this build: rebuild-from-source for source
 * checkouts, the Tauri plugin updater otherwise.
 */
const UpdateChecker: React.FC<UpdateCheckerProps> = ({ className = "" }) => {
  const [mode, setMode] = useState<ModeState>("loading");

  useEffect(() => {
    let active = true;
    commands
      .getUpdateMode()
      .then((value) => {
        if (active) setMode(value);
      })
      .catch(() => {
        if (active) setMode("plugin");
      });
    return () => {
      active = false;
    };
  }, []);

  // This file is .ts, so the elements are built without JSX.
  if (mode === "loading") return null;
  if (mode === "source") {
    return React.createElement(SourceUpdateChecker, { className });
  }
  return React.createElement(PluginUpdateChecker, { className });
};

export default UpdateChecker;
