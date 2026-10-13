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

export function createEpoch(): Epoch {
  let latest = 0;
  return {
    next(): number {
      latest += 1;
      return latest;
    },
    isCurrent(n: number): boolean {
      return n >= 1 && n === latest;
    },
  };
}
