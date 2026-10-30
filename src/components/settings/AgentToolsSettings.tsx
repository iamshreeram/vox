import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { SettingContainer } from "../ui/SettingContainer";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";
import { useSettingsStore } from "../../stores/settingsStore";
import {
  MAX_PROJECT_ROOTS,
  appendProjectRoot,
  canAddProjectRoot,
} from "./agentToolsForm";

interface AgentToolsSettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

interface ProjectRootsEditorProps {
  descriptionMode: "inline" | "tooltip";
  grouped: boolean;
}

// Saves go through the settings store updater, which calls the Rust command
// that validates every entry. A rejected list is rolled back by the store and
// the Rust error text is shown as a toast by that updater.
const ProjectRootsEditor: React.FC<ProjectRootsEditorProps> = ({
  descriptionMode,
  grouped,
}) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const storedRoots = getSetting("agent_project_roots") ?? [];
  const [draft, setDraft] = useState("");
  const pending = isUpdating("agent_project_roots");
  const atLimit = storedRoots.length >= MAX_PROJECT_ROOTS;

  const handleAdd = async () => {
    if (!canAddProjectRoot(storedRoots, draft)) return;
    const next = appendProjectRoot(storedRoots, draft);
    await updateSetting("agent_project_roots", next);
    // Keep the typed text when the save was rejected, so the user can fix it.
    const saved =
      useSettingsStore.getState().settings?.agent_project_roots ?? [];
    if (saved.length === next.length) setDraft("");
  };

  const handleRemove = (index: number) => {
    void updateSetting(
      "agent_project_roots",
      storedRoots.filter((_, i) => i !== index),
    );
  };

  return (
    <SettingContainer
      title={t("settings.experimental.agentTools.projectRootsLabel")}
      description={t(
        "settings.experimental.agentTools.projectRootsDescription",
      )}
      descriptionMode={descriptionMode}
      grouped={grouped}
      layout="stacked"
    >
      <div className="space-y-2">
        {storedRoots.length === 0 ? (
          <p className="text-xs text-muted">
            {t("settings.experimental.agentTools.noProjectRoots")}
          </p>
        ) : (
          <ul className="space-y-1">
            {storedRoots.map((root, index) => (
              <li
                key={`${index}-${root}`}
                className="flex items-center justify-between gap-2"
              >
                <span className="truncate font-mono text-xs">{root}</span>
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={pending}
                  onClick={() => handleRemove(index)}
                >
                  {t("settings.experimental.agentTools.removeProjectRoot")}
                </Button>
              </li>
            ))}
          </ul>
        )}
        <div className="flex items-center gap-2">
          <Input
            type="text"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            placeholder={t(
              "settings.experimental.agentTools.projectRootPlaceholder",
            )}
            variant="compact"
            disabled={pending || atLimit}
            className="flex-1 min-w-[260px]"
          />
          <Button
            variant="secondary"
            size="sm"
            disabled={
              pending || atLimit || !canAddProjectRoot(storedRoots, draft)
            }
            onClick={handleAdd}
          >
            {t("settings.experimental.agentTools.addProjectRoot")}
          </Button>
        </div>
        {atLimit && (
          <p className="text-xs text-muted">
            {t("settings.experimental.agentTools.projectRootsLimit", {
              max: MAX_PROJECT_ROOTS,
            })}
          </p>
        )}
      </div>
    </SettingContainer>
  );
};

export const AgentToolsSettings: React.FC<AgentToolsSettingsProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const toolsEnabled = getSetting("agent_tools_enabled") ?? false;
    const guardActions = getSetting("agent_tools_guard_actions") ?? true;

    return (
      <>
        <ToggleSwitch
          checked={toolsEnabled}
          onChange={(enabled) => updateSetting("agent_tools_enabled", enabled)}
          isUpdating={isUpdating("agent_tools_enabled")}
          label={t("settings.experimental.agentTools.enabledLabel")}
          description={t("settings.experimental.agentTools.enabledDescription")}
          descriptionMode={descriptionMode}
          grouped={grouped}
          tooltipPosition="bottom"
        />
        {toolsEnabled && (
          <>
            <ToggleSwitch
              checked={guardActions}
              onChange={(enabled) =>
                updateSetting("agent_tools_guard_actions", enabled)
              }
              isUpdating={isUpdating("agent_tools_guard_actions")}
              label={t("settings.experimental.agentTools.guardLabel")}
              description={t(
                "settings.experimental.agentTools.guardDescription",
              )}
              descriptionMode={descriptionMode}
              grouped={grouped}
              tooltipPosition="bottom"
            />
            <ProjectRootsEditor
              descriptionMode={descriptionMode}
              grouped={grouped}
            />
          </>
        )}
        <div className="px-3 py-2">
          <p className="text-xs text-muted">
            {t("settings.experimental.agentTools.privacyText")}
          </p>
        </div>
      </>
    );
  },
);
