//! Lazily loaded speech-recognition session behind one owner lock.
//!
//! `IndicSlot` owns at most one `IndicSession`. A loader builds it on the first call, the session
//! is reused while the slot is in use, it is dropped after an idle period (`UnloadPolicy`), and
//! after a failure the slot waits `BACKOFF_MS` before it tries to load again.
//!
//! Admission is non-blocking: the owner lock is taken with `try_lock` only, so a second caller
//! while a call runs gets `SlotOutcome::Busy` at once and changes nothing. Nothing here knows the
//! real engine; the caller wires it in through `IndicSession`. Modelled on the Tamil voice
//! lifecycle in `natural_tamil.rs`.

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};
use std::thread;
use std::time::{Duration, Instant};

/// How long a failed load or run keeps the slot in backoff before the next attempt.
pub const BACKOFF_MS: u64 = 30_000;

/// Name of the thread that ticks the slot.
const WATCHER_THREAD_NAME: &str = "indic-stt-watch";

/// One loaded recognition session. `transcribe` runs on the caller's thread.
pub trait IndicSession: Send {
    /// Transcribes 16 kHz mono samples. The error text is shown in the status line.
    fn transcribe(&mut self, samples: &[f32]) -> Result<String, String>;
}

/// Monotonic millisecond clock. Injected so tests can move time by hand.
pub trait Clock: Send + Sync {
    /// Milliseconds since a fixed origin. Never goes backwards.
    fn now_ms(&self) -> u64;
}

/// `Clock` backed by `std::time::Instant`, with its origin at construction.
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    /// Starts a clock at zero now.
    pub fn new() -> SystemClock {
        SystemClock {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> SystemClock {
        SystemClock::new()
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        duration_ms(self.origin.elapsed())
    }
}

/// When an idle session is dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnloadPolicy {
    /// Drop the session after every call.
    Immediately,
    /// Keep the session until the slot itself is dropped.
    Never,
    /// Drop the session once it has been idle for this long.
    After(Duration),
}

impl UnloadPolicy {
    /// The documented default: unload after five idle minutes.
    pub fn default_idle() -> UnloadPolicy {
        UnloadPolicy::After(Duration::from_secs(300))
    }
}

/// Result of one `transcribe` call on the slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotOutcome {
    /// The recognised text.
    Text(String),
    /// Another call holds the slot. Nothing changed; the caller returns an error and does not
    /// fall back.
    Busy,
    /// The load or run failed just now, or the slot is in backoff. The caller falls back to the
    /// normal model path.
    Unavailable(String),
}

/// Lifecycle state, kept in `Meta`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotState {
    Unloaded,
    Ready { last_used_ms: u64 },
    Backoff { until_ms: u64 },
}

/// The session storage. Touched only while the owner lock is held.
struct Owner {
    session: Option<Box<dyn IndicSession>>,
}

/// State and the last error. Always locked briefly, and only after the owner lock if both are
/// needed, so there is no lock cycle.
struct Meta {
    state: SlotState,
    last_error: Option<String>,
}

/// A lazily loaded recognition session with idle unload and failure backoff.
pub struct IndicSlot {
    owner: Mutex<Owner>,
    meta: Mutex<Meta>,
    loader: Box<dyn Fn() -> Result<Box<dyn IndicSession>, String> + Send + Sync>,
    clock: Arc<dyn Clock>,
    watcher_started: AtomicBool,
    watcher_spawns: AtomicUsize,
    watcher_alive: Arc<AtomicBool>,
}

impl IndicSlot {
    /// Builds an empty slot. Nothing is loaded and no thread is started until the first call.
    pub fn new(
        loader: Box<dyn Fn() -> Result<Box<dyn IndicSession>, String> + Send + Sync>,
        clock: Arc<dyn Clock>,
    ) -> IndicSlot {
        IndicSlot {
            owner: Mutex::new(Owner { session: None }),
            meta: Mutex::new(Meta {
                state: SlotState::Unloaded,
                last_error: None,
            }),
            loader,
            clock,
            watcher_started: AtomicBool::new(false),
            watcher_spawns: AtomicUsize::new(0),
            watcher_alive: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Runs one recognition call.
    ///
    /// Returns `Busy` at once if another call holds the slot. Loads the session when it is not
    /// loaded (unless in backoff), and drops it again after a failure or, for `Immediately`,
    /// after success.
    pub fn transcribe(&self, samples: &[f32], policy: UnloadPolicy) -> SlotOutcome {
        let Some(mut owner) = self.try_owner() else {
            return SlotOutcome::Busy;
        };
        let now = self.clock.now_ms();
        if let SlotState::Backoff { until_ms } = self.state() {
            if now < until_ms {
                return SlotOutcome::Unavailable(self.backoff_message());
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
            return self.enter_backoff(now, "no speech session is loaded".to_string());
        };
        let result = catch_unwind(AssertUnwindSafe(|| session.transcribe(samples)));
        match result {
            Ok(Ok(text)) => {
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
                SlotOutcome::Text(text)
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
                        "speech session panicked: {}",
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

    #[cfg_attr(not(test), allow(dead_code))]
    /// Name of the current state: `"unloaded"`, `"ready"` or `"backoff"`.
    pub fn state_name(&self) -> &'static str {
        match self.state() {
            SlotState::Unloaded => "unloaded",
            SlotState::Ready { .. } => "ready",
            SlotState::Backoff { .. } => "backoff",
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    /// True while a session is held (the state is `Ready`).
    pub fn is_loaded(&self) -> bool {
        matches!(self.state(), SlotState::Ready { .. })
    }

    /// The message of the most recent failure. Cleared by the next successful run.
    pub fn last_error(&self) -> Option<String> {
        self.with_meta(|meta| meta.last_error.clone())
    }

    /// True once `spawn_watcher` has started the watcher for this slot.
    pub fn watcher_started(&self) -> bool {
        self.watcher_started.load(Ordering::SeqCst)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    /// How many watcher threads were actually spawned for this slot (0 or 1).
    pub fn watcher_spawn_count(&self) -> usize {
        self.watcher_spawns.load(Ordering::SeqCst)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    /// True while the watcher thread is alive.
    pub fn watcher_running(&self) -> bool {
        self.watcher_alive.load(Ordering::SeqCst)
    }

    /// Shared flag that stays true while the watcher thread is alive (tests watch it after the
    /// slot is dropped).
    #[cfg(test)]
    pub(crate) fn watcher_alive_flag(&self) -> Arc<AtomicBool> {
        self.watcher_alive.clone()
    }

    /// Poisons the owner lock from a caught panic, so tests can check recovery.
    #[cfg(test)]
    pub(crate) fn poison_owner_for_test(&self) {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let _guard = self.owner.lock();
            panic!("poisoning the owner lock for a test");
        }));
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
    fn load_session(&self) -> Result<Box<dyn IndicSession>, String> {
        match catch_unwind(AssertUnwindSafe(|| (self.loader)())) {
            Ok(result) => result,
            Err(payload) => Err(format!(
                "loading the speech session panicked: {}",
                panic_message(payload.as_ref())
            )),
        }
    }

    /// Records a failure, starts the backoff and returns `Unavailable`.
    fn enter_backoff(&self, now: u64, message: String) -> SlotOutcome {
        self.with_meta(|meta| {
            meta.state = SlotState::Backoff {
                until_ms: now.saturating_add(BACKOFF_MS),
            };
            meta.last_error = Some(message.clone());
        });
        SlotOutcome::Unavailable(message)
    }

    /// Message for a call that arrives during backoff.
    fn backoff_message(&self) -> String {
        self.last_error()
            .unwrap_or_else(|| "speech recognition is cooling down after an error".to_string())
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

/// Starts the single watcher thread for `slot`. It calls `tick(policy_fn())` every `period` and
/// exits when the slot is dropped. Calling it again does nothing.
///
/// The thread holds only a `Weak` reference, so it never keeps the slot alive.
pub fn spawn_watcher(
    slot: &Arc<IndicSlot>,
    policy_fn: Arc<dyn Fn() -> UnloadPolicy + Send + Sync>,
    period: Duration,
) {
    if slot.watcher_started.swap(true, Ordering::SeqCst) {
        return;
    }
    slot.watcher_alive.store(true, Ordering::SeqCst);
    let alive = AliveGuard(slot.watcher_alive.clone());
    let weak = Arc::downgrade(slot);
    let spawned = thread::Builder::new()
        .name(WATCHER_THREAD_NAME.to_string())
        .spawn(move || {
            let _alive = alive;
            loop {
                thread::sleep(period);
                let Some(slot) = weak.upgrade() else {
                    break;
                };
                slot.tick((policy_fn)());
            }
        });
    match spawned {
        Ok(_) => {
            slot.watcher_spawns.fetch_add(1, Ordering::SeqCst);
        }
        Err(_) => {
            slot.watcher_started.store(false, Ordering::SeqCst);
        }
    }
}

/// Clears the alive flag when the watcher thread ends, however it ends.
struct AliveGuard(Arc<AtomicBool>);

impl Drop for AliveGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
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
