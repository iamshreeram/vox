import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { SettingContainer } from "../ui/SettingContainer";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";
import {
  controlDisabled,
  earlySpeechUsable,
  endpointStatus,
  tokenSummary,
  validateText,
  type RtBoolKey,
  type RtTextKey,
} from "./realtimeForm";

interface RealtimeSectionProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

const K = "settings.experimental.realtime";

/** Typed settings field for each toggle key (must match the store routing). */
/** Boolean settings keys only, so `updateSetting` always receives a boolean. */
type BoolSettingKey =
  | "exp_rt_enabled"
  | "exp_rt_direct_endpoint"
  | "exp_rt_stream_reply"
  | "exp_rt_early_speech"
  | "exp_rt_fast_voice"
  | "exp_rt_ack"
  | "exp_rt_prewarm"
  | "exp_rt_auto_proxy"
  | "exp_rt_proxy_enabled"
  | "exp_rt_timing_log";

const BOOL_FIELD: Record<RtBoolKey, BoolSettingKey> = {
  enabled: "exp_rt_enabled",
  direct_endpoint: "exp_rt_direct_endpoint",
  stream_reply: "exp_rt_stream_reply",
  early_speech: "exp_rt_early_speech",
  fast_voice: "exp_rt_fast_voice",
  ack: "exp_rt_ack",
  prewarm: "exp_rt_prewarm",
  auto_proxy: "exp_rt_auto_proxy",
  proxy_enabled: "exp_rt_proxy_enabled",
  timing_log: "exp_rt_timing_log",
};

interface RtTextFieldProps {
  fieldKey: RtTextKey;
  label: string;
  description: string;
  placeholder: string;
  stored: string;
  disabled: boolean;
  isPassword?: boolean;
  onSave: (value: string) => Promise<void>;
  descriptionMode: "inline" | "tooltip";
  grouped: boolean;
  /** Extra line under the input (used for the token summary). */
  footer?: React.ReactNode;
}

/**
 * A text input that validates locally and saves on blur or Enter. The stored
 * value is shown as the draft only for non-password fields, so a token is never
 * rendered back into the DOM.
 */
const RtTextField: React.FC<RtTextFieldProps> = ({
  fieldKey,
  label,
  description,
  placeholder,
  stored,
  disabled,
  isPassword = false,
  onSave,
  descriptionMode,
  grouped,
  footer,
}) => {
  const { t } = useTranslation();
  const [draft, setDraft] = useState(isPassword ? "" : stored);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isPassword) setDraft(stored);
  }, [stored, isPassword]);

  const commit = async () => {
    // An empty password box means "keep the stored token", not "clear it".
    if (isPassword && draft.trim() === "") return;
    const check = validateText(fieldKey, draft);
    if (!check.ok) {
      setError(t(`${K}.errors.${check.reason}`));
      return;
    }
    setError(null);
    if (isPassword) {
      await onSave(check.value);
      setDraft("");
      return;
    }
    if (check.value !== draft) setDraft(check.value);
    if (check.value !== stored) await onSave(check.value);
  };

  const clear = async () => {
    setError(null);
    setDraft("");
    await onSave("");
  };

  return (
    <SettingContainer
      title={label}
      description={description}
      descriptionMode={descriptionMode}
      grouped={grouped}
      disabled={disabled}
    >
      <div className="flex flex-col gap-1 items-end">
        <div className="flex items-center gap-2">
          <Input
            type={isPassword ? "password" : "text"}
            value={draft}
            placeholder={placeholder}
            disabled={disabled}
            autoComplete="off"
            spellCheck={false}
            aria-label={label}
            onChange={(e) => setDraft(e.target.value)}
            onBlur={() => void commit()}
            onKeyDown={(e) => {
              if (e.key === "Enter") void commit();
            }}
          />
          {isPassword && stored !== "" && !disabled && (
            <Button variant="secondary" size="sm" onClick={() => void clear()}>
              {t(`${K}.token.clear`)}
            </Button>
          )}
        </div>
        {footer}
        {error !== null && (
          <p role="alert" className="text-xs text-red-400">
            {error}
          </p>
        )}
      </div>
    </SettingContainer>
  );
};

export const RealtimeSection: React.FC<RealtimeSectionProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const master = getSetting("exp_rt_enabled") ?? false;
    const direct = getSetting("exp_rt_direct_endpoint") ?? false;
    const streamReply = getSetting("exp_rt_stream_reply") ?? true;
    const proxyEnabled = getSetting("exp_rt_proxy_enabled") ?? false;
    const endpointUrl = getSetting("exp_rt_endpoint_url") ?? "";
    const model = getSetting("exp_rt_model") ?? "";
    const token = getSetting("exp_rt_token") ?? "";
    const proxyUrl = getSetting("exp_rt_proxy_url") ?? "";
    const ttsEnabled = getSetting("tts_enabled") ?? false;

    const summary = tokenSummary(token);
    const status = endpointStatus({
      master,
      direct,
      url: endpointUrl,
      model,
      token,
    });

    const boolToggle = (
      key: RtBoolKey,
      label: string,
      description: string,
      extraDisabled = false,
    ) => {
      const field = BOOL_FIELD[key];
      return (
        <ToggleSwitch
          checked={getSetting(field) ?? false}
          onChange={(enabled) => updateSetting(field, enabled)}
          disabled={controlDisabled(key, master) || extraDisabled}
          isUpdating={isUpdating(field)}
          label={label}
          description={description}
          descriptionMode={descriptionMode}
          grouped={grouped}
          tooltipPosition="bottom"
        />
      );
    };

    const endpointDisabled =
      controlDisabled("direct_endpoint", master) || !direct;
    const proxyUrlDisabled =
      controlDisabled("proxy_enabled", master) || !proxyEnabled;

    return (
      <div className="space-y-4">
        <p className="text-xs text-mid-gray px-3">{t(`${K}.intro`)}</p>

        {boolToggle(
          "enabled",
          t(`${K}.enabled.label`),
          t(`${K}.enabled.description`),
        )}

        <ToggleSwitch
          checked={ttsEnabled}
          onChange={(enabled) => updateSetting("tts_enabled", enabled)}
          disabled={!master}
          isUpdating={isUpdating("tts_enabled")}
          label={t(`${K}.speakReplies.label`)}
          description={t(`${K}.speakReplies.description`)}
          descriptionMode={descriptionMode}
          grouped={grouped}
          tooltipPosition="bottom"
        />

        {boolToggle(
          "direct_endpoint",
          t(`${K}.directEndpoint.label`),
          t(`${K}.directEndpoint.description`),
        )}
        <RtTextField
          fieldKey="endpoint_url"
          label={t(`${K}.endpoint.label`)}
          description={t(`${K}.endpoint.description`)}
          placeholder={t(`${K}.endpoint.placeholder`)}
          stored={endpointUrl}
          disabled={endpointDisabled}
          onSave={(value) => updateSetting("exp_rt_endpoint_url", value)}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
        <RtTextField
          fieldKey="model"
          label={t(`${K}.model.label`)}
          description={t(`${K}.model.description`)}
          placeholder={t(`${K}.model.placeholder`)}
          stored={model}
          disabled={endpointDisabled}
          onSave={(value) => updateSetting("exp_rt_model", value)}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
        <RtTextField
          fieldKey="token"
          label={t(`${K}.token.label`)}
          description={t(`${K}.token.description`)}
          placeholder={t(`${K}.token.placeholder`)}
          stored={token}
          disabled={endpointDisabled}
          isPassword
          onSave={(value) => updateSetting("exp_rt_token", value)}
          descriptionMode={descriptionMode}
          grouped={grouped}
          footer={
            <span className="text-xs text-mid-gray">
              {summary.hasToken
                ? t(`${K}.token.set`, { count: token.length })
                : t(`${K}.token.none`)}
            </span>
          }
        />
        <p
          role="status"
          aria-live="polite"
          className="text-xs text-mid-gray px-3"
        >
          {t(`${K}.status.${status}`)}
        </p>

        {boolToggle(
          "stream_reply",
          t(`${K}.streamReply.label`),
          t(`${K}.streamReply.description`),
        )}
        {boolToggle(
          "early_speech",
          t(`${K}.earlySpeech.label`),
          t(`${K}.earlySpeech.description`),
          !earlySpeechUsable(master, streamReply),
        )}
        {boolToggle(
          "fast_voice",
          t(`${K}.fastVoice.label`),
          t(`${K}.fastVoice.description`),
        )}
        {boolToggle("ack", t(`${K}.ack.label`), t(`${K}.ack.description`))}
        {boolToggle(
          "prewarm",
          t(`${K}.prewarm.label`),
          t(`${K}.prewarm.description`),
        )}
        {boolToggle(
          "auto_proxy",
          t(`${K}.autoProxy.label`),
          t(`${K}.autoProxy.description`),
        )}
        {boolToggle(
          "proxy_enabled",
          t(`${K}.proxy.label`),
          t(`${K}.proxy.description`),
        )}
        <RtTextField
          fieldKey="proxy_url"
          label={t(`${K}.proxyUrl.label`)}
          description={t(`${K}.proxyUrl.description`)}
          placeholder={t(`${K}.proxyUrl.placeholder`)}
          stored={proxyUrl}
          disabled={proxyUrlDisabled}
          onSave={(value) => updateSetting("exp_rt_proxy_url", value)}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
        {boolToggle(
          "timing_log",
          t(`${K}.timingLog.label`),
          t(`${K}.timingLog.description`),
        )}
      </div>
    );
  },
);

RealtimeSection.displayName = "RealtimeSection";
