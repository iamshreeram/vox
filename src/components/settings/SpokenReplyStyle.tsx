import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  commands,
  type ComposerPreview,
  type ComposerProviderInfo,
  type TtsComposerFallback,
  type TtsComposerMode,
  type TtsComposerSettingsPatch,
} from "@/bindings";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { Select, type SelectOption } from "../ui/Select";
import { SettingContainer } from "../ui/SettingContainer";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";
import {
  commandPatch,
  commandReadiness,
  composerFromApp,
  draftFromSettings,
  editCommandDraft,
  isCommandDraftDirty,
  patchWith,
  statusLabelKey,
  type CommandDraft,
} from "./composerForm";

interface SpokenReplyStyleProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

const MODES: TtsComposerMode[] = ["off", "model", "command"];
const FALLBACKS: TtsComposerFallback[] = ["short_notice", "verbatim", "silent"];
const TIMEOUT_MIN_SECS = 2;
const TIMEOUT_MAX_SECS = 60;

type Field =
  | "mode"
  | "provider"
  | "model"
  | "command"
  | "style"
  | "timeout"
  | "send"
  | "fallback";

/** Returns `value` when it is one of `options`, otherwise null. */
function pickOption<T extends string>(
  options: readonly T[],
  value: string | null,
): T | null {
  return options.find((option) => option === value) ?? null;
}

function blurOnEnter(event: React.KeyboardEvent<HTMLInputElement>): void {
  if (event.key === "Enter") {
    event.currentTarget.blur();
  }
}

/**
 * Spoken reply style: the composer mode (off / model / command), its provider
 * or command setup, the style text, timeout, fallback, a preview, and the
 * outcome of the most recent composed reply. All non-visual rules live in
 * composerForm.ts.
 */
export const SpokenReplyStyle: React.FC<SpokenReplyStyleProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, refreshSettings } = useSettings();

    const saved = composerFromApp({
      tts_composer_mode: getSetting("tts_composer_mode"),
      tts_composer_provider_id: getSetting("tts_composer_provider_id"),
      tts_composer_model: getSetting("tts_composer_model"),
      tts_composer_command_path: getSetting("tts_composer_command_path"),
      tts_composer_command_args: getSetting("tts_composer_command_args"),
      tts_composer_prompt_flag: getSetting("tts_composer_prompt_flag"),
      tts_composer_command_ack: getSetting("tts_composer_command_ack"),
      tts_composer_timeout_secs: getSetting("tts_composer_timeout_secs"),
      tts_composer_style: getSetting("tts_composer_style"),
      tts_composer_send_user_utterance: getSetting(
        "tts_composer_send_user_utterance",
      ),
      tts_composer_fallback: getSetting("tts_composer_fallback"),
    });

    // Local drafts for text fields and the command form. They are re-synced
    // from the saved settings whenever the saved value changes.
    const [modelDraft, setModelDraft] = useState(saved.model);
    const [styleDraft, setStyleDraft] = useState(saved.style);
    const [timeoutDraft, setTimeoutDraft] = useState(
      String(saved.timeout_secs),
    );
    const [commandDraft, setCommandDraft] = useState<CommandDraft>(() =>
      draftFromSettings(saved),
    );

    const [providers, setProviders] = useState<ComposerProviderInfo[]>([]);
    const [providersFailed, setProvidersFailed] = useState(false);
    const [pending, setPending] = useState(0);
    const [errors, setErrors] = useState<Partial<Record<Field, string>>>({});

    const [previewing, setPreviewing] = useState(false);
    const [preview, setPreview] = useState<ComposerPreview | null>(null);
    const [previewError, setPreviewError] = useState<string | null>(null);

    const [lastOutcome, setLastOutcome] = useState<string | null>(null);

    const busy = pending > 0;

    useEffect(() => {
      setModelDraft(saved.model);
    }, [saved.model]);

    useEffect(() => {
      setStyleDraft(saved.style);
    }, [saved.style]);

    useEffect(() => {
      setTimeoutDraft(String(saved.timeout_secs));
    }, [saved.timeout_secs]);

    useEffect(() => {
      setCommandDraft(draftFromSettings(saved));
      // Only the command fields are watched; `saved` is a new object per render.
    }, [
      saved.command_path,
      saved.command_args,
      saved.prompt_flag,
      saved.command_ack,
    ]);

    useEffect(() => {
      let cancelled = false;
      commands.listTtsComposerProviders().then(
        (list) => {
          if (!cancelled) setProviders(list);
        },
        () => {
          if (!cancelled) setProvidersFailed(true);
        },
      );
      return () => {
        cancelled = true;
      };
    }, []);

    useEffect(() => {
      let disposed = false;
      let unlisten: UnlistenFn | null = null;
      void listen<{ outcome: string }>("speech-composer-status", (event) => {
        setLastOutcome(event.payload.outcome);
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

    const setFieldError = (field: Field, message: string | undefined) => {
      setErrors((prev) => ({ ...prev, [field]: message }));
    };

    /** Saves one patch, refreshes the store on success, and restores the
     * local draft (via `restore`) on error. */
    const save = async (
      field: Field,
      patch: TtsComposerSettingsPatch,
      restore?: () => void,
    ): Promise<void> => {
      setPending((n) => n + 1);
      setFieldError(field, undefined);
      try {
        const result = await commands.changeTtsComposerSettings(patch);
        if (result.status === "error") {
          setFieldError(field, result.error);
          restore?.();
          return;
        }
        await refreshSettings();
      } catch (error) {
        setFieldError(
          field,
          error instanceof Error ? error.message : String(error),
        );
        restore?.();
      } finally {
        setPending((n) => n - 1);
      }
    };

    const handleModeChange = (value: string | null) => {
      const mode = pickOption(MODES, value);
      if (mode === null || mode === saved.mode) return;
      void save("mode", patchWith({ mode }));
    };

    const handleProviderChange = (value: string | null) => {
      if (value === null || value === saved.provider_id) return;
      void save("provider", patchWith({ provider_id: value }));
    };

    const commitModel = () => {
      const next = modelDraft.trim();
      if (next === saved.model) {
        setModelDraft(saved.model);
        return;
      }
      void save("model", patchWith({ model: next }), () =>
        setModelDraft(saved.model),
      );
    };

    const commitStyle = () => {
      const next = styleDraft.trim();
      if (next === saved.style) {
        setStyleDraft(saved.style);
        return;
      }
      void save("style", patchWith({ style: next }), () =>
        setStyleDraft(saved.style),
      );
    };

    const commitTimeout = () => {
      setFieldError("timeout", undefined);
      const value = Number(timeoutDraft.trim());
      if (
        !Number.isInteger(value) ||
        value < TIMEOUT_MIN_SECS ||
        value > TIMEOUT_MAX_SECS
      ) {
        setFieldError(
          "timeout",
          t("settings.advanced.tts.composer.timeoutRange"),
        );
        setTimeoutDraft(String(saved.timeout_secs));
        return;
      }
      if (value === saved.timeout_secs) return;
      void save("timeout", patchWith({ timeout_secs: value }), () =>
        setTimeoutDraft(String(saved.timeout_secs)),
      );
    };

    const handleSendUtteranceChange = (checked: boolean) => {
      if (checked === saved.send_user_utterance) return;
      void save("send", patchWith({ send_user_utterance: checked }));
    };

    const handleFallbackChange = (value: string | null) => {
      const fallback = pickOption(FALLBACKS, value);
      if (fallback === null || fallback === saved.fallback) return;
      void save("fallback", patchWith({ fallback }));
    };

    const commandDirty = isCommandDraftDirty(commandDraft, saved);

    const saveCommand = () => {
      void save("command", commandPatch(commandDraft), () =>
        setCommandDraft(draftFromSettings(saved)),
      );
    };

    const handlePreview = async () => {
      setPreviewing(true);
      setPreviewError(null);
      try {
        const result = await commands.previewSpeechComposer();
        if (result.status === "error") {
          setPreviewError(result.error);
        } else {
          setPreview(result.data);
        }
      } catch (error) {
        setPreviewError(error instanceof Error ? error.message : String(error));
      } finally {
        setPreviewing(false);
      }
    };

    const modeOptions: SelectOption[] = MODES.map((mode) => ({
      value: mode,
      label: t(`settings.advanced.tts.composer.mode.${mode}`),
    }));

    const providerOptions: SelectOption[] = providers.map((provider) => ({
      value: provider.id,
      label: provider.label,
    }));

    const fallbackOptions: SelectOption[] = FALLBACKS.map((fallback) => ({
      value: fallback,
      label: t(`settings.advanced.tts.composer.fallback.${fallback}`),
    }));

    const providerInfo = providers.find(
      (provider) => provider.id === saved.provider_id,
    );

    const readiness = commandReadiness(saved);

    const renderError = (field: Field) => {
      const message = errors[field];
      if (!message) return null;
      return (
        <p role="alert" className="text-xs text-danger mt-1">
          {message}
        </p>
      );
    };

    const rowDescription = {
      descriptionMode,
      grouped,
    } as const;

    return (
      <>
        <div className="px-3 pt-3 pb-1">
          <p className="text-xs text-mid-gray">
            {t("settings.advanced.tts.composer.description")}
          </p>
        </div>

        <SettingContainer
          title={t("settings.advanced.tts.composer.modeLabel")}
          description={t("settings.advanced.tts.composer.modeDescription")}
          {...rowDescription}
        >
          <div className="flex flex-col">
            <Select
              value={saved.mode}
              options={modeOptions}
              onChange={handleModeChange}
              isClearable={false}
              disabled={busy}
              className="min-w-[220px]"
            />
            {renderError("mode")}
          </div>
        </SettingContainer>

        {saved.mode === "model" && (
          <>
            <SettingContainer
              title={t("settings.advanced.tts.composer.providerLabel")}
              description={t(
                "settings.advanced.tts.composer.providerDescription",
              )}
              {...rowDescription}
            >
              <div className="flex flex-col">
                <Select
                  value={saved.provider_id}
                  options={providerOptions}
                  onChange={handleProviderChange}
                  isClearable={false}
                  disabled={busy}
                  className="min-w-[220px]"
                />
                {providersFailed && (
                  <p role="alert" className="text-xs text-danger mt-1">
                    {t("settings.advanced.tts.composer.providersLoadFailed")}
                  </p>
                )}
                {renderError("provider")}
              </div>
            </SettingContainer>

            <SettingContainer
              title={t("settings.advanced.tts.composer.modelLabel")}
              description={t("settings.advanced.tts.composer.modelDescription")}
              {...rowDescription}
            >
              <div className="flex flex-col">
                <Input
                  type="text"
                  variant="compact"
                  value={modelDraft}
                  onChange={(event) => setModelDraft(event.target.value)}
                  onBlur={commitModel}
                  onKeyDown={blurOnEnter}
                  placeholder={t(
                    "settings.advanced.tts.composer.modelPlaceholder",
                  )}
                  disabled={busy}
                  className="min-w-[260px]"
                />
                {renderError("model")}
              </div>
            </SettingContainer>

            {providerInfo && (
              <p className="text-xs text-mid-gray px-3">
                {t("settings.advanced.tts.composer.dataFlowModel", {
                  provider: providerInfo.label,
                  baseUrl: providerInfo.base_url,
                })}
              </p>
            )}
            <p className="text-xs text-mid-gray px-3">
              {t("settings.advanced.tts.composer.latencyModel")}
            </p>
          </>
        )}

        {saved.mode === "command" && (
          <>
            <SettingContainer
              title={t("settings.advanced.tts.composer.commandPathLabel")}
              description={t(
                "settings.advanced.tts.composer.commandPathDescription",
              )}
              {...rowDescription}
            >
              <Input
                type="text"
                variant="compact"
                value={commandDraft.path}
                onChange={(event) => {
                  const value = event.target.value;
                  setCommandDraft((d) => editCommandDraft(d, "path", value));
                }}
                placeholder={t(
                  "settings.advanced.tts.composer.commandPathPlaceholder",
                )}
                disabled={busy}
                className="min-w-[320px]"
              />
            </SettingContainer>

            <SettingContainer
              title={t("settings.advanced.tts.composer.commandArgsLabel")}
              description={t(
                "settings.advanced.tts.composer.commandArgsDescription",
              )}
              {...rowDescription}
            >
              <Input
                type="text"
                variant="compact"
                value={commandDraft.args}
                onChange={(event) => {
                  const value = event.target.value;
                  setCommandDraft((d) => editCommandDraft(d, "args", value));
                }}
                placeholder={t(
                  "settings.advanced.tts.composer.commandArgsPlaceholder",
                )}
                disabled={busy}
                className="min-w-[320px]"
              />
            </SettingContainer>

            <SettingContainer
              title={t("settings.advanced.tts.composer.promptFlagLabel")}
              description={t(
                "settings.advanced.tts.composer.promptFlagDescription",
              )}
              {...rowDescription}
            >
              <Input
                type="text"
                variant="compact"
                value={commandDraft.flag}
                onChange={(event) => {
                  const value = event.target.value;
                  setCommandDraft((d) => editCommandDraft(d, "flag", value));
                }}
                placeholder={t(
                  "settings.advanced.tts.composer.promptFlagPlaceholder",
                )}
                disabled={busy}
                className="w-32"
              />
            </SettingContainer>

            <div className="py-2 flex flex-col gap-2">
              <ToggleSwitch
                checked={commandDraft.acknowledge}
                onChange={(checked) =>
                  setCommandDraft((d) => ({ ...d, acknowledge: checked }))
                }
                disabled={busy}
                label={t("settings.advanced.tts.composer.ackLabel")}
                description={t("settings.advanced.tts.composer.ackDescription")}
                descriptionMode={descriptionMode}
                grouped={grouped}
              />
              <p role="note" className="px-3 text-xs text-warning">
                {t("settings.advanced.tts.composer.ackWarning")}
              </p>
            </div>

            <div className="px-3 p-2 flex flex-col gap-2">
              <div className="flex items-center gap-3">
                <Button
                  variant="secondary"
                  size="md"
                  onClick={saveCommand}
                  disabled={!commandDirty || busy}
                >
                  {t("settings.advanced.tts.composer.saveCommand")}
                </Button>
                <span className="text-xs text-mid-gray">
                  {t("settings.advanced.tts.composer.readinessLabel")}{" "}
                  {t(`settings.advanced.tts.composer.readiness.${readiness}`)}
                </span>
              </div>
              {renderError("command")}
            </div>

            {saved.command_path !== "" && (
              <p className="text-xs text-mid-gray px-3">
                {t("settings.advanced.tts.composer.dataFlowCommand", {
                  path: saved.command_path,
                })}
              </p>
            )}
            <p className="text-xs text-mid-gray px-3">
              {t("settings.advanced.tts.composer.latencyCommand")}
            </p>
          </>
        )}

        {saved.mode !== "off" && saved.send_user_utterance && (
          <p className="text-xs text-mid-gray px-3">
            {t("settings.advanced.tts.composer.dataFlowUtterance")}
          </p>
        )}

        <SettingContainer
          title={t("settings.advanced.tts.composer.styleLabel")}
          description={t("settings.advanced.tts.composer.styleDescription")}
          {...rowDescription}
        >
          <div className="flex flex-col">
            <Input
              type="text"
              variant="compact"
              value={styleDraft}
              onChange={(event) => setStyleDraft(event.target.value)}
              onBlur={commitStyle}
              onKeyDown={blurOnEnter}
              placeholder={t("settings.advanced.tts.composer.stylePlaceholder")}
              disabled={busy}
              className="min-w-[320px]"
            />
            {renderError("style")}
          </div>
        </SettingContainer>

        <SettingContainer
          title={t("settings.advanced.tts.composer.timeoutLabel")}
          description={t("settings.advanced.tts.composer.timeoutDescription")}
          {...rowDescription}
        >
          <div className="flex flex-col">
            <div className="flex items-center gap-2">
              <Input
                type="number"
                variant="compact"
                min={TIMEOUT_MIN_SECS}
                max={TIMEOUT_MAX_SECS}
                step={1}
                value={timeoutDraft}
                onChange={(event) => setTimeoutDraft(event.target.value)}
                onBlur={commitTimeout}
                onKeyDown={blurOnEnter}
                disabled={busy}
                className="w-24"
              />
              <span className="text-xs text-mid-gray">
                {t("settings.advanced.tts.composer.timeoutHint")}
              </span>
            </div>
            {renderError("timeout")}
          </div>
        </SettingContainer>

        <ToggleSwitch
          checked={saved.send_user_utterance}
          onChange={handleSendUtteranceChange}
          isUpdating={busy}
          label={t("settings.advanced.tts.composer.sendUtteranceLabel")}
          description={t(
            "settings.advanced.tts.composer.sendUtteranceDescription",
          )}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
        {renderError("send")}

        <SettingContainer
          title={t("settings.advanced.tts.composer.fallbackLabel")}
          description={t("settings.advanced.tts.composer.fallbackDescription")}
          {...rowDescription}
        >
          <div className="flex flex-col">
            <Select
              value={saved.fallback}
              options={fallbackOptions}
              onChange={handleFallbackChange}
              isClearable={false}
              disabled={busy}
              className="min-w-[220px]"
            />
            {renderError("fallback")}
          </div>
        </SettingContainer>

        <SettingContainer
          title={t("settings.advanced.tts.composer.previewLabel")}
          description={t("settings.advanced.tts.composer.previewDescription")}
          {...rowDescription}
        >
          <div className="flex flex-col gap-2">
            <Button
              variant="secondary"
              size="md"
              onClick={() => void handlePreview()}
              disabled={previewing}
            >
              {previewing
                ? t("settings.advanced.tts.composer.previewRunning")
                : t("settings.advanced.tts.composer.previewButton")}
            </Button>
            {preview && (
              <div className="text-sm">
                <p>
                  <span className="text-mid-gray">
                    {t("settings.advanced.tts.composer.previewSpeech")}
                  </span>{" "}
                  <span>{preview.speech}</span>
                </p>
                <p>
                  <span className="text-mid-gray">
                    {t("settings.advanced.tts.composer.previewEmotion")}
                  </span>{" "}
                  <span>
                    {t(
                      `settings.advanced.tts.composer.emotion.${preview.emotion}`,
                    )}
                  </span>
                </p>
              </div>
            )}
            {previewError && (
              <p role="alert" className="text-xs text-danger">
                {previewError}
              </p>
            )}
          </div>
        </SettingContainer>

        <p aria-live="polite" className="text-sm px-3">
          <span className="text-mid-gray">
            {t("settings.advanced.tts.composer.statusLabel")}
          </span>{" "}
          <span>
            {lastOutcome === null
              ? t("settings.advanced.tts.composer.statusNone")
              : t(statusLabelKey(lastOutcome))}
          </span>
        </p>
      </>
    );
  },
);

SpokenReplyStyle.displayName = "SpokenReplyStyle";
