// Vox orb page: the small always-on-top orb window (DESIGN.md Rev 5, 'Frontend').
// It only reads the conversation face and sends gestures to the native side, which decides
// drag, click, menu and activation. The commands used here are fixed names, invoked directly
// (they are not in bindings.ts yet). Rejections are swallowed: the page logs nothing.
import {
  useEffect,
  useReducer,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type {
  CSSProperties,
  KeyboardEvent,
  MouseEvent,
  PointerEvent,
} from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import VoxAvatar from "../chat/VoxAvatar";
import { avatarFace, type AvatarFace } from "../chat/chatModel";
import {
  GAZE_MAX_UNITS,
  GLINT_MS,
  POUNCE_MS,
  faceFromEvent,
  gazeOffset,
  initialFaceView,
  initialOrbState,
  isPrimaryPress,
  keyCommand,
  nextGlintDelay,
  orbClasses,
  orbAccentStyle,
  orbFace,
  orbReducer,
  type FaceView,
} from "./orbModel";
import { orbRingColour, resolveOrbAccent } from "./orbAccentResolve";
import { getOrbLook, subscribeOrbLook } from "./orbAppearance";
import {
  initialRefusal,
  refusalReducer,
  startRefusalTimers,
} from "./orbRefusal";

/** The conversation states: a refusal's text clears when one of them starts. */
const CONVERSATION_STATES = ["listening", "thinking", "speaking", "acting"];

const REDUCED_MOTION_QUERY = "(prefers-reduced-motion: reduce)";
const FACES: readonly AvatarFace[] = [
  "idle",
  "listening",
  "thinking",
  "acting",
  "speaking",
  "sleep",
  "concerned",
];

/** Fire-and-forget native command: a rejection is ignored on purpose. */
function send(command: string, args?: Record<string, unknown>): void {
  invoke(command, args).catch(() => undefined);
}

/** Unknown or missing faces fall back to idle. */
function toFace(value: unknown): AvatarFace {
  const face = FACES.find((candidate) => candidate === value);
  return face ?? "idle";
}

/** The face name carried by a value, or the empty string when it carries none. */
function faceName(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function fieldOf(payload: unknown, name: string): unknown {
  if (typeof payload !== "object" || payload === null) return undefined;
  return (payload as Record<string, unknown>)[name];
}

function toPoint(payload: unknown): { x: number; y: number } | null {
  const x = fieldOf(payload, "x");
  const y = fieldOf(payload, "y");
  return typeof x === "number" && typeof y === "number" ? { x, y } : null;
}

function toGesture(payload: unknown): { id: number; result: string } | null {
  const id = fieldOf(payload, "id");
  const result = fieldOf(payload, "result");
  return typeof id === "number" && typeof result === "string"
    ? { id, result }
    : null;
}

export default function OrbApp() {
  const { t } = useTranslation();
  const [state, dispatch] = useReducer(orbReducer, initialOrbState);
  const [gaze, setGaze] = useState({ x: "0", y: "0" });
  // Refs survive the StrictMode double mount, so ids and the initial fetch are counted once.
  const faceRef = useRef<FaceView>(initialFaceView());
  const pressCounterRef = useRef(0);
  const reducedRef = useRef(false);
  const fetchedRef = useRef(false);
  // The face the conversation shows when no refusal is in progress (restored after `concerned`).
  const baseFaceRef = useRef<AvatarFace>("idle");
  const [refusal, dispatchRefusal] = useReducer(refusalReducer, initialRefusal);
  const look = useSyncExternalStore(subscribeOrbLook, getOrbLook);

  const reducedMotion = state.reducedMotion;
  const pouncing = state.pouncing;

  useEffect(() => {
    let disposed = false;
    const cleanups: Array<() => void> = [];

    const query = window.matchMedia(REDUCED_MOTION_QUERY);
    const applyReduced = (on: boolean): void => {
      reducedRef.current = on;
      dispatch({ type: "reducedMotion", on });
      // Reduced motion holds the orb centred: a gaze already applied must not stay off-centre.
      if (on) setGaze({ x: "0", y: "0" });
      send("orb_set_reduced_motion", { reduced: on });
    };
    applyReduced(query.matches);
    const onQueryChange = (event: MediaQueryListEvent): void => {
      applyReduced(event.matches);
    };
    query.addEventListener("change", onQueryChange);
    cleanups.push(() => query.removeEventListener("change", onQueryChange));

    if (!fetchedRef.current) {
      fetchedRef.current = true;
      invoke("orb_get_state")
        .then((payload: unknown) => {
          // Only before any conversation event: a live event always wins over the snapshot.
          if (faceRef.current.seq === 0) {
            const face = toFace(orbFace(faceName(fieldOf(payload, "face"))));
            baseFaceRef.current = face;
            dispatch({ type: "face", face });
          }
        })
        .catch(() => undefined);
    }

    const subscribe = (pending: Promise<UnlistenFn>): void => {
      pending
        .then((unlisten) => {
          if (disposed) unlisten();
          else cleanups.push(unlisten);
        })
        .catch(() => undefined);
    };

    subscribe(
      listen<unknown>("converse-chat", (event) => {
        const next = faceFromEvent(faceRef.current, event.payload);
        if (next === faceRef.current) return;
        faceRef.current = next;
        const face = toFace(orbFace(avatarFace(next.state, next.end)));
        baseFaceRef.current = face;
        dispatch({ type: "face", face });
        // A conversation that starts ends the refusal text (A-C2).
        if (CONVERSATION_STATES.includes(next.state)) {
          dispatchRefusal({ type: "clear" });
        }
      }),
    );

    subscribe(
      listen<unknown>("orb-refusal", (event) => {
        dispatchRefusal({ type: "refuse", payload: event.payload });
      }),
    );

    subscribe(
      listen<unknown>("orb-gaze", (event) => {
        if (reducedRef.current) return;
        const point = toPoint(event.payload);
        if (point === null) return;
        // gazeOffset clamps each axis; dividing back gives the normalised value for CSS.
        const { dx, dy } = gazeOffset(point);
        setGaze({
          x: String(dx / GAZE_MAX_UNITS),
          y: String(dy / GAZE_MAX_UNITS),
        });
      }),
    );

    subscribe(
      listen<unknown>("orb-gesture", (event) => {
        const gesture = toGesture(event.payload);
        if (gesture === null) return;
        dispatch({ type: "gesture", id: gesture.id, result: gesture.result });
      }),
    );

    return () => {
      disposed = true;
      for (const cleanup of cleanups) cleanup();
    };
  }, []);

  // A refusal: `concerned` for CONCERNED_MS, then the text clears after REFUSAL_TEXT_MS. Each
  // refusal starts its own timers; a newer refusal or a click cancels the older ones.
  useEffect(() => {
    if (refusal.seq === 0) return undefined;
    const seq = refusal.seq;
    dispatch({ type: "face", face: "concerned" });
    return startRefusalTimers(
      {
        setTimeout: (fn, ms) => setTimeout(fn, ms),
        clearTimeout: (handle) =>
          clearTimeout(handle as ReturnType<typeof setTimeout>),
      },
      {
        onConcernedEnd: () =>
          dispatch({ type: "face", face: baseFaceRef.current }),
        onTextEnd: () => dispatchRefusal({ type: "clear", seq }),
      },
    );
  }, [refusal.seq]);

  // A click pounce lasts POUNCE_MS.
  useEffect(() => {
    if (!pouncing) return undefined;
    const timer = setTimeout(() => dispatch({ type: "pounceEnd" }), POUNCE_MS);
    return () => clearTimeout(timer);
  }, [pouncing]);

  // Random glints (A6) for as long as reduced motion is off.
  useEffect(() => {
    if (reducedMotion) return undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const scheduleGlint = (): void => {
      timer = setTimeout(() => {
        dispatch({ type: "glint", on: true });
        timer = setTimeout(() => {
          dispatch({ type: "glint", on: false });
          scheduleGlint();
        }, GLINT_MS);
      }, nextGlintDelay(Math.random));
    };
    scheduleGlint();
    return () => {
      if (timer !== undefined) clearTimeout(timer);
    };
  }, [reducedMotion]);

  const onPointerDown = (event: PointerEvent<HTMLDivElement>): void => {
    if (!isPrimaryPress(event)) return;
    dispatchRefusal({ type: "clear" });
    pressCounterRef.current += 1;
    const id = pressCounterRef.current;
    dispatch({ type: "down", id });
    // If the native side does not take the press (for example in a browser preview), cancel it
    // locally so the orb does not stay in the grabbing state.
    invoke("orb_pointer_down", { id }).catch(() =>
      dispatch({ type: "gesture", id, result: "cancel" }),
    );
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const onContextMenu = (event: MouseEvent<HTMLDivElement>): void => {
    event.preventDefault();
    send("orb_menu");
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    const command = keyCommand(event);
    if (command === null) return;
    event.preventDefault();
    if (command === "activate") dispatchRefusal({ type: "clear" });
    send(command === "activate" ? "orb_activate" : "orb_menu");
  };

  const classes = ["orb-root", ...orbClasses(state)].join(" ");
  // An orb accent the user picked drives the ring; otherwise the orb follows the app accent tokens.
  const accentStyle =
    look.orbAccent === null
      ? orbAccentStyle(state.face)
      : {
          "--chat-accent": orbRingColour(
            state.face,
            resolveOrbAccent(look.accent, look.orbAccent, look.bucket),
            look.bucket,
          ),
        };
  const style = {
    ...accentStyle,
    "--gaze-x": gaze.x,
    "--gaze-y": gaze.y,
  } as CSSProperties;

  return (
    <>
      <div
        data-testid="orb"
        role="button"
        tabIndex={0}
        aria-label={t("orb.label")}
        className={classes}
        style={style}
        onPointerDown={onPointerDown}
        onPointerEnter={() => dispatch({ type: "hover", on: true })}
        onPointerLeave={() => dispatch({ type: "hover", on: false })}
        onContextMenu={onContextMenu}
        onKeyDown={onKeyDown}
      >
        <VoxAvatar face={state.face} />
      </div>
      {/* The face is announced as text, not only as colour and motion (U13). */}
      <span role="status" className="orb-sr-only">
        {t("orb.stateAnnounce", {
          name: t("orb.assistantName"),
          state: t(`orb.face.${state.face}`),
        })}
      </span>
      {/* The refusal text is announced as text (A-C2); the code itself is never shown. */}
      <span
        role="status"
        data-testid="orb-refusal-status"
        className="orb-sr-only"
      >
        {refusal.key === null ? "" : t(refusal.key)}
      </span>
    </>
  );
}
