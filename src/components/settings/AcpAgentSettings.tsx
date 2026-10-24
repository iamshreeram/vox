import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { SettingContainer } from "../ui/SettingContainer";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";
import { useSettingsStore } from "../../stores/settingsStore";
import {
  ACP_TIMEOUT_MAX_SECS,
  ACP_TIMEOUT_MIN_SECS,
  type AcpFormError,
  parseAcpArgs,
  parseAcpCommand,
  parseAcpTimeout,
} from "./acpAgentForm";

interface AcpAgentSettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const AcpAgentSettings: React.FC<AcpAgentSettingsProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const enabled = getSetting("acp_agent_enabled") ?? false;
    const storedCommand = getSetting("acp_agent_command") ?? null;
    const storedArgs = getSetting("acp_agent_args") ?? "";
    const storedTimeout = getSetting("acp_agent_turn_timeout_secs") ?? 120;

    const [commandDraft, setCommandDraft] = useState(storedCommand ?? "");
    const [commandError, setCommandError] = useState<AcpFormError | null>(null);
    const [argsDraft, setArgsDraft] = useState(storedArgs);
    const [argsError, setArgsError] = useState<AcpFormError | null>(null);
    const [timeoutDraft, setTimeoutDraft] = useState(String(storedTimeout));
    const [timeoutError, setTimeoutError] = useState<AcpFormError | null>(null);
    const [testing, setTesting] = useState(false);
    const [testLine, setTestLine] = useState<string | null>(null);

    useEffect(() => {
      setCommandDraft(storedCommand ?? "");
      setCommandError(null);
    }, [storedCommand]);

    useEffect(() => {
      setArgsDraft(storedArgs);
      setArgsError(null);
    }, [storedArgs]);

    useEffect(() => {
      setTimeoutDraft(String(storedTimeout));
      setTimeoutError(null);
    }, [storedTimeout]);

    // Literal keys only, so the settings search index can see every string.
    const errorText = (code: AcpFormError): string => {
      switch (code) {
        case "notAbsolute":
          return t("settings.advanced.acpAgent.errors.notAbsolute");
        case "tooLong":
          return t("settings.advanced.acpAgent.errors.tooLong");
        case "controlChars":
          return t("settings.advanced.acpAgent.errors.controlChars");
        case "tooMany":
          return t("settings.advanced.acpAgent.errors.tooMany");
        case "argTooLong":
          return t("settings.advanced.acpAgent.errors.argTooLong");
        case "outOfRange":
          return t("settings.advanced.acpAgent.errors.outOfRange");
        case "notInteger":
          return t("settings.advanced.acpAgent.errors.notInteger");
      }
    };

    // An invalid draft reverts to the stored value and shows the error.
    // updateSetting reverts the store on a backend rejection, so the stored
    // value is re-read afterwards to show what was actually kept.
    const commitCommand = async () => {
      const parsed = parseAcpCommand(commandDraft);
      if (!parsed.ok) {
        setCommandError(parsed.error);
        setCommandDraft(storedCommand ?? "");
        return;
      }
      setCommandError(null);
      if (parsed.value === storedCommand) return;
      await updateSetting("acp_agent_command", parsed.value);
      setCommandDraft(
        useSettingsStore.getState().settings?.acp_agent_command ?? "",
      );
    };

    const commitArgs = async () => {
      const parsed = parseAcpArgs(argsDraft);
      if (!parsed.ok) {
        setArgsError(parsed.error);
        setArgsDraft(storedArgs);
        return;
      }
      setArgsError(null);
      if (parsed.value === storedArgs) return;
      await updateSetting("acp_agent_args", parsed.value);
      setArgsDraft(useSettingsStore.getState().settings?.acp_agent_args ?? "");
    };

    const commitTimeout = async () => {
      const parsed = parseAcpTimeout(timeoutDraft);
      if (!parsed.ok) {
        setTimeoutError(parsed.error);
        setTimeoutDraft(String(storedTimeout));
        return;
      }
      setTimeoutError(null);
      if (parsed.value === storedTimeout) return;
      await updateSetting("acp_agent_turn_timeout_secs", parsed.value);
      setTimeoutDraft(
        String(
          useSettingsStore.getState().settings?.acp_agent_turn_timeout_secs ??
            storedTimeout,
        ),
      );
    };

    const canTest = enabled && !!storedCommand && !testing;

    const runTest = async () => {
      setTesting(true);
      setTestLine(null);
      const result = await commands.acpAgentTestConnection();
      setTestLine(result.status === "ok" ? result.data : result.error);
      setTesting(false);
    };

    const blurOnEnter = (event: React.KeyboardEvent<HTMLInputElement>) => {
      if (event.key === "Enter") event.currentTarget.blur();
    };

    return (
      <>
        <div className="px-4 pt-2">
          <p className="text-xs text-mid-gray">
            {t("settings.advanced.acpAgent.description")}
          </p>
        </div>
        <div
          role="note"
          className="mx-4 my-2 rounded-md border border-warning/60 bg-warning/10 p-3 text-xs font-medium text-warning"
        >
          {t("settings.advanced.acpAgent.trustWarning")}
        </div>
        <ToggleSwitch
          checked={enabled}
          onChange={(value) => updateSetting("acp_agent_enabled", value)}
          isUpdating={isUpdating("acp_agent_enabled")}
          label={t("settings.advanced.acpAgent.enabled.label")}
          description={t("settings.advanced.acpAgent.enabled.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
          tooltipPosition="bottom"
        />
        <SettingContainer
          title={t("settings.advanced.acpAgent.command.label")}
          description={t("settings.advanced.acpAgent.command.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        >
          <div className="flex flex-col items-end gap-1">
            <Input
              type="text"
              value={commandDraft}
              placeholder={t("settings.advanced.acpAgent.command.placeholder")}
              onChange={(event) => {
                setCommandDraft(event.target.value);
                setCommandError(null);
              }}
              onBlur={() => void commitCommand()}
              onKeyDown={blurOnEnter}
              aria-label={t("settings.advanced.acpAgent.command.label")}
              disabled={isUpdating("acp_agent_command")}
              variant="compact"
              className="w-72 font-mono"
            />
            {commandError !== null && (
              <p
                role="alert"
                className="max-w-xs text-end text-xs text-red-500"
              >
                {errorText(commandError)}
              </p>
            )}
          </div>
        </SettingContainer>
        <SettingContainer
          title={t("settings.advanced.acpAgent.args.label")}
          description={t("settings.advanced.acpAgent.args.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        >
          <div className="flex flex-col items-end gap-1">
            <Input
              type="text"
              value={argsDraft}
              placeholder={t("settings.advanced.acpAgent.args.placeholder")}
              onChange={(event) => {
                setArgsDraft(event.target.value);
                setArgsError(null);
              }}
              onBlur={() => void commitArgs()}
              onKeyDown={blurOnEnter}
              aria-label={t("settings.advanced.acpAgent.args.label")}
              disabled={isUpdating("acp_agent_args")}
              variant="compact"
              className="w-48 font-mono"
            />
            {argsError !== null && (
              <p
                role="alert"
                className="max-w-xs text-end text-xs text-red-500"
              >
                {errorText(argsError)}
              </p>
            )}
          </div>
        </SettingContainer>
        <SettingContainer
          title={t("settings.advanced.acpAgent.timeout.label")}
          description={t("settings.advanced.acpAgent.timeout.description", {
            min: ACP_TIMEOUT_MIN_SECS,
            max: ACP_TIMEOUT_MAX_SECS,
          })}
          descriptionMode={descriptionMode}
          grouped={grouped}
        >
          <div className="flex flex-col items-end gap-1">
            <Input
              type="text"
              inputMode="numeric"
              value={timeoutDraft}
              onChange={(event) => {
                setTimeoutDraft(event.target.value);
                setTimeoutError(null);
              }}
              onBlur={() => void commitTimeout()}
              onKeyDown={blurOnEnter}
              aria-label={t("settings.advanced.acpAgent.timeout.label")}
              disabled={isUpdating("acp_agent_turn_timeout_secs")}
              variant="compact"
              className="w-28"
            />
            {timeoutError !== null && (
              <p
                role="alert"
                className="max-w-xs text-end text-xs text-red-500"
              >
                {errorText(timeoutError)}
              </p>
            )}
          </div>
        </SettingContainer>
        <div className="flex flex-col items-start gap-1 px-4 py-2">
          <Button
            variant="secondary"
            size="sm"
            disabled={!canTest}
            onClick={() => void runTest()}
          >
            {testing
              ? t("settings.advanced.acpAgent.testing")
              : t("settings.advanced.acpAgent.testConnection")}
          </Button>
          <p aria-live="polite" className="text-xs">
            {testLine ?? ""}
          </p>
        </div>
      </>
    );
  },
);
