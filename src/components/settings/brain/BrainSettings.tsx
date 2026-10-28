import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { useSettings } from "../../../hooks/useSettings";
import { BrainEndpointGroup } from "./BrainEndpointGroup";
import { BrainFeatureGroup } from "./BrainFeatureGroup";
import { BrainTestGroup } from "./BrainTestGroup";
import { destinationKey } from "./brainModel";

/**
 * The Brain page: one language-model endpoint shared by the features that inherit it.
 * The sidebar section wrapper (data-settings-section) is provided by App.
 */
export const BrainSettings: React.FC = () => {
  const { t } = useTranslation();
  const { settings } = useSettings();

  const savedAck = settings?.brain_test_ack_host ?? "";
  const destination = destinationKey(
    settings?.brain_provider_id ?? "",
    settings?.brain_base_url ?? "",
  );
  // The acknowledgement this page just recorded. It stands in for the stored value until
  // the settings refresh arrives, and is dropped as soon as the stored value changes.
  const [pendingAck, setPendingAck] = useState<string | null>(null);
  useEffect(() => {
    setPendingAck(null);
  }, [savedAck]);
  const acked = savedAck === destination || pendingAck === destination;

  return (
    <div
      data-testid="brain-page"
      className="max-w-3xl w-full mx-auto space-y-6"
    >
      <SettingsGroup
        title={t("settings.brain.endpoint.title")}
        description={t("settings.brain.description")}
      >
        <BrainEndpointGroup acked={acked} />
      </SettingsGroup>
      <SettingsGroup title={t("settings.brain.test.title")}>
        <BrainTestGroup acked={acked} onAcknowledged={setPendingAck} />
      </SettingsGroup>
      <SettingsGroup
        title={t("settings.brain.feature.title")}
        description={t("settings.brain.feature.description")}
      >
        <BrainFeatureGroup />
      </SettingsGroup>
    </div>
  );
};
