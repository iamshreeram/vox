// Pure UI logic for the natural voice rows in Spoken Replies settings.
// STUB: implemented in step S6. Spec = naturalVoiceState.test.ts (authoritative).

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

/** Whole-number percent, floored and clamped to 0..100; 0 when total is not positive. */
export function downloadPercent(downloaded: number, total: number): number {
  void downloaded;
  void total;
  throw new Error("not implemented");
}

/**
 * hidden: no status yet or unsupported platform. Otherwise decided by the model state:
 * not_installed -> download; downloading -> downloading; verifying/installing/deleting/loading
 * -> working; ready -> ready, or failed when health is failed (failure only matters once the
 * model is installed).
 */
export function naturalRowState(status: NaturalStatusLike | null): NaturalRow {
  void status;
  throw new Error("not implemented");
}

/** i18n key for a failure code: settings.tts.natural.failure.<code>, unknown -> .unknown */
export function failureI18nKey(failure: string | null): string {
  void failure;
  throw new Error("not implemented");
}

/** Rounded mebibytes, e.g. 92361271 -> "88 MB". */
export function formatMegabytes(bytes: number): string {
  void bytes;
  throw new Error("not implemented");
}

/** True when the natural engine is selected on a supported platform but the model is missing. */
export function selectedEngineNeedsModel(
  engine: "system" | "natural",
  status: NaturalStatusLike | null,
): boolean {
  void engine;
  void status;
  throw new Error("not implemented");
}
