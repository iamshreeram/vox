// Globe palette (UI enterprise DESIGN Revision 3, G1, D1, D3, A4, A2b). Pure: no DOM, no invoke.
// The centre globe's colour comes from ONE const table and ONE precedence function, so the orb
// page, the conversation window and the Appearance preview all paint the same globe.
import type { OrbColor } from "../bindings";
import type { AvatarFace } from "../chat/chatModel";
import { ACCENT_IDS, type AccentId } from "../styles/accents";

/** The three gradient stops of the sphere, each a #RRGGBB hex (light, mid, deep). */
export interface GlobeStops {
  readonly hi: string;
  readonly mid: string;
  readonly lo: string;
}

/**
 * Hand-tuned vibrant stops per accent. Amber is the legacy brand gold, so the default look is
 * kept wherever gold is chosen. Each mid hue is kept >= 25 deg from the others (tested).
 */
export const GLOBE_STOPS: Record<AccentId, GlobeStops> = {
  amber: { hi: "#FFE27A", mid: "#F5B921", lo: "#B97A00" },
  blue: { hi: "#9CC8FF", mid: "#3B82F6", lo: "#1E3A8A" },
  violet: { hi: "#D4C2FF", mid: "#8B5CF6", lo: "#4C1D95" },
  emerald: { hi: "#8AF0C8", mid: "#10B981", lo: "#065F46" },
  rose: { hi: "#FFB3CE", mid: "#F43F5E", lo: "#881337" },
  // Slate is a muted blue-grey; its mid hue sits 25+ deg from blue, emerald and amber.
  slate: { hi: "#C3E2E8", mid: "#6EA2AA", lo: "#2E4A50" },
};

/** The concerned face: a red globe in every mode (A4 precedence 1). */
export const CONCERNED_STOPS: GlobeStops = {
  hi: "#FFB4B4",
  mid: "#EF4444",
  lo: "#7F1D1D",
};

/** The specular highlight: white at 55% opacity (replaces the old #fff8dc). */
export const SHINE = { colour: "#FFFFFF", opacity: 0.55 } as const;

/**
 * The accent the Auto mode shows for each face (D1). `concerned` never reaches this map (A4
 * precedence 1 handles it first); its entry is a placeholder only so the record is complete.
 */
export const AUTO_FACE_ACCENT: Record<AvatarFace, AccentId> = {
  idle: "blue",
  listening: "emerald",
  thinking: "violet",
  acting: "rose",
  speaking: "amber",
  sleep: "slate",
  concerned: "rose",
};

/** What the globe paints. `table` uses GLOBE_STOPS, `app` uses the CSS app stops (A2b). */
export type GlobeResolution =
  | { kind: "table"; accent: AccentId; stops: GlobeStops }
  | { kind: "app"; accent: AccentId }
  | { kind: "concerned"; stops: GlobeStops };

const isAccent = (value: unknown): value is AccentId =>
  typeof value === "string" &&
  (ACCENT_IDS as readonly string[]).includes(value);

/**
 * The globe for one face and orb colour (A4, ordered):
 *   1. face concerned            -> red globe, in every mode;
 *   2. orb colour auto           -> the face's state accent (sleep is slate);
 *   3. orb colour app            -> the app accent, for every other face including sleep;
 *   4. an explicit accent        -> that accent, for every other face including sleep.
 * An unknown orb colour reads as auto. The app accent is only named in the `app` result.
 */
export function resolveGlobe(
  face: AvatarFace,
  orbColor: OrbColor,
  appAccent: AccentId,
): GlobeResolution {
  if (face === "concerned")
    return { kind: "concerned", stops: CONCERNED_STOPS };
  if (orbColor === "app") return { kind: "app", accent: appAccent };
  const accent: AccentId = isAccent(orbColor)
    ? orbColor
    : (AUTO_FACE_ACCENT[face] ?? AUTO_FACE_ACCENT.idle);
  return { kind: "table", accent, stops: GLOBE_STOPS[accent] };
}

/**
 * The app-mode stops as CSS (A2b). The orb sets `--vox-app-accent` per instance from the
 * document's `--color-accent-a`, so the globe follows the theme's effective accent live.
 */
export const appModeStops: GlobeStops = {
  hi: "color-mix(in srgb, var(--vox-app-accent) 55%, white)",
  mid: "var(--vox-app-accent)",
  lo: "color-mix(in srgb, var(--vox-app-accent) 55%, black)",
};
