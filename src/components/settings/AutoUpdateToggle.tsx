import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";
import { commands, type SourceUpdateMode } from "../../bindings";

interface AutoUpdateToggleProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * Opt-in silent source update. Sits under the update-checks toggle and is only
 * usable when update checks are on (and not forced off) in a source build.
 */
export const AutoUpdateToggle: React.FC<AutoUpdateToggleProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating, updateChecksLocked } =
    useSettings();
  const [mode, setMode] = useState<SourceUpdateMode | null>(null);

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

  // Same as the update-checks toggle: wait for the lock state, honour forced off.
  const checksOn = getSetting("update_checks_enabled") ?? true;
  const checksDisabled = updateChecksLocked !== false || !checksOn;
  // Not loaded yet: render nothing rather than flash an enabled switch.
  if (mode === null) return null;
  const notSource = mode !== "source";
  const disabled = checksDisabled || notSource;
  const enabled = getSetting("auto_update_enabled") ?? false;

  let description = t("settings.debug.autoUpdate.description");
  if (notSource) {
    description = t("settings.debug.autoUpdate.notSourceDescription");
  } else if (checksDisabled) {
    description = t("settings.debug.autoUpdate.checksOffDescription");
  }

  return (
    <ToggleSwitch
      checked={disabled ? false : enabled}
      onChange={(value) => updateSetting("auto_update_enabled", value)}
      isUpdating={isUpdating("auto_update_enabled")}
      disabled={disabled}
      label={t("settings.debug.autoUpdate.label")}
      description={description}
      descriptionMode={descriptionMode}
      grouped={grouped}
    />
  );
};
