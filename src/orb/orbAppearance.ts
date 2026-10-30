// The orb page's look: the app accent, the orb's own accent and the bucket the page renders in. The
// appearance consumer writes it (main.tsx, onApplied); OrbApp reads it through useSyncExternalStore.
// Pure: no DOM and no invoke, so the static test in orbRefusal.test.ts covers it.
import type { AccentBucket, AccentId } from "../styles/accents";
import { DEFAULT_ACCENT } from "../styles/accents";

export interface OrbLook {
  /** The app accent of the applied snapshot. */
  accent: AccentId;
  /** The orb's own accent, or null to follow the app accent. */
  orbAccent: AccentId | null;
  /** The bucket the orb page renders in. */
  bucket: AccentBucket;
}

let look: OrbLook = { accent: DEFAULT_ACCENT, orbAccent: null, bucket: "dark" };
const listeners = new Set<() => void>();

export function getOrbLook(): OrbLook {
  return look;
}

/** Stores a new look and tells the listeners. An identical look changes nothing. */
export function setOrbLook(next: OrbLook): void {
  if (
    next.accent === look.accent &&
    next.orbAccent === look.orbAccent &&
    next.bucket === look.bucket
  ) {
    return;
  }
  look = next;
  for (const listener of listeners) listener();
}

/** Subscribes to look changes. Returns the unsubscribe function. */
export function subscribeOrbLook(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
