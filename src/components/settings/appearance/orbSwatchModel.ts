// Pure drawing rules for the orb colour swatches (UI enterprise S2c, DESIGN D4). No React.
// The swatch of each option is a small sphere painted from the same GLOBE_STOPS table the globe
// uses; Auto is four state colours in quarters; Follow app is a ring in the app accent.
import { GLOBE_STOPS, type GlobeStops } from "@/orb/globePalette";
import { contrastRatio } from "@/styles/wcag";
import type { AccentId } from "@/styles/accents";
import type { OrbChoice } from "./appearanceModel";

/** Near-black and white: the two colours the selected check may use inside a swatch. */
export const CHECK_DARK = "#111111";
export const CHECK_LIGHT = "#FFFFFF";

/** The four state colours shown as quarters of the Auto swatch (the globe's own mids). */
export const AUTO_QUARTERS: readonly string[] = [
  GLOBE_STOPS.blue.mid,
  GLOBE_STOPS.emerald.mid,
  GLOBE_STOPS.violet.mid,
  GLOBE_STOPS.amber.mid,
];

export type OrbSwatchSpec =
  | { kind: "sphere"; stops: GlobeStops; check: string }
  | { kind: "auto"; quarters: readonly string[]; check: string }
  | { kind: "app"; check: string };

/**
 * The check colour for a sphere: whichever of near-black or white gives the larger minimum
 * contrast over the sphere's mid and light (hi) stops. Asserted >= 3:1 by the swatch test.
 */
export function checkColourFor(stops: GlobeStops): string {
  const worst = (fg: string): number =>
    Math.min(contrastRatio(fg, stops.mid), contrastRatio(fg, stops.hi));
  return worst(CHECK_DARK) >= worst(CHECK_LIGHT) ? CHECK_DARK : CHECK_LIGHT;
}

/** The drawing spec of one orb option. `app` shows the app accent through its own token. */
export function orbSwatchSpec(choice: OrbChoice): OrbSwatchSpec {
  if (choice === "auto") {
    return { kind: "auto", quarters: AUTO_QUARTERS, check: CHECK_DARK };
  }
  if (choice === "app") {
    return { kind: "app", check: "var(--color-accent-a)" };
  }
  const stops: GlobeStops = GLOBE_STOPS[choice as AccentId];
  return { kind: "sphere", stops, check: checkColourFor(stops) };
}

/**
 * A gradient id that is unique per swatch instance. React's useId output contains colons, which
 * are not safe in url(#...), so they are stripped. Several swatches live in one document.
 */
export function sphereGradientId(instanceId: string): string {
  return `orb-sphere-${instanceId.replace(/[^A-Za-z0-9_-]/g, "")}`;
}
