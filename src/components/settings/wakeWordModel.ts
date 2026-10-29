/** The settings the wake toggle reads to decide whether ambient listening blocks it. */
export interface WakeToggleAmbientSettings {
  experimental_enabled?: boolean;
  ambient_mode_enabled?: boolean;
}

/**
 * The wake toggle is disabled while ambient listening is effective: Experimental is on and
 * the stored ambient flag is on. Same rule as the backend's `ambient_effective`.
 */
export function wakeToggleDisabledByAmbient(
  settings: WakeToggleAmbientSettings,
): boolean {
  return (
    (settings.experimental_enabled ?? false) &&
    (settings.ambient_mode_enabled ?? false)
  );
}
