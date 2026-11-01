//! Lazily loaded language-identifier session (an installed multilingual Whisper) behind one
//! owner lock, with idle unload and failure backoff.
//!
//! Mirrors `indic_stt_slot.rs` (same `Clock`, `UnloadPolicy`, `BACKOFF_MS`, `try_lock` admission:
//! a second caller while a call runs gets `LidOutcome::Busy` at once). Does not change
//! `IndicSlot`, and has no watcher thread: the transcription idle thread calls `tick`. See
//! `.ram-status/auto-lang-route-20261101b/DESIGN.md` (Revision 3).

#![allow(dead_code)] // `state_name` has no caller yet.

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};
use std::time::Duration;

use super::auto_lang::{LidOutcome, LidResult};
use super::indic_stt_slot::{Clock, UnloadPolicy, BACKOFF_MS};

/// One loaded identifier session. `identify` runs on the caller's thread.
pub trait LidSession: Send {
    /// Runs the model with no language hint on 16 kHz mono samples (already capped by the
    /// caller) and returns the detected code plus the model's own transcript.
    fn identify(&mut self, samples: &[f32]) -> Result<LidResult, String>;
}

/// Builds a session on first use. The error text is kept as the slot's last error.
pub type LidLoader = Box<dyn Fn() -> Result<Box<dyn LidSession>, String> + Send + Sync>;

/// Lifecycle state, kept in `Meta`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotState {
    Unloaded,
    Ready { last_used_ms: u64 },
    Backoff { until_ms: u64 },
}

/// The session storage. Touched only while the owner lock is held.
struct Owner {
    session: Option<Box<dyn LidSession>>,
}

/// State and the last error. Always locked briefly, and only after the owner lock if both are
/// needed, so there is no lock cycle.
struct Meta {
    state: SlotState,
    last_error: Option<String>,
}

/// A lazily loaded identifier with idle unload and failure backoff.
pub struct LidSlot {
    owner: Mutex<Owner>,
    meta: Mutex<Meta>,
    loader: LidLoader,
    clock: Arc<dyn Clock>,
}

impl LidSlot {
    /// Builds an empty slot. Nothing is loaded and no thread is started until the first call.
    pub fn new(loader: LidLoader, clock: Arc<dyn Clock>) -> LidSlot {
        LidSlot {
            owner: Mutex::new(Owner { session: None }),
            meta: Mutex::new(Meta {
                state: SlotState::Unloaded,
                last_error: None,
            }),
            loader,
            clock,
        }
    }

    /// Runs one identification. `Busy` at once if another call holds the slot. Loads when not
    /// loaded (unless in backoff). A load failure, run failure or panic gives `Unavailable` and
    /// starts a `BACKOFF_MS` backoff; the session is dropped after a failure and, for
    /// `UnloadPolicy::Immediately`, after every call.
    pub fn identify(&self, samples: &[f32], policy: UnloadPolicy) -> LidOutcome {
        if samples.is_empty() {
            // Nothing to identify: no load, no backoff, no state change.
            return LidOutcome::Unavailable("empty audio".to_string());
        }
        let Some(mut owner) = self.try_owner() else {
            return LidOutcome::Busy;
        };
        let now = self.clock.now_ms();
        if let SlotState::Backoff { until_ms } = self.state() {
            if now < until_ms {
                return LidOutcome::Unavailable(self.backoff_message());
            }
        }
        if owner.session.is_none() {
            match self.load_session() {
                Ok(session) => owner.session = Some(session),
                Err(message) => return self.enter_backoff(now, message),
            }
            self.set_state(SlotState::Ready { last_used_ms: now });
        }
        let Some(session) = owner.session.as_mut() else {
            return self.enter_backoff(now, "no language session is loaded".to_string());
        };
        let result = catch_unwind(AssertUnwindSafe(|| session.identify(samples)));
        match result {
            Ok(Ok(found)) => {
                let used_at = self.clock.now_ms();
                self.with_meta(|meta| {
                    meta.last_error = None;
                    meta.state = SlotState::Ready {
                        last_used_ms: used_at,
                    };
                });
                if policy == UnloadPolicy::Immediately {
                    owner.session = None;
                    self.set_state(SlotState::Unloaded);
                }
                LidOutcome::Done(found)
            }
            Ok(Err(message)) => {
                owner.session = None;
                self.enter_backoff(now, message)
            }
            Err(payload) => {
                owner.session = None;
                self.enter_backoff(
                    now,
                    format!(
                        "language session panicked: {}",
                        panic_message(payload.as_ref())
                    ),
                )
            }
        }
    }

    /// Applies the unload policy to an idle session and clears an expired backoff.
    ///
    /// Does nothing while a call holds the slot. Never runs the session.
    pub fn tick(&self, policy: UnloadPolicy) {
        let Some(mut owner) = self.try_owner() else {
            return;
        };
        let now = self.clock.now_ms();
        match self.state() {
            SlotState::Ready { last_used_ms } => {
                if unload_due(policy, now, last_used_ms) {
                    owner.session = None;
                    self.set_state(SlotState::Unloaded);
                }
            }
            SlotState::Backoff { until_ms } if now >= until_ms => {
                self.set_state(SlotState::Unloaded);
            }
            _ => {}
        }
    }

    /// True while a session is held (the state is `Ready`).
    pub fn is_loaded(&self) -> bool {
        matches!(self.state(), SlotState::Ready { .. })
    }

    /// "unloaded", "ready" or "backoff".
    pub fn state_name(&self) -> &'static str {
        match self.state() {
            SlotState::Unloaded => "unloaded",
            SlotState::Ready { .. } => "ready",
            SlotState::Backoff { .. } => "backoff",
        }
    }

    /// The last load or run error, if any.
    pub fn last_error(&self) -> Option<String> {
        self.with_meta(|meta| meta.last_error.clone())
    }

    /// Takes the owner lock without waiting. `None` means another call holds it. A poisoned lock
    /// is recovered and its session dropped, as a session that panicked inside the lock cannot
    /// be trusted.
    fn try_owner(&self) -> Option<MutexGuard<'_, Owner>> {
        match self.owner.try_lock() {
            Ok(guard) => Some(guard),
            Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Poisoned(poisoned)) => {
                let mut guard = poisoned.into_inner();
                // Clear the flag so only this access drops the session, not every later one.
                self.owner.clear_poison();
                guard.session = None;
                self.with_meta(|meta| {
                    if matches!(meta.state, SlotState::Ready { .. }) {
                        meta.state = SlotState::Unloaded;
                    }
                });
                Some(guard)
            }
        }
    }

    /// Builds a session through the loader. A loader error or panic becomes a message.
    fn load_session(&self) -> Result<Box<dyn LidSession>, String> {
        match catch_unwind(AssertUnwindSafe(|| (self.loader)())) {
            Ok(result) => result,
            Err(payload) => Err(format!(
                "loading the language session panicked: {}",
                panic_message(payload.as_ref())
            )),
        }
    }

    /// Records a failure, starts the backoff and returns `Unavailable`.
    fn enter_backoff(&self, now: u64, message: String) -> LidOutcome {
        self.with_meta(|meta| {
            meta.state = SlotState::Backoff {
                until_ms: now.saturating_add(BACKOFF_MS),
            };
            meta.last_error = Some(message.clone());
        });
        LidOutcome::Unavailable(message)
    }

    /// Message for a call that arrives during backoff.
    fn backoff_message(&self) -> String {
        self.last_error()
            .unwrap_or_else(|| "language identification is cooling down after an error".to_string())
    }

    fn state(&self) -> SlotState {
        self.with_meta(|meta| meta.state)
    }

    fn set_state(&self, state: SlotState) {
        self.with_meta(|meta| meta.state = state);
    }

    fn with_meta<R>(&self, f: impl FnOnce(&mut Meta) -> R) -> R {
        let mut meta = self.meta.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut meta)
    }
}

/// True when the policy says a session last used at `last_used` must be dropped at `now`.
fn unload_due(policy: UnloadPolicy, now: u64, last_used: u64) -> bool {
    match policy {
        UnloadPolicy::Never => false,
        UnloadPolicy::Immediately => true,
        UnloadPolicy::After(idle) => now.saturating_sub(last_used) >= duration_ms(idle),
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "unknown panic".to_string()
    }
}
