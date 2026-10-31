// Bidi and invisible-character stripping for transcript text (security). Pure: no DOM.
import { describe, expect, it } from "vitest";
import { stripBidiAndInvisible } from "./textSafety";

const STRIPPED_CODE_POINTS: number[] = [
  0x061c, // ARABIC LETTER MARK
  0x200b, // ZERO WIDTH SPACE
  0x200c, // ZERO WIDTH NON-JOINER
  0x200e, // LEFT-TO-RIGHT MARK
  0x200f, // RIGHT-TO-LEFT MARK
  0x202a, // LRE
  0x202b, // RLE
  0x202c, // PDF
  0x202d, // LRO
  0x202e, // RLO
  0x2060, // WORD JOINER
  0x2066, // LRI
  0x2067, // RLI
  0x2068, // FSI
  0x2069, // PDI
  0xfeff, // ZERO WIDTH NO-BREAK SPACE (BOM)
];

describe("stripBidiAndInvisible", () => {
  it("removes every listed bidi and invisible code point", () => {
    for (const cp of STRIPPED_CODE_POINTS) {
      const ch = String.fromCodePoint(cp);
      expect(stripBidiAndInvisible(`a${ch}b`), `U+${cp.toString(16)}`).toBe(
        "ab",
      );
    }
  });

  it("removes a right-to-left override that would reorder the visible text", () => {
    const spoof = "pay\u202Eevil\u202C now";
    expect(stripBidiAndInvisible(spoof)).toBe("payevil now");
  });

  it("keeps ordinary text, punctuation and non-Latin scripts unchanged", () => {
    const plain = "Hello, wörld! 你好 مرحبا — ok?";
    expect(stripBidiAndInvisible(plain)).toBe(plain);
  });

  it("preserves a family emoji ZWJ sequence (U+200D is deliberately kept)", () => {
    // Exception: U+200D joins emoji. Stripping it would split the family into three people.
    const family = "\u{1F468}\u200D\u{1F469}\u200D\u{1F467}";
    expect(stripBidiAndInvisible(family)).toBe(family);
  });

  it("preserves the U+FE0F variation selector used by emoji presentation", () => {
    const heart = "\u2764\uFE0F";
    expect(stripBidiAndInvisible(heart)).toBe(heart);
  });

  it("returns an empty string for an empty input", () => {
    expect(stripBidiAndInvisible("")).toBe("");
  });

  it("handles a 100k character string well under the budget", () => {
    const chunk = "ab\u202Ecd\u200Bé\u{1F468}\u200D\u{1F469} ";
    const big = chunk.repeat(Math.ceil(100_000 / chunk.length));
    const started = performance.now();
    const out = stripBidiAndInvisible(big);
    expect(performance.now() - started).toBeLessThan(200);
    expect(out).not.toMatch(/[\u202E\u200B]/);
    expect(out).toContain("\u200D");
  });
});
