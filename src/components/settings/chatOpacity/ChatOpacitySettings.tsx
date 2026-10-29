import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import { Button } from "../../ui/Button";
import { SettingContainer } from "../../ui/SettingContainer";
import { useSettings } from "../../../hooks/useSettings";
import {
  CHAT_OPACITY_DEFAULT,
  CHAT_OPACITY_MAX,
  CHAT_OPACITY_MIN,
  CHAT_OPACITY_STEP,
  clampChatOpacity,
  createOpacityWriter,
  sliderValue,
  type OpacityWriter,
} from "./chatOpacityForm";

interface ChatOpacitySettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/** Quiet period after the last slider move before the value is sent. */
const LIVE_DEBOUNCE_MS = 120;

/**
 * "Conversation window opacity": how see-through the conversation window is, in percent
 * opaque. Moving the slider updates the open window live (debounced); releasing it writes the
 * final value at once. Reset returns to the default.
 */
export const ChatOpacitySettings: React.FC<ChatOpacitySettingsProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, refreshSettings } = useSettings();
    const stored = getSetting("chat_opacity_pct") ?? null;

    const [shown, setShown] = useState<number>(sliderValue(stored));
    const writerRef = useRef<OpacityWriter | null>(null);
    if (writerRef.current === null) {
      writerRef.current = createOpacityWriter({
        send: (pct) => {
          const call = commands.changeChatOpacitySetting(pct);
          void call.catch(() => undefined).then(() => refreshSettings());
        },
        schedule: (run) => window.setTimeout(run, LIVE_DEBOUNCE_MS),
        cancel: (handle) => window.clearTimeout(handle as number),
      });
    }
    const writer = writerRef.current;

    // Follow the stored value, except while the user has a change waiting to be sent.
    useEffect(() => {
      if (!writer.hasPending()) setShown(sliderValue(stored));
    }, [stored, writer]);

    const label = t("settings.chatOpacity.value", { value: shown });

    return (
      <SettingContainer
        title={t("settings.chatOpacity.title")}
        description={t("settings.chatOpacity.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      >
        <div className="flex items-center gap-3">
          <input
            type="range"
            data-testid="chat-opacity-slider"
            aria-label={t("settings.chatOpacity.title")}
            aria-valuetext={label}
            className="w-40 cursor-pointer"
            min={CHAT_OPACITY_MIN}
            max={CHAT_OPACITY_MAX}
            step={CHAT_OPACITY_STEP}
            value={shown}
            onChange={(event) => {
              const next = clampChatOpacity(Number(event.target.value));
              setShown(next);
              writer.change(next);
            }}
            onPointerUp={() => writer.release()}
            onKeyUp={() => writer.release()}
          />
          <span
            data-testid="chat-opacity-value"
            className="w-16 text-end text-sm tabular-nums"
          >
            {label}
          </span>
          <Button
            variant="secondary"
            size="sm"
            data-testid="chat-opacity-reset"
            onClick={() => {
              setShown(CHAT_OPACITY_DEFAULT);
              writer.reset();
            }}
          >
            {t("settings.chatOpacity.reset")}
          </Button>
        </div>
      </SettingContainer>
    );
  });

ChatOpacitySettings.displayName = "ChatOpacitySettings";
