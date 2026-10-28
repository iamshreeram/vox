import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { previewLines } from "./vaultRagForm";

interface VaultRagPreviewProps {
  enabled: boolean;
  onError: (text: string | null) => void;
}

/** Search preview for the vault index: plain text lines only, never HTML. */
export const VaultRagPreview: React.FC<VaultRagPreviewProps> = ({
  enabled,
  onError,
}) => {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const [preview, setPreview] = useState<string[] | null>(null);

  const runPreview = async (): Promise<void> => {
    const result = await commands.vaultSearchPreview(query);
    setPreview(result.status === "ok" ? previewLines(result.data) : null);
    onError(result.status === "error" ? result.error : null);
  };

  return (
    <div className="flex flex-col gap-2 px-3 py-3">
      <div className="flex gap-2">
        <Input
          type="text"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder={t("settings.experimental.vaultRag.preview.placeholder")}
          variant="compact"
          disabled={!enabled}
          className="flex-1"
        />
        <Button
          variant="secondary"
          size="sm"
          disabled={!enabled || query.trim() === ""}
          onClick={() => runPreview()}
        >
          {t("settings.experimental.vaultRag.preview.label")}
        </Button>
      </div>
      {preview !== null && preview.length === 0 && (
        <p className="text-xs text-mid-gray">
          {t("settings.experimental.vaultRag.preview.empty")}
        </p>
      )}
      {preview !== null && preview.length > 0 && (
        <ul className="flex flex-col gap-1 font-mono text-xs">
          {preview.map((line, index) => (
            <li key={`${index}-${line}`}>{line}</li>
          ))}
        </ul>
      )}
    </div>
  );
};
