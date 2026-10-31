// Copy helper for the per-message Copy button (UI enterprise T2, DESIGN B7). Pure: the clipboard is
// passed in, so every failure shape (absent, rejected, throwing) is testable without a browser.
import { describe, expect, it, vi } from "vitest";
import { copyText } from "./clipboardCopy";

describe("copyText", () => {
  it("writes the exact raw text and reports success", async () => {
    const writeText = vi.fn(async (_value: string) => undefined);
    const raw = "**raw** `markdown`\n- kept";
    await expect(copyText({ writeText }, raw)).resolves.toBe(true);
    expect(writeText).toHaveBeenCalledTimes(1);
    expect(writeText).toHaveBeenCalledWith(raw);
  });

  it("reports failure when the write is rejected (permission denied)", async () => {
    const writeText = vi.fn(async () => {
      throw new Error("NotAllowedError");
    });
    await expect(copyText({ writeText }, "x")).resolves.toBe(false);
  });

  it("reports failure when writeText throws synchronously", async () => {
    const clipboard = {
      writeText: () => {
        throw new Error("boom");
      },
    };
    await expect(copyText(clipboard, "x")).resolves.toBe(false);
  });

  it("reports failure when there is no clipboard (unavailable)", async () => {
    await expect(copyText(undefined, "x")).resolves.toBe(false);
    await expect(copyText(null, "x")).resolves.toBe(false);
    await expect(
      copyText({} as { writeText(t: string): Promise<void> }, "x"),
    ).resolves.toBe(false);
  });
});
