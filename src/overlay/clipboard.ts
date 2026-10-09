export type ClipboardWriter = { writeText: (text: string) => Promise<void> };

export const copyOverlayText = async (
  text: string,
  clipboard: ClipboardWriter,
): Promise<boolean> => {
  try {
    await clipboard.writeText(text);
    return true;
  } catch (error) {
    console.error("Failed to copy overlay text to clipboard:", error);
    return false;
  }
};
