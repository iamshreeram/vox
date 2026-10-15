//! Bounded, ordered delivery of job observer events (S2b, R2-6, R3-4).
//! One dedicated std thread per JobManager. Finish events are queued and each
//! holds one of FINISH_RESERVATIONS reservations taken at start; Progress is a
//! single coalescing slot. Every observer call runs under catch_unwind, and the
//! lifecycle side only ever takes short locks (never waits on an observer).

use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;

use super::{JobObserver, JobSnapshot};

/// Number of Finish deliveries that may be pending at once.
pub(super) const FINISH_RESERVATIONS: usize = 4;

/// One observer call waiting for the delivery thread.
struct Event {
    observer: Arc<dyn JobObserver>,
    snap: JobSnapshot,
}

/// Queue state guarded by one mutex.
struct State {
    /// Finish events, delivered first and in order.
    finishes: VecDeque<Event>,
    /// Latest progress event; newer progress overwrites older progress.
    progress: Option<Event>,
    /// Finish reservations not currently held by a job or a queued event.
    free_reservations: usize,
    /// Id of the newest job whose Finish was queued; older progress is stale.
    last_finish_id: Option<u64>,
    /// Set when the owning JobManager is dropped; the thread exits once drained.
    closed: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

/// Handle to the delivery thread owned by one JobManager.
pub(super) struct Delivery {
    shared: Arc<Shared>,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Delivery {
    /// Starts the delivery thread. If the thread cannot be created, queued
    /// events are never delivered and reservations are never returned, so later
    /// starts report Busy instead of blocking.
    pub(super) fn spawn() -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                finishes: VecDeque::new(),
                progress: None,
                free_reservations: FINISH_RESERVATIONS,
                last_finish_id: None,
                closed: false,
            }),
            wake: Condvar::new(),
        });
        let worker = Arc::clone(&shared);
        let _ = thread::Builder::new()
            .name("agent-job-delivery".to_string())
            .spawn(move || run(&worker));
        Delivery { shared }
    }

    /// Takes one Finish reservation without blocking. False when none is left.
    pub(super) fn try_reserve(&self) -> bool {
        let mut st = lock(&self.shared.state);
        if st.free_reservations == 0 {
            return false;
        }
        st.free_reservations -= 1;
        true
    }

    /// Returns a reservation that was taken but never used (spawn failed).
    pub(super) fn cancel_reservation(&self) {
        let mut st = lock(&self.shared.state);
        st.free_reservations = (st.free_reservations + 1).min(FINISH_RESERVATIONS);
    }

    /// Replaces the pending progress event. Progress for a job whose Finish is
    /// already queued is dropped, so progress never follows its Finish.
    pub(super) fn offer_progress(&self, observer: Arc<dyn JobObserver>, snap: JobSnapshot) {
        let mut st = lock(&self.shared.state);
        if st.closed || st.last_finish_id.is_some_and(|id| snap.id <= id) {
            return;
        }
        st.progress = Some(Event { observer, snap });
        self.shared.wake.notify_one();
    }

    /// Queues the Finish of a job. Never blocks; the caller holds a reservation
    /// taken by try_reserve, which is returned after the observer returns.
    pub(super) fn enqueue_finish(&self, observer: Arc<dyn JobObserver>, snap: JobSnapshot) {
        let mut st = lock(&self.shared.state);
        if st.progress.as_ref().is_some_and(|p| p.snap.id == snap.id) {
            st.progress = None;
        }
        st.last_finish_id = Some(snap.id);
        st.finishes.push_back(Event { observer, snap });
        self.shared.wake.notify_one();
    }
}

impl Drop for Delivery {
    fn drop(&mut self) {
        let mut st = lock(&self.shared.state);
        st.closed = true;
        self.shared.wake.notify_all();
    }
}

/// Next event to deliver: Finish first, then progress. None means exit.
fn next_event(shared: &Shared) -> Option<(Event, bool)> {
    let mut st = lock(&shared.state);
    loop {
        if let Some(ev) = st.finishes.pop_front() {
            return Some((ev, true));
        }
        if let Some(ev) = st.progress.take() {
            return Some((ev, false));
        }
        if st.closed {
            return None;
        }
        st = shared
            .wake
            .wait(st)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}

/// Delivery thread body: delivers events in order until closed and drained.
fn run(shared: &Shared) {
    while let Some((event, is_finish)) = next_event(shared) {
        // A panicking observer is isolated; the job lifecycle never sees it.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if is_finish {
                event.observer.on_finish(&event.snap);
            } else {
                event.observer.on_progress(&event.snap);
            }
        }));
        if is_finish {
            let mut st = lock(&shared.state);
            st.free_reservations = (st.free_reservations + 1).min(FINISH_RESERVATIONS);
        }
    }
}
