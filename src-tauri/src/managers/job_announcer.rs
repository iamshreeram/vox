//! Low-priority speech for background jobs and the confirmation guard (M3).
//!
//! Announcements are spoken only when the speaker is idle. While a conversation
//! is active or a confirmation is in progress they wait in a small queue (max 4,
//! oldest progress dropped first). A finish that finds the speaker busy is
//! retried until it succeeds or `give_up_after` elapses; a progress update that
//! finds the speaker busy is dropped. The mouth is never called while the
//! announcer lock is held.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use crate::managers::job_summary::SpeechGuard;

/// Maximum number of queued announcements.
const MAX_PENDING: usize = 4;

/// Default delay between retries of a busy finish announcement.
const DEFAULT_RETRY_INTERVAL: Duration = Duration::from_secs(1);

/// Default time after which a busy finish announcement is dropped.
const DEFAULT_GIVE_UP_AFTER: Duration = Duration::from_secs(5 * 60);

/// A mouth that can speak only when nothing else is speaking.
pub trait LowPriorityMouth: Send + Sync {
    /// Speaks `text` only if the speaker is idle. Returns `true` when it was spoken.
    fn try_speak_if_idle(&self, text: &str) -> bool;
}

/// M3 S3c-1: `LowPriorityMouth` backed by the app's TTS manager. Speaks only when the
/// manager is idle (`TtsManager::speak_if_idle`), never superseding current speech.
pub struct TtsLowPriorityMouth(pub Arc<crate::managers::tts::TtsManager>);

impl TtsLowPriorityMouth {
    /// Trait-object constructor for the announcer and the confirmation gate.
    pub fn shared(tts: Arc<crate::managers::tts::TtsManager>) -> Arc<dyn LowPriorityMouth> {
        Arc::new(TtsLowPriorityMouth(tts))
    }
}

impl LowPriorityMouth for TtsLowPriorityMouth {
    fn try_speak_if_idle(&self, text: &str) -> bool {
        self.0.speak_if_idle(text)
    }
}

/// Kind of a background announcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceKind {
    /// A progress update.
    Progress,
    /// The job finished.
    Finish,
}

/// Queues background announcements and speaks them when the speaker is idle.
pub struct Announcer {
    shared: Arc<Shared>,
}

/// State shared between the announcer, its guards and the retry thread.
struct Shared {
    mouth: Arc<dyn LowPriorityMouth>,
    retry_interval: Duration,
    give_up_after: Duration,
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Default)]
struct State {
    conversation_active: bool,
    confirming: usize,
    pending: VecDeque<Entry>,
    next_seq: u64,
    /// Sequence number of the entry currently being offered to the mouth.
    in_flight: Option<u64>,
    retry_armed: bool,
}

struct Entry {
    seq: u64,
    kind: AnnounceKind,
    text: String,
    /// When a finish first found the speaker busy.
    first_failed: Option<Instant>,
    /// M4: summary guard for a finish that carries summary text (`None` = ordinary).
    guard: Option<SpeechGuard>,
}

/// Copy of the front entry taken out of the queue for one speech attempt.
struct Claim {
    seq: u64,
    kind: AnnounceKind,
    text: String,
    first_failed: Option<Instant>,
    guard: Option<SpeechGuard>,
}

/// Outcome of a drain pass.
enum Pass {
    /// Nothing is left to retry right now.
    Idle,
    /// A finish is waiting for the speaker to become idle.
    Busy,
}

impl Announcer {
    /// Creates an announcer that speaks through `mouth`.
    pub fn new(mouth: Arc<dyn LowPriorityMouth>) -> Self {
        Self::with_timing(mouth, DEFAULT_RETRY_INTERVAL, DEFAULT_GIVE_UP_AFTER)
    }

    /// Creates an announcer with an explicit retry interval and give-up bound.
    pub fn with_timing(
        mouth: Arc<dyn LowPriorityMouth>,
        retry_interval: std::time::Duration,
        give_up_after: std::time::Duration,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                mouth,
                retry_interval,
                give_up_after,
                state: Mutex::new(State::default()),
                wake: Condvar::new(),
            }),
        }
    }

    /// Queues one announcement (speaks now if idle, otherwise later).
    pub fn announce(&self, kind: AnnounceKind, text: String) {
        self.enqueue_entry(kind, text, None);
    }

    /// M4: queues one announcement that carries a summary guard. The guard is checked
    /// at every mouth attempt; an invalid guard speaks its fallback text instead.
    pub fn announce_guarded(&self, kind: AnnounceKind, text: String, guard: SpeechGuard) {
        self.enqueue_entry(kind, text, Some(guard));
    }

    /// Shared queueing path for `announce` and `announce_guarded`.
    fn enqueue_entry(&self, kind: AnnounceKind, text: String, guard: Option<SpeechGuard>) {
        if text.trim().is_empty() {
            return;
        }
        {
            let mut st = lock(&self.shared.state);
            let seq = st.next_seq;
            st.next_seq = st.next_seq.wrapping_add(1);
            enqueue(
                &mut st,
                Entry {
                    seq,
                    kind,
                    text,
                    first_failed: None,
                    guard,
                },
            );
        }
        drain(&self.shared);
    }

    /// Marks a conversation as active or not (announcements wait while active).
    pub fn set_conversation_active(&self, active: bool) {
        lock(&self.shared.state).conversation_active = active;
        if !active {
            drain(&self.shared);
        }
    }

    /// Holds speech while a confirmation is in progress. Released on drop.
    pub fn confirmation_guard(&self) -> ConfirmationGuard {
        lock(&self.shared.state).confirming += 1;
        ConfirmationGuard {
            shared: self.shared.clone(),
        }
    }

    /// Removes and returns the queued announcements.
    pub fn take_pending(&self) -> Vec<String> {
        let mut st = lock(&self.shared.state);
        let in_flight = st.in_flight;
        let mut taken = Vec::new();
        let mut kept = VecDeque::new();
        for entry in st.pending.drain(..) {
            if Some(entry.seq) == in_flight {
                kept.push_back(entry);
            } else {
                taken.push(entry.text);
            }
        }
        st.pending = kept;
        taken
    }

    /// Speaks queued announcements now that the speaker is idle.
    #[cfg(test)]
    pub fn notify_idle(&self) {
        self.shared.wake.notify_all();
        drain(&self.shared);
    }
}

/// RAII hold on announcements during a confirmation.
pub struct ConfirmationGuard {
    shared: Arc<Shared>,
}

impl Drop for ConfirmationGuard {
    fn drop(&mut self) {
        let flush = {
            let mut st = lock(&self.shared.state);
            st.confirming = st.confirming.saturating_sub(1);
            st.confirming == 0 && !st.conversation_active
        };
        if flush {
            drain(&self.shared);
        }
    }
}

/// Locks the state, recovering from a poisoned mutex (state stays usable).
fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// True when announcements may be spoken now.
fn gate_open(st: &State) -> bool {
    !st.conversation_active && st.confirming == 0
}

/// Adds an entry, dropping the oldest progress when the queue is full.
/// A full queue with no progress to drop discards the incoming entry.
fn enqueue(st: &mut State, entry: Entry) {
    if st.pending.len() >= MAX_PENDING {
        let oldest_progress = st
            .pending
            .iter()
            .position(|e| e.kind == AnnounceKind::Progress);
        match oldest_progress {
            Some(pos) => {
                st.pending.remove(pos);
            }
            None => return,
        }
    }
    st.pending.push_back(entry);
}

/// Removes the entry with the given sequence number, if still queued.
fn remove_seq(st: &mut State, seq: u64) {
    st.pending.retain(|e| e.seq != seq);
}

/// Takes a copy of the front entry for one attempt, when the gate is open and
/// no other attempt is in flight.
fn claim_front(shared: &Shared) -> Option<Claim> {
    let mut st = lock(&shared.state);
    if !gate_open(&st) || st.in_flight.is_some() {
        return None;
    }
    let front = st.pending.front()?;
    let claim = Claim {
        seq: front.seq,
        kind: front.kind,
        text: front.text.clone(),
        first_failed: front.first_failed,
        guard: front.guard.clone(),
    };
    st.in_flight = Some(claim.seq);
    Some(claim)
}

/// Offers queued entries to the mouth in order until the queue is empty, the
/// gate closes, or a finish finds the speaker busy (then a retry is armed).
fn drain(shared: &Arc<Shared>) -> Pass {
    while let Some(claim) = claim_front(shared) {
        let spoken = speak_claim(shared, &claim);
        let mut st = lock(&shared.state);
        st.in_flight = None;
        if spoken {
            remove_seq(&mut st, claim.seq);
            continue;
        }
        match claim.kind {
            AnnounceKind::Progress => remove_seq(&mut st, claim.seq),
            AnnounceKind::Finish => {
                let started = claim.first_failed.unwrap_or_else(Instant::now);
                if started.elapsed() >= shared.give_up_after {
                    remove_seq(&mut st, claim.seq);
                    continue;
                }
                if let Some(entry) = st.pending.iter_mut().find(|e| e.seq == claim.seq) {
                    entry.first_failed = Some(started);
                }
                drop(st);
                arm_retry(shared);
                return Pass::Busy;
            }
        }
    }
    Pass::Idle
}

/// Offers one claimed entry to the mouth. A guarded entry is spoken only while its
/// ticket is still valid at acceptance; otherwise its fallback text is offered instead.
/// The mouth runs with no announcer lock held.
fn speak_claim(shared: &Shared, claim: &Claim) -> bool {
    match claim.guard.as_ref() {
        None => shared.mouth.try_speak_if_idle(&claim.text),
        Some(guard) => guard.tokens.run_if_valid(&guard.ticket, |valid| {
            let text = if valid {
                claim.text.as_str()
            } else {
                guard.fallback.as_str()
            };
            shared.mouth.try_speak_if_idle(text)
        }),
    }
}

/// Starts the retry thread if none is running.
fn arm_retry(shared: &Arc<Shared>) {
    {
        let mut st = lock(&shared.state);
        if st.retry_armed {
            return;
        }
        st.retry_armed = true;
    }
    let worker = shared.clone();
    let spawned = thread::Builder::new()
        .name("job-announcer-retry".to_string())
        .spawn(move || retry_loop(worker));
    if spawned.is_err() {
        lock(&shared.state).retry_armed = false;
    }
}

/// Retries busy finishes every retry interval until none are left.
fn retry_loop(shared: Arc<Shared>) {
    loop {
        drop(wait_for(&shared, lock(&shared.state)));
        match drain(&shared) {
            Pass::Busy => continue,
            Pass::Idle => {
                let mut st = lock(&shared.state);
                if gate_open(&st) && !st.pending.is_empty() {
                    continue;
                }
                st.retry_armed = false;
                return;
            }
        }
    }
}

/// Waits up to one retry interval (or until woken) and returns the guard.
fn wait_for<'a>(shared: &'a Shared, st: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
    match shared.wake.wait_timeout(st, shared.retry_interval) {
        Ok((guard, _)) => guard,
        Err(poisoned) => poisoned.into_inner().0,
    }
}

#[cfg(test)]
#[path = "job_announcer_tests.rs"]
mod job_announcer_tests;
