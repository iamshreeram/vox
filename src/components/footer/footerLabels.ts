// Pure footer labels (UI enterprise S4a, U8). The footer shows the app version only once it is
// known: a bare "v" while the version loads (or when it cannot be read) is hidden instead.

/** `v<version>`, or null when the version is not known yet. */
export function versionLabel(
  version: string | null | undefined,
): string | null {
  const trimmed = (version ?? "").trim();
  return trimmed === "" ? null : `v${trimmed}`;
}
