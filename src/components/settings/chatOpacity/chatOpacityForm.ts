// Pure rules for the "Conversation window transparency" slider (Track T, DESIGN Revision 3,
// item 6). No React and no commands here: the component applies these results. The range and the
// default are the page's own constants, so the slider and the window can never disagree.
import {
  CHAT_OPACITY_DEFAULT as DEFAULT_PCT,
  CHAT_OPACITY_MAX as MAX_PCT,
  CHAT_OPACITY_MIN as MIN_PCT,
} from "../../../chat/chatOpacity";

export {
  CHAT_OPACITY_DEFAULT,
  CHAT_OPACITY_MAX,
  CHAT_OPACITY_MIN,
} from "../../../chat/chatOpacity";

/** Slider step, in percent. */
export const CHAT_OPACITY_STEP = 1;

/** A value clamped to 20..100 and rounded to whole percent. A non-finite value reads as 70. */
export function clampChatOpacity(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_PCT;
  return Math.min(MAX_PCT, Math.max(MIN_PCT, Math.round(value)));
}

/** The slider position for a stored value; a missing value reads as the default (70). */
export function sliderValue(stored: number | null | undefined): number {
  if (stored === null || stored === undefined) return DEFAULT_PCT;
  return clampChatOpacity(stored);
}

/** The argument object of `change_chat_opacity_setting`. */
export function chatOpacityCommandArgs(pct: number): { pct: number } {
  return { pct: clampChatOpacity(pct) };
}

/** Injected timer and sender of the writer (the component passes the browser timers). */
export interface OpacityWriterDeps {
  send: (pct: number) => void;
  schedule: (run: () => void) => unknown;
  cancel: (handle: unknown) => void;
}

export interface OpacityWriter {
  /** Shows a value at once and queues it: the latest value wins after the quiet period. */
  change: (value: number) => void;
  /** Sends a pending value at once (the slider was released). */
  release: () => void;
  /** Sends the default at once and drops any pending change. */
  reset: () => void;
  /** Whether a change is waiting to be sent. */
  hasPending: () => boolean;
  /** The last value the writer was given. */
  current: () => number;
}

/** Live changes are debounced (one command per quiet period); release and reset write at once. */
export function createOpacityWriter(deps: OpacityWriterDeps): OpacityWriter {
  let current = DEFAULT_PCT;
  let pending = false;
  let handle: unknown = null;

  const dropTimer = (): void => {
    if (handle !== null) {
      deps.cancel(handle);
      handle = null;
    }
  };

  const flush = (): void => {
    pending = false;
    deps.send(current);
  };

  return {
    change(value: number): void {
      current = clampChatOpacity(value);
      pending = true;
      dropTimer();
      handle = deps.schedule(() => {
        handle = null;
        flush();
      });
    },
    release(): void {
      if (!pending) return;
      dropTimer();
      flush();
    },
    reset(): void {
      dropTimer();
      pending = false;
      current = DEFAULT_PCT;
      deps.send(DEFAULT_PCT);
    },
    hasPending: () => pending,
    current: () => current,
  };
}
