// The bubble page component (DESIGN.md R6-4, A7-2, A8-2, A8-3). Lifecycle: register the
// `bubbles-state` listener first, then send `bubbles_ready` once for this window instance. Every
// snapshot replaces the list (see bubbleModel.applySnapshot). Text is rendered as React text
// nodes only. Without Tauri (browser preview) the page warns once and keeps rendering nothing.
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  applySnapshot,
  createState,
  fitBubbles,
  visibleBubbles,
  type PageState,
  type Snapshot,
  type VisibleBubble,
} from "./bubbleModel";

export const BUBBLES_EVENT = "bubbles-state";
/** The page shows at most this many bubbles, the newest ones. */
export const MAX_BUBBLES = 3;
/** Space between two bubbles, in CSS pixels. Must match `.bubble-stack` in bubbles.css. */
const GAP_PX = 8;
/** Tick period for fades and expiry, in milliseconds. */
const TICK_MS = 40;
const REDUCED_MOTION_QUERY = "(prefers-reduced-motion: reduce)";

let warned = false;

/** Logs one content-free warning per page load. */
function warnOnce(reason: string): void {
  if (warned) return;
  warned = true;
  console.warn(`bubbles: ${reason}`);
}

/** The window instance from `?instance=N`, or null when missing or malformed. */
export function readInstance(search: string): number | null {
  const raw = new URLSearchParams(search).get("instance");
  if (raw === null || !/^\d+$/.test(raw)) return null;
  const value = Number(raw);
  return Number.isSafeInteger(value) ? value : null;
}

function isSnapshot(value: unknown): value is Snapshot {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as Partial<Snapshot>;
  return (
    typeof candidate.instance === "number" &&
    typeof candidate.rev === "number" &&
    Array.isArray(candidate.items)
  );
}

function prefersReducedMotion(): boolean {
  return (
    typeof window.matchMedia === "function" &&
    window.matchMedia(REDUCED_MOTION_QUERY).matches
  );
}

/** The newest `count` bubbles of an oldest-first list. */
function newest(
  list: readonly VisibleBubble[],
  count: number,
): VisibleBubble[] {
  return list.slice(Math.max(0, list.length - count));
}

export default function BubblesApp(): React.ReactElement {
  const [instance] = useState(() => readInstance(window.location.search));
  const [state, setState] = useState<PageState>(() =>
    createState(instance ?? -1),
  );
  const [now, setNow] = useState(() => Date.now());
  // How many of the newest bubbles fit the measured container. Reset on every new snapshot.
  const [fit, setFit] = useState(MAX_BUBBLES);
  const [reducedMotion] = useState(prefersReducedMotion);
  const logRef = useRef<HTMLDivElement>(null);

  // Listener first, then the single handshake command for this instance.
  useEffect(() => {
    if (instance === null) {
      warnOnce("no instance id in the page URL");
      return undefined;
    }
    let cancelled = false;
    let unlisten: UnlistenFn | null = null;

    listen<unknown>(BUBBLES_EVENT, (event) => {
      if (!isSnapshot(event.payload)) return;
      const snapshot = event.payload;
      const at = Date.now();
      setState((previous) => applySnapshot(previous, snapshot, at));
      setNow(at);
      setFit(MAX_BUBBLES);
    })
      .then((stop) => {
        if (cancelled) {
          stop();
          return;
        }
        unlisten = stop;
        invoke("bubbles_ready", { instance }).catch(() =>
          warnOnce("bubbles_ready was not accepted"),
        );
      })
      .catch(() => warnOnce("no Tauri runtime; bubbles are not shown"));

    return () => {
      cancelled = true;
      if (unlisten !== null) unlisten();
    };
  }, [instance]);

  const visible = useMemo(() => visibleBubbles(state, now), [state, now]);
  const hasAlive = visible.length > 0;

  // One timer, running only while a bubble is alive. It stops on unmount too.
  useEffect(() => {
    if (!hasAlive) return undefined;
    const timer = window.setInterval(() => setNow(Date.now()), TICK_MS);
    return () => window.clearInterval(timer);
  }, [hasAlive]);

  const shown = newest(visible, Math.min(fit, MAX_BUBBLES));

  // Drop the oldest rendered bubbles until the measured stack fits the window. The newest one is
  // always kept. Runs after every render, so a change in heights is measured again.
  useLayoutEffect(() => {
    const container = logRef.current;
    if (container === null) return;
    const nodes = container.querySelectorAll<HTMLElement>(".bubble");
    if (nodes.length <= 1) return;
    const styles = getComputedStyle(container);
    const available =
      container.clientHeight -
      parseFloat(styles.paddingTop) -
      parseFloat(styles.paddingBottom);
    const heights = Array.from(nodes, (node) => node.offsetHeight);
    const keep = fitBubbles(heights, available, GAP_PX);
    if (keep < nodes.length) setFit(keep);
  });

  return (
    <div
      ref={logRef}
      role="log"
      aria-live="polite"
      aria-relevant="additions"
      aria-atomic="false"
      className="bubble-stack"
      data-anchor={state.anchor}
    >
      {shown.map((bubble) => (
        <div
          key={bubble.id}
          data-testid="bubble"
          data-role={bubble.role}
          className={`bubble bubble-${bubble.role}`}
          style={{ opacity: reducedMotion ? 1 : bubble.opacity }}
        >
          {bubble.text}
        </div>
      ))}
    </div>
  );
}
