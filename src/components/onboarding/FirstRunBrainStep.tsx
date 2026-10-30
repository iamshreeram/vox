import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands, type BrainTestResult, type Result } from "@/bindings";
import { Button } from "../ui/Button";
import { SettingsGroup } from "../ui/SettingsGroup";
import { useSettings } from "../../hooks/useSettings";
import { BrainEndpointGroup } from "../settings/brain/BrainEndpointGroup";
import { BrainTestGroup } from "../settings/brain/BrainTestGroup";
import {
  destinationKey,
  destinationLabel,
  isLoopbackUrl,
} from "../settings/brain/brainModel";

interface FirstRunBrainStepProps {
  /** Called after the outcome was recorded. The parent decides what comes next. */
  onFinished: () => void;
}

/**
 * First-run Brain step. It reuses the Brain page building blocks, so the endpoint,
 * token, acknowledgement and test behave exactly as on the Brain page. Nothing is sent
 * while the user types, and only "Test" (after the acknowledgement) makes a request.
 */
export const FirstRunBrainStep: React.FC<FirstRunBrainStepProps> = ({
  onFinished,
}) => {
  const { t } = useTranslation();
  const { settings, refreshSettings } = useSettings();
  const [busy, setBusy] = useState(false);
  // Same acknowledgement rules as the Brain page.
  const savedAck = settings?.brain_test_ack_host ?? "";
  const baseUrl = settings?.brain_base_url ?? "";
  const destination = destinationKey(
    settings?.brain_provider_id ?? "",
    baseUrl,
  );
  const [pendingAck, setPendingAck] = useState<string | null>(null);
  useEffect(() => {
    setPendingAck(null);
  }, [savedAck]);
  const acked = savedAck === destination || pendingAck === destination;
  const [draftMatches, setDraftMatches] = useState(true);
  // The destination whose connection test last succeeded. Only that unlocks the choice below.
  const [passedDestination, setPassedDestination] = useState<string | null>(
    null,
  );

  const handleTestResult = (tested: string, result: BrainTestResult) => {
    setPassedDestination(result.ok ? tested : null);
  };
  const canUse =
    passedDestination !== null && passedDestination === destination;
  const shownDestination = isLoopbackUrl(baseUrl)
    ? t("onboarding.firstRun.brain.localDestination")
    : destinationLabel(baseUrl);

  const complete = (result: Result<null, string>) => {
    if (result.status === "error") {
      toast.error(t("onboarding.firstRun.errors.finish"));
      return;
    }
    onFinished();
  };

  // Same fixed error text and settings refresh as the Brain page's own action.
  const useBrainForReplies = async (): Promise<boolean> => {
    let enabled = false;
    try {
      const result = await commands.brainUseForFeature("composer", true);
      enabled = result.status === "ok";
    } catch {
      enabled = false;
    }
    if (!enabled) toast.error(t("settings.brain.errors.feature"));
    try {
      await refreshSettings();
    } catch {
      // The settings store reports its own failures; the choice above still stands.
    }
    return enabled;
  };

  const handleSkip = async () => {
    setBusy(true);
    const result = await commands.firstRunFinishStep("brain", "skipped");
    setBusy(false);
    complete(result);
  };

  // The only place the Brain step is resolved as done: the user has tested the endpoint
  // and chosen to use it for spoken replies.
  const handleUseForReplies = async () => {
    setBusy(true);
    const enabled = await useBrainForReplies();
    if (!enabled) {
      setBusy(false);
      return;
    }
    const result = await commands.firstRunFinishStep("brain", "done");
    setBusy(false);
    complete(result);
  };

  return (
    <div className="h-screen w-full flex flex-col p-6 gap-4">
      <div className="max-w-[600px] w-full mx-auto flex-1 min-h-0 overflow-y-auto space-y-6">
        <div className="text-center space-y-2">
          <h1 className="text-xl font-semibold">
            {t("onboarding.firstRun.brain.title")}
          </h1>
          <p className="text-text/70 font-medium">
            {t("onboarding.firstRun.brain.description")}
          </p>
        </div>

        <SettingsGroup title={t("settings.brain.endpoint.title")}>
          <BrainEndpointGroup
            acked={acked}
            onDraftMatchChange={setDraftMatches}
          />
        </SettingsGroup>

        <SettingsGroup title={t("settings.brain.test.title")}>
          <BrainTestGroup
            acked={acked}
            draftMatches={draftMatches}
            onAcknowledged={setPendingAck}
            onResult={handleTestResult}
          />
          {canUse && (
            <div className="px-4 pb-3 space-y-2">
              <p className="text-sm text-muted">
                {t("onboarding.firstRun.brain.consentLine", {
                  destination: shownDestination,
                })}
              </p>
              <Button
                variant="primary"
                size="md"
                disabled={busy}
                onClick={() => void handleUseForReplies()}
              >
                {t("onboarding.firstRun.brain.useForReplies")}
              </Button>
            </div>
          )}
        </SettingsGroup>

        <div className="flex justify-start">
          <Button
            variant="secondary"
            size="md"
            disabled={busy}
            onClick={() => void handleSkip()}
          >
            {t("onboarding.firstRun.brain.skip")}
          </Button>
        </div>
      </div>
    </div>
  );
};
