// Copy helper for the per-message Copy button (UI enterprise T2, DESIGN B7). The clipboard is passed
// in, so every failure shape (absent, rejected, throwing) is testable. Never throws.

export interface ClipboardLike {
  writeText(text: string): Promise<void> | void;
}

/** Writes `text` to the clipboard. True on success; false when the clipboard is absent or refuses. */
export async function copyText(
  clipboard: ClipboardLike | null | undefined,
  text: string,
): Promise<boolean> {
  if (!clipboard || typeof clipboard.writeText !== "function") return false;
  try {
    await clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
