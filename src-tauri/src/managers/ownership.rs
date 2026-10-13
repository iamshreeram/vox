//! Process-wide ownership of the microphone and overlay: exactly one owner
//! (dictation or conversation) at a time. Acquisition is an atomic
//! compare-and-set; the returned `Lease` releases ownership on drop, including
//! during panic unwinding and early return.
//! Contract: DESIGN.md Revision 3 (O-C4') and Revision 4 (O-C4'').

// The wiring step (dictation and conversation entry points) is not in this
// change yet, so these items have no non-test callers.
#![allow(dead_code)]

use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

/// Locks the holder mutex, recovering the inner value if a previous holder
/// panicked. The guarded value is a plain `Option<Owner>` and stays valid.
fn lock_holder(mutex: &Mutex<Option<Owner>>) -> MutexGuard<'_, Option<Owner>> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Who currently owns the recorder and overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// Dictation (hotkey, wake word, CLI toggle) including its processing.
    Dictation,
    /// A hands-free conversation session, listening through draining.
    Conversation,
}

/// Returned when another owner already holds the gate. Carries the holder.
#[derive(Debug, PartialEq)]
pub struct Busy(pub Owner);

/// Single-owner gate. Share it with `Arc`; acquire with `try_acquire`.
#[derive(Debug)]
pub struct OwnershipGate {
    holder: std::sync::Mutex<Option<Owner>>,
}

impl OwnershipGate {
    /// Creates a gate with no owner.
    pub fn new() -> Arc<OwnershipGate> {
        Arc::new(OwnershipGate {
            holder: Mutex::new(None),
        })
    }

    /// Atomically claims the gate for `who`. Fails with `Busy(holder)` if any
    /// owner holds it, including `who` itself. The gate is unchanged on failure.
    pub fn try_acquire(self: &Arc<Self>, who: Owner) -> Result<Lease, Busy> {
        let mut holder = lock_holder(&self.holder);
        match *holder {
            Some(current) => Err(Busy(current)),
            None => {
                *holder = Some(who);
                Ok(Lease {
                    gate: Arc::clone(self),
                })
            }
        }
    }

    /// Returns the current holder, if any.
    pub fn current(&self) -> Option<Owner> {
        *lock_holder(&self.holder)
    }
}

/// Proof of ownership. Dropping it releases the gate.
#[derive(Debug)]
pub struct Lease {
    gate: Arc<OwnershipGate>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        *lock_holder(&self.gate.holder) = None;
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
                    gate.try_acquire(who).map(|lease| {
                        // Keep the winner's lease alive until both results exist.
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        lease
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

    #[test]
    fn o7_global_returns_the_same_arc_on_each_call() {
        let first = global();
        let second = global();
        assert!(Arc::ptr_eq(first, second));
        let _ = first.current();
    }
}
