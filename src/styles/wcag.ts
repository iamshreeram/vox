// WCAG 2.x helpers for "#rrggbb" colours (UI enterprise S2). Pure: used by the accent table test,
// the chat palette tests and the appearance checks. Not a rendering engine: alpha is composited
// over an opaque backdrop only.

const HEX = /^#[0-9a-fA-F]{6}$/;

function channels(hex: string): [number, number, number] {
  if (!HEX.test(hex)) throw new Error(`not a #rrggbb colour: ${hex}`);
  return [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16)) as [
    number,
    number,
    number,
  ];
}

/** WCAG relative luminance of a "#rrggbb" colour, in [0, 1]. */
export function relativeLuminance(hex: string): number {
  const [r, g, b] = channels(hex).map((byte) => {
    const s = byte / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** WCAG 2.x contrast ratio of two "#rrggbb" colours, in [1, 21]. */
export function contrastRatio(a: string, b: string): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** The colour `fg` at `alpha` composited over an opaque `backdrop`, as lowercase "#rrggbb". */
export function blendOver(fg: string, alpha: number, backdrop: string): string {
  const front = channels(fg);
  const back = channels(backdrop);
  const out = front.map((value, i) =>
    Math.round(value * alpha + back[i] * (1 - alpha)),
  );
  return `#${out.map((v) => v.toString(16).padStart(2, "0")).join("")}`;
}
