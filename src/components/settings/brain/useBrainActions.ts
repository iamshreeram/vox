import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  commands,
  type BrainFeature,
  type BrainTestResult,
  type Result,
} from "@/bindings";
import { useSettings } from "../../../hooks/useSettings";

type Settled<T> = { ok: true; data: T } | { ok: false };

export interface BrainActions {
  /** Saves provider, endpoint and model. Never call with an invalid endpoint. */
  saveConnection(
    providerId: string,
    baseUrl: string,
    model: string,
  ): Promise<boolean>;
  /** Stores a token (an empty string clears it). The value is never logged or kept. */
  saveToken(token: string): Promise<boolean>;
  acknowledge(): Promise<boolean>;
  useForFeature(feature: BrainFeature, useBrain: boolean): Promise<boolean>;
  test(): Promise<BrainTestResult | null>;
  fetchModels(): Promise<string[] | null>;
}

/**
 * The Brain page's only path to the backend. Every command refreshes the settings
 * afterwards. Failures show a static message, never the raw error text, which could
 * contain an address.
 */
export function useBrainActions(): BrainActions {
  const { t } = useTranslation();
  const { refreshSettings } = useSettings();

  async function settle<T>(
    call: () => Promise<Result<T, string>>,
    errorKey: string,
  ): Promise<Settled<T>> {
    let outcome: Settled<T> = { ok: false };
    try {
      const result = await call();
      if (result.status === "ok") {
        outcome = { ok: true, data: result.data };
      } else {
        toast.error(t(errorKey));
      }
    } catch {
      toast.error(t(errorKey));
    }
    try {
      await refreshSettings();
    } catch {
      // The settings store reports its own failures; the action outcome is still valid.
    }
    return outcome;
  }

  return {
    async saveConnection(providerId, baseUrl, model) {
      const outcome = await settle(
        () => commands.changeBrainConnectionSetting(providerId, baseUrl, model),
        "settings.brain.errors.save",
      );
      return outcome.ok;
    },
    async saveToken(token) {
      const outcome = await settle(
        () => commands.setBrainToken(token),
        token === ""
          ? "settings.brain.errors.clear"
          : "settings.brain.errors.token",
      );
      return outcome.ok;
    },
    async acknowledge() {
      const outcome = await settle(
        () => commands.brainAcknowledgeTest(),
        "settings.brain.errors.ack",
      );
      return outcome.ok;
    },
    async useForFeature(feature, useBrain) {
      const outcome = await settle(
        () => commands.brainUseForFeature(feature, useBrain),
        "settings.brain.errors.feature",
      );
      return outcome.ok;
    },
    async test() {
      const outcome = await settle(
        () => commands.brainTestConnection(),
        "settings.brain.errors.test",
      );
      return outcome.ok ? outcome.data : null;
    },
    async fetchModels() {
      const outcome = await settle(
        () => commands.brainFetchModels(),
        "settings.brain.errors.models",
      );
      return outcome.ok ? outcome.data : null;
    },
  };
}
