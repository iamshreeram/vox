// Pure helpers for the UI quality gate (UI enterprise S5, DESIGN Revision 5: A6, A6c, A6d, A6e).
// No Playwright runtime import: the only Playwright type is type-only, so bun can run the
// self-tests (tests/helpers/uiQuality.selftest.ts). Nothing here changes product code.
//
// Colour model (A6d/A6e): every CSS colour string is resolved by the BROWSER (resolveColourMap ->
// resolveColoursInPage: a 2D canvas with colorSpace 'srgb' paints the colour and reads one pixel).
// Nothing is parsed by regex on the production path. Out-of-gamut values are clamped by the canvas.
// The canvas stores premultiplied 8-bit values, so channels of translucent colours carry up to
// about 0.5/alpha levels of rounding; the pixel-reference test in ui-quality-reference.spec.ts
// bounds the end-to-end effect.

import type { Page } from "@playwright/test";
import type { AppearanceSnapshot } from "@/bindings";
import { resolveAppearance } from "../../src/lib/appearance/palette";
import { chatPaletteVariables } from "../../src/chat/chatTheme";
import { DEFAULT_ACCENT } from "../../src/styles/accents";

// ---------------------------------------------------------------------------
// Colour model
// ---------------------------------------------------------------------------

/** An sRGB colour with channels in 0..255 and alpha in 0..1 (straight, not premultiplied). */
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

/** Resolves a CSS colour string to sRGB RGBA, or null when it cannot be resolved (the row fails). */
export type ColourResolver = (css: string) => Rgba | null;

/**
 * Grey backdrop levels a translucent surface is evaluated over (A6e(c)): 0, 8, ..., 248, and 255.
 * Interior crossings between levels are found by bisection in the sweep.
 */
export const BACKDROP_LEVELS: readonly number[] = [
  ...Array.from({ length: 32 }, (_, i) => i * 8),
  255,
];

/**
 * Literal parser for hex, rgb()/rgba() and color(srgb). TEST FIXTURE ONLY: it lets the self-tests
 * run in bun without a browser. The production path resolves colours in the browser instead.
 */
export function parseSrgbLiteral(input: string): Rgba | null {
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

/**
 * Resolves colours in the page. Passed to page.evaluate, so it must be self-contained.
 * An sRGB 2D canvas paints each string into one pixel. A string is valid when assigning it to
 * fillStyle changes the value read back, checked against two different sentinels so that an
 * invalid string (which leaves the sentinel in place) is never mistaken for a valid one.
 * Throws when an sRGB canvas cannot be created: the run must fail, not guess.
 */
export function resolveColoursInPage(
  strings: string[],
): Record<string, { r: number; g: number; b: number; a: number } | null> {
  const canvas = document.createElement("canvas");
  canvas.width = 1;
  canvas.height = 1;
  const ctx = canvas.getContext("2d", {
    colorSpace: "srgb",
    willReadFrequently: true,
  } as CanvasRenderingContext2DSettings);
  if (ctx === null)
    throw new Error("sRGB 2D canvas unavailable: colour resolution cannot run");
  const sentinels = ["#010203", "#fdfeff"];
  const accepted = (css: string, sentinel: string): boolean => {
    ctx.fillStyle = sentinel;
    const before = ctx.fillStyle;
    ctx.fillStyle = css;
    return ctx.fillStyle !== before;
  };
  const out: Record<
    string,
    { r: number; g: number; b: number; a: number } | null
  > = {};
  for (const css of strings) {
    if (Object.prototype.hasOwnProperty.call(out, css)) continue;
    if (!accepted(css, sentinels[0]) && !accepted(css, sentinels[1])) {
      out[css] = null;
      continue;
    }
    ctx.clearRect(0, 0, 1, 1);
    ctx.fillStyle = css;
    ctx.fillRect(0, 0, 1, 1);
    // getImageData returns straight (un-premultiplied) channels.
    const px = ctx.getImageData(0, 0, 1, 1).data;
    out[css] = { r: px[0], g: px[1], b: px[2], a: px[3] / 255 };
  }
  return out;
}

/**
 * Resolves every candidate string once in the page and returns a synchronous resolver.
 * Strings the page did not resolve (or were never asked for) resolve to null, so the row fails.
 */
export async function resolveColourMap(
  page: Pick<Page, "evaluate">,
  strings: string[],
): Promise<ColourResolver> {
  const unique = [...new Set(strings)];
  const map = (await page.evaluate(resolveColoursInPage, unique)) as Record<
    string,
    Rgba | null
  >;
  return (css: string) =>
    Object.prototype.hasOwnProperty.call(map, css) ? map[css] : null;
}

/** Resolver backed by a map returned from the page (for tests that already hold resolved values). */
export function resolverOf(map: Record<string, Rgba | null>): ColourResolver {
  return (css: string) =>
    Object.prototype.hasOwnProperty.call(map, css) ? map[css] : null;
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
// Gradients (A6c, A6d, A6e). Linear gradients are sampled at their stop positions. Radial and
// conic gradients are geometry-free: each consecutive stop pair is swept over its full range, so
// the sweep covers every colour any pixel can show. Both interpolate premultiplied sRGB.
// ---------------------------------------------------------------------------

export interface GradientStop {
  color: Rgba;
  /** Position used for sampling, 0..1 (uniform for radial and conic). */
  pos: number;
}

export type Paint =
  | { kind: "gradient"; stops: GradientStop[] }
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
 * Splits one colour-stop argument into its colour text and trailing position tokens, e.g.
 * "rgb(0 0 0) 50%" -> { colourText: "rgb(0 0 0)", positions: ["50%"] }. Structure only: the
 * colour text itself is resolved by the browser.
 */
export function splitStop(arg: string): {
  colourText: string;
  positions: string[];
} {
  let rest = arg.trim();
  const positions: string[] = [];
  for (let i = 0; i < 2; i += 1) {
    const match = rest.match(/\s+(-?(?:\d+\.?\d*|\.\d+)(?:%|[a-z]+)?)$/i);
    if (match === null || match.index === undefined) break;
    positions.unshift(match[1]);
    rest = rest.slice(0, match.index).trim();
  }
  return { colourText: rest, positions };
}

/** Where the colour-interpolation space or a non-sRGB space is named (`in oklab`). */
function interpolationSpace(prelude: string): string | null {
  const match = prelude.match(/\bin\s+([a-z0-9-]+)/i);
  return match === null ? null : match[1].toLowerCase();
}

function isPrelude(kind: "linear" | "radial" | "conic", arg: string): boolean {
  if (kind === "linear")
    return /^(to\s|in\s|-?[\d.]+(deg|grad|rad|turn)\b)/i.test(arg);
  return /^(circle|ellipse|closest-|farthest-|at\s|from\s|in\s|[-+]?[\d.])/i.test(
    arg,
  );
}

export interface GradientSpec {
  kind: "linear" | "radial" | "conic";
  stops: Array<{ colourText: string; pos: number | null }>;
}

/**
 * Parses the inside of a gradient function. Returns a reason string when the gradient cannot be
 * measured: a non-sRGB interpolation space, a colour-interpolation hint, px stop positions on a
 * linear gradient, or fewer than two stops.
 */
export function parseGradientSpec(
  kind: "linear" | "radial" | "conic",
  inner: string,
): GradientSpec | string {
  const args = splitTopLevel(inner);
  if (args.length === 0) return "empty gradient";
  const prelude = isPrelude(kind, args[0]) ? args[0] : "";
  const space = interpolationSpace(prelude);
  if (space !== null && space !== "srgb")
    return `non-sRGB interpolation space (${space})`;
  const stopArgs = prelude ? args.slice(1) : args;
  if (stopArgs.length < 2) return "fewer than two colour stops";
  const stops: GradientSpec["stops"] = [];
  for (const arg of stopArgs) {
    if (/^-?[\d.]+%$/.test(arg)) return "colour-interpolation hint";
    const { colourText, positions } = splitStop(arg);
    if (positions.length > 2) return "unparseable stop positions";
    let pos: number | null = null;
    if (positions.length > 0) {
      if (kind === "linear") {
        if (!positions[0].endsWith("%"))
          return "px stop position (not resolvable without geometry)";
        pos = Number(positions[0].slice(0, -1)) / 100;
      }
    }
    stops.push({ colourText, pos });
  }
  return { kind, stops };
}

/** Parses one computed background-image item into a paint (top-most first in the list). */
export function paintOf(item: string, resolve: ColourResolver): Paint {
  const lower = item.toLowerCase();
  if (lower.startsWith("url(")) return unsupported("url() image");
  if (lower.startsWith("repeating-"))
    return unsupported("repeating gradient (not swept)");
  const match = lower.match(/^(linear|radial|conic)-gradient\(/);
  if (match === null || !lower.endsWith(")"))
    return unsupported(`unknown image: ${item.slice(0, 40)}`);
  const kind = match[1] as "linear" | "radial" | "conic";
  const spec = parseGradientSpec(kind, item.slice(match[0].length, -1));
  if (typeof spec === "string") return unsupported(spec);
  return gradientPaint(spec, resolve);
}

function unsupported(reason: string): Paint {
  return { kind: "unsupported", reason };
}

/** Stop positions: linear keeps its own (CSS fills missing ones evenly); radial and conic use uniform. */
function gradientPaint(spec: GradientSpec, resolve: ColourResolver): Paint {
  const n = spec.stops.length;
  let positions: number[];
  if (spec.kind === "linear") {
    const known: Array<number | null> = spec.stops.map((s) => s.pos);
    if (known[0] === null) known[0] = 0;
    if (known[n - 1] === null) known[n - 1] = 1;
    for (let i = 0; i < n; i += 1) {
      if (known[i] !== null) continue;
      let prev = i - 1;
      while (known[prev] === null) prev -= 1;
      let next = i + 1;
      while (known[next] === null) next += 1;
      const step =
        ((known[next] as number) - (known[prev] as number)) / (next - prev);
      known[i] = (known[prev] as number) + step * (i - prev);
    }
    positions = [];
    let floor = 0;
    for (const value of known) {
      const pos = Math.max(floor, value as number);
      floor = pos;
      positions.push(pos);
    }
  } else {
    positions = spec.stops.map((_, i) => i / (n - 1));
  }
  const stops: GradientStop[] = [];
  for (let i = 0; i < n; i += 1) {
    const color = resolve(spec.stops[i].colourText);
    if (color === null)
      return unsupported(
        `unresolvable stop colour: ${spec.stops[i].colourText.slice(0, 40)}`,
      );
    stops.push({ color, pos: positions[i] });
  }
  return { kind: "gradient", stops };
}

/**
 * Parses a computed `background-image` value into paints, TOP-most first (CSS order).
 * `none` gives []. url(), repeating gradients and unknown images are 'unsupported'.
 */
export function parseBackgroundImage(
  value: string,
  resolve: ColourResolver,
): Paint[] {
  const v = value.trim();
  if (v === "" || v === "none") return [];
  return splitTopLevel(v).map((item) => paintOf(item, resolve));
}

/**
 * Every colour string the measurement will need: background colours, gradient stop colours
 * (structure split only) and the extra strings given (text colours, ring colours). Collect these,
 * resolve them once in the page with resolveColourMap, then pass that resolver to the checks.
 */
export function candidateColours(
  layers: LayerCase[],
  extra: string[] = [],
): string[] {
  const out = new Set<string>(extra);
  for (const layer of layers) {
    out.add(layer.background);
    for (const item of splitTopLevel(layer.backgroundImage)) {
      const match = item.match(/^(linear|radial|conic)-gradient\((.*)\)$/i);
      if (match === null) continue;
      const kind = match[1].toLowerCase() as "linear" | "radial" | "conic";
      const spec = parseGradientSpec(kind, match[2]);
      if (typeof spec === "string") continue;
      for (const stop of spec.stops) out.add(stop.colourText);
    }
  }
  return [...out];
}

/** Colour sRGB gradient at position t, interpolated premultiplied (the CSS default). */
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
// Layer stack: the complete background of one element and its ancestors (A6e(b), (d))
// ---------------------------------------------------------------------------

/**
 * One chain element, ancestors first (outermost first). Every element on the chain is included,
 * even one with no background, because blend modes, masks, filters and opacity on any of them
 * change what the text sits on.
 */
export interface LayerCase {
  background: string;
  backgroundImage: string;
  /** Cumulative `opacity` of this element and its ancestors (1 when fully opaque). */
  opacity: number;
  mixBlendMode?: string;
  maskImage?: string;
  backdropFilter?: string;
  filter?: string;
  /** Element that owns this layer (tag#id.class), for finding rows. */
  by?: string;
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
  /** Worst contrast over every evaluated background. */
  ratio: number;
  required: number;
  /** Grey backdrop level of the worst case; null when the stack is opaque (no backdrop used). */
  backdrop: number | null;
  /** Gradient position where the worst contrast occurs (null for a flat background). */
  worstAt: number | null;
  /** Number of evaluated gradient sample positions (>= 65 per gradient segment). */
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

/**
 * Why a chain element cannot be measured: opacity below 1, a blend mode, a mask, a backdrop
 * filter or a filter. Any of these transforms what lies beneath it, so an opaque colour cannot
 * be trusted to be the visible colour below it. Null when the element is clean.
 */
export function layerEffect(layer: LayerCase): string | null {
  // Opacity below 1 is MEASURED (A6e(b)): buildStack multiplies it into the alpha.
  if (layer.mixBlendMode && layer.mixBlendMode !== "normal")
    return `mix-blend-mode ${layer.mixBlendMode}`;
  if (layer.maskImage && layer.maskImage !== "none")
    return `mask ${layer.maskImage}`;
  if (layer.backdropFilter && layer.backdropFilter !== "none")
    return `backdrop-filter ${layer.backdropFilter}`;
  if (layer.filter && layer.filter !== "none") return `filter ${layer.filter}`;
  return null;
}

type Item =
  | { kind: "solid"; color: Rgba }
  | { kind: "gradient"; stops: GradientStop[] }
  | { kind: "fail"; reason: string };

/** A paint operation of the measured stack, bottom-most first. */
export type Op =
  | { kind: "solid"; color: Rgba }
  | { kind: "gradient"; stops: GradientStop[] };

export interface Stack {
  /** Operations above the bypass point, bottom-most first. */
  ops: Op[];
  /** True when nothing opaque hides the backdrop, so the backdrop levels must be swept. */
  backdrop: boolean;
}

/**
 * Composes the complete background of an element bottom-up (A6e(b)). Scanning from the top,
 * the top-most provably opaque colour is the bypass point: everything beneath it is hidden and is
 * ignored. Every layer above it is measured, and an unsupported paint there fails the row.
 * Any effect on any chain element fails the row (layerEffect). Opacity below 1 is measured: each
 * element's cumulative opacity multiplies its alpha. More than one gradient above the bypass is
 * measured by the cross-product sweep (see sweep).
 */
export function buildStack(
  layers: LayerCase[],
  resolve: ColourResolver,
): Stack | string {
  for (const layer of layers) {
    const effect = layerEffect(layer);
    if (effect !== null) return `effect on the chain: ${effect}`;
  }
  const items: Item[] = [];
  for (const layer of layers) {
    const bg = resolve(layer.background);
    if (bg === null)
      items.push({
        kind: "fail",
        reason: `unparseable background-color: ${layer.background}`,
      });
    else if (bg.a > 0) items.push({ kind: "solid", color: bg });
    // Computed order is top-most first; the bottom-most image is painted first.
    for (const paint of [
      ...parseBackgroundImage(layer.backgroundImage, resolve),
    ].reverse()) {
      if (paint.kind === "unsupported")
        items.push({ kind: "fail", reason: paint.reason });
      else items.push({ kind: "gradient", stops: paint.stops });
    }
  }
  let bypass = -1;
  items.forEach((item, index) => {
    if (item.kind === "solid" && item.color.a === 1) bypass = index;
  });
  const start = Math.max(0, bypass);
  const above = items.slice(start);
  for (const item of above) {
    if (item.kind === "fail") return item.reason;
  }
  const ops: Op[] = above.map((item) =>
    item.kind === "solid"
      ? { kind: "solid", color: item.color }
      : { kind: "gradient", stops: (item as { stops: GradientStop[] }).stops },
  );
  return { ops, backdrop: bypass < 0 };
}

/** Cap on samples per gradient layer in the cross-product grid (A6e(b), N >= 2). */
export const GRID_CAP_PER_LAYER = 48;
/** Cap on the total evaluated combinations (gradient samples x backdrop levels). */
export const MAX_GRID_COMBOS = 200_000;

function clamp01(v: number): number {
  return Math.min(1, Math.max(0, v));
}

/**
 * Per-layer sample set for the cross-product grid: every stop position (clamped) plus a uniform
 * grid, at most `cap` values in total, sorted.
 */
function capSamples(stops: GradientStop[], cap: number): number[] {
  const fixed = [...new Set(stops.map((s) => clamp01(s.pos)))];
  const room = Math.max(2, cap - fixed.length);
  const uniform = Array.from({ length: room }, (_, i) => i / (room - 1));
  return [...new Set([...fixed, ...uniform])].sort((x, y) => x - y);
}

/** Position of the lowest and highest luminance of one gradient over black (alpha composited). */
function luminanceExtremes(stops: GradientStop[], points: number[]): number[] {
  let lo = points[0];
  let hi = points[0];
  let loL = Infinity;
  let hiL = -Infinity;
  for (const t of points) {
    const l = relativeLuminance(
      over(gradientColorAt(stops, t), { r: 0, g: 0, b: 0 }),
    );
    if (l < loL) {
      loL = l;
      lo = t;
    }
    if (l > hiL) {
      hiL = l;
      hi = t;
    }
  }
  return [lo, hi];
}

/** Cartesian product of value lists (each combination is one entry per list). */
function cartesian(lists: number[][]): number[][] {
  let out: number[][] = [[]];
  for (const list of lists) {
    const next: number[][] = [];
    for (const prefix of out) for (const v of list) next.push([...prefix, v]);
    out = next;
  }
  return out;
}

/**
 * The composed background with gradient k at position ts[k] (gradients in stack order, bottom
 * first), over a grey backdrop level `grey`.
 */
function composeWith(ops: Op[], grey: number, ts: number[]): Rgb {
  let acc: Rgb = { r: grey, g: grey, b: grey };
  let k = 0;
  for (const op of ops) {
    const color =
      op.kind === "solid" ? op.color : gradientColorAt(op.stops, ts[k++]);
    acc = over(color, acc);
  }
  return acc;
}

/** A measured property of the composed background, evaluated at every sample. */
export interface Metric {
  /** Contrast ratio at one composed background (higher is better). */
  ratioAt: (bg: Rgb) => number;
  /** The threshold the worst ratio must reach. */
  required: number;
  /** Signed luminance difference, used to find the luminance-equal crossings. */
  balanceAt: (bg: Rgb) => number;
  /** Further signed differences whose luminance-equal crossings are also searched (two-tone rings). */
  extraBalanceAt?: Array<(bg: Rgb) => number>;
}

export interface SweepResult {
  worst: number;
  worstAt: number | null;
  worstGrey: number | null;
  points: number;
}

/**
 * Worst ratio of a metric over the stack (A6c, A6e(c)). Evaluated on the grid of gradient sample
 * points x backdrop levels, plus the interior crossings (bisection) where the metric's boundary
 * or the luminance-equal line is crossed along the gradient at each level and along the backdrop
 * at each sample point.
 */
/**
 * Worst ratio of a metric over the complete composed stack (A6c, A6e(b), A6e(c)).
 *
 * DENSE-GRID CHECK, NOT A PROOF. Each gradient layer is sampled (one layer: >= 64 samples per
 * stop pair; two or more layers: at most GRID_CAP_PER_LAYER samples, always including the stop
 * positions). The composite is evaluated over the cross-product of the per-layer samples, times
 * the grey backdrop levels when the stack is translucent. If there are more than three gradient
 * layers, or the product exceeds MAX_GRID_COMBOS, the samples are subsampled uniformly and every
 * combination of the per-layer luminance extremes is added. Interior crossings are then found by
 * bisection along each axis (each gradient, then the backdrop) with the other axes held at their
 * samples. Tolerance: 1/255 per channel in the grid. A pass means no sampled or bisected point
 * fails; it does not prove that no point between samples fails.
 */
export function sweep(stack: Stack, metric: Metric): SweepResult {
  const grads = stack.ops.filter(
    (op): op is Extract<Op, { kind: "gradient" }> => op.kind === "gradient",
  );
  const n = grads.length;
  const levels: readonly number[] = stack.backdrop ? BACKDROP_LEVELS : [0];

  // Per-gradient sample sets (stack order, bottom first).
  let sets: number[][];
  let extra: number[][] = [];
  if (n <= 1) {
    sets = grads.map((g) => gradientSamplePoints(g.stops));
  } else {
    const product = (cap: number) => Math.pow(cap, n) * levels.length;
    let cap = GRID_CAP_PER_LAYER;
    const subsampled = n > 3 || product(cap) > MAX_GRID_COMBOS;
    if (subsampled)
      cap = Math.max(
        2,
        Math.floor(Math.pow(MAX_GRID_COMBOS / levels.length, 1 / n)),
      );
    sets = grads.map((g) => capSamples(g.stops, cap));
    if (subsampled) {
      const points = grads.map((g, k) => {
        const extremes = luminanceExtremes(g.stops, sets[k]);
        sets[k] = [...new Set([...sets[k], ...extremes])].sort((x, y) => x - y);
        return extremes;
      });
      extra = cartesian(points);
    }
  }

  const combos = [...cartesian(sets), ...extra];
  let worst = Infinity;
  let worstAt: number | null = null;
  let worstGrey: number | null = null;
  const evalAt = (ts: number[], g: number) => {
    const ratio = metric.ratioAt(composeWith(stack.ops, g, ts));
    if (ratio < worst) {
      worst = ratio;
      worstAt = ts.length > 0 ? ts[0] : null;
      worstGrey = stack.backdrop ? g : null;
    }
  };
  const boundary = (bg: Rgb) => metric.ratioAt(bg) - metric.required;
  const crossings: Array<(bg: Rgb) => number> = [
    boundary,
    metric.balanceAt,
    ...(metric.extraBalanceAt ?? []),
  ];

  for (const g of levels) for (const ts of combos) evalAt(ts, g);

  // Interior crossings along each gradient axis, other axes held at their samples.
  for (const g of levels) {
    for (const f of crossings) {
      for (let k = 0; k < n; k += 1) {
        const others = sets.filter((_, i) => i !== k);
        for (const fixed of cartesian(others)) {
          const at = (t: number) => {
            const ts = [...fixed.slice(0, k), t, ...fixed.slice(k)];
            return f(composeWith(stack.ops, g, ts));
          };
          const axis = sets[k];
          for (let i = 0; i < axis.length - 1; i += 1) {
            const flo = at(axis[i]);
            const fhi = at(axis[i + 1]);
            if (flo === 0 || fhi === 0 || Math.sign(flo) === Math.sign(fhi))
              continue;
            const root = bisectRoot(at, axis[i], axis[i + 1]);
            evalAt([...fixed.slice(0, k), root, ...fixed.slice(k)], g);
          }
        }
      }
    }
  }

  // Interior crossings along the backdrop at each gradient combination.
  if (stack.backdrop) {
    for (const ts of combos) {
      for (const f of crossings) {
        const at = (g: number) => f(composeWith(stack.ops, g, ts));
        for (let i = 0; i < levels.length - 1; i += 1) {
          const flo = at(levels[i]);
          const fhi = at(levels[i + 1]);
          if (flo === 0 || fhi === 0 || Math.sign(flo) === Math.sign(fhi))
            continue;
          evalAt(ts, bisectRoot(at, levels[i], levels[i + 1]));
        }
      }
    }
  }

  return { worst, worstAt, worstGrey, points: combos.length };
}

/** Composed background range (per channel) over the gradient sample points, over black. */
export function composedRange(
  layers: LayerCase[],
  resolve: ColourResolver,
): { min: Rgb; max: Rgb } | string {
  const stack = buildStack(layers, resolve);
  if (typeof stack === "string") return stack;
  const grads = stack.ops.filter(
    (op): op is Extract<Op, { kind: "gradient" }> => op.kind === "gradient",
  );
  const sets = grads.map((g) => gradientSamplePoints(g.stops));
  const colours = cartesian(sets).map((ts) => composeWith(stack.ops, 0, ts));
  return {
    min: {
      r: Math.min(...colours.map((c) => c.r)),
      g: Math.min(...colours.map((c) => c.g)),
      b: Math.min(...colours.map((c) => c.b)),
    },
    max: {
      r: Math.max(...colours.map((c) => c.r)),
      g: Math.max(...colours.map((c) => c.g)),
      b: Math.max(...colours.map((c) => c.b)),
    },
  };
}

/**
 * Worst-case contrast of one text node over the complete composed background (A6, A6c, A6e).
 * Unsupported paints, unresolvable colours and effects on the chain fail explicitly.
 */
export function evaluateTextContrast(
  text: TextCase,
  resolve: ColourResolver,
): ContrastVerdict {
  const required = requiredContrast(text.fontSizePx, text.fontWeight);
  const fail = (reason: string): ContrastVerdict => ({
    pass: false,
    ratio: 0,
    required,
    backdrop: null,
    worstAt: null,
    samples: 0,
    reason,
  });
  if (text.filters.length > 0) {
    return fail(`unsupported filter on text chain: ${text.filters.join(", ")}`);
  }
  const fgResolved = resolve(text.color);
  if (fgResolved === null)
    return fail(`unresolvable text colour: ${text.color}`);
  // Text opacity (cumulative over the text chain) multiplies the text colour's alpha (A6e(b)).
  const fg: Rgba = { ...fgResolved, a: fgResolved.a * text.opacity };
  const stack = buildStack(text.layers, resolve);
  if (typeof stack === "string")
    return fail(`unsupported background: ${stack}`);

  const textOver = (bg: Rgb): Rgb => over(fg, bg);
  const metric: Metric = {
    required,
    ratioAt: (bg) => contrastRatio(textOver(bg), bg),
    balanceAt: (bg) => relativeLuminance(bg) - relativeLuminance(textOver(bg)),
  };
  const result = sweep(stack, metric);
  const pass = result.worst >= required;
  return {
    pass,
    ratio: result.worst,
    required,
    backdrop: result.worstGrey,
    worstAt: result.worstAt,
    samples: result.points,
    reason: pass ? null : `contrast ${result.worst.toFixed(2)} < ${required}`,
  };
}

// ---------------------------------------------------------------------------
// Focus indicator (A6 (iv)): computed outline or box-shadow present on :focus-visible
// and different from the base state, ring colour >= 3:1 against the composed background.
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

/** Colour and spread of each outer (non-inset) shadow in a computed box-shadow list. */
export function parseShadows(
  boxShadow: string,
): Array<{ color: string; spread: number }> {
  if (boxShadow === "none") return [];
  const out: Array<{ color: string; spread: number }> = [];
  for (const part of splitTopLevel(boxShadow)) {
    const text = part.trim();
    if (text === "" || /\binset\b/.test(text)) continue;
    const colour = text.match(/rgba?\([^)]*\)|#[0-9a-f]{3,8}\b/i);
    if (colour === null) continue;
    const lengths = text
      .replace(colour[0], " ")
      .split(/\s+/)
      .filter((t) => /px$/.test(t))
      .map((t) => parseFloat(t));
    out.push({ color: colour[0], spread: lengths[3] ?? 0 });
  }
  return out;
}

/**
 * The colour texts of the ring that a focus change shows, or null when there is no indicator.
 * One text for a single-tone ring; two (outline, then a spread shadow) for a two-tone ring, where
 * the outline is at least 2px and the shadow spread at least 2px. Used by checkFocusRing and to
 * collect candidate colours before resolution.
 */
export function focusRingTexts(
  focused: FocusStyle,
  base: FocusStyle,
): string[] | null {
  const outlineVisible =
    focused.outlineStyle !== "none" && parseFloat(focused.outlineWidth) > 0;
  const outlineChanged =
    focused.outlineStyle !== base.outlineStyle ||
    focused.outlineWidth !== base.outlineWidth ||
    focused.outlineColor !== base.outlineColor;
  const shadowChanged =
    focused.boxShadow !== "none" && focused.boxShadow !== base.boxShadow;
  const spreadShadow = shadowChanged
    ? parseShadows(focused.boxShadow).find((s) => s.spread >= 2)
    : undefined;
  if (
    outlineVisible &&
    outlineChanged &&
    parseFloat(focused.outlineWidth) >= 2 &&
    spreadShadow !== undefined
  ) {
    return [focused.outlineColor, spreadShadow.color];
  }
  if (outlineVisible && outlineChanged) return [focused.outlineColor];
  if (shadowChanged) {
    const first = firstShadowColor(focused.boxShadow);
    return first === null ? null : [first];
  }
  return null;
}

/** Checks the focus ring of one element over its composed background (A6e(f)). */
export function checkFocusRing(
  focused: FocusStyle,
  base: FocusStyle,
  layers: LayerCase[],
  resolve: ColourResolver,
): FocusVerdict {
  const ringTexts = focusRingTexts(focused, base);
  if (ringTexts === null) {
    return {
      pass: false,
      ratio: null,
      reason: "no focus indicator on :focus-visible",
    };
  }
  const tones: Rgba[] = [];
  for (const text of ringTexts) {
    const tone = resolve(text);
    if (tone === null) {
      return {
        pass: false,
        ratio: null,
        reason: `unresolvable focus ring colour: ${text}`,
      };
    }
    tones.push(tone);
  }
  const stack = buildStack(layers, resolve);
  if (typeof stack === "string") {
    return {
      pass: false,
      ratio: null,
      reason: `unsupported background: ${stack}`,
    };
  }
  // Two tones: at every backdrop at least one tone must reach the ratio (max over tones).
  const metric: Metric = {
    required: MIN_FOCUS_RING_RATIO,
    ratioAt: (bg) =>
      Math.max(...tones.map((ring) => contrastRatio(over(ring, bg), bg))),
    balanceAt: (bg) =>
      relativeLuminance(over(tones[0], bg)) - relativeLuminance(bg),
    extraBalanceAt: tones
      .slice(1)
      .map(
        (ring) => (bg: Rgb) =>
          relativeLuminance(over(ring, bg)) - relativeLuminance(bg),
      ),
  };
  const worst = sweep(stack, metric).worst;
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

/**
 * Native radio groups (and role=radio members of a radiogroup) are reachable when one member is
 * a Tab stop the walk focused: arrow keys then move through every enabled member of the group.
 * Each group is the enabled member ids of one radio group; a group with no visited member adds
 * nothing, so its members stay unreached.
 */
export function expandRovingGroups(
  visitedIds: string[],
  groups: string[][],
): string[] {
  const visited = new Set(visitedIds);
  for (const group of groups) {
    if (group.some((id) => visited.has(id))) {
      for (const id of group) visited.add(id);
    }
  }
  return [...visited];
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

// ---------------------------------------------------------------------------
// Finding rows (R2g): element evidence, stable selector paths, paint attribution, crops.
// Pure functions over plain data: the page collects an ElementRaw, these format it.
// ---------------------------------------------------------------------------

export const ROW_HTML_LIMIT = 200;
export const ROW_TEXT_LIMIT = 60;
export const ROW_CSS_LIMIT = 300;
export const ROW_PARENT_LIMIT = 4;
/** Maximum element crops written per run (tests/ui-quality.spec.ts). */
export const CROP_CAP = 120;

/** One element's identity: tag, id, classes and its 1-based nth-of-type among same-tag siblings. */
export interface NodeRaw {
  tag: string;
  id: string;
  classes: string[];
  nth: number;
}

/** One element as read from the page. `ancestors` are nearest first. */
export interface ElementRaw {
  node: NodeRaw;
  ancestors: NodeRaw[];
  outerHTML: string;
  text: string;
  /** data-* attributes as name="value". */
  dataAttrs: string[];
  rect: { x: number; y: number; width: number; height: number };
}

export interface ElementDetail {
  /** Stable selector path: self and up to two ancestors, tag#id.class:nth-of-type(n). */
  path: string;
  html: string;
  parents: string[];
  rect: { x: number; y: number; width: number; height: number };
  text: string;
  src: string;
}

/** The computed CSS that made a paint unmeasurable, and the chain element that painted it. */
export interface PaintCause {
  css: string;
  by: string | null;
}

export function truncate(value: string, limit: number): string {
  return value.length > limit ? value.slice(0, limit) : value;
}

/** tag#id.class (first two classes). */
export function formatNode(node: NodeRaw): string {
  const cls = node.classes.filter(Boolean).slice(0, 2).join(".");
  return node.tag + (node.id ? `#${node.id}` : "") + (cls ? `.${cls}` : "");
}

/** Self and up to two ancestors (three levels), each with its nth-of-type, joined by " > ". */
export function selectorPath(node: NodeRaw, ancestors: NodeRaw[]): string {
  const chain = [...ancestors.slice(0, 2).reverse(), node];
  return chain.map((n) => `${formatNode(n)}:nth-of-type(${n.nth})`).join(" > ");
}

export function buildDetail(raw: ElementRaw): ElementDetail {
  const classAttr =
    raw.node.classes.length > 0 ? `class="${raw.node.classes.join(" ")}"` : "";
  return {
    path: selectorPath(raw.node, raw.ancestors),
    html: truncate(raw.outerHTML, ROW_HTML_LIMIT),
    parents: raw.ancestors.slice(0, ROW_PARENT_LIMIT).map(formatNode),
    rect: { ...raw.rect },
    text: truncate(raw.text.replace(/\s+/g, " ").trim(), ROW_TEXT_LIMIT),
    src: truncate(
      [...raw.dataAttrs, classAttr].filter((s) => s.length > 0).join(" "),
      ROW_HTML_LIMIT,
    ),
  };
}

function effectCss(layer: LayerCase): string | null {
  if (layer.mixBlendMode && layer.mixBlendMode !== "normal")
    return `mix-blend-mode: ${layer.mixBlendMode}`;
  if (layer.maskImage && layer.maskImage !== "none")
    return `mask-image: ${layer.maskImage}`;
  if (layer.backdropFilter && layer.backdropFilter !== "none")
    return `backdrop-filter: ${layer.backdropFilter}`;
  if (layer.filter && layer.filter !== "none") return `filter: ${layer.filter}`;
  return null;
}

/**
 * The computed property and value that make the stack of one element unmeasurable, mirroring
 * buildStack's order: effects on any chain layer first, then unsupported paints above the bypass.
 * Null when buildStack would measure the stack. `css` is truncated to ROW_CSS_LIMIT.
 */
export function offendingPaint(
  layers: LayerCase[],
  resolve: ColourResolver,
): PaintCause | null {
  for (const layer of layers) {
    const css = effectCss(layer);
    if (css !== null)
      return { css: truncate(css, ROW_CSS_LIMIT), by: layer.by ?? null };
  }
  type Entry = { opaque: boolean; fail: PaintCause | null };
  const entries: Entry[] = [];
  for (const layer of layers) {
    const by = layer.by ?? null;
    const bg = resolve(layer.background);
    if (bg === null) {
      entries.push({
        opaque: false,
        fail: {
          css: truncate(`background-color: ${layer.background}`, ROW_CSS_LIMIT),
          by,
        },
      });
    } else if (bg.a > 0) {
      entries.push({ opaque: bg.a === 1, fail: null });
    }
    const imageValue = layer.backgroundImage.trim();
    const images =
      imageValue === "" || imageValue === "none"
        ? []
        : splitTopLevel(imageValue);
    for (const item of images.reverse()) {
      const paint = paintOf(item, resolve);
      if (paint.kind === "unsupported") {
        entries.push({
          opaque: false,
          fail: {
            css: truncate(`background-image: ${item}`, ROW_CSS_LIMIT),
            by,
          },
        });
      } else {
        entries.push({ opaque: false, fail: null });
      }
    }
  }
  let bypass = -1;
  entries.forEach((e, index) => {
    if (e.opaque) bypass = index;
  });
  for (const e of entries.slice(Math.max(0, bypass))) {
    if (e.fail !== null) return e.fail;
  }
  return null;
}

export interface FindingRowInput {
  kind: string;
  surface: string;
  variant: string;
  selector: string;
  measured: string;
  required: string;
}

/** One JSONL finding row. Element and paint fields are null when the finding has no element. */
export interface FindingRow extends FindingRowInput {
  html: string | null;
  parents: string[] | null;
  rect: ElementDetail["rect"] | null;
  text: string | null;
  src: string | null;
  paintCss: string | null;
  paintBy: string | null;
  screenshot: string | null;
}

export function buildFindingRow(
  input: FindingRowInput,
  detail: ElementDetail | null,
  paint: PaintCause | null,
  screenshot: string | null,
): FindingRow {
  return {
    ...input,
    html: detail?.html ?? null,
    parents: detail?.parents ?? null,
    rect: detail?.rect ?? null,
    text: detail?.text ?? null,
    src: detail?.src ?? null,
    paintCss: paint?.css ?? null,
    paintBy: paint?.by ?? null,
    screenshot,
  };
}

/** Element crop rectangle clamped to the viewport with padding; null when nothing is visible. */
export function cropClip(
  rect: ElementDetail["rect"],
  viewport: { width: number; height: number },
  pad = 8,
): { x: number; y: number; width: number; height: number } | null {
  const x0 = Math.max(0, rect.x - pad);
  const y0 = Math.max(0, rect.y - pad);
  const x1 = Math.min(viewport.width, rect.x + rect.width + pad);
  const y1 = Math.min(viewport.height, rect.y + rect.height + pad);
  if (x1 <= x0 || y1 <= y0) return null;
  return { x: x0, y: y0, width: x1 - x0, height: y1 - y0 };
}
