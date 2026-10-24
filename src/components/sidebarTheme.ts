// Pure styling data for the settings sidebar. No React imports, so it can be
// unit-tested with plain Bun. Every Tailwind class is written out in full (never
// built by string concatenation) so Tailwind's build-time scanner can find it.

export const SECTION_IDS = [
  "general",
  "history",
  "models",
  "advanced",
  "postprocessing",
  "agents",
  "debug",
  "about",
] as const;

export type SidebarSectionId = (typeof SECTION_IDS)[number];

// One distinct hue per section so the inactive icons are easy to tell apart.
export const SECTION_ICON_CLASS: Record<SidebarSectionId, string> = {
  general: "text-sky-500",
  history: "text-violet-500",
  models: "text-emerald-500",
  advanced: "text-orange-500",
  postprocessing: "text-pink-500",
  agents: "text-fuchsia-500",
  debug: "text-amber-500",
  about: "text-blue-500",
};

const ITEM_BASE =
  "flex gap-2 items-center p-2 w-full rounded-lg cursor-pointer transition-all";

const ITEM_ACTIVE =
  "bg-linear-to-r from-accent-a to-accent-b text-on-accent shadow-md";

const ITEM_INACTIVE = "hover:bg-mid-gray/15 opacity-90 hover:opacity-100";

export function sidebarItemClass(isActive: boolean): string {
  return isActive
    ? `${ITEM_BASE} ${ITEM_ACTIVE}`
    : `${ITEM_BASE} ${ITEM_INACTIVE}`;
}
