// The conversation window's look (DESIGN Revision 3, A3): the orb colour, the app accent and the
// bucket of the chat surface. chat/main.tsx publishes it from onApplied; ChatWindow reads it through
// useSyncExternalStore. Pure: no DOM and no invoke.
import type { AppearanceSnapshot } from "@/bindings";
import type { ResolvedAppearance } from "@/lib/appearance/palette";
import { DEFAULT_ACCENT } from "../styles/accents";
import { AUTO_AVATAR_LOOK, type AvatarLook } from "./avatarStyle";

/** The chat look plus the revision it was applied from (a stale revision never replaces it). */
export interface ChatOrbLook extends AvatarLook {
  rev: number;
}

let look: ChatOrbLook = { rev: 0, ...AUTO_AVATAR_LOOK };
const listeners = new Set<() => void>();

export function getChatOrbLook(): ChatOrbLook {
  return look;
}

/**
 * Publishes a look. A lower revision is stale and is ignored. An equal revision may still change
 * the bucket (a live OS colour-scheme change re-applies the same snapshot). Returns true on change.
 */
export function publishChatOrbLook(next: ChatOrbLook): boolean {
  if (next.rev < look.rev) return false;
  if (
    next.rev === look.rev &&
    next.orbColor === look.orbColor &&
    next.appAccent === look.appAccent &&
    next.bucket === look.bucket
  ) {
    return false;
  }
  look = next;
  for (const listener of listeners) listener();
  return true;
}

/** Subscribes to look changes. Returns the unsubscribe function. */
export function subscribeChatOrbLook(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * The chat look of one applied snapshot. The bucket is the chat surface's palette (chat_theme),
 * not the app bucket. A missing orb colour reads as auto.
 */
export function chatOrbLookFor(
  resolved: Pick<ResolvedAppearance, "accent" | "chatPalette">,
  snapshot: Pick<AppearanceSnapshot, "rev" | "orb_accent">,
): ChatOrbLook {
  return {
    rev: snapshot.rev,
    orbColor: snapshot.orb_accent ?? "auto",
    appAccent: resolved.accent ?? DEFAULT_ACCENT,
    bucket: resolved.chatPalette,
  };
}
