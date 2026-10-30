// Pure styling data for the settings sidebar. No React imports, so it can be
// unit-tested with plain Bun. Every Tailwind class is written out in full (never
// built by string concatenation) so Tailwind's build-time scanner can find it.

export const SECTION_IDS = [
  "general",
  "appearance",
  "history",
  "models",
  "brain",
  "advanced",
  "postprocessing",
  "agents",
  "debug",
  "about",
] as const;

export type SidebarSectionId = (typeof SECTION_IDS)[number];

/** The visual style of the settings window (UI S4a A5): `restrained` only under data-style. */
export type SidebarStyle = "vibrant" | "restrained";

// One distinct hue per section so the inactive icons are easy to tell apart.
// Vibrant only: restrained icons are neutral (currentColor) and use sidebarIconClass.
export const SECTION_ICON_CLASS: Record<SidebarSectionId, string> = {
  general: "text-sky-500",
  appearance: "text-indigo-500",
  history: "text-violet-500",
  models: "text-emerald-500",
  brain: "text-rose-500",
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

// Restrained: a soft accent tint, accent text and a 3px accent indicator on the left edge.
// No gradient and no shadow. The indicator needs the item to be the positioning context.
const RESTRAINED_BASE = `${ITEM_BASE} relative`;

const RESTRAINED_ACTIVE =
  "bg-accent-soft text-accent-text before:absolute before:start-0 before:top-1.5 before:bottom-1.5 before:w-[3px] before:rounded-full before:bg-accent-a";

const RESTRAINED_INACTIVE =
  "text-text hover:bg-mid-gray/15 opacity-90 hover:opacity-100";

/**
 * The icon colour class of one sidebar row. Vibrant keeps the per-section hue (and the on-accent
 * icon when selected). Restrained is neutral: currentColor, and the accent only when selected.
 */
export function sidebarIconClass(
  id: SidebarSectionId,
  isActive: boolean,
  style: SidebarStyle = "vibrant",
): string {
  if (style === "restrained") {
    return isActive ? "text-accent-text" : "text-current";
  }
  return isActive ? "text-on-accent" : SECTION_ICON_CLASS[id];
}

export function sidebarItemClass(
  isActive: boolean,
  style: SidebarStyle = "vibrant",
): string {
  if (style === "restrained") {
    return isActive
      ? `${RESTRAINED_BASE} ${RESTRAINED_ACTIVE}`
      : `${RESTRAINED_BASE} ${RESTRAINED_INACTIVE}`;
  }
  return isActive
    ? `${ITEM_BASE} ${ITEM_ACTIVE}`
    : `${ITEM_BASE} ${ITEM_INACTIVE}`;
}
