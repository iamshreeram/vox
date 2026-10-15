import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { commands, type AgentJobStatusDto } from "@/bindings";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { SettingContainer } from "../ui/SettingContainer";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";
import {
  clampTimeoutMinutes,
  DEFAULT_TIMEOUT_MINUTES,
  formatElapsed,
  isRunning,
  MAX_TIMEOUT_MINUTES,
  MIN_TIMEOUT_MINUTES,
  stateLabelKey,
} from "../../lib/agentJobs";

interface JobsSettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

const IDLE_STATUS: AgentJobStatusDto = {
  state: "idle",
  project: null,
  elapsed_secs: 0,
};

// Shows the job state only. The DTO carries no job output, and none is rendered.
const JobStatusCard: React.FC = () => {
  const { t } = useTranslation();
  const [status, setStatus] = useState<AgentJobStatusDto>(IDLE_STATUS);
  const [cancelling, setCancelling] = useState(false);
  const running = isRunning(status.state);

  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | null = null;
    void commands.agentJobStatus().then((next) => {
      if (!disposed) setStatus(next);
    });
    void listen<AgentJobStatusDto>("agent-job-update", (event) => {
      setStatus(event.payload);
      setCancelling(false);
    }).then((fn) => {
      if (disposed) {
        fn();
      } else {
        unlisten = fn;
      }
    });
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, []);

  const handleCancel = async () => {
    setCancelling(true);
    await commands.agentJobCancel();
    setCancelling(false);
  };

  return (
    <div className="px-4 py-3 space-y-1">
      <p className="text-sm font-medium">
        {t("settings.experimental.jobs.status.title")}
      </p>
      <p className="text-sm">{t(stateLabelKey(status.state))}</p>
      {status.project !== null && (
        <p className="truncate font-mono text-xs">{status.project}</p>
      )}
      {running && (
        <p className="text-xs text-mid-gray">
          {t("settings.experimental.jobs.status.elapsed", {
            elapsed: formatElapsed(status.elapsed_secs),
          })}
        </p>
      )}
      <Button
        variant="secondary"
        size="sm"
        disabled={!running || cancelling}
        onClick={() => void handleCancel()}
      >
        {t("settings.experimental.jobs.status.cancel")}
      </Button>
    </div>
  );
};

const TimeoutField: React.FC<{
  grouped: boolean;
  descriptionMode: "inline" | "tooltip";
}> = ({ grouped, descriptionMode }) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const stored =
    getSetting("agent_jobs_timeout_minutes") ?? DEFAULT_TIMEOUT_MINUTES;
  const [draft, setDraft] = useState(String(stored));
  const pending = isUpdating("agent_jobs_timeout_minutes");

  useEffect(() => {
    setDraft(String(stored));
  }, [stored]);

  const commit = () => {
    const value = clampTimeoutMinutes(Number(draft));
    setDraft(String(value));
    if (value !== stored) {
      void updateSetting("agent_jobs_timeout_minutes", value);
    }
  };

  return (
    <SettingContainer
      title={t("settings.experimental.jobs.timeoutLabel")}
      description={t("settings.experimental.jobs.timeoutDescription", {
        min: MIN_TIMEOUT_MINUTES,
        max: MAX_TIMEOUT_MINUTES,
      })}
      descriptionMode={descriptionMode}
      grouped={grouped}
    >
      <Input
        type="number"
        min={MIN_TIMEOUT_MINUTES}
        max={MAX_TIMEOUT_MINUTES}
        step={1}
        value={draft}
        disabled={pending}
        variant="compact"
        className="w-24"
        onChange={(event) => setDraft(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") commit();
        }}
      />
    </SettingContainer>
  );
};

export const JobsSettings: React.FC<JobsSettingsProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const confirmActions = getSetting("agent_confirm_actions") ?? true;
    const jobsEnabled = getSetting("agent_jobs_enabled") ?? false;

    return (
      <>
        <ToggleSwitch
          checked={confirmActions}
          onChange={(enabled) =>
            updateSetting("agent_confirm_actions", enabled)
          }
          isUpdating={isUpdating("agent_confirm_actions")}
          label={t("settings.experimental.jobs.confirmLabel")}
          description={t("settings.experimental.jobs.confirmDescription")}
          descriptionMode={descriptionMode}
          grouped={grouped}
          tooltipPosition="bottom"
        />
        <ToggleSwitch
          checked={jobsEnabled}
          onChange={(enabled) => updateSetting("agent_jobs_enabled", enabled)}
          isUpdating={isUpdating("agent_jobs_enabled")}
          label={t("settings.experimental.jobs.enabledLabel")}
          description={t("settings.experimental.jobs.enabledDescription")}
          descriptionMode={descriptionMode}
          grouped={grouped}
          tooltipPosition="bottom"
        />
        <TimeoutField grouped={grouped} descriptionMode={descriptionMode} />
        <JobStatusCard />
        <div className="px-1 py-2">
          <p className="text-xs text-mid-gray">
            {t("settings.experimental.jobs.warning")}
          </p>
        </div>
      </>
    );
  },
);
