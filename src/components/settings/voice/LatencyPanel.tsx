import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { ToggleSwitch } from "../../ui/ToggleSwitch";
import { useSettings } from "../../../hooks/useSettings";
import {
  LATENCY_FIELDS,
  latencyView,
  parseLatencyEvent,
  type LatencyEventView,
  type LatencyFieldKey,
  type LatencyView,
} from "./latencyView";

interface LatencyPanelProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

const LATENCY_EVENT = "converse-latency";

/**
 * Toggle plus a live panel with the last turn and the rolling median and
 * 90th percentile. The panel listens only while it is on. Numbers only: no
 * audio, no text, no mode names.
 */
export const LatencyPanel: React.FC<LatencyPanelProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const enabled = getSetting("converse_latency_panel") ?? false;
  const [event, setEvent] = useState<LatencyEventView | null>(null);

  useEffect(() => {
    if (!enabled) return;
    let active = true;
    setEvent(null);
    const unlisten: Promise<UnlistenFn> = listen<unknown>(
      LATENCY_EVENT,
      (message) => {
        if (!active) return;
        const parsed = parseLatencyEvent(message.payload);
        if (parsed !== null) setEvent(parsed);
      },
    );
    return () => {
      active = false;
      void unlisten.then((fn) => fn());
    };
  }, [enabled]);

  return (
    <>
      <ToggleSwitch
        checked={enabled}
        onChange={(next) => void updateSetting("converse_latency_panel", next)}
        disabled={isUpdating("converse_latency_panel")}
        isUpdating={isUpdating("converse_latency_panel")}
        label={t("converse.voice.latency.label")}
        description={t("converse.voice.latency.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      />
      {enabled && <LatencyBody view={latencyView(event)} />}
    </>
  );
};

const LatencyBody: React.FC<{ view: LatencyView }> = ({ view }) => {
  const { t } = useTranslation();

  if (view.kind === "placeholder") {
    return (
      <p className="px-3 text-xs text-mid-gray">
        {t("converse.voice.latency.placeholder")}
      </p>
    );
  }

  const stageLabel = (key: LatencyFieldKey): string => {
    switch (key) {
      case "waitForSpeech":
        return t("converse.voice.latency.stage.waitForSpeech");
      case "transcription":
        return t("converse.voice.latency.stage.transcription");
      case "thinking":
        return t("converse.voice.latency.stage.thinking");
      case "toFirstSpeech":
        return t("converse.voice.latency.stage.toFirstSpeech");
      default:
        return t("converse.voice.latency.stage.speakerCheck");
    }
  };

  const ms = (value: number | null | undefined): string =>
    value === null || value === undefined
      ? ""
      : t("converse.voice.latency.ms", { ms: value });

  return (
    <div className="mx-3 flex flex-col gap-2 rounded-lg border border-mid-gray/30 p-3">
      <p className="text-xs text-mid-gray">
        {t("converse.voice.latency.turn", { turn: view.turn })}
      </p>
      <table className="w-full text-sm">
        <thead>
          <tr className="text-xs text-mid-gray">
            <th className="text-start font-normal">
              {t("converse.voice.latency.header.stage")}
            </th>
            <th className="text-end font-normal">
              {t("converse.voice.latency.header.last")}
            </th>
            <th className="text-end font-normal">
              {t("converse.voice.latency.header.median")}
            </th>
            <th className="text-end font-normal">
              {t("converse.voice.latency.header.p90")}
            </th>
          </tr>
        </thead>
        <tbody>
          {LATENCY_FIELDS.map((key) => {
            const last = view.last.find((line) => line.key === key);
            const stat = view.stats.find((line) => line.key === key);
            if (last === undefined && stat === undefined) return null;
            return (
              <tr key={key}>
                <td>{stageLabel(key)}</td>
                <td className="text-end">{ms(last?.ms)}</td>
                <td className="text-end">{ms(stat?.median)}</td>
                <td className="text-end">{ms(stat?.p90)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
      <p className="text-xs text-mid-gray">
        {t("converse.voice.latency.samples", { count: view.samples })}
      </p>
    </div>
  );
};
