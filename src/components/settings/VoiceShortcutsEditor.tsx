import React, { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type VoiceShortcut } from "@/bindings";
import { useSettings } from "../../hooks/useSettings";
import { Input } from "../ui/Input";
import { Button } from "../ui/Button";
import { SettingContainer } from "../ui/SettingContainer";
import {
  addRow,
  changeActionType,
  actionTarget,
  isDirty,
  MAX_VOICE_SHORTCUTS,
  removeRow,
  setActionTarget,
  updateRow,
  type VoiceShortcutActionType,
} from "../../lib/utils/voiceShortcutsDraft";

interface VoiceShortcutsEditorProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

const ACTION_TYPES: VoiceShortcutActionType[] = [
  "open_url",
  "open_path",
  "open_app",
];

const TARGET_PLACEHOLDER_KEYS: Record<VoiceShortcutActionType, string> = {
  open_url: "settings.advanced.voiceShortcuts.placeholder.url",
  open_path: "settings.advanced.voiceShortcuts.placeholder.folder",
  open_app: "settings.advanced.voiceShortcuts.placeholder.app",
};

const ACTION_LABEL_KEYS: Record<VoiceShortcutActionType, string> = {
  open_url: "settings.advanced.voiceShortcuts.actionType.url",
  open_path: "settings.advanced.voiceShortcuts.actionType.folder",
  open_app: "settings.advanced.voiceShortcuts.actionType.app",
};

export const VoiceShortcutsEditor: React.FC<VoiceShortcutsEditorProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { settings, getSetting, refreshSettings } = useSettings();
    const voiceCommandsEnabled = getSetting("voice_commands_enabled") ?? false;
    const saved = useMemo<VoiceShortcut[]>(
      () => settings?.voice_shortcuts ?? [],
      [settings],
    );

    const [draft, setDraft] = useState<VoiceShortcut[]>(saved);
    const [isSaving, setIsSaving] = useState(false);
    const [error, setError] = useState<string | null>(null);

    // Re-sync from the saved copy only when the user has no unsaved edits, so
    // a background settings refresh never discards in-progress typing.
    useEffect(() => {
      setDraft((current) => (isDirty(saved, current) ? current : saved));
    }, [saved]);

    const dirty = isDirty(saved, draft);
    const atLimit = draft.length >= MAX_VOICE_SHORTCUTS;

    const handleSave = async () => {
      setIsSaving(true);
      setError(null);
      try {
        const result = await commands.setVoiceShortcuts(draft);
        if (result.status === "ok") {
          await refreshSettings();
        } else {
          setError(result.error);
        }
      } finally {
        setIsSaving(false);
      }
    };

    return (
      <SettingContainer
        title={t("settings.advanced.voiceShortcuts.title")}
        description={t("settings.advanced.voiceShortcuts.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
        disabled={!voiceCommandsEnabled}
        layout="stacked"
      >
        <div className="space-y-2">
          {draft.length === 0 && (
            <p className="text-sm text-mid-gray">
              {t("settings.advanced.voiceShortcuts.empty")}
            </p>
          )}
          {draft.map((shortcut, index) => (
            <div key={index} className="flex flex-wrap items-center gap-2">
              <Input
                type="text"
                className="w-40"
                value={shortcut.phrase}
                onChange={(e) =>
                  setDraft(updateRow(draft, index, { phrase: e.target.value }))
                }
                placeholder={t(
                  "settings.advanced.voiceShortcuts.phrasePlaceholder",
                )}
                aria-label={t("settings.advanced.voiceShortcuts.phraseLabel")}
                variant="compact"
                disabled={!voiceCommandsEnabled || isSaving}
              />
              <select
                className="px-2 py-1 text-sm rounded-md border border-mid-gray/80 bg-mid-gray/10"
                value={shortcut.action.type}
                onChange={(e) =>
                  setDraft(
                    updateRow(draft, index, {
                      action: changeActionType(
                        shortcut,
                        e.target.value as VoiceShortcutActionType,
                      ).action,
                    }),
                  )
                }
                aria-label={t("settings.advanced.voiceShortcuts.typeLabel")}
                disabled={!voiceCommandsEnabled || isSaving}
              >
                {ACTION_TYPES.map((type) => (
                  <option key={type} value={type}>
                    {t(ACTION_LABEL_KEYS[type])}
                  </option>
                ))}
              </select>
              <Input
                type="text"
                className="flex-1 min-w-48"
                value={actionTarget(shortcut)}
                onChange={(e) =>
                  setDraft(
                    updateRow(
                      draft,
                      index,
                      setActionTarget(shortcut, e.target.value),
                    ),
                  )
                }
                placeholder={t(TARGET_PLACEHOLDER_KEYS[shortcut.action.type])}
                aria-label={t("settings.advanced.voiceShortcuts.targetLabel")}
                variant="compact"
                disabled={!voiceCommandsEnabled || isSaving}
              />
              <Button
                onClick={() => setDraft(removeRow(draft, index))}
                disabled={!voiceCommandsEnabled || isSaving}
                variant="secondary"
                size="sm"
              >
                {t("settings.advanced.voiceShortcuts.remove")}
              </Button>
            </div>
          ))}
          <div className="flex items-center gap-2 pt-1">
            <Button
              onClick={() => setDraft(addRow(draft))}
              disabled={!voiceCommandsEnabled || isSaving || atLimit}
              variant="secondary"
              size="md"
            >
              {t("settings.advanced.voiceShortcuts.add")}
            </Button>
            <Button
              onClick={handleSave}
              disabled={!voiceCommandsEnabled || isSaving || !dirty}
              variant="primary"
              size="md"
            >
              {t("settings.advanced.voiceShortcuts.save")}
            </Button>
          </div>
          {error && (
            <p role="alert" className="text-sm text-red-500">
              {error}
            </p>
          )}
        </div>
      </SettingContainer>
    );
  });
