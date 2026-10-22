import React, { useId } from "react";
import { useTranslation } from "react-i18next";
import Badge from "../ui/Badge";
import { Button } from "../ui/Button";
import { SettingContainer } from "../ui/SettingContainer";
import type { TtsEngine } from "@/bindings";
import type { EngineBadge, EngineCard } from "./ttsEngineState";

interface TtsEngineCardProps {
  card: EngineCard;
  onSelect: () => void;
  children?: React.ReactNode;
}

/**
 * One row of the engine picker. Name, badges and the selection control come from the
 * pure card model; the body (children) is rendered only for a supported engine.
 */
export const TtsEngineCard: React.FC<TtsEngineCardProps> = ({
  card,
  onSelect,
  children,
}) => {
  const { t } = useTranslation();
  const titleId = useId();

  const name = (id: TtsEngine): string => {
    switch (id) {
      case "phonon":
        return t("settings.advanced.tts.engines.phonon.name");
      case "natural":
        return t("settings.advanced.tts.natural.engine.natural");
      case "system":
        return t("settings.advanced.tts.natural.engine.system");
      default:
        return String(id);
    }
  };

  const badgeText = (badge: EngineBadge): string => {
    switch (badge.kind) {
      case "recommended":
        return t("settings.advanced.tts.engines.badge.recommended");
      case "runtime":
        switch (badge.runtime) {
          case "pure_rust":
            return t("settings.advanced.tts.engines.runtime.pureRust");
          case "onnx_runtime":
            return t("settings.advanced.tts.engines.runtime.onnxRuntime");
          case "operating_system":
            return t("settings.advanced.tts.engines.runtime.operatingSystem");
          default:
            return String(badge.runtime);
        }
      case "english_only":
        return t("settings.advanced.tts.engines.badge.englishOnly");
      case "size":
        return t("settings.advanced.tts.engines.badge.size", {
          size: badge.label,
        });
      case "license":
        return t("settings.advanced.tts.engines.badge.license", {
          license: badge.license,
        });
    }
  };

  const selectable = card.selectable;
  const frameClass = card.selected
    ? "border-logo-primary bg-logo-primary/5"
    : "border-mid-gray/20";

  return (
    <section
      data-testid="tts-engine-card"
      data-engine={card.id}
      data-selected={card.selected ? "true" : "false"}
      aria-labelledby={titleId}
      className={`flex flex-col gap-3 rounded-lg border p-3 ${frameClass} ${selectable ? "" : "opacity-70"}`}
    >
      <div className="flex items-start justify-between gap-3">
        <div className="flex flex-col gap-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 id={titleId} className="text-sm font-medium">
              {name(card.id)}
            </h3>
            {card.badges.map((badge, index) => (
              <Badge
                key={`${badge.kind}-${index}`}
                variant={badge.kind === "recommended" ? "primary" : "secondary"}
              >
                {badgeText(badge)}
              </Badge>
            ))}
          </div>
          {card.disabledReason !== null && (
            <p className="text-xs text-mid-gray">
              {t("settings.advanced.tts.engines.unavailable")}
            </p>
          )}
          {card.selected && card.disabledReason !== null && (
            <p className="text-xs text-mid-gray">
              {t("settings.advanced.tts.engines.unsupportedSelected")}
            </p>
          )}
        </div>
        <Button
          variant={card.selected ? "primary" : "secondary"}
          size="md"
          disabled={!selectable || card.selected}
          onClick={onSelect}
          aria-pressed={card.selected}
        >
          {card.selected
            ? t("settings.advanced.tts.engines.selected")
            : t("settings.advanced.tts.engines.select")}
        </Button>
      </div>
      {selectable && children}
    </section>
  );
};

export interface PhononEngineBodyProps {
  card: EngineCard;
  descriptionMode: "inline" | "tooltip";
  grouped: boolean;
  pending: boolean;
  onInstall: () => void;
  onCancel: () => void;
  onDelete: () => void;
  onImportFolder: () => void;
  onPreview: () => void;
}

/**
 * Phonon card body: description, help text, licence and responsible-use line, the model row
 * (install, progress, cancel, delete, error rows) and preview.
 */
export const PhononEngineBody: React.FC<PhononEngineBodyProps> = ({
  card,
  descriptionMode,
  grouped,
  pending,
  onInstall,
  onCancel,
  onDelete,
  onImportFolder,
  onPreview,
}) => {
  const { t } = useTranslation();
  const sizeBadge = card.badges.find(
    (badge): badge is Extract<EngineBadge, { kind: "size" }> =>
      badge.kind === "size",
  );
  const licenseBadge = card.badges.find(
    (badge): badge is Extract<EngineBadge, { kind: "license" }> =>
      badge.kind === "license",
  );
  const percent = Math.round((card.progress ?? 0) * 100);

  const errorText = (kind: string): string => {
    switch (kind) {
      case "corrupt":
        return t("settings.advanced.tts.engines.error.corrupt");
      case "disk":
        return t("settings.advanced.tts.engines.error.disk");
      case "blocked":
        return t("settings.advanced.tts.engines.error.blocked");
      case "proxy":
        return t("settings.advanced.tts.engines.error.proxy");
      default:
        return t("settings.advanced.tts.engines.error.generic");
    }
  };

  return (
    <div className="flex flex-col gap-2">
      <p className="text-xs text-mid-gray">
        {t("settings.advanced.tts.engines.phonon.description")}
        {card.showRateNote
          ? ` ${t("settings.advanced.tts.engines.phonon.fixedVoiceRate")}`
          : ""}
      </p>
      <p className="text-xs text-mid-gray">
        {t("settings.advanced.tts.engines.phonon.help")}
      </p>
      <p className="text-xs text-mid-gray">
        {t("settings.advanced.tts.engines.phonon.license", {
          license: licenseBadge?.license ?? "",
        })}
      </p>
      {card.usingSystemVoiceNote && (
        <p className="text-xs">
          {t("settings.advanced.tts.engines.usingSystemVoice")}
        </p>
      )}
      <SettingContainer
        title={t("settings.advanced.tts.engines.phonon.model.label")}
        description={t(
          "settings.advanced.tts.engines.phonon.model.description",
          {
            size: sizeBadge?.label ?? "",
          },
        )}
        descriptionMode={descriptionMode}
        grouped={grouped}
        layout="stacked"
      >
        <div className="flex flex-col gap-2">
          {card.primaryAction === "cancel" && (
            <div className="flex items-center gap-3">
              <div className="flex flex-1 flex-col gap-1">
                <span className="text-sm">
                  {t("settings.advanced.tts.natural.model.downloading", {
                    percent,
                  })}
                </span>
                <div className="h-1.5 w-full overflow-hidden rounded-full bg-mid-gray/20">
                  <div
                    className="h-full rounded-full bg-logo-primary transition-all"
                    style={{ width: `${percent}%` }}
                  />
                </div>
              </div>
              <Button
                variant="secondary"
                size="md"
                onClick={onCancel}
                disabled={pending}
              >
                {t("settings.advanced.tts.natural.model.cancel")}
              </Button>
            </div>
          )}
          {card.primaryAction === "delete" && (
            <div className="flex items-center justify-between gap-3">
              <span className="text-sm">
                {t("settings.advanced.tts.natural.model.ready")}
              </span>
              <Button
                variant="danger-ghost"
                size="md"
                onClick={onDelete}
                disabled={pending}
              >
                {t("settings.advanced.tts.natural.model.delete")}
              </Button>
            </div>
          )}
          {card.primaryAction === null && card.error === null && (
            <span className="text-sm text-mid-gray">
              {t("settings.advanced.tts.natural.model.working")}
            </span>
          )}
          {card.primaryAction === "install" && (
            <div className="flex flex-wrap items-center gap-2">
              <Button
                variant="primary"
                size="md"
                onClick={onInstall}
                disabled={pending}
              >
                {t("settings.advanced.tts.engines.install")}
              </Button>
              {card.showImportFromFolder && (
                <Button
                  variant="secondary"
                  size="md"
                  onClick={onImportFolder}
                  disabled={pending}
                >
                  {t("settings.advanced.tts.engines.installFromFolder")}
                </Button>
              )}
            </div>
          )}
          {card.error !== null && (
            <div className="flex flex-col gap-1 rounded-lg border border-warning/40 bg-warning/10 p-3">
              <p className="text-sm">{errorText(card.error.kind)}</p>
              {card.error.showImportHint && (
                <p className="text-xs text-mid-gray">
                  {card.error.kind === "proxy"
                    ? t("settings.advanced.tts.engines.proxyHint")
                    : t("settings.advanced.tts.engines.importHint")}
                </p>
              )}
            </div>
          )}
        </div>
      </SettingContainer>
      {/* ttsSpeakTest has no engine argument, so the preview speaks with the persisted
          engine: offer it only on the selected card, and only once the model is installed. */}
      {card.previewEnabled &&
        card.selected &&
        card.primaryAction === "delete" && (
          <div>
            <Button
              variant="secondary"
              size="md"
              onClick={onPreview}
              disabled={pending}
            >
              {t("settings.advanced.tts.natural.preview")}
            </Button>
          </div>
        )}
    </div>
  );
};
