// The avatar's per-instance paint (DESIGN Revision 3, G1, A1, A2b, A4). One pure function turns the
// face and the look into the CSS custom properties the avatar's <svg> sets inline, so the orb page,
// the conversation window and the Appearance preview paint the same globe. No DOM, no invoke.
import type { OrbColor } from "../bindings";
import { appModeStops, appRingColour, resolveGlobe } from "../orb/globePalette";
import { orbRingColour, resolveOrbAccent } from "../orb/orbAccentResolve";
import {
  DEFAULT_ACCENT,
  type AccentBucket,
  type AccentId,
} from "../styles/accents";
import type { AvatarFace } from "./chatModel";

/** What the avatar paints from: the orb colour, the app accent and the bucket of its surface. */
export interface AvatarLook {
  orbColor: OrbColor;
  appAccent: AccentId;
  bucket: AccentBucket;
}

/** An absent look reads as auto, with the default app accent and the dark bucket. */
export const AUTO_AVATAR_LOOK: AvatarLook = {
  orbColor: "auto",
  appAccent: DEFAULT_ACCENT,
  bucket: "dark",
};

/**
 * The custom properties the avatar sets on its own <svg>. Table modes and the concerned face carry
 * hex stops; app mode carries var() strings that follow the theme's live accent (A2b).
 */
export function avatarStyle(
  face: AvatarFace,
  look: AvatarLook,
): Record<string, string> {
  const globe = resolveGlobe(face, look.orbColor, look.appAccent);
  if (globe.kind === "app") {
    // The ring is the accent mixed into the bucket's surface colour (A7), never the text role.
    return {
      "--vox-app-accent": "var(--color-accent-a)",
      "--vox-orb-hi": appModeStops.hi,
      "--vox-orb-mid": appModeStops.mid,
      "--vox-orb-lo": appModeStops.lo,
      "--chat-accent": appRingColour(look.bucket),
    };
  }
  const pick = resolveOrbAccent(look.appAccent, look.orbColor, look.bucket);
  return {
    "--vox-orb-hi": globe.stops.hi,
    "--vox-orb-mid": globe.stops.mid,
    "--vox-orb-lo": globe.stops.lo,
    "--chat-accent": orbRingColour(face, pick, look.bucket),
  };
}
