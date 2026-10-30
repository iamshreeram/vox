// Pure helpers for the UI quality gate (UI enterprise S5, DESIGN Revision 3 D5 / A6 / A6c).
// No Playwright runtime import: the only Playwright type is type-only, so bun can run the
// self-tests (tests/helpers/uiQuality.selftest.ts) against exactly the code the e2e spec uses.
// Nothing here changes product code: it measures computed styles that the spec collects in page.

import type { Page } from "@playwright/test";
import type { AppearanceSnapshot } from "@/bindings";
import { resolveAppearance } from "../../src/lib/appearance/palette";
import { chatPaletteVariables } from "../../src/chat/chatTheme";
import { DEFAULT_ACCENT } from "../../src/styles/accents";

// ---------------------------------------------------------------------------
// Colour model
// ---------------------------------------------------------------------------

/** An sRGB colour with channels in 0..255 and alpha in 0..1. */
export interface Rgba {
  r: number;
  g: number;
  b: number;
  a: number;
}

/** An opaque sRGB colour, channels in 0..255. */
export interface Rgb {
  r: number;
  g: number;
  b: number;
}

/** The two backdrops the translucent windows are evaluated over (A6 / D5). */
export const BACKDROPS = {
  white: { r: 255, g: 255, b: 255 } as Rgb,
  black: { r: 0, g: 0, b: 0 } as Rgb,
} as const;

export type BackdropName = keyof typeof BACKDROPS;

/** Parses a CSS colour string as the browser serialises computed colours. Null when unsupported. */
export function parseCssColor(input: string): Rgba | null {
  const text = input.trim().toLowerCase();
  if (text === "transparent") return { r: 0, g: 0, b: 0, a: 0 };
  const hex = text.match(/^#([0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/);
  if (hex) {
    let digits = hex[1];
    if (digits.length <= 4) {
      digits = digits
        .split("")
        .map((c) => c + c)
        .join("");
    }
    const value = (start: number) =>
      parseInt(digits.slice(start, start + 2), 16);
    return {
      r: value(0),
      g: value(2),
      b: value(4),
      a: digits.length === 8 ? value(6) / 255 : 1,
    };
  }
  const fn = text.match(/^(rgba?|color)\((.*)\)$/);
  if (fn) {
    let body = fn[2].trim();
    if (fn[1] === "color") {
      const srgb = body.match(/^srgb\s+(.*)$/);
      if (!srgb) return null;
      body = srgb[1];
      const parts = body.split("/").map((s) => s.trim());
      const channels = parts[0].split(/\s+/).map(Number);
      if (channels.length !== 3 || channels.some((c) => Number.isNaN(c)))
        return null;
      const alpha = parts[1] === undefined ? 1 : parseAlpha(parts[1]);
      if (alpha === null) return null;
      return {
        r: channels[0] * 255,
        g: channels[1] * 255,
        b: channels[2] * 255,
        a: alpha,
      };
    }
    const parts = body.split("/").map((s) => s.trim());
    const tokens = parts[0].split(/[\s,]+/).filter((s) => s.length > 0);
    if (tokens.length !== 3 && tokens.length !== 4) return null;
    const channel = (token: string): number | null => {
      if (token.endsWith("%")) {
        const v = Number(token.slice(0, -1));
        return Number.isNaN(v) ? null : (v / 100) * 255;
      }
      const v = Number(token);
      return Number.isNaN(v) ? null : v;
    };
    const r = channel(tokens[0]);
    const g = channel(tokens[1]);
    const b = channel(tokens[2]);
    if (r === null || g === null || b === null) return null;
    let alpha = 1;
    if (parts[1] !== undefined) {
      const parsed = parseAlpha(parts[1]);
      if (parsed === null) return null;
      alpha = parsed;
    } else if (tokens.length === 4) {
      const parsed = parseAlpha(tokens[3]);
      if (parsed === null) return null;
      alpha = parsed;
    }
    return { r, g, b, a: alpha };
  }
  return null;
}

function parseAlpha(token: string): number | null {
  const t = token.trim();
  if (t.endsWith("%")) {
    const v = Number(t.slice(0, -1));
    return Number.isNaN(v) ? null : Math.min(1, Math.max(0, v / 100));
  }
  const v = Number(t);
  return Number.isNaN(v) ? null : Math.min(1, Math.max(0, v));
}

/** Source-over composite of a translucent colour onto an opaque one. */
export function over(top: Rgba, below: Rgb): Rgb {
  return {
    r: top.r * top.a + below.r * (1 - top.a),
    g: top.g * top.a + below.g * (1 - top.a),
    b: top.b * top.a + below.b * (1 - top.a),
  };
}

/** WCAG 2.x relative luminance of an opaque sRGB colour. */
export function relativeLuminance(c: Rgb): number {
  const lin = (v: number): number => {
    const s = Math.min(255, Math.max(0, v)) / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  };
  return 0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b);
}

/** WCAG contrast ratio between two opaque colours, always >= 1. */
export function contrastRatio(a: Rgb, b: Rgb): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  const hi = Math.max(la, lb);
  const lo = Math.min(la, lb);
  return (hi + 0.05) / (lo + 0.05);
}

// ---------------------------------------------------------------------------
// Paints: solid colours and sRGB linear gradients (A6c). Everything else is unsupported.
// ---------------------------------------------------------------------------

export interface GradientStop {
  color: Rgba;
  /** Position along the gradient line, 0..1. */
  pos: number;
}

export type Paint =
  | { kind: "linear"; stops: GradientStop[] }
  | { kind: "unsupported"; reason: string };

/** Minimum samples per gradient segment (A6c). */
export const MIN_SAMPLES_PER_SEGMENT = 64;
const BISECTION_STEPS = 60;

/** Splits a CSS argument list on top-level commas (ignores commas inside parentheses). */
export function splitTopLevel(input: string, separator = ","): string[] {
  const out: string[] = [];
  let depth = 0;
  let current = "";
  for (const ch of input) {
    if (ch === "(") depth += 1;
    if (ch === ")") depth -= 1;
    if (ch === separator && depth === 0) {
      out.push(current.trim());
      current = "";
    } else {
      current += ch;
    }
  }
  if (current.trim().length > 0) out.push(current.trim());
  return out;
}

/**
 * Parses a computed `background-image` value into paints, TOP-most first (CSS order).
 * `none` gives []. Anything other than a sRGB linear-gradient is returned as 'unsupported'.
 */
export function parseBackgroundImage(value: string): Paint[] {
  const v = value.trim();
  if (v === "" || v === "none") return [];
  if (/url\(/i.test(v)) {
    return [{ kind: "unsupported", reason: "url() image" }];
  }
  return splitTopLevel(v).map((item): Paint => {
    const lower = item.toLowerCase();
    if (lower.startsWith("linear-gradient(") && lower.endsWith(")")) {
      return parseLinearGradient(item.slice("linear-gradient(".length, -1));
    }
    if (/^(repeating-)?radial-gradient\(/.test(lower)) {
      return { kind: "unsupported", reason: "radial gradient" };
    }
    if (/^(repeating-)?conic-gradient\(/.test(lower)) {
      return { kind: "unsupported", reason: "conic gradient" };
    }
    return {
      kind: "unsupported",
      reason: `unknown image: ${item.slice(0, 40)}`,
    };
  });
}

const DIRECTION = /^(to\s+\S+(\s+\S+)?|-?[\d.]+(deg|grad|rad|turn))$/i;

/** Parses the inside of a linear-gradient(...) into an sRGB paint, or 'unsupported'. */
export function parseLinearGradient(inner: string): Paint {
  const args = splitTopLevel(inner);
  if (args.length === 0)
    return { kind: "unsupported", reason: "empty gradient" };
  let index = 0;
  const first = args[0];
  if (/\bin\s+/i.test(first) && !DIRECTION.test(first)) {
    // `in <space>` on its own first argument, or combined with a direction.
    const space = first.match(/\bin\s+([a-z-]+)/i);
    if (space === null || space[1].toLowerCase() !== "srgb") {
      return {
        kind: "unsupported",
        reason: `non-sRGB interpolation space (${space ? space[1] : "?"})`,
      };
    }
    index = 1;
  } else if (DIRECTION.test(first)) {
    index = 1;
  }
  const stopArgs = args.slice(index);
  if (stopArgs.length < 2) {
    return { kind: "unsupported", reason: "fewer than two colour stops" };
  }
  const parsed: Array<{ color: Rgba; pos: number | null }> = [];
  for (const arg of stopArgs) {
    if (/^-?[\d.]+%$/.test(arg)) {
      return { kind: "unsupported", reason: "colour-interpolation hint" };
    }
    const withPos =
      arg.match(/^(.*\))\s+(-?[\d.]+%)$/) ?? arg.match(/^(\S+)\s+(-?[\d.]+%)$/);
    const colourText = withPos ? withPos[1] : arg;
    const color = parseCssColor(colourText);
    if (color === null) {
      return {
        kind: "unsupported",
        reason: `unparseable stop colour: ${colourText.slice(0, 40)}`,
      };
    }
    if (!withPos && /\s-?[\d.]+px$/.test(arg)) {
      return {
        kind: "unsupported",
        reason: "px stop position (not resolvable without geometry)",
      };
    }
    const pos = withPos ? Number(withPos[2].slice(0, -1)) / 100 : null;
    parsed.push({ color, pos });
  }
  // CSS fills missing positions evenly between known neighbours; ends default to 0 and 1.
  const positions = parsed.map((p) => p.pos);
  if (positions[0] === null) positions[0] = 0;
  if (positions[positions.length - 1] === null)
    positions[positions.length - 1] = 1;
  for (let i = 0; i < positions.length; i += 1) {
    if (positions[i] !== null) continue;
    let prev = i - 1;
    while (positions[prev] === null) prev -= 1;
    let next = i + 1;
    while (positions[next] === null) next += 1;
    const step =
      ((positions[next] as number) - (positions[prev] as number)) /
      (next - prev);
    positions[i] = (positions[prev] as number) + step * (i - prev);
  }
  const stops: GradientStop[] = [];
  let floor = 0;
  parsed.forEach((p, i) => {
    const pos = Math.max(floor, positions[i] as number);
    floor = pos;
    stops.push({ color: p.color, pos });
  });
  return { kind: "linear", stops };
}

/** Colour of a sRGB gradient at position t, interpolated premultiplied (CSS default). */
export function gradientColorAt(stops: GradientStop[], t: number): Rgba {
  if (t <= stops[0].pos) return stops[0].color;
  const last = stops[stops.length - 1];
  if (t >= last.pos) return last.color;
  for (let i = 0; i < stops.length - 1; i += 1) {
    const a = stops[i];
    const b = stops[i + 1];
    if (t >= a.pos && t <= b.pos) {
      const span = b.pos - a.pos;
      if (span <= 0) return b.color;
      const f = (t - a.pos) / span;
      const alpha = a.color.a + (b.color.a - a.color.a) * f;
      const premult = (ca: Rgba, cb: Rgba, key: "r" | "g" | "b") => {
        const pa = ca[key] * ca.a;
        const pb = cb[key] * cb.a;
        return pa + (pb - pa) * f;
      };
      const channel = (key: "r" | "g" | "b") =>
        alpha <= 0 ? 0 : premult(a.color, b.color, key) / alpha;
      return { r: channel("r"), g: channel("g"), b: channel("b"), a: alpha };
    }
  }
  return last.color;
}

/** Sample positions for a gradient: >= MIN_SAMPLES_PER_SEGMENT per segment plus the stops. */
export function gradientSamplePoints(stops: GradientStop[]): number[] {
  const points: number[] = [0, 1];
  for (let i = 0; i < stops.length - 1; i += 1) {
    const a = stops[i].pos;
    const b = stops[i + 1].pos;
    if (b <= a) continue;
    for (let k = 0; k <= MIN_SAMPLES_PER_SEGMENT; k += 1) {
      points.push(a + ((b - a) * k) / MIN_SAMPLES_PER_SEGMENT);
    }
  }
  for (const s of stops) points.push(s.pos);
  return [...new Set(points.map((p) => Math.min(1, Math.max(0, p))))].sort(
    (x, y) => x - y,
  );
}

/** Bisection root of f on [lo, hi] given f(lo) and f(hi) have opposite signs. */
export function bisectRoot(
  f: (t: number) => number,
  lo: number,
  hi: number,
): number {
  let a = lo;
  let b = hi;
  let fa = f(a);
  for (let i = 0; i < BISECTION_STEPS; i += 1) {
    const mid = (a + b) / 2;
    const fm = f(mid);
    if (Math.sign(fm) === Math.sign(fa) && fm !== 0) {
      a = mid;
      fa = fm;
    } else {
      b = mid;
    }
  }
  return (a + b) / 2;
}

// ---------------------------------------------------------------------------
// Text contrast over the effective background (A6 (iii) / A6c)
// ---------------------------------------------------------------------------

/** One ancestor-or-self background layer, outermost first, as the browser computed it. */
export interface LayerCase {
  background: string;
  backgroundImage: string;
  /** Cumulative `opacity` of this element and its ancestors (1 when fully opaque). */
  opacity: number;
}

/** A text node as collected from the page. */
export interface TextCase {
  color: string;
  fontSizePx: number;
  fontWeight: number;
  /** Cumulative opacity of the text element and its ancestors. */
  opacity: number;
  layers: LayerCase[];
  /** Computed `filter` values on the chain that are not `none` (unsupported for contrast). */
  filters: string[];
}

export interface ContrastVerdict {
  pass: boolean;
  /** Worst contrast over the continuous background range. */
  ratio: number;
  required: number;
  backdrop: BackdropName;
  /** Gradient position where the worst contrast occurs (null for a flat background). */
  worstAt: number | null;
  /** Number of evaluated background positions (>= 65 per gradient segment). */
  samples: number;
  reason: string | null;
}

/** WCAG large-text rule: 24px, or 18.66px bold. */
export function requiredContrast(
  fontSizePx: number,
  fontWeight: number,
): number {
  const large = fontSizePx >= 24 || (fontSizePx >= 18.66 && fontWeight >= 700);
  return large ? 3 : 4.5;
}

type Op =
  | { kind: "solid"; color: Rgba }
  | { kind: "linear"; stops: GradientStop[] };

/** Flattens layers (outermost first) into paint operations, bottom-most first, or a reason string. */
function flattenLayers(layers: LayerCase[]): Op[] | string {
  const ops: Op[] = [];
  for (const layer of layers) {
    const bg = parseCssColor(layer.background);
    if (bg === null) return `unparseable background-color: ${layer.background}`;
    if (bg.a > 0)
      ops.push({ kind: "solid", color: { ...bg, a: bg.a * layer.opacity } });
    const images = parseBackgroundImage(layer.backgroundImage);
    // Computed order is top-most first; the bottom-most image is painted first.
    for (const paint of [...images].reverse()) {
      if (paint.kind === "unsupported") return paint.reason;
      ops.push({
        kind: "linear",
        stops: paint.stops.map((s) => ({
          ...s,
          color: { ...s.color, a: s.color.a * layer.opacity },
        })),
      });
    }
  }
  return ops;
}

/** The composed background at a gradient position t (flat when there are no gradients). */
function backgroundAt(ops: Op[], base: Rgb, t: number): Rgb {
  let acc = base;
  for (const op of ops) {
    const color = op.kind === "solid" ? op.color : gradientColorAt(op.stops, t);
    acc = over(color, acc);
  }
  return acc;
}

function gradientPoints(ops: Op[]): number[] {
  const points = new Set<number>([0]);
  for (const op of ops) {
    if (op.kind === "linear") {
      for (const p of gradientSamplePoints(op.stops)) points.add(p);
    }
  }
  return [...points].sort((a, b) => a - b);
}

/**
 * Worst-case contrast of one text node over one opaque backdrop. Layers are composed
 * outermost first with their alpha; gradients are evaluated over the continuous range
 * (sampled at >= 64 points per segment, plus bisection for the luminance-equal and the
 * 4.5:1-boundary positions). Unsupported paints and filters fail explicitly.
 */
export function evaluateTextContrast(
  text: TextCase,
  backdrop: BackdropName,
): ContrastVerdict {
  const base = BACKDROPS[backdrop];
  const required = requiredContrast(text.fontSizePx, text.fontWeight);
  const fail = (reason: string, samples = 0): ContrastVerdict => ({
    pass: false,
    ratio: 0,
    required,
    backdrop,
    worstAt: null,
    samples,
    reason,
  });
  if (text.filters.length > 0) {
    return fail(`unsupported filter on text chain: ${text.filters.join(", ")}`);
  }
  const fg = parseCssColor(text.color);
  if (fg === null) return fail(`unparseable text colour: ${text.color}`);
  const ops = flattenLayers(text.layers);
  if (typeof ops === "string") return fail(`unsupported background: ${ops}`);

  const textAlpha = fg.a * text.opacity;
  const textAt = (bg: Rgb): Rgb => over({ ...fg, a: textAlpha }, bg);
  const contrastAt = (t: number): number => {
    const bg = backgroundAt(ops, base, t);
    return contrastRatio(textAt(bg), bg);
  };

  const points = gradientPoints(ops);
  let worst = Infinity;
  let worstAt: number | null = null;
  const consider = (t: number) => {
    const c = contrastAt(t);
    if (c < worst) {
      worst = c;
      worstAt = t;
    }
  };
  for (const t of points) consider(t);

  // Interior roots: background luminance equal to the text luminance, and the required-ratio
  // boundary. Found by sign changes between neighbouring samples, then bisected.
  const lumDiff = (t: number): number => {
    const bg = backgroundAt(ops, base, t);
    return relativeLuminance(bg) - relativeLuminance(textAt(bg));
  };
  const boundary = (t: number): number => contrastAt(t) - required;
  for (const f of [lumDiff, boundary]) {
    for (let i = 0; i < points.length - 1; i += 1) {
      const lo = points[i];
      const hi = points[i + 1];
      const flo = f(lo);
      const fhi = f(hi);
      if (flo === 0 || fhi === 0 || Math.sign(flo) === Math.sign(fhi)) continue;
      consider(bisectRoot(f, lo, hi));
    }
  }

  return {
    pass: worst >= required,
    ratio: worst,
    required,
    backdrop,
    worstAt,
    samples: points.length,
    reason:
      worst >= required ? null : `contrast ${worst.toFixed(2)} < ${required}`,
  };
}

/** Evaluates one text node over both backdrops and returns the worst verdict (A6: white AND black). */
export function evaluateTextOverBackdrops(text: TextCase): ContrastVerdict {
  const verdicts = (Object.keys(BACKDROPS) as BackdropName[]).map((b) =>
    evaluateTextContrast(text, b),
  );
  const failing = verdicts.find((v) => !v.pass);
  if (failing !== undefined) return failing;
  return verdicts.reduce((worst, v) => (v.ratio < worst.ratio ? v : worst));
}

/**
 * Composed backgrounds of an element over a backdrop, one per sampled gradient position
 * (used for the focus-ring contrast). Unsupported paints return a reason instead.
 */
export function backgroundsOf(
  layers: LayerCase[],
  backdrop: BackdropName,
): Rgb[] | string {
  const ops = flattenLayers(layers);
  if (typeof ops === "string") return ops;
  return gradientPoints(ops).map((t) =>
    backgroundAt(ops, BACKDROPS[backdrop], t),
  );
}

// ---------------------------------------------------------------------------
// Focus indicator (A6 (iv)): computed outline or box-shadow present on :focus-visible
// and different from the base state, ring colour >= 3:1 against the adjacent background.
// ---------------------------------------------------------------------------

export interface FocusStyle {
  outlineStyle: string;
  outlineWidth: string;
  outlineColor: string;
  boxShadow: string;
}

export interface FocusVerdict {
  pass: boolean;
  ratio: number | null;
  reason: string | null;
}

export const MIN_FOCUS_RING_RATIO = 3;

/** The first colour in a computed box-shadow (browsers serialise shadow colours as rgb()/rgba()). */
function firstShadowColor(boxShadow: string): string | null {
  const match = boxShadow.match(/rgba?\([^)]*\)|#[0-9a-f]{3,8}\b/i);
  return match ? match[0] : null;
}

/** Checks the focus ring of one element. `backdrops` are the composed backgrounds around it. */
export function checkFocusRing(
  focused: FocusStyle,
  base: FocusStyle,
  layers: LayerCase[],
  backdrop: BackdropName,
): FocusVerdict {
  const outlineVisible =
    focused.outlineStyle !== "none" && parseFloat(focused.outlineWidth) > 0;
  const outlineChanged =
    focused.outlineStyle !== base.outlineStyle ||
    focused.outlineWidth !== base.outlineWidth ||
    focused.outlineColor !== base.outlineColor;
  const shadowChanged =
    focused.boxShadow !== "none" && focused.boxShadow !== base.boxShadow;
  if (!((outlineVisible && outlineChanged) || shadowChanged)) {
    return {
      pass: false,
      ratio: null,
      reason: "no focus indicator on :focus-visible",
    };
  }
  let ringText: string | null = null;
  if (outlineVisible && outlineChanged) {
    ringText = focused.outlineColor;
  } else {
    ringText = firstShadowColor(focused.boxShadow);
  }
  const ring = ringText === null ? null : parseCssColor(ringText);
  if (ring === null) {
    return {
      pass: false,
      ratio: null,
      reason: `unparseable focus ring colour: ${ringText}`,
    };
  }
  const backgrounds = backgroundsOf(layers, backdrop);
  if (typeof backgrounds === "string") {
    return {
      pass: false,
      ratio: null,
      reason: `unsupported background: ${backgrounds}`,
    };
  }
  let worst = Infinity;
  for (const bg of backgrounds) {
    const composed = over(ring, bg);
    worst = Math.min(worst, contrastRatio(composed, bg));
  }
  const pass = worst >= MIN_FOCUS_RING_RATIO;
  return {
    pass,
    ratio: worst,
    reason: pass
      ? null
      : `focus ring contrast ${worst.toFixed(2)} < ${MIN_FOCUS_RING_RATIO}`,
  };
}

// ---------------------------------------------------------------------------
// Bounded Tab walk (A6 (iv)): <= 300 steps, roving tab-stop groups via arrow keys.
// ---------------------------------------------------------------------------

export const MAX_TAB_STEPS = 300;

export interface TabWalkResult {
  /** Enabled interactive elements that the walk never focused. */
  unreached: string[];
  /** True when the walk hit the step cap before covering every element. */
  capped: boolean;
}

/** Compares the enabled interactive set with the ids the walk focused. */
export function analyzeTabWalk(
  enabledIds: string[],
  visitedIds: string[],
  steps: number,
  cap = MAX_TAB_STEPS,
): TabWalkResult {
  const visited = new Set(visitedIds);
  const unreached = enabledIds.filter((id) => !visited.has(id));
  return { unreached, capped: steps >= cap && unreached.length > 0 };
}

/** Roles whose members form a roving tab-stop group (only one member is in the Tab order). */
export const ROVING_GROUP_ROLES = ["radiogroup", "tablist"] as const;

/** The arrow key that moves between members of a roving group. */
export function rovingArrowKey(
  groupRole: string,
  orientation: string | null,
): "ArrowRight" | "ArrowDown" | null {
  if (groupRole !== "radiogroup" && groupRole !== "tablist") return null;
  if (orientation === "vertical") return "ArrowDown";
  return "ArrowRight";
}

// ---------------------------------------------------------------------------
// Accessible names (A6 (v))
// ---------------------------------------------------------------------------

export interface NameParts {
  /** Text of the elements named by aria-labelledby, joined by spaces. */
  labelledBy?: string;
  ariaLabel?: string;
  /** Text of the associated <label> element(s). */
  labelText?: string;
  /** Visible text content (buttons, links, role=button, options). */
  visibleText?: string;
  /** alt text of a descendant image. */
  alt?: string;
  title?: string;
}

/** The accessible name: first non-empty source in the order aria-labelledby, aria-label, label, visible text, alt, title. */
export function accessibleName(parts: NameParts): string {
  const order = [
    parts.labelledBy,
    parts.ariaLabel,
    parts.labelText,
    parts.visibleText,
    parts.alt,
    parts.title,
  ];
  for (const candidate of order) {
    const trimmed = (candidate ?? "").replace(/\s+/g, " ").trim();
    if (trimmed.length > 0) return trimmed;
  }
  return "";
}

export interface NameVerdict {
  pass: boolean;
  name: string;
}

export function checkAccessibleName(parts: NameParts): NameVerdict {
  const name = accessibleName(parts);
  return { pass: name.length > 0, name };
}

// ---------------------------------------------------------------------------
// Font size, target size, overflow (A6 (vi))
// ---------------------------------------------------------------------------

export const MIN_FONT_PX = 12;
export const MIN_TARGET_PX = 24;
export const MAX_REDUCED_MOTION_SECONDS = 0.01;

export function fontSizeOk(px: number): boolean {
  return px >= MIN_FONT_PX;
}

export function targetSizeOk(width: number, height: number): boolean {
  return width >= MIN_TARGET_PX && height >= MIN_TARGET_PX;
}

/** Horizontal overflow in CSS px: how far the scroll width exceeds the viewport (0 when none). */
export function overflowPx(scrollWidth: number, viewportWidth: number): number {
  return Math.max(0, Math.ceil(scrollWidth - viewportWidth));
}

// ---------------------------------------------------------------------------
// Motion under reduced-motion emulation (A6 (vi))
// ---------------------------------------------------------------------------

export interface MotionSample {
  selector: string;
  animationName: string;
  animationDuration: string;
  transitionProperty: string;
  transitionDuration: string;
}

/** Parses a computed time list ("0.3s, 10ms") into seconds. */
export function parseSeconds(list: string): number[] {
  return list
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0)
    .map((s) => {
      if (s.endsWith("ms")) return Number(s.slice(0, -2)) / 1000;
      if (s.endsWith("s")) return Number(s.slice(0, -1));
      return NaN;
    });
}

/** Documented essential motion exceptions. Empty: every animation is suppressed under reduced motion. */
export const ESSENTIAL_MOTION: ReadonlyArray<{
  selector: RegExp;
  reason: string;
}> = [];

/** Returns a violation reason for one element, or null. */
export function motionViolation(sample: MotionSample): string | null {
  if (ESSENTIAL_MOTION.some((e) => e.selector.test(sample.selector)))
    return null;
  const longest = (list: string): number => {
    const values = parseSeconds(list).filter((v) => !Number.isNaN(v));
    return values.length === 0 ? 0 : Math.max(...values);
  };
  if (
    sample.animationName !== "none" &&
    longest(sample.animationDuration) > MAX_REDUCED_MOTION_SECONDS
  ) {
    return `animation ${sample.animationName} ${sample.animationDuration}`;
  }
  if (
    sample.transitionProperty !== "none" &&
    longest(sample.transitionDuration) > MAX_REDUCED_MOTION_SECONDS
  ) {
    return `transition ${sample.transitionProperty} ${sample.transitionDuration}`;
  }
  return null;
}

// ---------------------------------------------------------------------------
// Backdrop-filter / blur scanner (A6 (vi), A-list: no blur anywhere)
// ---------------------------------------------------------------------------

export function blurViolation(
  backdropFilter: string,
  filter: string,
): string | null {
  if (backdropFilter !== "none" && backdropFilter !== "")
    return `backdrop-filter: ${backdropFilter}`;
  if (/blur\(/i.test(filter)) return `filter: ${filter}`;
  return null;
}

// ---------------------------------------------------------------------------
// Console error collector (A6 (vi))
// ---------------------------------------------------------------------------

/** Minimal shape of a Playwright console message / page error used by the collector. */
interface ConsoleLike {
  type(): string;
  text(): string;
  location?(): { url: string };
}

/** Collects console errors and uncaught page errors. Pass the page's `on` method. */
export class ConsoleErrorCollector {
  private readonly messages: string[] = [];

  attach(page: Pick<Page, "on">): void {
    page.on("console", (message: ConsoleLike) => {
      if (message.type() !== "error") return;
      const url = message.location?.().url ?? "";
      this.messages.push(url ? `${message.text()} @ ${url}` : message.text());
    });
    page.on("pageerror", (error: Error) => {
      this.messages.push(`pageerror: ${error.message}`);
    });
  }

  /** Records a message directly (used by the negative fixtures and tests). */
  record(message: string): void {
    this.messages.push(message);
  }

  /** Returns and clears the collected messages. */
  drain(): string[] {
    return this.messages.splice(0, this.messages.length);
  }
}

// ---------------------------------------------------------------------------
// Appearance convergence (A1/A2, D5)
// ---------------------------------------------------------------------------

/** What a consumer page must show once it has applied a committed snapshot. */
export interface ExpectedAppearance {
  dataTheme: string | null;
  dataAccent: string | null;
  dataStyle: string | null;
  chatVariables: Record<string, string>;
}

/** What the consumer page actually shows. `chatVariables` is read from the root inline style. */
export interface ObservedAppearance {
  dataTheme: string | null;
  dataAccent: string | null;
  dataStyle: string | null;
  chatVariables: Record<string, string>;
}

/** The DOM a committed snapshot must produce, mirroring applyResolvedAppearance (palette.ts). */
export function expectedAppearance(
  snapshot: AppearanceSnapshot,
  prefersDark: boolean,
): ExpectedAppearance {
  const resolved = resolveAppearance(snapshot, prefersDark);
  const accent = resolved.accent ?? DEFAULT_ACCENT;
  return {
    dataTheme: resolved.theme === "system" ? null : resolved.theme,
    dataAccent:
      resolved.accent !== null && resolved.accent !== "amber"
        ? resolved.accent
        : null,
    dataStyle: resolved.style === "restrained" ? "restrained" : null,
    chatVariables: chatPaletteVariables(resolved.chatPalette, accent),
  };
}

/** Field-by-field differences between expected and observed; empty when converged. */
export function convergenceMismatches(
  expected: ExpectedAppearance,
  observed: ObservedAppearance,
): string[] {
  const out: string[] = [];
  const attr = (name: string, want: string | null, got: string | null) => {
    if (want !== got)
      out.push(
        `data-${name}: want ${want ?? "(absent)"}, got ${got ?? "(absent)"}`,
      );
  };
  attr("theme", expected.dataTheme, observed.dataTheme);
  attr("accent", expected.dataAccent, observed.dataAccent);
  attr("style", expected.dataStyle, observed.dataStyle);
  for (const [name, want] of Object.entries(expected.chatVariables)) {
    const got = observed.chatVariables[name] ?? "";
    if (normaliseCss(got) !== normaliseCss(want)) {
      out.push(`${name}: want ${want}, got ${got || "(absent)"}`);
    }
  }
  return out;
}

function normaliseCss(value: string): string {
  return value.replace(/\s+/g, "").toLowerCase();
}
