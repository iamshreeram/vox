// Stop control visibility (UI enterprise T1b, DESIGN Revision 4 B2b). Pure: ChatWindow renders the
// button only when this returns true, and passes the same session id it rendered with.
import type { AvatarFace } from "./chatModel";

const STOPPABLE: ReadonlySet<AvatarFace> = new Set<AvatarFace>([
  "listening",
  "thinking",
  "acting",
  "speaking",
]);

/** True while the displayed face is busy AND the window knows the active session id. */
export function stopVisible(
  face: AvatarFace,
  activeSession: number | null,
): boolean {
  return activeSession !== null && STOPPABLE.has(face);
}
