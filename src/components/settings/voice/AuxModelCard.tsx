import React from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../ui/Button";
import { SettingContainer } from "../../ui/SettingContainer";
import type { DisplayAuxError, VoiceModelCardView } from "./voiceModelCard";

interface AuxModelCardProps {
  /** Already translated. */
  title: string;
  /** Already translated. */
  description: string;
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
  view: VoiceModelCardView;
  /** Hides every control while the status is still loading. */
  loaded: boolean;
  onDownload: () => void;
  onCancel: () => void;
  onDelete: () => void;
}

/**
 * Status, progress and the Download / Cancel / Delete buttons of one optional
 * model. Pure presentation: the caller owns the state and the actions.
 * Every visible string is a literal t() key, so the search index can see it.
 */
export const AuxModelCard: React.FC<AuxModelCardProps> = ({
  title,
  description,
  descriptionMode = "tooltip",
  grouped = false,
  view,
  loaded,
  onDownload,
  onCancel,
  onDelete,
}) => {
  const { t } = useTranslation();

  const statusText = (): string => {
    switch (view.status) {
      case "downloading":
        return t("converse.voice.model.state.downloading", {
          percent: view.percent,
        });
      case "verifying":
        return t("converse.voice.model.state.verifying");
      case "ready":
        return t("converse.voice.model.state.ready");
      case "error":
        return t("converse.voice.model.state.error");
      default:
        return t("converse.voice.model.state.not_installed");
    }
  };

  const errorText = (error: DisplayAuxError): string => {
    switch (error) {
      case "busy":
        return t("converse.voice.model.error.busy");
      case "corrupt":
        return t("converse.voice.model.error.corrupt");
      case "url_not_allowed":
        return t("converse.voice.model.error.url_not_allowed");
      case "download":
        return t("converse.voice.model.error.download");
      case "io":
        return t("converse.voice.model.error.io");
      case "not_installed":
        return t("converse.voice.model.error.not_installed");
      default:
        return t("converse.voice.model.error.unknown");
    }
  };

  return (
    <SettingContainer
      title={title}
      description={description}
      descriptionMode={descriptionMode}
      grouped={grouped}
      layout="stacked"
    >
      <div className="flex flex-col gap-2">
        {!loaded ? (
          <span className="text-sm text-mid-gray">
            {t("converse.voice.model.checking")}
          </span>
        ) : (
          <>
            <div className="flex items-center justify-between gap-3">
              <div className="flex flex-1 flex-col gap-1">
                <span className="text-sm">{statusText()}</span>
                {view.status === "downloading" && (
                  <div
                    role="progressbar"
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={view.percent}
                    className="h-1.5 w-full overflow-hidden rounded-full bg-mid-gray/20"
                  >
                    <div
                      className="h-full rounded-full bg-logo-primary transition-all"
                      style={{ width: `${view.percent}%` }}
                    />
                  </div>
                )}
              </div>
              <div className="flex items-center gap-2">
                {view.canDownload && (
                  <Button variant="primary" size="md" onClick={onDownload}>
                    {t("converse.voice.model.download")}
                  </Button>
                )}
                {view.canCancel && (
                  <Button variant="secondary" size="md" onClick={onCancel}>
                    {t("converse.voice.model.cancel")}
                  </Button>
                )}
                {view.canDelete && (
                  <Button variant="danger-ghost" size="md" onClick={onDelete}>
                    {t("converse.voice.model.delete")}
                  </Button>
                )}
              </div>
            </div>
            {view.error !== null && (
              <p role="alert" className="text-xs text-danger">
                {errorText(view.error)}
              </p>
            )}
          </>
        )}
      </div>
    </SettingContainer>
  );
};
