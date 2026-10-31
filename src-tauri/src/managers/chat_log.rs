//! Live conversation transcript for the chat window (DESIGN-CHAT Rev 5: A4, A9, A12, A13).
//!
//! Implementation of the contract (DESIGN-CHAT Rev 6). Do not change public signatures
//! without a design revision; do not weaken `chat_log_tests.rs`.
//!
//! Contract summary (see the design file for the reasoning):
//! * Hooks (`say`, `reset`, `set_state`, `set_recording`) never wait on the worker, never emit
//!   and never do I/O. They clean + cap text, take a short inbox lock and wake the worker.
//! * One worker thread owns the log, assigns a global monotonic `seq` to every applied event
//!   (an entry's `id` is its `seq`) and calls the emitter AFTER releasing the log lock.
//! * Bounds: pending entries <= `PENDING_CAP` (extra entries are dropped and counted, then a
//!   `system:dropped:<n>` System entry is applied), log <= `LOG_CAP`, text <= `TEXT_CAP`
//!   chars, Clear waiters <= `CLEAR_WAITERS_CAP`. Reset/Clear are flags: a new wipe discards
//!   every still-pending entry (they would be wiped anyway) and coalesces with a pending wipe.
//! * Nothing is written to disk or logged.

use serde::Serialize;
use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Maximum entries kept in the log (oldest dropped).
pub const LOG_CAP: usize = 200;
/// Maximum characters per entry (cut on a char boundary, ends with `…` when cut).
pub const TEXT_CAP: usize = 2000;
/// Maximum entries admitted but not yet applied.
pub const PENDING_CAP: usize = 256;
/// Maximum simultaneously registered Clear waiters.
pub const CLEAR_WAITERS_CAP: usize = 8;
/// Tauri event name carrying [`ChatEvent`].
pub const CHAT_EVENT: &str = "converse-chat";

/// Who said it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ChatRole {
    /// The person speaking to Vox.
    User,
    /// Vox (text actually handed to speech).
    Assistant,
    /// A fixed note such as `system:started` or `system:dropped:3`.
    System,
}

/// One transcript line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ChatEntry {
    /// Equals the `seq` of the event that carried the entry.
    pub id: u64,
    /// Milliseconds since the Unix epoch when applied.
    pub ts_ms: u64,
    /// Speaker.
    pub role: ChatRole,
    /// Cleaned, capped text.
    pub text: String,
}

/// What happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ChatEventKind {
    /// A line was appended.
    Entry {
        /// The new line.
        entry: ChatEntry,
    },
    /// A new conversation session started; the log was emptied.
    Reset,
    /// The user cleared the log.
    Clear,
    /// The conversation state changed.
    State {
        /// `listening`, `thinking`, `speaking`, `acting` or `idle`.
        state: String,
        /// End reason label when the session ended.
        end: Option<String>,
    },
    /// Recording was switched on or off.
    Config {
        /// Whether new lines are being recorded.
        recording: bool,
    },
}

/// Admission tuple of one line, taken when the line is said. Internal: never serialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    /// Session token current when the line was said.
    pub(crate) session_token: u64,
    /// Recording epoch current when the line was said.
    pub(crate) rec_epoch: u64,
}

/// A consistent copy of the admission tuple (recording flag, epoch and session token).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateState {
    /// Current session token.
    pub(crate) session_token: u64,
    /// Current recording epoch.
    pub(crate) rec_epoch: u64,
    /// Whether lines are being recorded.
    pub(crate) recording: bool,
}

/// Admission gate for the transcript. One lock covers the recording flag, the recording epoch
/// and the session token, so a stamp and the state it is judged against are always read from
/// the same moment.
pub struct AdmissionGate {
    state: Mutex<GateState>,
}

impl AdmissionGate {
    /// A gate starting in the given recording state, with session token 0 and epoch 0.
    pub fn new(recording: bool) -> AdmissionGate {
        AdmissionGate {
            state: Mutex::new(GateState {
                session_token: 0,
                rec_epoch: 0,
                recording,
            }),
        }
    }

    /// The lock is never held across user code, so recover from poisoning.
    fn lock(&self) -> MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stamp for a line said now. `None` while recording is off.
    pub fn stamp(&self) -> Option<Stamp> {
        let state = self.lock();
        state.recording.then_some(Stamp {
            session_token: state.session_token,
            rec_epoch: state.rec_epoch,
        })
    }

    /// A consistent copy of the whole tuple.
    pub fn state(&self) -> GateState {
        *self.lock()
    }

    /// Switch recording on or off. The epoch moves on every call, in both directions.
    pub fn set_recording(&self, on: bool) {
        let mut state = self.lock();
        state.recording = on;
        state.rec_epoch = state.rec_epoch.saturating_add(1);
    }

    /// Start a new session: bumps the session token and returns it. Recording is unchanged.
    #[cfg_attr(not(test), allow(dead_code))] // Session start wiring is a later step.
    pub fn bump_session(&self) -> u64 {
        let mut state = self.lock();
        state.session_token = state.session_token.saturating_add(1);
        state.session_token
    }

    /// Whether a stamp still matches the current tuple while recording is on.
    #[cfg_attr(not(test), allow(dead_code))] // Only tests judge stamps directly; the hub compares tuples.
    pub fn is_current(&self, stamp: Stamp) -> bool {
        let state = self.lock();
        state.recording
            && state.session_token == stamp.session_token
            && state.rec_epoch == stamp.rec_epoch
    }
}

/// One ordered event on the `converse-chat` stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatEvent {
    /// Global, strictly increasing sequence number (never reset during an app run).
    pub seq: u64,
    /// The payload.
    pub kind: ChatEventKind,
    /// Admission stamp of an Entry, taken when the line was said. Internal only: skipped by
    /// serde, so the wire shape of `converse-chat` does not change.
    #[serde(skip)]
    pub(crate) stamp: Option<Stamp>,
}

/// Authoritative state at one `seq`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ChatSnapshot {
    /// Highest `seq` applied when the snapshot was taken.
    pub seq: u64,
    /// Current entries, oldest first (at most `LOG_CAP`).
    pub entries: Vec<ChatEntry>,
    /// Current conversation state label (`idle` initially).
    pub state: String,
    /// End reason of the last session, if any.
    pub end: Option<String>,
    /// Whether new lines are currently recorded.
    pub recording: bool,
    /// Id of the session the window displays (None when idle). The log has no access to the
    /// session registry, so `get_chat_snapshot` fills it in; `snapshot()` leaves it None.
    pub active_session: Option<u64>,
}

/// Why a Clear request did not complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClearError {
    /// Too many Clear requests are already waiting.
    Busy,
    /// The worker did not apply the Clear in time.
    Timeout,
    /// The worker has stopped.
    Dead,
}

/// Receives events in `seq` order from the worker thread only.
pub trait ChatEmitter: Send + Sync + 'static {
    /// Deliver one event. May block (the hooks never wait for it).
    fn emit(&self, event: &ChatEvent);

    /// Deliver one speech mark, in order with `emit`. `on` marks the begin of a speech scope
    /// (with the stamp it was admitted under) or its end (`None`). Marks are not transcript
    /// entries. The default ignores them.
    fn speech(&self, _scope: u64, _on: bool, _stamp: Option<Stamp>) {}
}

/// Tunables (tests shorten the Clear wait).
#[derive(Debug, Clone, Copy)]
pub struct ChatLogConfig {
    /// How long `clear()` waits for the worker before returning `Timeout`.
    pub clear_timeout: Duration,
}

impl Default for ChatLogConfig {
    fn default() -> Self {
        Self {
            clear_timeout: Duration::from_secs(2),
        }
    }
}

/// The transcript log plus its worker. Share as `Arc<ChatLog>`.
/// Private: reply channel of one Clear caller (`Ok` only after the wipe was applied).
type ClearReply = SyncSender<Result<(), ClearError>>;

/// Private: the wipe the worker must apply. Reset wins over Clear.
enum WipeKind {
    Reset,
    Clear,
}

/// Private: an admitted line waiting for the worker.
struct PendingEntry {
    role: ChatRole,
    text: String,
    /// Admission tuple taken when the line was said.
    stamp: Stamp,
}

/// Private: one item in the worker's queue, in call order: a line or a speech mark. Marks are
/// not counted as pending and never reach the log.
enum Queued {
    Line(PendingEntry),
    Mark {
        scope: u64,
        on: bool,
        stamp: Option<Stamp>,
    },
}

/// Private: everything hooks hand to the worker. Guarded by `Shared::inbox`.
struct Inbox {
    /// Lines and speech marks the worker has not taken yet, in call order.
    entries: VecDeque<Queued>,
    /// Admitted lines not yet applied (queued plus taken-but-in-flight). Bounded by PENDING_CAP.
    /// Decremented by the worker only after an entry was applied.
    pending: usize,
    /// Lines refused because `pending` was full; reported as a System marker, then reset.
    dropped: usize,
    /// Pending wipe. A new wipe discards every queued line.
    wipe: Option<WipeKind>,
    /// Clear callers waiting for the wipe (bounded by CLEAR_WAITERS_CAP). Tagged by id so a
    /// timed-out caller can remove exactly its own registration.
    clear_waiters: Vec<(u64, ClearReply)>,
    /// Source of waiter ids.
    next_waiter: u64,
    /// Latest requested state (coalesced: only the final value lands).
    state: Option<(String, Option<String>)>,
    /// Latest requested recording flag (coalesced).
    recording: Option<bool>,
    /// Sticky. Never cleared, and never part of a batch the worker swaps out.
    shutdown: bool,
}

impl Inbox {
    fn new() -> Inbox {
        Inbox {
            entries: VecDeque::new(),
            pending: 0,
            dropped: 0,
            wipe: None,
            clear_waiters: Vec::new(),
            next_waiter: 0,
            state: None,
            recording: None,
            shutdown: false,
        }
    }

    /// True when the worker has something to apply (or must stop).
    fn has_work(&self) -> bool {
        !self.entries.is_empty()
            || self.dropped > 0
            || self.wipe.is_some()
            || self.state.is_some()
            || self.recording.is_some()
            || !self.clear_waiters.is_empty()
    }

    /// Drop queued lines. They would be wiped anyway, so they are not counted as dropped. Only
    /// lines count toward `pending`. Speech marks are dropped too when `marks` is set (reset);
    /// a clear keeps them, since the hub keeps its speech scopes across a clear.
    fn discard_queued(&mut self, marks: bool) {
        let lines = self
            .entries
            .iter()
            .filter(|item| matches!(item, Queued::Line(_)))
            .count();
        self.entries.retain(|item| match item {
            Queued::Line(_) => false,
            Queued::Mark { .. } => !marks,
        });
        self.pending = self.pending.saturating_sub(lines);
    }
}

/// Private: the part of the inbox the worker takes in one swap. Never includes `shutdown`.
struct Batch {
    entries: VecDeque<Queued>,
    dropped: usize,
    wipe: Option<WipeKind>,
    waiters: Vec<(u64, ClearReply)>,
    state: Option<(String, Option<String>)>,
    recording: Option<bool>,
}

/// Private: state owned by the worker and read by `snapshot`. Guarded by `Shared::log`.
struct LogState {
    entries: VecDeque<ChatEntry>,
    /// Last assigned seq (global, starts at 0). Assigned under this lock.
    seq: u64,
    state: String,
    end: Option<String>,
    /// Last applied recording flag.
    recording: bool,
}

/// Private: state shared by the hooks, the worker and the snapshot reader.
struct Shared {
    inbox: Mutex<Inbox>,
    wake: Condvar,
    log: Mutex<LogState>,
    /// Set by the worker on exit (also from a drop guard).
    stopped: Mutex<bool>,
    stopped_cv: Condvar,
    clear_timeout: Duration,
}

impl Shared {
    fn new(recording: bool, clear_timeout: Duration) -> Shared {
        Shared {
            inbox: Mutex::new(Inbox::new()),
            wake: Condvar::new(),
            log: Mutex::new(LogState {
                entries: VecDeque::new(),
                seq: 0,
                state: "idle".to_string(),
                end: None,
                recording,
            }),
            stopped: Mutex::new(false),
            stopped_cv: Condvar::new(),
            clear_timeout,
        }
    }

    /// Hooks: a poisoned inbox means do nothing.
    fn lock_inbox_hook(&self) -> Option<MutexGuard<'_, Inbox>> {
        self.inbox.lock().ok()
    }

    /// Worker and Clear callers: the inbox lock is never held across user code, so recover.
    fn lock_inbox(&self) -> MutexGuard<'_, Inbox> {
        self.inbox.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The log lock is never held across the emitter, so recover from poisoning.
    fn lock_log(&self) -> MutexGuard<'_, LogState> {
        self.log.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn notify(&self) {
        self.wake.notify_all();
    }

    fn mark_shutdown(&self) {
        self.lock_inbox().shutdown = true;
        self.notify();
    }

    fn mark_stopped(&self) {
        let mut stopped = self.stopped.lock().unwrap_or_else(|e| e.into_inner());
        *stopped = true;
        self.stopped_cv.notify_all();
    }
}

/// Private: marks the log stopped and fails any Clear waiter when the worker exits, including
/// when it unwinds, so no caller can wait forever.
struct StopGuard {
    shared: Arc<Shared>,
}

impl Drop for StopGuard {
    fn drop(&mut self) {
        let waiters = {
            let mut inbox = self.shared.lock_inbox();
            inbox.shutdown = true;
            std::mem::take(&mut inbox.clear_waiters)
        };
        for (_, reply) in waiters {
            let _ = reply.send(Err(ClearError::Dead));
        }
        self.shared.mark_stopped();
    }
}

/// Private: cleaning for a hook. Strips control characters except `\n`, turns `\t` into a
/// space, drops `\r`, trims and caps the length.
fn clean_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '\n' => out.push('\n'),
            '\t' => out.push(' '),
            '\r' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    cap_text(out.trim())
}

/// Private: cap to TEXT_CAP chars. A cut text keeps TEXT_CAP-1 chars and then `…`.
fn cap_text(text: &str) -> String {
    if text.chars().count() <= TEXT_CAP {
        return text.to_string();
    }
    let mut capped: String = text.chars().take(TEXT_CAP - 1).collect();
    capped.push('…');
    capped
}

/// Private: wall-clock milliseconds for an applied entry.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Private: the emitter may panic. Isolate it so the worker and the locks survive.
fn emit_guarded(emitter: &dyn ChatEmitter, event: &ChatEvent) {
    let _ = catch_unwind(AssertUnwindSafe(|| emitter.emit(event)));
}

/// Private: wait until there is work or shutdown, then swap the batch out under the lock.
/// Returns the batch and whether this is the final one.
fn next_batch(shared: &Shared) -> (Batch, bool) {
    let mut inbox = shared.lock_inbox();
    while !inbox.has_work() && !inbox.shutdown {
        inbox = shared.wake.wait(inbox).unwrap_or_else(|e| e.into_inner());
    }
    let stopping = inbox.shutdown;
    let batch = Batch {
        entries: std::mem::take(&mut inbox.entries),
        dropped: std::mem::take(&mut inbox.dropped),
        wipe: inbox.wipe.take(),
        waiters: std::mem::take(&mut inbox.clear_waiters),
        state: inbox.state.take(),
        recording: inbox.recording.take(),
    };
    (batch, stopping)
}

/// Private: append one entry to the log (under the log lock) and emit it (lock released).
fn push_entry(
    shared: &Shared,
    emitter: &dyn ChatEmitter,
    role: ChatRole,
    text: String,
    stamp: Option<Stamp>,
) {
    let event = {
        let mut log = shared.lock_log();
        log.seq += 1;
        let entry = ChatEntry {
            id: log.seq,
            ts_ms: now_ms(),
            role,
            text,
        };
        if log.entries.len() >= LOG_CAP {
            log.entries.pop_front();
        }
        log.entries.push_back(entry.clone());
        ChatEvent {
            seq: log.seq,
            kind: ChatEventKind::Entry { entry },
            stamp,
        }
    };
    emit_guarded(emitter, &event);
}

/// Private: one applied entry leaves the pending count. Done after it was applied.
fn release_pending(shared: &Shared) {
    let mut inbox = shared.lock_inbox();
    inbox.pending = inbox.pending.saturating_sub(1);
}

/// Private: deliver one speech mark. Panic-isolated like every other emit.
fn emit_speech_guarded(emitter: &dyn ChatEmitter, scope: u64, on: bool, stamp: Option<Stamp>) {
    let _ = catch_unwind(AssertUnwindSafe(|| emitter.speech(scope, on, stamp)));
}

/// Private: empty the log and emit Reset or Clear.
fn apply_wipe(shared: &Shared, emitter: &dyn ChatEmitter, kind: WipeKind) {
    let event = {
        let mut log = shared.lock_log();
        log.entries.clear();
        log.seq += 1;
        let kind = match kind {
            WipeKind::Reset => ChatEventKind::Reset,
            WipeKind::Clear => ChatEventKind::Clear,
        };
        ChatEvent {
            seq: log.seq,
            kind,
            stamp: None,
        }
    };
    emit_guarded(emitter, &event);
}

/// Private: apply the latest state if it differs from the last applied one.
fn apply_state(shared: &Shared, emitter: &dyn ChatEmitter, state: String, end: Option<String>) {
    let event = {
        let mut log = shared.lock_log();
        if log.state == state && log.end == end {
            return;
        }
        log.seq += 1;
        log.state = state.clone();
        log.end = end.clone();
        ChatEvent {
            seq: log.seq,
            kind: ChatEventKind::State { state, end },
            stamp: None,
        }
    };
    emit_guarded(emitter, &event);
}

/// Private: apply the latest recording flag if it differs from the last applied one.
fn apply_recording(shared: &Shared, emitter: &dyn ChatEmitter, on: bool) {
    let event = {
        let mut log = shared.lock_log();
        if log.recording == on {
            return;
        }
        log.seq += 1;
        log.recording = on;
        ChatEvent {
            seq: log.seq,
            kind: ChatEventKind::Config { recording: on },
            stamp: None,
        }
    };
    emit_guarded(emitter, &event);
}

/// Private: apply one batch in this order: wipe (then ack every waiter), state, recording,
/// entries, and finally the `system:dropped` marker. Recording goes before the entries so a
/// line is never applied while the flag it was recorded under is still unapplied. The marker
/// goes last so the pending count is already free when it is announced.
fn apply_batch(shared: &Shared, emitter: &dyn ChatEmitter, batch: Batch) {
    let Batch {
        entries,
        dropped,
        wipe,
        waiters,
        state,
        recording,
    } = batch;
    let wiped = match wipe {
        Some(kind) => {
            apply_wipe(shared, emitter, kind);
            true
        }
        None => false,
    };
    // Drops counted before a wipe belong to the old session. Announcing them after the wipe
    // would report them in the new one, so a wiped batch never carries a drop marker.
    let dropped = if wiped { 0 } else { dropped };
    for (_, reply) in waiters {
        let answer = if wiped { Ok(()) } else { Err(ClearError::Dead) };
        let _ = reply.send(answer);
    }
    if let Some((state, end)) = state {
        apply_state(shared, emitter, state, end);
    }
    if let Some(on) = recording {
        apply_recording(shared, emitter, on);
    }
    for item in entries {
        match item {
            Queued::Line(pending) => {
                push_entry(
                    shared,
                    emitter,
                    pending.role,
                    pending.text,
                    Some(pending.stamp),
                );
                release_pending(shared);
            }
            Queued::Mark { scope, on, stamp } => {
                emit_speech_guarded(emitter, scope, on, stamp);
            }
        }
    }
    if dropped > 0 {
        let text = format!("system:dropped:{dropped}");
        push_entry(shared, emitter, ChatRole::System, text, None);
    }
}

/// Private: the worker loop. Thread name `chat-log`.
fn run_worker(shared: Arc<Shared>, emitter: Arc<dyn ChatEmitter>) {
    let _stop = StopGuard {
        shared: Arc::clone(&shared),
    };
    loop {
        let (batch, stopping) = next_batch(&shared);
        apply_batch(&shared, emitter.as_ref(), batch);
        if stopping {
            return;
        }
    }
}

/// The transcript log plus its worker. Share as `Arc<ChatLog>`.
pub struct ChatLog {
    shared: Arc<Shared>,
    /// Admission gate: recording flag, recording epoch and session token.
    gate: AdmissionGate,
}

impl Drop for ChatLog {
    /// Dropping the last handle stops the worker without joining it.
    fn drop(&mut self) {
        self.shared.mark_shutdown();
    }
}

impl ChatLog {
    /// Start a log with default tunables. `recording` is the initial recording flag.
    pub fn start(emitter: Arc<dyn ChatEmitter>, recording: bool) -> Arc<ChatLog> {
        Self::start_with(emitter, recording, ChatLogConfig::default())
    }

    /// Start a log with explicit tunables and spawn the worker thread.
    pub fn start_with(
        emitter: Arc<dyn ChatEmitter>,
        recording: bool,
        config: ChatLogConfig,
    ) -> Arc<ChatLog> {
        let shared = Arc::new(Shared::new(recording, config.clear_timeout));
        let worker_shared = Arc::clone(&shared);
        let spawned = thread::Builder::new()
            .name("chat-log".to_string())
            .spawn(move || run_worker(worker_shared, emitter));
        if spawned.is_err() {
            // No worker: behave as a stopped log (hooks no-op, clear returns Dead).
            shared.mark_shutdown();
            shared.mark_stopped();
        }
        Arc::new(ChatLog {
            shared,
            gate: AdmissionGate::new(recording),
        })
    }

    /// Record one line. Non-blocking. No-op when recording is off, after shutdown, or when the
    /// cleaned text is empty. Cleaning: strip control characters except `\n` (`\t` becomes a
    /// space), trim, cap to `TEXT_CAP` chars.
    pub fn say(&self, role: ChatRole, text: &str) {
        // One stamp per line, taken here on the caller thread before any cleaning or queueing.
        let Some(stamp) = self.gate.stamp() else {
            return;
        };
        let cleaned = clean_text(text);
        if cleaned.is_empty() {
            return;
        }
        let Some(mut inbox) = self.shared.lock_inbox_hook() else {
            return;
        };
        if inbox.shutdown {
            return;
        }
        if inbox.pending >= PENDING_CAP {
            inbox.dropped += 1;
        } else {
            inbox.entries.push_back(Queued::Line(PendingEntry {
                role,
                text: cleaned,
                stamp,
            }));
            inbox.pending += 1;
        }
        drop(inbox);
        self.shared.notify();
    }

    /// A reply started being spoken. Stamped on the caller thread like a line, so nothing is
    /// queued while recording is off. Queued in order with the lines, bypassing `PENDING_CAP`.
    /// Not a transcript entry. No-op after shutdown.
    pub fn speech_begin(&self, scope: u64) {
        let Some(stamp) = self.gate.stamp() else {
            return;
        };
        self.queue_mark(scope, true, Some(stamp));
    }

    /// A reply stopped being spoken. Never refused by the recording flag. Queued in order with
    /// the lines, bypassing `PENDING_CAP`. No-op after shutdown.
    pub fn speech_end(&self, scope: u64) {
        self.queue_mark(scope, false, None);
    }

    /// Private: queue one speech mark behind the lines already queued.
    fn queue_mark(&self, scope: u64, on: bool, stamp: Option<Stamp>) {
        let Some(mut inbox) = self.shared.lock_inbox_hook() else {
            return;
        };
        if inbox.shutdown {
            return;
        }
        inbox.entries.push_back(Queued::Mark { scope, on, stamp });
        drop(inbox);
        self.shared.notify();
    }

    /// Session boundary: empty the log (pending entries are discarded, a `Reset` event follows).
    pub fn reset(&self) {
        let Some(mut inbox) = self.shared.lock_inbox_hook() else {
            return;
        };
        if inbox.shutdown {
            return;
        }
        inbox.discard_queued(true);
        inbox.wipe = Some(WipeKind::Reset);
        drop(inbox);
        self.shared.notify();
    }

    /// Latest conversation state (coalesced; the final value always lands).
    pub fn set_state(&self, state: &str, end: Option<&str>) {
        let Some(mut inbox) = self.shared.lock_inbox_hook() else {
            return;
        };
        if inbox.shutdown {
            return;
        }
        inbox.state = Some((state.to_string(), end.map(str::to_string)));
        drop(inbox);
        self.shared.notify();
    }

    /// Switch recording on or off (takes effect for later `say` calls immediately; the final
    /// value always lands as a `Config` event; existing entries are kept).
    pub fn set_recording(&self, on: bool) {
        // The gate moves first and synchronously, so no later `say` is stamped under the old flag.
        self.gate.set_recording(on);
        let Some(mut inbox) = self.shared.lock_inbox_hook() else {
            return;
        };
        if inbox.shutdown {
            return;
        }
        inbox.recording = Some(on);
        drop(inbox);
        self.shared.notify();
    }

    /// Current recording flag as seen by the admission gate.
    #[cfg_attr(not(test), allow(dead_code))] // Only tests read it; production reads the snapshot.
    pub fn recording(&self) -> bool {
        self.gate.state().recording
    }

    /// The admission gate. Hubs admit lines against its current tuple.
    #[cfg_attr(not(test), allow(dead_code))] // Consumers arrive with the bubble wiring.
    pub fn gate(&self) -> &AdmissionGate {
        &self.gate
    }

    /// User-initiated Clear. Waits up to `clear_timeout` for the worker (never call from a
    /// hook thread). `Ok` means the log was emptied before returning.
    pub fn clear(&self) -> Result<(), ClearError> {
        let (reply, rx) = sync_channel::<Result<(), ClearError>>(1);
        let id = {
            let Some(mut inbox) = self.shared.lock_inbox_hook() else {
                return Err(ClearError::Dead);
            };
            if inbox.shutdown {
                return Err(ClearError::Dead);
            }
            if inbox.clear_waiters.len() >= CLEAR_WAITERS_CAP {
                return Err(ClearError::Busy);
            }
            inbox.discard_queued(false);
            if !matches!(inbox.wipe, Some(WipeKind::Reset)) {
                inbox.wipe = Some(WipeKind::Clear);
            }
            inbox.next_waiter += 1;
            let id = inbox.next_waiter;
            inbox.clear_waiters.push((id, reply));
            id
        };
        self.shared.notify();
        match rx.recv_timeout(self.shared.clear_timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Disconnected) => Err(ClearError::Dead),
            Err(RecvTimeoutError::Timeout) => {
                // Remove our own registration so the list stays bounded.
                self.shared
                    .lock_inbox()
                    .clear_waiters
                    .retain(|(waiter, _)| *waiter != id);
                // An ack may have raced in while we were removing ourselves. Otherwise this is a
                // timeout (`Disconnected` only means our own sender was just dropped unacked).
                match rx.try_recv() {
                    Ok(result) => result,
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => {
                        Err(ClearError::Timeout)
                    }
                }
            }
        }
    }

    /// Consistent view of the log (brief lock; never touches the emitter).
    pub fn snapshot(&self) -> ChatSnapshot {
        let log = self.shared.lock_log();
        ChatSnapshot {
            seq: log.seq,
            entries: log.entries.iter().cloned().collect(),
            state: log.state.clone(),
            end: log.end.clone(),
            recording: log.recording,
            active_session: None,
        }
    }

    /// Sticky shutdown: later hooks no-op, queued Clear waiters complete or fail explicitly
    /// and the worker exits.
    pub fn shutdown(&self) {
        self.shared.mark_shutdown();
    }

    /// Wait until the worker thread has exited; false on timeout.
    #[cfg_attr(not(test), allow(dead_code))] // Only tests wait for the worker to stop.
    pub fn wait_stopped(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut stopped = self
            .shared
            .stopped
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        loop {
            if *stopped {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (guard, _) = match self.shared.stopped_cv.wait_timeout(stopped, deadline - now) {
                Ok(pair) => pair,
                Err(e) => e.into_inner(),
            };
            stopped = guard;
        }
    }
}
