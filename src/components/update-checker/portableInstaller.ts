// Portable installs can't self-update in place (no installer, and Windows won't
// let a running exe replace itself). Instead of dumping the user on the releases
// page to hand-pick one of ~27 assets, deep-link the NSIS setup.exe for their
// platform and architecture.
//
// Vox release URLs are sourced from the updater manifest. If no matching
// installer is present, send the user to the repository releases page; if no
// releases are published yet, this page makes that state clear.

export const PORTABLE_RELEASES_URL =
  "https://github.com/iamshreeram/vox/releases/latest";

/**
 * Pick the NSIS installer URL for the running target out of the update manifest.
 * Falls back to Vox's repository releases page whenever there is no matching entry.
 *
 * @param rawJson `Update.rawJson`, the deserialized `latest.json` manifest
 * @param platformName value from `@tauri-apps/plugin-os` `platform()`
 * @param archName value from `@tauri-apps/plugin-os` `arch()` ("x86_64", "aarch64")
 */
export function resolvePortableInstallerUrl(
  rawJson: Record<string, unknown> | undefined,
  platformName: string,
  archName: string,
): string {
  // NSIS is a Windows-only bundle; nothing else has an installer to link to.
  if (platformName !== "windows") return PORTABLE_RELEASES_URL;

  const platforms = rawJson?.platforms;
  if (!platforms || typeof platforms !== "object") return PORTABLE_RELEASES_URL;

  const entry = (platforms as Record<string, unknown>)[
    `windows-${archName}-nsis`
  ];
  if (!entry || typeof entry !== "object") return PORTABLE_RELEASES_URL;

  const url = (entry as Record<string, unknown>).url;
  return typeof url === "string" ? url : PORTABLE_RELEASES_URL;
}
