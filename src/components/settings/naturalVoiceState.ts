// Pure UI logic for the natural voice rows in Spoken Replies settings.
// Spec = naturalVoiceState.test.ts (authoritative).

export type NaturalStatusLike = {
  supported: boolean;
  /** not_installed | downloading | verifying | installing | deleting | loading | ready */
  model: string;
  downloaded: number;
  total: number;
  /** ready | failed */
  health: string;
  /** load | synth | sink | worker_exit (when health is failed) */
  failure: string | null;
};

/** Which control set the model row shows. */
export type NaturalRow =
  | "hidden"
  | "download"
  | "downloading"
  | "working"
  | "ready"
  | "failed";

/** Size of the one-time natural voice model download, in bytes. */
export const NATURAL_MODEL_SIZE_BYTES = 120_673_052;

const WORKING_STATES = new Set([
  "verifying",
  "installing",
  "deleting",
  "loading",
]);
const FAILURE_CODES = new Set(["load", "synth", "sink", "worker_exit"]);

/** Whole-number percent, floored and clamped to 0..100; 0 when total is not positive. */
export function downloadPercent(downloaded: number, total: number): number {
  if (!Number.isFinite(downloaded) || !Number.isFinite(total) || total <= 0) {
    return 0;
  }
  const percent = Math.floor((downloaded / total) * 100);
  return Math.min(100, Math.max(0, percent));
}

/**
 * hidden: no status yet or unsupported platform. Otherwise decided by the model state:
 * not_installed -> download; downloading -> downloading; verifying/installing/deleting/loading
 * -> working; ready -> ready, or failed when health is failed (failure only matters once the
 * model is installed).
 */
export function naturalRowState(status: NaturalStatusLike | null): NaturalRow {
  if (status === null || !status.supported) return "hidden";
  if (status.model === "downloading") return "downloading";
  if (WORKING_STATES.has(status.model)) return "working";
  if (status.model === "ready") {
    return status.health === "failed" ? "failed" : "ready";
  }
  return "download";
}

/** i18n key for a failure code: settings.advanced.tts.natural.failure.<code>, unknown -> .unknown */
export function failureI18nKey(failure: string | null): string {
  const code =
    failure !== null && FAILURE_CODES.has(failure) ? failure : "unknown";
  return `settings.advanced.tts.natural.failure.${code}`;
}

/** Rounded mebibytes, e.g. 92361271 -> "88 MB". */
export function formatMegabytes(bytes: number): string {
  const safe = Number.isFinite(bytes) && bytes > 0 ? bytes : 0;
  return `${Math.round(safe / (1024 * 1024))} MB`;
}

/** True when the natural engine is selected on a supported platform but the model is missing. */
export function selectedEngineNeedsModel(
  engine: "system" | "natural",
  status: NaturalStatusLike | null,
): boolean {
  if (engine !== "natural" || status === null || !status.supported) {
    return false;
  }
  return status.model !== "ready";
}

/** Model states during which the settings row polls for status updates. */
export function isBusyModelState(model: string): boolean {
  return model === "downloading" || WORKING_STATES.has(model);
}
