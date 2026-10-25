//! Settings generation gate (DESIGN-PR4-PLANNER.md Revision 4, A3-R3).
//!
//! A seqlock over the launch-affecting agent settings: the generation is even when the
//! settings are stable and odd while an update is in progress. A prepared launch records the
//! even generation it was prepared under; admission refuses it when the generation has moved
//! on or an update is running.
//!
//! Why the dead-code allow: the settings writers that call `with_update` land in a later step,
//! so outside tests nothing uses this module yet.

#![cfg_attr(not(test), allow(dead_code))]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

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
    pub fn is_stable_at(&self, epoch: u64) -> bool {
        epoch.is_multiple_of(2) && self.generation() == epoch
    }
}

/// Takes the lock, ignoring poisoning: a panic in an earlier update must not block later ones.
fn lock_ignoring_poison(lock: &Mutex<()>) -> MutexGuard<'_, ()> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
