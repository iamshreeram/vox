import assert from "node:assert/strict";
import { copyOverlayText } from "./clipboard";

const successfulClipboard = {
  writeText: async () => {},
};

const failedClipboard = {
  writeText: async () => {
    throw new Error("clipboard unavailable");
  },
};

assert.equal(await copyOverlayText("copied text", successfulClipboard), true);
assert.equal(await copyOverlayText("copied text", failedClipboard), false);

const expectedText = "first line\nsecond line with trailing space ";
let forwardedText = "";
const recordingClipboard = {
  writeText: async (text: string) => {
    forwardedText = text;
  },
};
assert.equal(await copyOverlayText(expectedText, recordingClipboard), true);
assert.equal(forwardedText, expectedText);

console.log("overlay clipboard: all assertions passed");
