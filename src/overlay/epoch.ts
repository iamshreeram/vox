// Epoch guard for overlay show/hide ordering (DESIGN.md section 0, O-C2).
// Each call to next() issues a new token; only the most recently issued token
// is current. Async continuations capture a token and must check isCurrent()
// before mutating shared state.

export interface Epoch {
  /** Issue a new token. Every previously issued token becomes stale. */
  next(): number;
  /** True only for the most recently issued token. Never-issued numbers are false. */
  isCurrent(n: number): boolean;
}

// INERT STUB: contract only, intentionally not implemented (red-first).
export function createEpoch(): Epoch {
  throw new Error("not implemented");
}
