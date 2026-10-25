//! Settings generation gate (DESIGN-PR4-PLANNER.md Revision 4, A3-R3).
//!
//! A seqlock over the launch-affecting agent settings: the generation is even when the
//! settings are stable and odd while an update is in progress. A prepared launch records the
//! even generation it was prepared under; admission refuses it when the generation has moved
//! on or an update is running.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

/// The one gate of the process. The settings writers, the launch preparation and the production
/// `AcpAgentManager` all share it (see `process_gate`).
static PROCESS_GATE: OnceLock<Arc<AgentSettingsGate>> = OnceLock::new();

/// The process-wide settings gate. Every caller gets the same `Arc`, so a setter's update is
/// seen by every prepared launch and by the manager's admission check.
pub fn process_gate() -> Arc<AgentSettingsGate> {
    PROCESS_GATE
        .get_or_init(|| Arc::new(AgentSettingsGate::new()))
        .clone()
}

/// Seqlock over the agent launch settings. Writers are serialized by `update_lock`.
#[derive(Debug, Default)]
pub struct AgentSettingsGate {
    generation: AtomicU64,
    update_lock: Mutex<()>,
}

/// Ends an update on every exit path, including a panic in the update closure.
struct UpdateEnd<'a> {
    generation: &'a AtomicU64,
}

impl Drop for UpdateEnd<'_> {
    fn drop(&mut self) {
        // Why: back to even (stable). Runs while the update lock is still held.
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

impl AgentSettingsGate {
    /// A stable gate at generation 0.
    pub fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
            update_lock: Mutex::new(()),
        }
    }

    /// The current generation. Even means stable, odd means an update is in progress.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Runs `update` as one settings change. The generation is odd for its whole duration
    /// and even again afterwards, also when `update` panics. Updates never overlap.
    pub fn with_update<T>(&self, update: impl FnOnce() -> T) -> T {
        let _serial = lock_ignoring_poison(&self.update_lock);
        self.generation.fetch_add(1, Ordering::SeqCst);
        let _end = UpdateEnd {
            generation: &self.generation,
        };
        update()
    }

    /// The generation to prepare a launch under, or `None` while an update is in progress.
    pub fn prepare_stable(&self) -> Option<u64> {
        let generation = self.generation();
        (generation.is_multiple_of(2)).then_some(generation)
    }

    /// True when `epoch` is even and still the current generation.
    // Why: admission uses `admission_allowed` under the manager lock; this is the same check for
    // callers outside it, and only the gate tests call it today.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_stable_at(&self, epoch: u64) -> bool {
        epoch.is_multiple_of(2) && self.generation() == epoch
    }
}

/// Takes the lock, ignoring poisoning: a panic in an earlier update must not block later ones.
fn lock_ignoring_poison(lock: &Mutex<()>) -> MutexGuard<'_, ()> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Runs `build` under the seqlock protocol of a launch preparation (A3-R3): the generation must be
/// even before and after the read, and is retried once. Returns the even generation the launch
/// is stamped with, together with what `build` produced from settings read inside that window.
/// `Err(None)` means an update was in progress (the caller reports a transient failure).
pub fn read_stable<T, E>(
    gate: &AgentSettingsGate,
    mut build: impl FnMut() -> Result<T, E>,
) -> Result<(u64, T), Option<E>> {
    for _attempt in 0..2 {
        let Some(before) = gate.prepare_stable() else {
            continue;
        };
        let built = build().map_err(Some)?;
        if gate.generation() == before {
            return Ok((before, built));
        }
    }
    Err(None)
}
