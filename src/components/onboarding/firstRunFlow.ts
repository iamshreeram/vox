import type { AppSettings, FirstRunStep } from "@/bindings";

type FirstRunSettings = Pick<
  AppSettings,
  "first_run_setup_pending" | "first_run_tts" | "first_run_brain"
>;

/**
 * The next first-run step still open, or null when the first run is over.
 * Mirrors the Rust `first_run::next_step`: Voice comes before Brain, and a
 * step is open while its outcome is unset.
 */
export function nextFirstRunStep(
  settings: FirstRunSettings,
): FirstRunStep | null {
  if (!settings.first_run_setup_pending) return null;
  if (
    settings.first_run_tts === undefined ||
    settings.first_run_tts === "unset"
  ) {
    return "voice";
  }
  if (
    settings.first_run_brain === undefined ||
    settings.first_run_brain === "unset"
  ) {
    return "brain";
  }
  return null;
}
