import { describe, expect, it } from "vitest";
import { stopVisible } from "./stopControl";
import type { AvatarFace } from "./chatModel";

// T1b (DESIGN Revision 4, B2b): the Stop control is shown only while the displayed face is one of
// the four busy faces AND the window knows the active session id. Matrix over every face.
const ALL_FACES: AvatarFace[] = [
  "idle",
  "listening",
  "thinking",
  "acting",
  "speaking",
  "sleep",
  "concerned",
];
const BUSY: AvatarFace[] = ["listening", "thinking", "acting", "speaking"];

describe("Stop visibility matrix", () => {
  for (const face of ALL_FACES) {
    it(`face ${face} with no session: hidden`, () => {
      expect(stopVisible(face, null)).toBe(false);
    });
    it(`face ${face} with session 7: ${BUSY.includes(face) ? "shown" : "hidden"}`, () => {
      expect(stopVisible(face, 7)).toBe(BUSY.includes(face));
    });
  }

  it("session id 0 still counts as a session", () => {
    expect(stopVisible("speaking", 0)).toBe(true);
  });
});
