//! The plan clock of the conversation planner (DESIGN-PR4-PLANNER A4).
//!
//! The clock is the monotonic time plus a per-thread offset. Production never moves the offset;
//! tests move it with `advance_plan_clock`, so the 15-minute plan cap is exercised without
//! sleeping. The module sits outside `managers/converse*`, which the source-contract tests scan.

use std::cell::Cell;
use std::time::{Duration, Instant};

thread_local! {
    /// Offset of the plan clock on this thread. Zero outside tests.
    static OFFSET: Cell<Duration> = const { Cell::new(Duration::ZERO) };
}

/// The plan clock: the monotonic time plus this thread's offset.
pub(crate) fn agent_now() -> Instant {
    Instant::now() + OFFSET.with(Cell::get)
}

/// Test seam: moves the plan clock of this thread forward by `by`.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn advance_plan_clock(by: Duration) {
    OFFSET.with(|offset| offset.set(offset.get() + by));
}
