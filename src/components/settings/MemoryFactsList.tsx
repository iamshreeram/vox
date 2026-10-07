import React, { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands, type Fact } from "@/bindings";
import { Button } from "../ui/Button";

/**
 * Minimal browser for Phase 1's Memory feature: lets a user see, add, and
 * delete the facts Vox has stored, using the existing memory* Tauri
 * commands. Only rendered while Memory is enabled (see MemoryToggle).
 */
export const MemoryFactsList: React.FC = () => {
  const { t } = useTranslation();
  const [facts, setFacts] = useState<Fact[]>([]);
  const [draft, setDraft] = useState("");
  const [isLoading, setIsLoading] = useState(false);

  const refresh = useCallback(async () => {
    const result = await commands.memoryListAll();
    if (result.status === "ok") {
      setFacts(result.data);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const handleAdd = async () => {
    const text = draft.trim();
    if (!text) return;
    setIsLoading(true);
    try {
      const result = await commands.memoryRemember(text);
      if (result.status === "ok") {
        setDraft("");
        await refresh();
        toast.success(t("settings.advanced.memory.rememberedToast"));
      } else {
        toast.error(result.error);
      }
    } finally {
      setIsLoading(false);
    }
  };

  const handleForget = async (id: number) => {
    const result = await commands.memoryForget(id);
    if (result.status === "ok") {
      await refresh();
    }
  };

  return (
    <div className="px-4 py-3 space-y-3">
      <div className="flex gap-2">
        <input
          type="text"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") handleAdd();
          }}
          placeholder={t("settings.advanced.memory.addPlaceholder")}
          className="flex-1 px-3 py-1.5 text-sm rounded-lg border border-mid-gray/20 bg-background focus:outline-none focus:ring-1 focus:ring-logo-primary"
        />
        <Button
          variant="secondary"
          size="sm"
          onClick={handleAdd}
          disabled={isLoading || !draft.trim()}
        >
          {t("settings.advanced.memory.addButton")}
        </Button>
      </div>

      {facts.length === 0 ? (
        <p className="text-xs text-mid-gray">
          {t("settings.advanced.memory.empty")}
        </p>
      ) : (
        <ul className="space-y-1.5 max-h-64 overflow-y-auto">
          {facts.map((fact) => (
            <li
              key={fact.id}
              className="flex items-center justify-between gap-2 text-sm bg-mid-gray/5 rounded-lg px-3 py-1.5"
            >
              <span className="truncate">{fact.text}</span>
              <Button
                variant="danger-ghost"
                size="sm"
                onClick={() => handleForget(fact.id)}
              >
                {t("settings.advanced.memory.forgetButton")}
              </Button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
};
