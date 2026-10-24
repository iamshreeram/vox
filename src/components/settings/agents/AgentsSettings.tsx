import React from "react";
import { useTranslation } from "react-i18next";
import { PlacedGroups } from "../PlacedGroups";

export const AgentsSettings: React.FC = () => {
  const { t } = useTranslation();

  return (
    <div className="max-w-3xl w-full mx-auto space-y-3">
      <h2 className="px-1 text-xs font-medium text-mid-gray uppercase tracking-wide">
        {t("settings.agents.title")}
      </h2>
      <PlacedGroups section="agents" />
    </div>
  );
};
