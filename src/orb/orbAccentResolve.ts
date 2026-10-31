// Orb accent resolution (DESIGN Revision 3, D3, A5). Pure: no DOM, no invoke. The orb colour is
// the OrbColor string from the snapshot: 'auto' (state accent per face), 'app' (the app accent)
// or one of the six accents. Null, missing or unknown values read as 'auto'.
import type { OrbColor } from "../bindings";
import {
  ACCENTS,
  ACCENT_IDS,
  DEFAULT_ACCENT,
  type AccentBucket,
  type AccentId,
  type AccentRoles,
} from "../styles/accents";
import { ORB_COLOR_IDS } from "../lib/appearance/consumer";
import { AUTO_FACE_ACCENT } from "./globePalette";
import { ORB_ACCENT } from "./orbModel";

/**
 * The resolved orb colour. `orbColor` is the normalised value. `id` and `roles` name the accent
 * for `app` and the explicit accents. For `auto` they hold the idle placeholder only: the ring of
 * each face is resolved by orbRingColour, which reads the face's state accent.
 */
export interface OrbAccentPick {
  orbColor: OrbColor;
  appAccent: AccentId;
  id: AccentId;
  roles: AccentRoles;
}

/** The concerned face keeps its alarm colour; it is a state colour, not an accent. */
export const CONCERNED_COLOUR = "#FF6B6B";

const isAccentId = (value: unknown): value is AccentId =>
  typeof value === "string" &&
  (ACCENT_IDS as readonly string[]).includes(value);

const isOrbColor = (value: unknown): value is OrbColor =>
  typeof value === "string" &&
  (ORB_COLOR_IDS as readonly string[]).includes(value);

/**
 * The orb colour and accent. An unknown or missing orb colour reads as 'auto'. An unknown app
 * accent falls back to the default accent. Never throws.
 */
export function resolveOrbAccent(
  snapshotAccent: AccentId,
  orbColor: OrbColor | null | undefined,
  bucket: AccentBucket,
): OrbAccentPick {
  const appAccent = isAccentId(snapshotAccent)
    ? snapshotAccent
    : DEFAULT_ACCENT;
  const normalised: OrbColor = isOrbColor(orbColor) ? orbColor : "auto";
  let id: AccentId;
  if (normalised === "auto") id = AUTO_FACE_ACCENT.idle;
  else if (normalised === "app") id = appAccent;
  else id = normalised;
  return { orbColor: normalised, appAccent, id, roles: ACCENTS[id][bucket] };
}

/**
 * The ring colour of one face for a resolved orb colour. Accent-a faces (idle, thinking,
 * speaking) use the first gradient end and accent-b faces (listening, acting) the second on dark.
 * On light the accent text role is used for every accent face: the gradient ends are below 3:1 on
 * the light surface, the text role is at least 4.5:1. `auto` takes each face's state accent.
 * `concerned` keeps its fixed alarm colour.
 */
export function orbRingColour(
  face: string,
  pick: OrbAccentPick,
  bucket: AccentBucket,
): string {
  if (face === "concerned") return CONCERNED_COLOUR;
  const roles =
    pick.orbColor === "auto"
      ? ACCENTS[
          AUTO_FACE_ACCENT[face as keyof typeof AUTO_FACE_ACCENT] ??
            AUTO_FACE_ACCENT.idle
        ][bucket]
      : pick.roles;
  if (bucket === "light") return roles.text;
  const slot = ORB_ACCENT[face] ?? ORB_ACCENT.idle;
  return slot === "var(--color-accent-b)" ? roles.fillB : roles.fillA;
}
