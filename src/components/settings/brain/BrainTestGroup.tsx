import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import type { BrainTestResult } from "@/bindings";
import { Button } from "../../ui/Button";
import { useSettings } from "../../../hooks/useSettings";
import { useBrainActions } from "./useBrainActions";
import {
  BRAIN_TEST_PROMPT,
  brainViewOf,
  configState,
  destinationKey,
  destinationLabel,
  testKindKey,
} from "./brainModel";

interface BrainTestGroupProps {
  /** The saved destination has been acknowledged for the connection test. */
  acked: boolean;
  /** The endpoint drafts match the saved connection, so the test targets the address shown. */
  draftMatches: boolean;
  /** Called with the destination key after the acknowledgement was recorded. */
  onAcknowledged: (destination: string) => void;
}

/**
 * The connection test. It sends one fixed message and only after the user has confirmed
 * which destination receives it. Nothing the user said is sent.
 */
export const BrainTestGroup: React.FC<BrainTestGroupProps> = ({
  acked,
  draftMatches,
  onAcknowledged,
}) => {
  const { t } = useTranslation();
  const { settings } = useSettings();
  const actions = useBrainActions();
  const [testing, setTesting] = useState(false);
  const [outcome, setOutcome] = useState<{
    destination: string;
    result: BrainTestResult;
  } | null>(null);

  if (settings === null) return null;

  const provider = settings.brain_provider_id ?? "";
  const baseUrl = settings.brain_base_url ?? "";
  const destination = destinationKey(provider, baseUrl);
  const label = destinationLabel(baseUrl);
  const state = configState(brainViewOf(settings));
  const canTest = acked && draftMatches && state === "ready" && !testing;
  // A result belongs to the destination it was measured against.
  const shown =
    outcome !== null && outcome.destination === destination
      ? outcome.result
      : null;

  const handleAcknowledge = async (checked: boolean) => {
    if (!checked || acked) return;
    if (await actions.acknowledge()) onAcknowledged(destination);
  };

  const handleTest = async () => {
    setTesting(true);
    const result = await actions.test();
    setTesting(false);
    if (result !== null) setOutcome({ destination, result });
  };

  const resultText =
    shown === null
      ? null
      : shown.kind === "Ok"
        ? t("settings.brain.test.ok", { ms: shown.latency_ms })
        : t(testKindKey(shown.kind));

  return (
    <div className="px-4 py-3 space-y-3">
      <p className="text-sm text-mid-gray">
        {t("settings.brain.test.description")}
      </p>
      {label !== "" && (
        <div data-testid="brain-ack" className="flex items-start gap-2">
          <input
            id="brain-ack-checkbox"
            type="checkbox"
            className="mt-1"
            checked={acked}
            disabled={state === "unconfigured"}
            onChange={(event) => void handleAcknowledge(event.target.checked)}
          />
          <label htmlFor="brain-ack-checkbox" className="text-sm">
            {t("settings.brain.test.ack", {
              prompt: BRAIN_TEST_PROMPT,
              destination: label,
            })}
          </label>
        </div>
      )}
      <div className="flex items-center gap-3">
        <Button
          data-testid="brain-test"
          variant="primary"
          size="md"
          disabled={!canTest}
          onClick={() => void handleTest()}
        >
          {t("settings.brain.test.run")}
        </Button>
      </div>
      {resultText !== null && (
        <p data-testid="brain-test-result" role="status" className="text-sm">
          {resultText}
        </p>
      )}
    </div>
  );
};
