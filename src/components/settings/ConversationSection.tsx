import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { commands } from "@/bindings";
import { Button } from "../ui/Button";
import { Dropdown } from "../ui/Dropdown";
import { SettingContainer } from "../ui/SettingContainer";
import { Slider } from "../ui/Slider";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";
import { useOsType } from "../../hooks/useOsType";
import { formatKeyCombination } from "../../lib/utils/keyboard";
import {
  CONVERSE_BARGE_IN_VALUES,
  normalizeConverseBargeIn,
  type ConverseBargeIn,
} from "./converseBargeIn";
import {
  CONVERSE_LIMITS,
  buttonAction,
  clampSetting,
  endLabelKey,
  isSessionActive,
  parseConverseState,
  stateLabelKey,
  type ConverseSetting,
  type ConverseStateEvent,
} from "./converseForm";

interface ConversationSectionProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

const CONVERSE_EVENT = "converse-state";
const CONVERSE_ERROR_EVENT = "converse-error";

type ConverseNumericKey =
  | "converse_silence_ms"
  | "converse_max_idle_turns"
  | "converse_context_turns";

/**
 * Hands-free conversation: start/stop, the live state, the last end reason,
 * the three conversation tuning numbers, and the current shortcut. Event
 * payloads are untrusted and parsed in converseForm.ts.
 */
export const ConversationSection: React.FC<ConversationSectionProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const osType = useOsType();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const [active, setActive] = useState(false);
    const [state, setState] = useState<ConverseStateEvent>({
      state: "idle",
      end: null,
    });
    const [busy, setBusy] = useState(false);
    // True while the session is draining; cleared only by the final idle event.
    const [stopping, setStopping] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [chatWindowError, setChatWindowError] = useState<string | null>(null);

    const converseBinding = getSetting("bindings")?.converse;
    const bargeIn = normalizeConverseBargeIn(getSetting("converse_barge_in"));
    const chatLogEnabled = getSetting("converse_chat_log") ?? true;
    const chatOpenOnStartEnabled =
      getSetting("converse_chat_open_on_start") ?? true;

    useEffect(() => {
      let cancelled = false;
      const unlisteners: UnlistenFn[] = [];

      const subscribe = async () => {
        const stateUnlisten = await listen<unknown>(CONVERSE_EVENT, (event) => {
          const parsed = parseConverseState(event.payload);
          setState(parsed);
          setActive(isSessionActive(parsed.state));
          if (parsed.state === "idle") {
            setStopping(false);
          }
        });
        const errorUnlisten = await listen<unknown>(
          CONVERSE_ERROR_EVENT,
          (event) => {
            setError(
              typeof event.payload === "string"
                ? event.payload
                : t("converse.section.errors.unknown"),
            );
          },
        );
        if (cancelled) {
          stateUnlisten();
          errorUnlisten();
        } else {
          unlisteners.push(stateUnlisten, errorUnlisten);
        }
      };
      void subscribe();

      commands.converseStatus().then(
        (status) => {
          if (!cancelled) {
            setActive(status === "running" || status === "stopping");
            if (status === "running") {
              setState({ state: "listening", end: null });
            }
            if (status === "stopping") {
              setStopping(true);
            }
          }
        },
        () => {
          // Status is informational; the event stream keeps it current.
        },
      );

      return () => {
        cancelled = true;
        unlisteners.forEach((unlisten) => unlisten());
      };
    }, [t]);

    const action = buttonAction(active, busy);
    const buttonDisabled = action === "disabled" || stopping;

    const handleOpenChatWindow = async () => {
      setChatWindowError(null);
      const result = await commands.toggleChatWindowCommand();
      if (result.status === "error") {
        setChatWindowError(result.error);
      }
    };

    const handleClick = async () => {
      if (action === "disabled") return;
      setBusy(true);
      setError(null);
      try {
        if (action === "start") {
          const result = await commands.converseStart();
          if (result.status === "error") {
            setError(result.error);
          } else {
            setActive(true);
          }
        } else {
          const stopped = await commands.converseStop();
          if (stopped) {
            setStopping(true);
          } else {
            setActive(false);
          }
        }
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setBusy(false);
      }
    };

    const stateKey = stateLabelKey(state.state);
    const endKey = endLabelKey(state.end);

    const numericSettings: {
      setting: ConverseSetting;
      key: ConverseNumericKey;
      label: string;
      description: string;
      unit: string;
    }[] = [
      {
        setting: "silenceMs",
        key: "converse_silence_ms",
        label: t("converse.section.silenceLabel"),
        description: t("converse.section.silenceDescription"),
        unit: t("converse.section.silenceUnit"),
      },
      {
        setting: "idleTurns",
        key: "converse_max_idle_turns",
        label: t("converse.section.idleTurnsLabel"),
        description: t("converse.section.idleTurnsDescription"),
        unit: t("converse.section.idleTurnsUnit"),
      },
      {
        setting: "contextTurns",
        key: "converse_context_turns",
        label: t("converse.section.contextTurnsLabel"),
        description: t("converse.section.contextTurnsDescription"),
        unit: t("converse.section.contextTurnsUnit"),
      },
    ];

    return (
      <>
        <SettingContainer
          title={t("converse.section.title")}
          description={t("converse.section.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        >
          <div className="flex flex-col gap-2">
            <Button
              variant={active ? "danger" : "primary"}
              size="md"
              onClick={() => void handleClick()}
              disabled={buttonDisabled}
            >
              {active
                ? t("converse.section.stopButton")
                : t("converse.section.startButton")}
            </Button>
            <p aria-live="polite" className="text-sm">
              <span className="text-mid-gray">
                {t("converse.section.stateLabel")}
              </span>{" "}
              <span>{t(stateKey)}</span>
            </p>
            {!active && endKey !== null && (
              <p className="text-xs text-mid-gray">
                {t("converse.section.endLabel")} {t(endKey)}
              </p>
            )}
            {error !== null && (
              <p role="alert" className="text-xs text-red-400">
                {error}
              </p>
            )}
          </div>
        </SettingContainer>

        {numericSettings.map((item) => {
          const limit = CONVERSE_LIMITS[item.setting];
          const value = clampSetting(
            item.setting,
            getSetting(item.key) ?? limit.default,
          );
          return (
            <Slider
              key={item.key}
              value={value}
              onChange={(next) =>
                void updateSetting(item.key, clampSetting(item.setting, next))
              }
              min={limit.min}
              max={limit.max}
              step={limit.step}
              label={item.label}
              description={item.description}
              descriptionMode={descriptionMode}
              grouped={grouped}
              formatValue={(v) =>
                t("converse.section.valueFormat", {
                  value: Math.round(v),
                  unit: item.unit,
                })
              }
            />
          );
        })}

        <SettingContainer
          title={t("converse.section.bargeIn.label")}
          description={t("converse.section.bargeIn.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        >
          <Dropdown
            options={CONVERSE_BARGE_IN_VALUES.map((mode) => ({
              value: mode,
              label: t(`converse.section.bargeIn.options.${mode}`),
            }))}
            selectedValue={bargeIn}
            onSelect={(value) =>
              void updateSetting("converse_barge_in", value as ConverseBargeIn)
            }
            disabled={isUpdating("converse_barge_in")}
          />
        </SettingContainer>

        <SettingContainer
          title={t("converse.section.shortcutLabel")}
          description={t("converse.section.shortcutDescription")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        >
          <div className="px-2 py-1 text-sm font-semibold bg-mid-gray/10 border border-mid-gray/80 rounded-md">
            {converseBinding
              ? formatKeyCombination(converseBinding.current_binding, osType)
              : t("converse.section.shortcutNone")}
          </div>
        </SettingContainer>

        <SettingContainer
          title={t("settings.conversation.openChatWindow")}
          description={t("settings.conversation.chatLog.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        >
          <div className="flex flex-col gap-2">
            <Button
              variant="secondary"
              size="md"
              onClick={() => void handleOpenChatWindow()}
            >
              {t("settings.conversation.openChatWindow")}
            </Button>
            {chatWindowError !== null && (
              <p role="alert" className="text-xs text-red-400">
                {chatWindowError}
              </p>
            )}
          </div>
        </SettingContainer>

        <ToggleSwitch
          checked={chatLogEnabled}
          onChange={(next) => void updateSetting("converse_chat_log", next)}
          disabled={isUpdating("converse_chat_log")}
          isUpdating={isUpdating("converse_chat_log")}
          label={t("settings.conversation.chatLog.title")}
          description={t("settings.conversation.chatLog.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />

        <ToggleSwitch
          checked={chatOpenOnStartEnabled}
          onChange={(next) =>
            void updateSetting("converse_chat_open_on_start", next)
          }
          disabled={
            !chatLogEnabled || isUpdating("converse_chat_open_on_start")
          }
          isUpdating={isUpdating("converse_chat_open_on_start")}
          label={t("settings.conversation.chatOpenOnStart.label")}
          description={t("settings.conversation.chatOpenOnStart.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />

        <p className="text-xs text-mid-gray px-4">
          {t("converse.section.privacyNote")}
        </p>
        <p className="text-xs text-mid-gray px-4">
          {t("converse.section.wakeWordNote")}
        </p>
      </>
    );
  });

ConversationSection.displayName = "ConversationSection";
