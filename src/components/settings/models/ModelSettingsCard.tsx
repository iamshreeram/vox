import React from "react";
import { useTranslation } from "react-i18next";
import { LanguageSelector } from "../LanguageSelector";
import { TranslateToEnglish } from "../TranslateToEnglish";
import { TamilSttRow } from "../TamilSttRow";
import { useModelStore } from "../../../stores/modelStore";
import { useTamilSttStatus } from "../../../hooks/useTamilSttStatus";
import type { ModelInfo } from "@/bindings";

export const ModelSettingsCard: React.FC = () => {
  const { t } = useTranslation();
  const { currentModel, models } = useModelStore();
  // Tamil can be picked while the active model does not list it (for example an English-only
  // model), so the picker is shown whenever Tamil recognition is available.
  const { available: tamilAvailable } = useTamilSttStatus();

  const currentModelInfo = models.find((m: ModelInfo) => m.id === currentModel);

  const showLanguageSelector =
    currentModelInfo?.supports_language_selection ?? false;
  const supportsTranslation = currentModelInfo?.supports_translation ?? false;

  // Don't render anything if no model is selected. The Tamil row is always shown with a model
  // so that the pack can be installed from here even when the model has no language picker.
  if (!currentModel || !currentModelInfo) {
    return null;
  }

  return (
    <>
      <h2 className="px-3 pt-3 pb-1 text-xs font-medium text-muted uppercase tracking-wide">
        {t("settings.modelSettings.title", {
          model: currentModelInfo.name,
        })}
      </h2>
      {(showLanguageSelector || tamilAvailable) && (
        <LanguageSelector
          descriptionMode="tooltip"
          grouped={true}
          supportedLanguages={currentModelInfo.supported_languages}
          supportsLanguageDetection={
            currentModelInfo.supports_language_detection
          }
        />
      )}
      {supportsTranslation && (
        <TranslateToEnglish descriptionMode="tooltip" grouped={true} />
      )}
      <TamilSttRow descriptionMode="tooltip" grouped={true} />
    </>
  );
};
