/**
 * Pure gating rules for the optional voice toggles. No React.
 * Spec = voiceGates.test.ts.
 */

/** Smart turn can be switched on only when its model is Ready. */
export function smartTurnToggleEnabled(modelReady: boolean): boolean {
  return modelReady;
}

/** Shown when smart turn is on but the model is missing: the fixed silence rule applies. */
export function smartTurnFallbackNote(
  enabled: boolean,
  modelReady: boolean,
): boolean {
  return enabled && !modelReady;
}

/** The speaker filter can be switched on only when the model is Ready AND a voice is enrolled. */
export function speakerFilterToggleEnabled(
  modelReady: boolean,
  enrolled: boolean,
): boolean {
  return modelReady && enrolled;
}

/** Shown when the speaker filter is on but cannot run yet. */
export function speakerFilterInactiveNote(
  enabled: boolean,
  modelReady: boolean,
  enrolled: boolean,
): boolean {
  return enabled && !speakerFilterToggleEnabled(modelReady, enrolled);
}

/** A voiceprint can be deleted only when one exists and no enrollment is running. */
export function allowsDeleteVoiceprint(
  enrolled: boolean,
  enrollmentActive: boolean,
): boolean {
  return enrolled && !enrollmentActive;
}

/**
 * Whether a toggle is disabled. A switch that is on can always be turned off,
 * even when its gate is closed, so the user is never stuck on.
 */
export function toggleDisabled(checked: boolean, gateOpen: boolean): boolean {
  return !checked && !gateOpen;
}
