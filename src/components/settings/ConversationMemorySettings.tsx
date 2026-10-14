import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { Input } from "../ui/Input";
import { SettingContainer } from "../ui/SettingContainer";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";

interface ConversationMemoryRowProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const ConversationRecordsToggle: React.FC<ConversationMemoryRowProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const recordsEnabled = getSetting("conversation_records_enabled") ?? false;

    return (
      <ToggleSwitch
        checked={recordsEnabled}
        onChange={(enabled) =>
          updateSetting("conversation_records_enabled", enabled)
        }
        isUpdating={isUpdating("conversation_records_enabled")}
        label={t("settings.experimental.conversationMemory.recordsLabel")}
        description={t(
          "settings.experimental.conversationMemory.recordsDescription",
        )}
        descriptionMode={descriptionMode}
        grouped={grouped}
        tooltipPosition="bottom"
      />
    );
  });

export const ConversationVaultToggle: React.FC<ConversationMemoryRowProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const recordsEnabled = getSetting("conversation_records_enabled") ?? false;
    const vaultPath = getSetting("vault_path") ?? "";
    const vaultToggleEnabled =
      getSetting("conversation_records_to_vault") ?? false;

    return (
      <ToggleSwitch
        checked={vaultToggleEnabled}
        onChange={(enabled) =>
          updateSetting("conversation_records_to_vault", enabled)
        }
        isUpdating={isUpdating("conversation_records_to_vault")}
        disabled={!recordsEnabled || vaultPath.trim() === ""}
        label={t("settings.experimental.conversationMemory.vaultToggleLabel")}
        description={t(
          "settings.experimental.conversationMemory.vaultToggleDescription",
        )}
        descriptionMode={descriptionMode}
        grouped={grouped}
        tooltipPosition="bottom"
      />
    );
  });

export const ConversationVaultPathField: React.FC<ConversationMemoryRowProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const recordsEnabled = getSetting("conversation_records_enabled") ?? false;
    const vaultPath = getSetting("vault_path") ?? "";
    const [localValue, setLocalValue] = useState(vaultPath);

    React.useEffect(() => {
      setLocalValue(vaultPath);
    }, [vaultPath]);

    if (!recordsEnabled) return null;

    return (
      <SettingContainer
        title={t("settings.experimental.conversationMemory.vaultPathLabel")}
        description={t(
          "settings.experimental.conversationMemory.vaultPathDescription",
        )}
        descriptionMode={descriptionMode}
        grouped={grouped}
      >
        <Input
          type="text"
          value={localValue}
          onChange={(event) => setLocalValue(event.target.value)}
          onBlur={() => updateSetting("vault_path", localValue)}
          placeholder=""
          variant="compact"
          disabled={isUpdating("vault_path")}
          className="flex-1 min-w-[360px]"
        />
      </SettingContainer>
    );
  });

export const ConversationVaultContextToggle: React.FC<ConversationMemoryRowProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const recordsEnabled = getSetting("conversation_records_enabled") ?? false;
    const contextEnabled = getSetting("vault_context_enabled") ?? false;

    return (
      <div
        title={
          recordsEnabled
            ? undefined
            : t("settings.experimental.conversationMemory.recordsLabel")
        }
      >
        <ToggleSwitch
          checked={contextEnabled}
          onChange={(enabled) =>
            updateSetting("vault_context_enabled", enabled)
          }
          isUpdating={isUpdating("vault_context_enabled")}
          disabled={!recordsEnabled}
          label={t("settings.experimental.conversationMemory.contextLabel")}
          description={t(
            "settings.experimental.conversationMemory.contextDescription",
          )}
          descriptionMode={descriptionMode}
          grouped={grouped}
          tooltipPosition="bottom"
        />
      </div>
    );
  });
