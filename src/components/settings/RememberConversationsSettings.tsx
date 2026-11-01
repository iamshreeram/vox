import React, { useCallback, useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import { useSettings } from "../../hooks/useSettings";
import { Button } from "../ui/Button";
import { SettingContainer } from "../ui/SettingContainer";
import { ToggleSwitch } from "../ui/ToggleSwitch";

interface RememberConversationsSettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * "Remember recent conversations" toggle (on by default for new installs), plus the clear control and the count of
 * remembered summaries. Separate from the transcript-saving toggles above it: that feature
 * writes records; this one keeps short summaries that are sent back to the model later.
 * The clear uses an inline two-step confirm (no native dialog). A failed count read shows
 * nothing; the rest of the group stays usable.
 */
export const RememberConversationsSettings: React.FC<RememberConversationsSettingsProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const enabled = getSetting("conversation_memory_enabled") ?? true;

    // null = unknown (the count command failed or has not answered yet).
    const [count, setCount] = useState<number | null>(null);
    const [confirming, setConfirming] = useState(false);
    const [clearing, setClearing] = useState(false);
    const [clearFailed, setClearFailed] = useState(false);
    const confirmId = useId();

    const refreshCount = useCallback(async () => {
      try {
        setCount(await commands.conversationMemoryCount());
      } catch (error) {
        console.error("Failed to read remembered conversation count:", error);
        setCount(null);
      }
    }, []);

    useEffect(() => {
      void refreshCount();
    }, [refreshCount]);

    const handleToggle = async (next: boolean) => {
      await updateSetting("conversation_memory_enabled", next);
      void refreshCount();
    };

    const runClear = async () => {
      setClearing(true);
      setClearFailed(false);
      let failed = false;
      try {
        const result = await commands.clearConversationMemory();
        if (result.status === "error") {
          console.error(
            "Failed to clear remembered conversations:",
            result.error,
          );
          failed = true;
        }
      } catch (error) {
        console.error("Failed to clear remembered conversations:", error);
        failed = true;
      } finally {
        setClearing(false);
        setConfirming(false);
      }
      setClearFailed(failed);
      // Re-read the count either way so the line shows what is actually stored.
      await refreshCount();
    };

    const clearDisabled = clearing || count === 0;

    return (
      <>
        <ToggleSwitch
          checked={enabled}
          onChange={(next) => void handleToggle(next)}
          isUpdating={isUpdating("conversation_memory_enabled")}
          label={t("settings.experimental.rememberConversations.label")}
          description={t(
            "settings.experimental.rememberConversations.description",
          )}
          descriptionMode={descriptionMode}
          grouped={grouped}
          tooltipPosition="bottom"
        />
        <SettingContainer
          title={t("settings.experimental.rememberConversations.clearTitle")}
          description={t(
            "settings.experimental.rememberConversations.clearDescription",
          )}
          descriptionMode={descriptionMode}
          grouped={grouped}
          layout="stacked"
        >
          <div className="flex flex-col gap-2">
            {count !== null && (
              <p className="text-sm" role="status" aria-live="polite">
                {t("settings.experimental.rememberConversations.countLabel", {
                  count,
                })}
              </p>
            )}
            {confirming ? (
              <div
                role="group"
                aria-labelledby={confirmId}
                className="flex flex-col gap-2"
              >
                <p id={confirmId} className="text-sm">
                  {t(
                    "settings.experimental.rememberConversations.clearConfirm",
                  )}
                </p>
                <div className="flex items-center gap-2">
                  <Button
                    variant="danger"
                    size="md"
                    onClick={() => void runClear()}
                    disabled={clearing}
                  >
                    {t(
                      "settings.experimental.rememberConversations.clearConfirmAction",
                    )}
                  </Button>
                  <Button
                    variant="secondary"
                    size="md"
                    onClick={() => setConfirming(false)}
                    disabled={clearing}
                  >
                    {t(
                      "settings.experimental.rememberConversations.clearCancel",
                    )}
                  </Button>
                </div>
              </div>
            ) : (
              <Button
                variant="danger-ghost"
                size="md"
                onClick={() => {
                  setClearFailed(false);
                  setConfirming(true);
                }}
                disabled={clearDisabled}
                className="self-start"
              >
                {t("settings.experimental.rememberConversations.clearButton")}
              </Button>
            )}
            {clearFailed && (
              <p role="alert" className="text-xs text-danger">
                {t("settings.experimental.rememberConversations.clearFailed")}
              </p>
            )}
          </div>
        </SettingContainer>
      </>
    );
  });
