import React, { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Lock } from "lucide-react";
import {
  commands,
  type NaturalVoiceOption,
  type PersonaView,
} from "@/bindings";
import { Button } from "../../ui/Button";
import { Dropdown } from "../../ui/Dropdown";
import { Input } from "../../ui/Input";
import { SettingContainer } from "../../ui/SettingContainer";
import { Textarea } from "../../ui/Textarea";
import { useSettings } from "../../../hooks/useSettings";
import {
  PERSONA_LIMITS,
  personaErrorIssue,
  validatePersonaDraft,
  type PersonaIssue,
} from "./personaForm";

interface PersonaSettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

interface EditorState {
  /** Null for a new persona. */
  editingId: string | null;
  name: string;
  style: string;
  voice: string;
}

const NO_VOICE = "";

/**
 * Personas: a picker (apply one to the current settings), and a custom
 * persona editor. Built-in personas are read-only. Backend rejections are
 * shown as translated text, never as raw codes.
 */
export const PersonaSettings: React.FC<PersonaSettingsProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const { refreshSettings } = useSettings();
  const [personas, setPersonas] = useState<PersonaView[]>([]);
  const [voices, setVoices] = useState<NaturalVoiceOption[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [issues, setIssues] = useState<(PersonaIssue | "generic")[]>([]);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setPersonas(await commands.listPersonas());
    } catch (err) {
      console.warn("Listing personas failed:", err);
    }
  }, []);

  useEffect(() => {
    void load();
    commands
      .listNaturalVoices()
      .then((list) => setVoices(list))
      .catch((err: unknown) => {
        console.warn("Listing natural voices failed:", err);
      });
  }, [load]);

  const customCount = personas.filter((view) => !view.builtin).length;
  const active = personas.find((view) => view.active) ?? null;
  const picked =
    personas.find((view) => view.persona.id === selectedId) ?? active;
  const limitReached = customCount >= PERSONA_LIMITS.maxCustom;

  const issueText = (issue: PersonaIssue | "generic"): string => {
    switch (issue) {
      case "name_empty":
        return t("converse.voice.persona.error.name_empty");
      case "name_too_long":
        return t("converse.voice.persona.error.name_too_long", {
          max: PERSONA_LIMITS.nameMaxChars,
        });
      case "style_empty":
        return t("converse.voice.persona.error.style_empty");
      case "style_too_long":
        return t("converse.voice.persona.error.style_too_long", {
          max: PERSONA_LIMITS.styleMaxChars,
        });
      case "control_chars":
        return t("converse.voice.persona.error.control_chars");
      case "unknown_voice":
        return t("converse.voice.persona.error.unknown_voice");
      case "limit_reached":
        return t("converse.voice.persona.error.limit_reached", {
          max: PERSONA_LIMITS.maxCustom,
        });
      case "builtin_immutable":
        return t("converse.voice.persona.error.builtin_immutable");
      default:
        return t("converse.voice.persona.error.generic");
    }
  };

  const apply = async () => {
    if (picked === null) return;
    setBusy(true);
    setIssues([]);
    try {
      const result = await commands.applyPersona(picked.persona.id);
      if (result.status === "error") {
        setIssues([personaErrorIssue(result.error)]);
      } else {
        await refreshSettings();
      }
    } catch (err) {
      console.warn("Applying the persona failed:", err);
      setIssues(["generic"]);
    } finally {
      setBusy(false);
      await load();
    }
  };

  const openEditor = (view: PersonaView | null) => {
    setIssues([]);
    setEditor(
      view === null
        ? { editingId: null, name: "", style: "", voice: NO_VOICE }
        : {
            editingId: view.persona.id,
            name: view.persona.name,
            style: view.persona.style,
            voice: view.persona.voice ?? NO_VOICE,
          },
    );
  };

  const save = async () => {
    if (editor === null) return;
    const editingView =
      editor.editingId === null
        ? null
        : (personas.find((view) => view.persona.id === editor.editingId) ??
          null);
    const check = validatePersonaDraft(
      { name: editor.name, style: editor.style, voice: editor.voice },
      {
        customCount,
        editingId: editor.editingId,
        isBuiltin: editingView?.builtin ?? false,
        knownVoices: voices.map((voice) => voice.id),
      },
    );
    if (!check.ok) {
      setIssues(check.issues);
      return;
    }
    setBusy(true);
    setIssues([]);
    try {
      const result = await commands.savePersona({
        id: editor.editingId ?? "",
        name: check.persona.name,
        style: check.persona.style,
        voice: check.persona.voice,
      });
      if (result.status === "error") {
        setIssues([personaErrorIssue(result.error)]);
      } else {
        setEditor(null);
      }
    } catch (err) {
      console.warn("Saving the persona failed:", err);
      setIssues(["generic"]);
    } finally {
      setBusy(false);
      await load();
    }
  };

  const remove = async (id: string) => {
    setBusy(true);
    setIssues([]);
    try {
      const result = await commands.deletePersona(id);
      if (result.status === "error") {
        setIssues([personaErrorIssue(result.error)]);
      } else if (editor?.editingId === id) {
        setEditor(null);
      }
    } catch (err) {
      console.warn("Deleting the persona failed:", err);
      setIssues(["generic"]);
    } finally {
      setBusy(false);
      await load();
    }
  };

  const issueList = issues.length > 0 && (
    <ul role="alert" className="flex flex-col gap-1 text-xs text-red-400">
      {issues.map((issue, index) => (
        <li key={`${issue}-${index}`}>{issueText(issue)}</li>
      ))}
    </ul>
  );

  return (
    <>
      <SettingContainer
        title={t("converse.voice.persona.label")}
        description={t("converse.voice.persona.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
        layout="stacked"
      >
        <div className="flex flex-col gap-2">
          <div className="flex items-center gap-2">
            <Dropdown
              options={personas.map((view) => ({
                value: view.persona.id,
                label: view.active
                  ? t("converse.voice.persona.activeOption", {
                      name: view.persona.name,
                    })
                  : view.persona.name,
              }))}
              selectedValue={picked?.persona.id ?? null}
              onSelect={(value) => setSelectedId(value)}
              disabled={busy || personas.length === 0}
              className="flex-1 min-w-[240px]"
            />
            <Button
              variant="primary"
              size="md"
              onClick={() => void apply()}
              disabled={busy || picked === null}
            >
              {t("converse.voice.persona.apply")}
            </Button>
          </div>
          <div className="flex flex-col gap-1">
            <span className="text-xs text-mid-gray">
              {t("converse.voice.persona.customHeading")}
            </span>
            {customCount === 0 && (
              <span className="text-xs text-mid-gray">
                {t("converse.voice.persona.customEmpty")}
              </span>
            )}
            {personas
              .filter((view) => !view.builtin)
              .map((view) => (
                <div
                  key={view.persona.id}
                  className="flex items-center justify-between gap-2 text-sm"
                >
                  <span>{view.persona.name}</span>
                  <div className="flex items-center gap-2">
                    <Button
                      variant="secondary"
                      size="sm"
                      onClick={() => openEditor(view)}
                      disabled={busy}
                    >
                      {t("converse.voice.persona.edit")}
                    </Button>
                    <Button
                      variant="danger-ghost"
                      size="sm"
                      onClick={() => void remove(view.persona.id)}
                      disabled={busy}
                    >
                      {t("converse.voice.persona.delete")}
                    </Button>
                  </div>
                </div>
              ))}
            {personas
              .filter((view) => view.builtin)
              .map((view) => (
                <div
                  key={view.persona.id}
                  className="flex items-center gap-2 text-sm text-mid-gray"
                >
                  <Lock aria-hidden="true" className="w-3.5 h-3.5 shrink-0" />
                  <span>{view.persona.name}</span>
                  <span className="text-xs">
                    {t("converse.voice.persona.builtinTag")}
                  </span>
                </div>
              ))}
            <p className="text-xs text-mid-gray">
              {t("converse.voice.persona.builtinLocked")}
            </p>
          </div>
          {limitReached ? (
            <p className="text-xs text-mid-gray">
              {t("converse.voice.persona.error.limit_reached", {
                max: PERSONA_LIMITS.maxCustom,
              })}
            </p>
          ) : (
            editor === null && (
              <Button
                variant="secondary"
                size="md"
                onClick={() => openEditor(null)}
                disabled={busy}
                className="self-start"
              >
                {t("converse.voice.persona.new")}
              </Button>
            )
          )}
          {editor !== null && (
            <div className="flex flex-col gap-2 rounded-lg border border-mid-gray/30 p-3">
              <label className="flex flex-col gap-1 text-sm">
                <span>{t("converse.voice.persona.nameLabel")}</span>
                <Input
                  value={editor.name}
                  onChange={(event) =>
                    setEditor({ ...editor, name: event.target.value })
                  }
                  disabled={busy}
                />
                <span className="text-xs text-mid-gray">
                  {t("converse.voice.persona.charCount", {
                    count: [...editor.name.trim()].length,
                    max: PERSONA_LIMITS.nameMaxChars,
                  })}
                </span>
              </label>
              <label className="flex flex-col gap-1 text-sm">
                <span>{t("converse.voice.persona.styleLabel")}</span>
                <Textarea
                  value={editor.style}
                  onChange={(event) =>
                    setEditor({ ...editor, style: event.target.value })
                  }
                  disabled={busy}
                  variant="compact"
                />
                <span className="text-xs text-mid-gray">
                  {t("converse.voice.persona.charCount", {
                    count: [...editor.style.trim()].length,
                    max: PERSONA_LIMITS.styleMaxChars,
                  })}
                </span>
              </label>
              {voices.length > 0 && (
                <div className="flex flex-col gap-1 text-sm">
                  <span>{t("converse.voice.persona.voiceLabel")}</span>
                  <Dropdown
                    options={[
                      {
                        value: NO_VOICE,
                        label: t("converse.voice.persona.noVoice"),
                      },
                      ...voices.map((voice) => ({
                        value: voice.id,
                        label: voice.name,
                      })),
                    ]}
                    selectedValue={editor.voice}
                    onSelect={(value) => setEditor({ ...editor, voice: value })}
                    disabled={busy}
                    className="min-w-[240px]"
                  />
                </div>
              )}
              {issueList}
              <div className="flex items-center gap-2">
                <Button
                  variant="primary"
                  size="md"
                  onClick={() => void save()}
                  disabled={busy}
                >
                  {t("converse.voice.persona.save")}
                </Button>
                <Button
                  variant="secondary"
                  size="md"
                  onClick={() => {
                    setEditor(null);
                    setIssues([]);
                  }}
                  disabled={busy}
                >
                  {t("converse.voice.persona.cancel")}
                </Button>
              </div>
            </div>
          )}
          {editor === null && issueList}
        </div>
      </SettingContainer>
    </>
  );
};
