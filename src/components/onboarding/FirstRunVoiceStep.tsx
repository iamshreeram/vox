import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { ask, open } from "@tauri-apps/plugin-dialog";
import {
  commands,
  type Result,
  type TtsEngine,
  type TtsEngineInfo,
} from "@/bindings";
import { Button } from "../ui/Button";
import { useSettings } from "../../hooks/useSettings";
import { useNaturalVoice } from "../../hooks/useNaturalVoice";
import { phononInstallState, usePhononVoice } from "../../hooks/usePhononVoice";
import { buildEngineCards } from "../settings/ttsEngineState";
import { NaturalVoiceBody } from "../settings/TtsSettings";
import { PhononEngineBody, TtsEngineCard } from "../settings/TtsEngineCard";

interface FirstRunVoiceStepProps {
  /** Called after the outcome was recorded. The parent decides what comes next. */
  onFinished: () => void;
}

/**
 * First-run Voice step: pick the text-to-speech engine. The rows are the same
 * shared engine cards as Settings, and nothing is downloaded until the user asks.
 */
export const FirstRunVoiceStep: React.FC<FirstRunVoiceStepProps> = ({
  onFinished,
}) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting } = useSettings();
  const natural = useNaturalVoice();
  const phonon = usePhononVoice();
  const [catalog, setCatalog] = useState<TtsEngineInfo[]>([]);
  const [phononPending, setPhononPending] = useState(false);
  const [recording, setRecording] = useState(false);

  const engine: TtsEngine = getSetting("tts_engine") ?? "system";

  useEffect(() => {
    let active = true;
    commands
      .listTtsEngines()
      .then((list) => {
        if (active) setCatalog(Array.isArray(list) ? list : []);
      })
      .catch((error: unknown) => {
        console.warn("Listing speech engines failed:", error);
      });
    return () => {
      active = false;
    };
  }, []);

  const cards = buildEngineCards(catalog, engine, {
    phonon: phononInstallState(phonon.status, phonon.error),
  });

  const handleSelectEngine = (id: TtsEngine) => {
    if (id === engine) return;
    updateSetting("tts_engine", id);
  };

  const withPhononPending = async (action: () => Promise<void>) => {
    setPhononPending(true);
    try {
      await action();
    } finally {
      setPhononPending(false);
    }
  };

  const handlePhononDelete = () =>
    withPhononPending(async () => {
      const confirmed = await ask(
        t("settings.advanced.tts.engines.confirmDelete"),
        {
          title: t("settings.advanced.tts.engines.phonon.name"),
          kind: "warning",
        },
      );
      if (confirmed) {
        await phonon.remove();
      }
    });

  const handlePhononImport = () =>
    withPhononPending(async () => {
      const picked = await open({ directory: true, multiple: false });
      if (typeof picked === "string") {
        await phonon.importFolder(picked);
      }
    });

  const handlePhononPreview = () =>
    withPhononPending(async () => {
      const result = await commands.ttsSpeakTest(
        t("settings.advanced.tts.testPhrase"),
      );
      if (result.status === "error") {
        console.warn("Phonon preview failed:", result.error);
      }
    });

  // Records the outcome, then hands over. A failed write keeps the step open.
  const finish = async (outcome: Promise<Result<null, string>>) => {
    if (recording) return;
    setRecording(true);
    const result = await outcome;
    setRecording(false);
    if (result.status === "error") {
      toast.error(t("onboarding.firstRun.errors.finish"));
      return;
    }
    onFinished();
  };

  return (
    <div className="h-screen w-full flex flex-col p-6 gap-4">
      <div className="max-w-[600px] w-full mx-auto flex-1 min-h-0 overflow-y-auto space-y-6">
        <div className="text-center space-y-2">
          <h1 className="text-xl font-semibold">
            {t("onboarding.firstRun.voice.title")}
          </h1>
          <p className="text-text/70 font-medium">
            {t("onboarding.firstRun.voice.description")}
          </p>
        </div>

        <div className="flex flex-col gap-3" data-testid="tts-engine-picker">
          {cards.map((card) => (
            <TtsEngineCard
              key={card.id}
              card={card}
              onSelect={() => handleSelectEngine(card.id)}
            >
              {card.id === "phonon" && (
                <PhononEngineBody
                  card={card}
                  descriptionMode="inline"
                  grouped={false}
                  pending={phononPending}
                  onInstall={() => withPhononPending(phonon.install)}
                  onCancel={() => withPhononPending(phonon.cancel)}
                  onDelete={handlePhononDelete}
                  onImportFolder={handlePhononImport}
                  onPreview={handlePhononPreview}
                />
              )}
              {card.id === "natural" && (
                <NaturalVoiceBody
                  descriptionMode="inline"
                  grouped={false}
                  engine={engine}
                  natural={natural}
                />
              )}
            </TtsEngineCard>
          ))}
        </div>

        <div className="flex items-center justify-between gap-3">
          <Button
            variant="secondary"
            size="md"
            disabled={recording}
            onClick={() =>
              void finish(commands.firstRunFinishStep("voice", "skipped"))
            }
          >
            {t("onboarding.firstRun.voice.skip")}
          </Button>
          <Button
            variant="primary"
            size="md"
            disabled={recording}
            onClick={() =>
              void finish(commands.firstRunFinishStep("voice", "done"))
            }
          >
            {t("onboarding.firstRun.voice.continue")}
          </Button>
        </div>
      </div>
    </div>
  );
};
