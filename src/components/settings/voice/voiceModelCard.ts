/**
 * Pure state for an optional model card (smart turn, speaker model). No React.
 * Spec = voiceModelCard.test.ts. Reuses the natural-voice percent rule.
 */
import { downloadPercent } from "../naturalVoiceState";

/** Mirrors the backend `AuxState`. */
export type AuxStateName =
  | "not_installed"
  | "downloading"
  | "verifying"
  | "ready"
  | "error";

/** Errors the card can show. `unknown` covers codes this build does not know. */
export type DisplayAuxError =
  | "busy"
  | "corrupt"
  | "url_not_allowed"
  | "download"
  | "io"
  | "not_installed"
  | "unknown";

export interface VoiceModelCardView {
  status: AuxStateName;
  /** Whole percent 0..100; only meaningful while downloading. */
  percent: number;
  canDownload: boolean;
  canCancel: boolean;
  canDelete: boolean;
  /** True while the backend is working on the model; no other action applies. */
  busy: boolean;
  error: DisplayAuxError | null;
}

const KNOWN_STATES: readonly AuxStateName[] = [
  "not_installed",
  "downloading",
  "verifying",
  "ready",
  "error",
];

const KNOWN_ERRORS: readonly DisplayAuxError[] = [
  "busy",
  "corrupt",
  "url_not_allowed",
  "download",
  "io",
  "not_installed",
];

/**
 * Maps a backend error code to what the card shows. A cancelled download is
 * not an error. A missing code means no error.
 */
export function auxErrorDisplay(code: string | null): DisplayAuxError | null {
  if (code === null || code === "cancelled") return null;
  return (KNOWN_ERRORS as readonly string[]).includes(code)
    ? (code as DisplayAuxError)
    : "unknown";
}

/**
 * Button enablement and status for one model. Unknown states are treated as
 * not installed, the only state from which a download makes sense.
 */
export function voiceModelCard(
  state: string,
  downloaded: number,
  total: number,
  error: string | null,
): VoiceModelCardView {
  const status: AuxStateName = (KNOWN_STATES as readonly string[]).includes(
    state,
  )
    ? (state as AuxStateName)
    : "not_installed";

  const downloading = status === "downloading";
  const working = downloading || status === "verifying";
  return {
    status,
    percent: downloading ? downloadPercent(downloaded, total) : 0,
    canDownload: status === "not_installed" || status === "error",
    canCancel: downloading,
    canDelete: status === "ready",
    busy: working,
    error: auxErrorDisplay(error),
  };
}
