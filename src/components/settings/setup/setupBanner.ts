// Pure model of the one setup-status banner (UI enterprise S4a, U6). General and Models render the
// same component. It lists every missing prerequisite once, with one primary action (Open Brain)
// and one secondary action: "Use own setup" when it lists features that inherit the Brain, or
// "Dismiss" when the only prerequisite is an unconnected Brain (the old Finish setup callout).

import type { BrainFeature } from "@/bindings";

export type BannerPage = "general" | "models";
export type BannerFeatureState = "ready" | "needs_setup" | "needs_consent";

export interface BannerInheritance {
  feature: BrainFeature;
  state: BannerFeatureState;
}

export interface BannerInput {
  page: BannerPage;
  /** Features on this page that inherit the Brain, with their setup state. */
  inherit: readonly BannerInheritance[];
  /** True when no Brain provider or model is configured. */
  brainUnset: boolean;
}

export type BannerItem =
  | { kind: "feature"; feature: BrainFeature; state: BannerFeatureState }
  | { kind: "brain_unset" };

export interface BannerModel {
  visible: boolean;
  items: BannerItem[];
  primary: "open_brain";
  secondary: "use_own" | "dismiss" | null;
}

/** Each missing prerequisite appears once, in the order given. */
export function setupBannerModel(input: BannerInput): BannerModel {
  const items: BannerItem[] = [];
  if (input.page === "general") {
    const seen = new Set<BrainFeature>();
    for (const entry of input.inherit) {
      if (seen.has(entry.feature)) continue;
      seen.add(entry.feature);
      items.push({
        kind: "feature",
        feature: entry.feature,
        state: entry.state,
      });
    }
  } else if (input.brainUnset) {
    items.push({ kind: "brain_unset" });
  }

  const hasFeature = items.some((item) => item.kind === "feature");
  return {
    visible: items.length > 0,
    items,
    primary: "open_brain",
    secondary: hasFeature ? "use_own" : items.length > 0 ? "dismiss" : null,
  };
}
