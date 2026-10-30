// Orb accent resolution (DESIGN Revision 3, D3). Pure: no DOM, no invoke. The orb page uses it only
// when the user picked an orb accent; otherwise the orb keeps following the app accent tokens.
import {
  ACCENTS,
  ACCENT_IDS,
  DEFAULT_ACCENT,
  type AccentBucket,
  type AccentId,
  type AccentRoles,
} from "../styles/accents";
import { ORB_ACCENT } from "./orbModel";

/** The resolved accent: its id and its roles for one colour bucket. */
export interface OrbAccentPick {
  id: AccentId;
  roles: AccentRoles;
}

/** The concerned face keeps its alarm colour; it is a state colour, not an accent. */
export const CONCERNED_COLOUR = "#FF6B6B";

const isAccentId = (value: unknown): value is AccentId =>
  typeof value === "string" &&
  (ACCENT_IDS as readonly string[]).includes(value);

/**
 * The orb accent: `orbAccent` when it is one of the six accents, otherwise the app accent. An
 * unknown app accent falls back to the default accent. Never throws.
 */
export function resolveOrbAccent(
  snapshotAccent: AccentId,
  orbAccent: AccentId | null,
  bucket: AccentBucket,
): OrbAccentPick {
  const app = isAccentId(snapshotAccent) ? snapshotAccent : DEFAULT_ACCENT;
  const id = isAccentId(orbAccent) ? orbAccent : app;
  return { id, roles: ACCENTS[id][bucket] };
}

/**
 * The ring colour of one face for a picked accent. Accent-a faces (idle, thinking, speaking) use
 * the first gradient end and accent-b faces (listening, acting) the second on dark. On light the
 * accent text role is used for every accent face: the gradient ends are below 3:1 on the light
 * surface, the text role is at least 4.5:1. `concerned` keeps its fixed alarm colour.
 */
export function orbRingColour(
  face: string,
  pick: OrbAccentPick,
  bucket: AccentBucket,
): string {
  if (face === "concerned") return CONCERNED_COLOUR;
  if (bucket === "light") return pick.roles.text;
  const slot = ORB_ACCENT[face] ?? ORB_ACCENT.idle;
  return slot === "var(--color-accent-b)" ? pick.roles.fillB : pick.roles.fillA;
}
