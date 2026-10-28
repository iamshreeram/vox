// Pure rules for the Floating orb settings group (DESIGN.md 'Settings (G1)' and 'Frontend' ->
// Settings UI). No React and no commands here: the component applies these results.

export const ORB_SIZES = ["small", "medium", "large"] as const;
export type OrbSizeValue = (typeof ORB_SIZES)[number];

/** Smallest and largest free orb size, in logical pixels (Revision 6). */
export const ORB_PX_MIN = 48;
export const ORB_PX_MAX = 192;
/** Slider step, in logical pixels. */
export const ORB_PX_STEP = 4;
/** Edge length of each preset, in logical pixels. */
export const ORB_PRESETS = { small: 72, medium: 96, large: 128 } as const;

/** The size used when nothing else is known (a missing or unknown size reads as medium). */
const DEFAULT_PX = ORB_PRESETS.medium;

/**
 * A free size clamped to the range and rounded to the slider step. A non-finite value reads as the
 * medium preset.
 */
export function clampOrbPx(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_PX;
  const stepped = Math.round(value / ORB_PX_STEP) * ORB_PX_STEP;
  return Math.min(ORB_PX_MAX, Math.max(ORB_PX_MIN, stepped));
}

/**
 * The effective size in logical pixels: a finite free size (clamped) wins, otherwise the preset of
 * `size` (a missing or unknown size reads as medium).
 */
export function effectiveOrbPx(
  size: string | null | undefined,
  px: number | null | undefined,
): number {
  if (typeof px === "number" && Number.isFinite(px)) return clampOrbPx(px);
  return ORB_PRESETS[sizeValue(size)];
}

/** The preset whose edge equals `px` exactly, or `null` when no preset matches. */
export function presetFor(px: number): OrbSizeValue | null {
  for (const size of ORB_SIZES) {
    if (ORB_PRESETS[size] === px) return size;
  }
  return null;
}

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

/** The choices of "When I talk to Vox": Automatic, Orb and bubbles, Conversation window. */
export const SURFACE_OPTIONS = ["auto", "bubbles", "window"] as const;
export type SurfaceOption = (typeof SURFACE_OPTIONS)[number];

/**
 * The choice shown for a stored chat presentation. `floating` is Orb and bubbles, `window` is
 * Conversation window. A missing, null or unknown value is Automatic.
 */
export function surfaceValue(
  presentation: string | null | undefined,
): SurfaceOption {
  if (presentation === "floating") return "bubbles";
  if (presentation === "window") return "window";
  return "auto";
}

/** The presentation to store for a choice. Automatic stores null (the platform default). */
export function surfaceToPresentation(
  option: SurfaceOption,
): "floating" | "window" | null {
  if (option === "bubbles") return "floating";
  if (option === "window") return "window";
  return null;
}

export interface SurfaceHintInput {
  converseChatLog: boolean | null | undefined;
  wakeWordEnabled: boolean | null | undefined;
  wakeWordAction: string | null | undefined;
  surface: SurfaceOption;
  /** `null` while the support answer is still loading. */
  supported: boolean | null | undefined;
}

export interface SurfaceHints {
  chatLogOff: boolean;
  wakeDictation: boolean;
}

/**
 * Which hints under the surface choice are shown. Both hide when the surface is Conversation
 * window or the platform is known to be unsupported (a loading answer does not hide them).
 * The chat-log hint shows when the log is off. The wake hint shows when Hey Jarvis is on and
 * starts dictation (a missing action reads as voice_request).
 */
export function surfaceHints(input: SurfaceHintInput): SurfaceHints {
  const relevant = input.surface !== "window" && input.supported !== false;
  const action = input.wakeWordAction ?? "voice_request";
  return {
    chatLogOff: relevant && input.converseChatLog === false,
    wakeDictation:
      relevant && input.wakeWordEnabled === true && action !== "conversation",
  };
}
