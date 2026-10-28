// Pure rules for the Floating orb settings group (DESIGN.md 'Settings (G1)' and 'Frontend' ->
// Settings UI). No React and no commands here: the component applies these results.

export const ORB_SIZES = ["small", "medium", "large"] as const;
export type OrbSizeValue = (typeof ORB_SIZES)[number];

/** Shape of the `get_orb_support` answer (see `OrbSupport` in bindings). */
export interface OrbSupportState {
  supported: boolean;
  block: string | null;
}

const REASON_PREFIX = "settings.floatingOrb.reason.";

/** The size shown and stored by default: a missing or unknown size reads as medium. */
export function sizeValue(size: string | null | undefined): OrbSizeValue {
  return (ORB_SIZES as readonly string[]).includes(size ?? "")
    ? (size as OrbSizeValue)
    : "medium";
}

/**
 * The reason text key for an unsupported block. `null` (or no block) has no reason. A block this
 * build does not recognise falls back to the generic reason.
 */
export function supportReasonKey(
  block: string | null | undefined,
): string | null {
  if (block === null || block === undefined) return null;
  switch (block) {
    case "wayland":
    case "unknown_backend":
    case "no_compositor":
    case "unsupported_os":
      return `${REASON_PREFIX}${block}`;
    default:
      return `${REASON_PREFIX}unknown`;
  }
}

export interface ToggleState {
  checked: boolean;
  disabled: boolean;
  reasonKey: string | null;
}

/**
 * The Show orb toggle. While the support answer is still loading the toggle is disabled and shows
 * the stored value. An unsupported system disables it, unchecks it and names the reason.
 */
export function toggleState(
  support: OrbSupportState | null | undefined,
  orbVisible: boolean | null | undefined,
): ToggleState {
  const stored = orbVisible === true;
  if (support === null || support === undefined) {
    return { checked: stored, disabled: true, reasonKey: null };
  }
  if (!support.supported) {
    return {
      checked: false,
      disabled: true,
      reasonKey: supportReasonKey(support.block),
    };
  }
  return { checked: stored, disabled: false, reasonKey: null };
}
