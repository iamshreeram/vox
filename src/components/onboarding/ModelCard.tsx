import React from "react";
import { useTranslation } from "react-i18next";
import {
  AudioLines,
  Check,
  Download,
  Globe,
  HardDrive,
  Languages,
  Loader2,
} from "lucide-react";
import type { ModelInfo } from "@/bindings";
import { formatModelSize } from "../../lib/utils/format";
import {
  getTranslatedModelDescription,
  getTranslatedModelName,
} from "../../lib/utils/modelTranslation";
import {
  getLanguageLabel,
  getUniqueCapabilityLanguages,
} from "../../lib/constants/languages";
import ModelRow, {
  type ModelRowBadge,
  type ModelRowLabels,
} from "@/components/settings/models/ModelRow";
import type { RowStatus } from "@/components/settings/models/modelRowState";
import { useSettingsStore } from "@/stores/settingsStore";

// Get display text for model's language support
const getLanguageDisplayText = (
  supportedLanguages: string[],
  t: (key: string, options?: Record<string, unknown>) => string,
): string => {
  const capabilityLanguages = getUniqueCapabilityLanguages(supportedLanguages);
  if (capabilityLanguages.length === 1) {
    const langCode = capabilityLanguages[0];
    const langName = getLanguageLabel(langCode) || langCode;
    return t("modelSelector.capabilities.languageOnly", { language: langName });
  }
  return t("modelSelector.capabilities.languageCount", {
    total: capabilityLanguages.length,
  });
};

// Legacy = a blob (Url-sourced) .bin/ONNX model, kept runnable but no longer the
// advertised download (catalog GGUFs supersede it).
export const isLegacySource = (model: ModelInfo): boolean =>
  typeof model.source === "object" && "Url" in model.source;

// Extract a GGUF quantization label from a filename, if present (e.g. "Q8_0").
const getQuantLabel = (filename: string): string | null => {
  const match = filename.match(
    /[._-](IQ\d+_\w+|Q\d+(?:_\w+)?|F16|BF16|F32)\.gguf$/i,
  );
  return match ? match[1].toUpperCase() : null;
};

export type ModelCardStatus =
  | "downloadable"
  | "downloading"
  | "verifying"
  | "extracting"
  | "switching"
  | "active"
  | "available";

// Map the STT card status onto the shared row status (DESIGN.md Rev 5, item 4).
const toRowStatus = (status: ModelCardStatus): RowStatus => {
  switch (status) {
    case "downloadable":
      return "notInstalled";
    case "available":
      return "ready";
    default:
      return status;
  }
};

interface ModelCardProps {
  model: ModelInfo;
  variant?: "default" | "featured";
  status?: ModelCardStatus;
  disabled?: boolean;
  className?: string;
  onSelect: (modelId: string) => void;
  onDownload?: (modelId: string) => void;
  onDelete?: (modelId: string) => void;
  onCancel?: (modelId: string) => void;
  downloadProgress?: number;
  downloadSpeed?: number; // MB/s
  showRecommended?: boolean;
}

const ModelCard: React.FC<ModelCardProps> = ({
  model,
  variant = "default",
  status = "downloadable",
  disabled = false,
  className = "",
  onSelect,
  onDownload,
  onDelete,
  onCancel,
  downloadProgress,
  downloadSpeed,
  showRecommended = true,
}) => {
  const { t } = useTranslation();
  const debugMode = useSettingsStore(
    (state) => state.settings?.debug_mode ?? false,
  );

  // Get translated model name and description
  const displayName = getTranslatedModelName(model, t);
  const displayDescription = getTranslatedModelDescription(model, t);
  const showModelSize =
    status === "downloadable" || status === "available" || status === "active";
  const formattedModelSize = formatModelSize(Number(model.size_mb));
  const quantLabel = getQuantLabel(model.filename);
  const capabilityLanguages = getUniqueCapabilityLanguages(
    model.supported_languages,
  );

  const badges: ModelRowBadge[] = [];
  if (showRecommended && model.is_recommended) {
    badges.push({ label: t("onboarding.recommended"), variant: "primary" });
  }
  if (status === "active") {
    badges.push({
      label: t("modelSelector.active"),
      variant: "primary",
      icon: <Check className="w-3 h-3 mr-1" />,
    });
  }
  if (model.is_custom) {
    badges.push({ label: t("modelSelector.custom"), variant: "secondary" });
  }
  if (isLegacySource(model)) {
    badges.push({ label: t("modelSelector.legacy"), variant: "secondary" });
  }
  if (status === "switching") {
    badges.push({
      label: t("modelSelector.switching"),
      variant: "secondary",
      icon: <Loader2 className="w-3 h-3 mr-1 animate-spin" />,
    });
  }

  const labels: ModelRowLabels = {
    select: t("settings.models.row.select"),
    active: t("modelSelector.active"),
    download: t("settings.models.row.download"),
    cancel: t("modelSelector.cancel"),
    cancelDownload: t("modelSelector.cancelDownload"),
    delete: t("common.delete"),
    retry: t("settings.models.row.retry"),
    edit: t("settings.models.row.edit"),
    verifying: t("modelSelector.verifyingGeneric"),
    extracting: t("modelSelector.extractingGeneric"),
    switching: t("modelSelector.switching"),
    userAdded: t("settings.models.row.userAdded"),
    progress: t("settings.models.row.progress"),
    downloading: (percentage) => t("modelSelector.downloading", { percentage }),
    speed: (speed) => t("modelSelector.downloadSpeed", { speed }),
    deleteModel: (modelName) => t("modelSelector.deleteModel", { modelName }),
  };

  const tags = (
    <>
      {capabilityLanguages.length > 0 && (
        <div
          className="flex items-center gap-1 text-xs text-text/50"
          title={
            capabilityLanguages.length === 1
              ? t("modelSelector.capabilities.singleLanguage")
              : t("modelSelector.capabilities.languageSelection")
          }
        >
          <Globe className="w-3.5 h-3.5" />
          <span>{getLanguageDisplayText(model.supported_languages, t)}</span>
        </div>
      )}
      {model.supports_translation && (
        <div
          className="flex items-center gap-1 text-xs text-text/50"
          title={t("modelSelector.capabilities.translation")}
        >
          <Languages className="w-3.5 h-3.5" />
          <span>{t("modelSelector.capabilities.translate")}</span>
        </div>
      )}
      {model.supports_streaming && (
        <div
          className="flex items-center gap-1 text-xs text-text/50"
          title={t("modelSelector.capabilities.streaming")}
        >
          <AudioLines className="w-3.5 h-3.5" />
          <span>{t("modelSelector.streaming")}</span>
        </div>
      )}
    </>
  );

  const meta = showModelSize ? (
    <>
      {status === "downloadable" ? (
        <Download className="w-3.5 h-3.5" />
      ) : (
        <HardDrive className="w-3.5 h-3.5" />
      )}
      <span>{formattedModelSize}</span>
      {debugMode && quantLabel && (
        <span className="text-text/40">{quantLabel}</span>
      )}
    </>
  ) : undefined;

  const aside =
    model.accuracy_score > 0 || model.speed_score > 0 ? (
      <div className="hidden sm:flex items-center ms-4">
        <div className="space-y-1">
          <div className="flex items-center gap-2">
            <p className="text-xs text-text/60 w-24 text-end">
              {t("onboarding.modelCard.accuracy")}
            </p>
            <div className="w-16 h-1.5 bg-mid-gray/20 rounded-full overflow-hidden">
              <div
                className="h-full bg-logo-primary rounded-full"
                style={{ width: `${model.accuracy_score * 100}%` }}
              />
            </div>
          </div>
          <div className="flex items-center gap-2">
            <p className="text-xs text-text/60 w-24 text-end">
              {t("onboarding.modelCard.speed")}
            </p>
            <div className="w-16 h-1.5 bg-mid-gray/20 rounded-full overflow-hidden">
              <div
                className="h-full bg-logo-primary rounded-full"
                style={{ width: `${model.speed_score * 100}%` }}
              />
            </div>
          </div>
        </div>
      </div>
    ) : undefined;

  return (
    <ModelRow
      id={model.id}
      name={displayName}
      description={displayDescription}
      badges={badges}
      status={toRowStatus(status)}
      progress={downloadProgress}
      speedMbPerSecond={downloadSpeed}
      variant={variant}
      className={className}
      disabled={disabled}
      rowSelect
      statusBadges={false}
      labels={labels}
      onSelect={onSelect}
      onDownload={onDownload}
      onDelete={onDelete}
      onCancel={onCancel}
      tags={tags}
      meta={meta}
      aside={aside}
    />
  );
};

export default ModelCard;
