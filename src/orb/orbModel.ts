// Pure model of the orb page (DESIGN.md Rev 5, 'Frontend' and 'Reference behaviour').
// No React and no Tauri imports: the page (OrbApp.tsx) feeds it events and applies its output.
// Only the `state` events of converse-chat reach the orb view; entries never do.

import type { AvatarFace } from "../chat/chatModel";

/** Largest gaze shift of the body and the highlight, in SVG user units (viewBox 160). */
export const GAZE_MAX_UNITS = 4;
/** Random gap between two highlight glints (A6 blink), in milliseconds. */
export const GLINT_MIN_MS = 2500;
export const GLINT_MAX_MS = 7000;
/** Length of one glint. */
export const GLINT_MS = 130;
/** Length of the click pounce (A3). */
export const POUNCE_MS = 450;

const clampUnit = (value: number): number => {
  if (!Number.isFinite(value)) return 0;
  return Math.min(1, Math.max(-1, value));
};

/** Maps a normalised cursor position in [-1, 1] to body offsets in units (each axis clamped). */
export function gazeOffset(point: { x: number; y: number }): {
  dx: number;
  dy: number;
} {
  return {
    dx: clampUnit(point.x) * GAZE_MAX_UNITS,
    dy: clampUnit(point.y) * GAZE_MAX_UNITS,
  };
}

/** Delay until the next glint. `rng` returns a value in [0, 1); out-of-range values are clamped. */
export function nextGlintDelay(rng: () => number): number {
  const raw = rng();
  const unit = Number.isFinite(raw) ? Math.min(Math.max(raw, 0), 0.999999) : 0;
  return GLINT_MIN_MS + unit * (GLINT_MAX_MS - GLINT_MIN_MS);
}

export interface OrbState {
  hover: boolean;
  grabbing: boolean;
  pouncing: boolean;
  glint: boolean;
  reducedMotion: boolean;
  /** Id of the press waiting for the native gesture result, or null. */
  gestureId: number | null;
  face: AvatarFace;
}

export const initialOrbState: OrbState = {
  hover: false,
  grabbing: false,
  pouncing: false,
  glint: false,
  reducedMotion: false,
  gestureId: null,
  face: "idle",
};

export type OrbAction =
  | { type: "hover"; on: boolean }
  | { type: "down"; id: number }
  | { type: "gesture"; id: number; result: string }
  | { type: "pounceEnd" }
  | { type: "reducedMotion"; on: boolean }
  | { type: "glint"; on: boolean }
  | { type: "face"; face: AvatarFace };

export function orbReducer(state: OrbState, action: OrbAction): OrbState {
  switch (action.type) {
    case "hover":
      return { ...state, hover: action.on };
    case "down":
      return {
        ...state,
        grabbing: true,
        pouncing: false,
        gestureId: action.id,
      };
    case "gesture": {
      if (state.gestureId !== action.id) return state;
      // Only a still press that ends as a click pounces; drag and cancel just release the grab.
      const pounce = action.result === "click" && !state.reducedMotion;
      return {
        ...state,
        grabbing: false,
        pouncing: pounce,
        gestureId: null,
      };
    }
    case "pounceEnd":
      return state.pouncing ? { ...state, pouncing: false } : state;
    case "reducedMotion":
      return action.on
        ? { ...state, reducedMotion: true, pouncing: false, glint: false }
        : { ...state, reducedMotion: false };
    case "glint":
      return { ...state, glint: action.on && !state.reducedMotion };
    case "face":
      return { ...state, face: action.face };
  }
}

/** Class names for the root element, in a fixed order; only active states are listed. */
export function orbClasses(state: OrbState): string[] {
  const classes: string[] = [];
  if (state.hover) classes.push("is-hover");
  if (state.grabbing) classes.push("is-grabbing");
  if (state.pouncing) classes.push("is-pouncing");
  if (state.glint) classes.push("is-glint");
  if (state.reducedMotion) classes.push("is-reduced");
  return classes;
}

/**
 * A press starts a gesture only with the primary button and without Ctrl: Ctrl+click is the macOS
 * context click, which the native menu handles through the context menu event.
 */
export function isPrimaryPress(event: {
  button: number;
  ctrlKey: boolean;
}): boolean {
  return event.button === 0 && !event.ctrlKey;
}

/**
 * Keyboard map: Enter and Space activate; ContextMenu and Shift+F10 open the menu.
 * Auto-repeat keydowns (holding a key) map to nothing, so one press is one command.
 */
export function keyCommand(event: {
  key: string;
  shiftKey: boolean;
  repeat?: boolean;
}): "activate" | "menu" | null {
  if (event.repeat === true) return null;
  if (event.key === "Enter" || event.key === " ") return "activate";
  if (event.key === "ContextMenu") return "menu";
  if (event.key === "F10" && event.shiftKey) return "menu";
  return null;
}

/** The face view: the latest accepted `state` event of converse-chat. */
export interface FaceView {
  seq: number;
  state: string;
  end: string | null;
}

export function initialFaceView(): FaceView {
  return { seq: 0, state: "idle", end: null };
}

/**
 * Applies one converse-chat payload. Only `{seq, kind: {type: "state", state: string}}` with a
 * seq greater than the current one changes the view; anything else returns `prev` itself.
 */
export function faceFromEvent(prev: FaceView, event: unknown): FaceView {
  if (typeof event !== "object" || event === null) return prev;
  const { seq, kind } = event as { seq?: unknown; kind?: unknown };
  if (typeof seq !== "number" || !Number.isFinite(seq)) return prev;
  if (typeof kind !== "object" || kind === null) return prev;
  const { type, state, end } = kind as {
    type?: unknown;
    state?: unknown;
    end?: unknown;
  };
  if (type !== "state" || typeof state !== "string") return prev;
  if (seq <= prev.seq) return prev;
  return {
    seq,
    state,
    end: typeof end === "string" ? end : null,
  };
}
