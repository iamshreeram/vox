// Pure helpers for the Experimental "Voice agent tools" settings (design R4-4 and D5).
// The Rust side (commands/agent_tools.rs) is authoritative for validation; these
// helpers only give instant feedback and keep the section-id check testable.

// Sidebar sections that the open-settings event may switch to. "debug" is
// deliberately absent: it must not be reachable from an external event.
export const SETTINGS_SECTION_IDS = [
  "general",
  "appearance",
  "history",
  "models",
  "advanced",
  "postprocessing",
  "agents",
  "experimental",
  "about",
] as const;

export type SettingsSectionId = (typeof SETTINGS_SECTION_IDS)[number];

/** Maps a legacy section id to the section that replaces it. Other ids pass through. */
export function resolveSectionAlias(
  id: SettingsSectionId,
): Exclude<SettingsSectionId, "experimental"> {
  return id === "experimental" ? "agents" : id;
}

/** True only for the exact string ids in SETTINGS_SECTION_IDS. */
export function isSettingsSectionId(
  value: unknown,
): value is SettingsSectionId {
  return (
    typeof value === "string" &&
    (SETTINGS_SECTION_IDS as readonly string[]).includes(value)
  );
}

/** Maximum number of project folders, matching the Rust validator. */
export const MAX_PROJECT_ROOTS = 20;

/**
 * Whether a typed folder path may be added to the current list: it must be
 * non-empty after trimming, not already present, and the list must have room.
 */
export function canAddProjectRoot(
  roots: readonly string[],
  candidate: string,
): boolean {
  const trimmed = candidate.trim();
  return (
    trimmed !== "" &&
    roots.length < MAX_PROJECT_ROOTS &&
    !roots.includes(trimmed)
  );
}

/** Returns a new list with the trimmed candidate appended. Caller checks canAddProjectRoot first. */
export function appendProjectRoot(
  roots: readonly string[],
  candidate: string,
): string[] {
  return [...roots, candidate.trim()];
}
