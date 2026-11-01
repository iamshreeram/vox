//! Process-wide ownership of the microphone and overlay: exactly one owner
//! (dictation or conversation) at a time. Acquisition is an atomic
//! compare-and-set; the returned `Lease` releases ownership on drop, including
//! during panic unwinding and early return.
//! Contract: DESIGN.md Revision 3 (O-C4') and Revision 4 (O-C4'').

use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// Locks the slot mutex, recovering the inner value if a previous holder
/// panicked. The guarded value is plain data and stays valid.
fn lock_slot(mutex: &Mutex<Slot>) -> MutexGuard<'_, Slot> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Who currently owns the recorder and overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// Dictation (hotkey, wake word, CLI toggle) including its processing.
    Dictation,
    /// A hands-free conversation session, listening through draining.
    Conversation,
    /// Speaker enrollment wizard. Holds the gate across its recording windows
    /// (design A3'): dictation and conversation are refused until it finishes,
    /// is cancelled, expires or is dropped.
    Enrollment,
    /// The background update install (B2b). While it holds the gate, dictation,
    /// conversation and enrollment starts are refused with `Busy(Updater)`.
    Updater,
}

/// Returned when another owner already holds the gate. Carries the holder.
#[derive(Debug, PartialEq)]
pub struct Busy(pub Owner);

/// What the gate guards: the current holder and the monotonic instant the gate
/// last became free (creation, or the last lease drop). One mutex guards both,
/// so `idle_since` and `holder` never disagree.
#[derive(Debug)]
struct Slot {
    holder: Option<Owner>,
    idle_since: Instant,
}

/// Single-owner gate. Share it with `Arc`; acquire with `try_acquire`.
#[derive(Debug)]
pub struct OwnershipGate {
    slot: Mutex<Slot>,
}

impl OwnershipGate {
    /// Creates a gate with no owner. The gate counts as idle from this moment.
    pub fn new() -> Arc<OwnershipGate> {
        Arc::new(OwnershipGate {
            slot: Mutex::new(Slot {
                holder: None,
                idle_since: Instant::now(),
            }),
        })
    }

    /// Atomically claims the gate for `who`. Fails with `Busy(holder)` if any
    /// owner holds it, including `who` itself. The gate is unchanged on failure.
    pub fn try_acquire(self: &Arc<Self>, who: Owner) -> Result<Lease, Busy> {
        let mut slot = lock_slot(&self.slot);
        match slot.holder {
            Some(current) => Err(Busy(current)),
            None => {
                let idle_before = slot.idle_since;
                slot.holder = Some(who);
                Ok(Lease {
                    gate: Arc::clone(self),
                    owner: who,
                    idle_before,
                })
            }
        }
    }

    /// Returns the current holder, if any. Does not acquire anything.
    // Read by the update install's admission check (B2b); until then only the tests read it.
    #[allow(dead_code)]
    pub fn holder(&self) -> Option<Owner> {
        lock_slot(&self.slot).holder
    }

    /// Returns the current holder, if any. Only the tests observe the holder
    /// directly; production code gates on `try_acquire` alone.
    #[cfg(test)]
    pub fn current(&self) -> Option<Owner> {
        self.holder()
    }

    /// The monotonic instant the gate became free: `None` while a lease is held,
    /// otherwise the last release (or creation, if it was never held). Non-acquiring.
    // Read by the update idleness check (B2b); until then only the tests read it.
    #[allow(dead_code)]
    pub fn idle_since(&self) -> Option<Instant> {
        let slot = lock_slot(&self.slot);
        match slot.holder {
            Some(_) => None,
            None => Some(slot.idle_since),
        }
    }
}

/// Time elapsed from `since` to `now`, saturating at zero. Pure, so the idle
/// arithmetic is testable without a clock.
// Used by the update idleness check (B2b); nothing in this step reads it yet.
#[allow(dead_code)]
pub fn idle_for(now: Instant, since: Instant) -> Duration {
    now.saturating_duration_since(since)
}

/// Proof of ownership. Dropping it releases the gate.
#[derive(Debug)]
pub struct Lease {
    gate: Arc<OwnershipGate>,
    owner: Owner,
    /// The idle instant the gate had just before this lease was taken.
    idle_before: Instant,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut slot = lock_slot(&self.gate.slot);
        slot.holder = None;
        // The update installer is not user activity: releasing it restores the idle instant
        // it found, so a failed or deferred commit does not restart the idle clock (B6).
        slot.idle_since = match self.owner {
            Owner::Updater => self.idle_before,
            Owner::Dictation | Owner::Conversation | Owner::Enrollment => Instant::now(),
        };
    }
}

/// The process-wide singleton gate. Returns the same `Arc` on every call.
pub fn global() -> &'static Arc<OwnershipGate> {
    static GATE: OnceLock<Arc<OwnershipGate>> = OnceLock::new();
    GATE.get_or_init(OwnershipGate::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::Barrier;
    use std::thread;

    /// Simulated session phases a conversation passes through while it holds
    /// the lease. The lease must block dictation in every one of them.
    const SESSION_PHASES: [&str; 5] = ["listening", "thinking", "speaking", "acting", "draining"];

    fn join_or_resume<T>(handle: thread::JoinHandle<T>) -> T {
        match handle.join() {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    // O7: a fresh gate has no holder and dictation can acquire it.
    #[test]
    fn o7_acquire_dictation_succeeds_on_a_fresh_gate() {
        let gate = OwnershipGate::new();
        assert_eq!(gate.current(), None);
        let lease = gate.try_acquire(Owner::Dictation);
        assert!(lease.is_ok());
        assert_eq!(gate.current(), Some(Owner::Dictation));
    }

    #[test]
    fn o7_second_acquire_by_either_owner_is_busy_and_changes_nothing() {
        let gate = OwnershipGate::new();
        let _held = gate.try_acquire(Owner::Dictation).expect("first acquire");
        assert_eq!(
            gate.try_acquire(Owner::Dictation).err(),
            Some(Busy(Owner::Dictation))
        );
        assert_eq!(
            gate.try_acquire(Owner::Conversation).err(),
            Some(Busy(Owner::Dictation))
        );
        assert_eq!(gate.current(), Some(Owner::Dictation));
    }

    #[test]
    fn o7_same_owner_cannot_acquire_twice() {
        let gate = OwnershipGate::new();
        let _held = gate
            .try_acquire(Owner::Conversation)
            .expect("first acquire");
        assert_eq!(
            gate.try_acquire(Owner::Conversation).err(),
            Some(Busy(Owner::Conversation))
        );
    }

    #[test]
    fn o7_after_dropping_the_lease_anyone_can_acquire() {
        let gate = OwnershipGate::new();
        let lease = gate.try_acquire(Owner::Dictation).expect("first acquire");
        drop(lease);
        assert_eq!(gate.current(), None);
        let conversation = gate.try_acquire(Owner::Conversation);
        assert!(conversation.is_ok());
        drop(conversation);
        assert!(gate.try_acquire(Owner::Dictation).is_ok());
    }

    #[test]
    fn o7_conversation_lease_blocks_dictation_in_every_session_phase() {
        let gate = OwnershipGate::new();
        let lease = gate
            .try_acquire(Owner::Conversation)
            .expect("conversation acquire");
        for phase in SESSION_PHASES {
            assert_eq!(
                gate.try_acquire(Owner::Dictation).err(),
                Some(Busy(Owner::Conversation)),
                "dictation must be refused while conversation is {phase}"
            );
            assert_eq!(gate.current(), Some(Owner::Conversation), "phase {phase}");
        }
        drop(lease);
        assert!(gate.try_acquire(Owner::Dictation).is_ok());
    }

    #[test]
    fn o7_two_racing_threads_yield_exactly_one_winner() {
        let gate = OwnershipGate::new();
        let barrier = Arc::new(Barrier::new(2));
        let racers = [Owner::Dictation, Owner::Conversation];
        let handles: Vec<_> = racers
            .into_iter()
            .map(|who| {
                let gate = gate.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    gate.try_acquire(who).inspect(|_| {
                        // Keep the winner's lease alive until both results exist.
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    })
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(join_or_resume).collect();
        let oks = results.iter().filter(|r| r.is_ok()).count();
        let errs: Vec<_> = results.iter().filter_map(|r| r.as_ref().err()).collect();
        assert_eq!(oks, 1, "exactly one racer must win");
        assert_eq!(errs.len(), 1, "exactly one racer must get Busy");
        assert!(matches!(errs[0], Busy(_)));
    }

    #[test]
    fn o7_lease_is_released_when_the_holder_panics() {
        let gate = OwnershipGate::new();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let _lease = gate
                .try_acquire(Owner::Conversation)
                .expect("acquire before panic");
            panic!("simulated session failure");
        }));
        assert!(outcome.is_err(), "the closure must panic");
        assert_eq!(gate.current(), None, "panic must release the lease");
        assert!(gate.try_acquire(Owner::Dictation).is_ok());
    }

    #[test]
    fn o7_lease_is_released_on_early_return() {
        fn start_then_bail(gate: &Arc<OwnershipGate>) -> Result<(), &'static str> {
            let _lease = gate.try_acquire(Owner::Dictation).map_err(|_| "busy")?;
            Err("preflight failed")
        }
        let gate = OwnershipGate::new();
        assert_eq!(start_then_bail(&gate), Err("preflight failed"));
        assert_eq!(gate.current(), None, "early return must release the lease");
        assert!(gate.try_acquire(Owner::Conversation).is_ok());
    }

    #[test]
    fn o7_current_reflects_the_holder() {
        let gate = OwnershipGate::new();
        assert_eq!(gate.current(), None);
        let dictation = gate.try_acquire(Owner::Dictation).expect("dictation");
        assert_eq!(gate.current(), Some(Owner::Dictation));
        drop(dictation);
        assert_eq!(gate.current(), None);
        let conversation = gate.try_acquire(Owner::Conversation).expect("conversation");
        assert_eq!(gate.current(), Some(Owner::Conversation));
        drop(conversation);
        assert_eq!(gate.current(), None);
    }

    /// Every owner, for exhaustive checks.
    const ALL_OWNERS: [Owner; 4] = [
        Owner::Dictation,
        Owner::Conversation,
        Owner::Enrollment,
        Owner::Updater,
    ];

    // B2a: the update installer owns the gate like the other owners, and every other owner
    // is refused with Busy(Updater) while it holds the gate.
    #[test]
    fn b2a_updater_lease_refuses_every_other_owner_with_busy_updater() {
        let gate = OwnershipGate::new();
        let _updater = gate.try_acquire(Owner::Updater).expect("updater acquire");
        for requester in [Owner::Dictation, Owner::Conversation, Owner::Enrollment] {
            assert_eq!(
                gate.try_acquire(requester).err(),
                Some(Busy(Owner::Updater)),
                "{requester:?} must be refused while the update holds the gate"
            );
        }
        assert_eq!(gate.holder(), Some(Owner::Updater));
    }

    #[test]
    fn b2a_held_updater_lease_is_refused_when_another_owner_races_for_it() {
        for _ in 0..300 {
            let gate = OwnershipGate::new();
            let barrier = Arc::new(Barrier::new(2));
            let racers = [Owner::Updater, Owner::Dictation];
            let handles: Vec<_> = racers
                .into_iter()
                .map(|who| {
                    let gate = gate.clone();
                    let barrier = barrier.clone();
                    thread::spawn(move || {
                        barrier.wait();
                        gate.try_acquire(who)
                    })
                })
                .collect();
            let results: Vec<_> = handles.into_iter().map(join_or_resume).collect();
            assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        }
    }

    #[test]
    fn b2a_holder_reflects_each_owner_and_is_none_after_drop() {
        for owner in ALL_OWNERS {
            let gate = OwnershipGate::new();
            assert_eq!(gate.holder(), None);
            let lease = gate.try_acquire(owner).expect("acquire free gate");
            assert_eq!(gate.holder(), Some(owner));
            drop(lease);
            assert_eq!(gate.holder(), None, "{owner:?} must clear on drop");
        }
    }

    #[test]
    fn b2a_idle_since_is_none_while_held_and_advances_on_each_release() {
        let gate = OwnershipGate::new();
        let created = gate
            .idle_since()
            .expect("a free gate is idle from creation");
        let lease = gate.try_acquire(Owner::Dictation).expect("acquire");
        assert_eq!(gate.idle_since(), None, "held gate is not idle");
        drop(lease);
        let released = gate.idle_since().expect("released gate is idle");
        assert!(released >= created, "release must not move idleness back");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let again = gate.try_acquire(Owner::Enrollment).expect("acquire");
        drop(again);
        let later = gate.idle_since().expect("released gate is idle again");
        assert!(later > released, "each release restarts the idle clock");
    }

    #[test]
    fn b2a_idle_for_is_the_elapsed_time_and_never_negative() {
        let since = Instant::now();
        let now = since + Duration::from_secs(7);
        assert_eq!(idle_for(now, since), Duration::from_secs(7));
        assert_eq!(idle_for(since, now), Duration::ZERO);
        assert_eq!(idle_for(since, since), Duration::ZERO);
    }

    // O8 (design A3'): Enrollment is mutually exclusive with both other owners.
    #[test]
    fn o8_enrollment_is_mutually_exclusive_with_dictation() {
        let gate = OwnershipGate::new();
        let enrollment = gate.try_acquire(Owner::Enrollment).expect("enrollment");
        assert_eq!(
            gate.try_acquire(Owner::Dictation).err(),
            Some(Busy(Owner::Enrollment))
        );
        drop(enrollment);
        let dictation = gate.try_acquire(Owner::Dictation).expect("dictation");
        assert_eq!(
            gate.try_acquire(Owner::Enrollment).err(),
            Some(Busy(Owner::Dictation))
        );
        assert_eq!(gate.current(), Some(Owner::Dictation));
        drop(dictation);
    }

    #[test]
    fn o8_enrollment_is_mutually_exclusive_with_conversation() {
        let gate = OwnershipGate::new();
        let enrollment = gate.try_acquire(Owner::Enrollment).expect("enrollment");
        assert_eq!(
            gate.try_acquire(Owner::Conversation).err(),
            Some(Busy(Owner::Enrollment))
        );
        drop(enrollment);
        let conversation = gate.try_acquire(Owner::Conversation).expect("conversation");
        assert_eq!(
            gate.try_acquire(Owner::Enrollment).err(),
            Some(Busy(Owner::Conversation))
        );
        assert_eq!(gate.current(), Some(Owner::Conversation));
        drop(conversation);
    }

    #[test]
    fn o8_enrollment_lease_releases_on_drop() {
        let gate = OwnershipGate::new();
        let enrollment = gate.try_acquire(Owner::Enrollment).expect("enrollment");
        assert_eq!(gate.current(), Some(Owner::Enrollment));
        drop(enrollment);
        assert_eq!(gate.current(), None);
        assert!(gate.try_acquire(Owner::Dictation).is_ok());
    }

    #[test]
    fn o8_enrollment_lease_releases_on_panic_unwind() {
        let gate = OwnershipGate::new();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let _lease = gate
                .try_acquire(Owner::Enrollment)
                .expect("acquire before panic");
            panic!("simulated wizard failure");
        }));
        assert!(outcome.is_err(), "the closure must panic");
        assert_eq!(
            gate.current(),
            None,
            "panic must release the enrollment lease"
        );
        assert!(gate.try_acquire(Owner::Conversation).is_ok());
    }

    // O8: for every held owner and every requester (including the same owner), the
    // refusal names the current holder, and the gate is unchanged.
    #[test]
    fn o8_busy_names_the_current_owner_for_every_pair() {
        for held in ALL_OWNERS {
            for requester in ALL_OWNERS {
                let gate = OwnershipGate::new();
                let _lease = gate.try_acquire(held).expect("first acquire");
                assert_eq!(
                    gate.try_acquire(requester).err(),
                    Some(Busy(held)),
                    "held={held:?} requester={requester:?}"
                );
                assert_eq!(gate.current(), Some(held));
            }
        }
    }

    #[test]
    fn o8_every_owner_can_acquire_once_the_gate_is_free() {
        for owner in ALL_OWNERS {
            let gate = OwnershipGate::new();
            let lease = gate.try_acquire(owner);
            assert!(lease.is_ok(), "{owner:?} must acquire a free gate");
            assert_eq!(gate.current(), Some(owner));
            drop(lease);
            assert_eq!(gate.current(), None);
        }
    }

    // B6 (validator major 1): an Updater lease is not user activity. Dropping it restores the
    // idle instant the gate had before, so a deferred commit cannot keep the idle clock reset.
    #[test]
    fn b6_updater_lease_drop_keeps_the_idle_instant() {
        let gate = OwnershipGate::new();
        let before = gate.idle_since().expect("a free gate is idle");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let lease = gate.try_acquire(Owner::Updater).expect("updater acquire");
        std::thread::sleep(std::time::Duration::from_millis(5));
        drop(lease);
        assert_eq!(
            gate.idle_since(),
            Some(before),
            "updater release must not refresh idleness"
        );
    }

    // B6: user work still refreshes idleness on release.
    #[test]
    fn b6_dictation_lease_drop_refreshes_the_idle_instant() {
        let gate = OwnershipGate::new();
        let before = gate.idle_since().expect("a free gate is idle");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let lease = gate
            .try_acquire(Owner::Dictation)
            .expect("dictation acquire");
        std::thread::sleep(std::time::Duration::from_millis(5));
        drop(lease);
        let after = gate.idle_since().expect("released gate is idle");
        assert!(after > before, "dictation release must restart idleness");
    }

    #[test]
    fn o7_global_returns_the_same_arc_on_each_call() {
        let first = global();
        let second = global();
        assert!(Arc::ptr_eq(first, second));
        let _ = first.current();
    }
}
