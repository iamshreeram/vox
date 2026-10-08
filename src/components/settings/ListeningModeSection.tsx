import React from "react";
import { useTranslation } from "react-i18next";
import { SettingsGroup } from "../ui/SettingsGroup";
import { AmbientModeToggle } from "./AmbientModeToggle";
import { WakeWordToggle } from "./WakeWordToggle";

export const ListeningModeSection: React.FC = () => {
  const { t } = useTranslation();

  return (
    <SettingsGroup
      title={t("settings.advanced.listeningMode.title")}
      description={t("settings.advanced.listeningMode.description")}
    >
      <WakeWordToggle descriptionMode="tooltip" grouped={true} />
      <AmbientModeToggle descriptionMode="tooltip" grouped={true} />
    </SettingsGroup>
  );
};
