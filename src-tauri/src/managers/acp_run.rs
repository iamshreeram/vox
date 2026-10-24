//! Turn timing and run registry for ACP turns (DESIGN.md Revision 3: S3
//! "Turn timing + cancel", C2 turn budget, C4 commit linearization, C12
//! delivery failure).
//!
//! No IO, no threads spawned here, no Tauri types. Tests: `acp_run_tests.rs`.
//!
//! [`TurnTiming`] is plain arithmetic over caller-supplied instants. Overlapping
//! approval waits count as the union of their time, so the turn is extended by
//! exactly the time it was paused, never more.
//!
//! [`RunRegistry`] owns the connection (generation plus non-blocking sink), the
//! single active run and its cancel latch. Begin, set_session, commit, cancel
//! and finish are each one critical section under one `Mutex`.
//!
//! Lock order: the registry lock is the OUTERMOST lock of the ACP stack. Never
//! call the permission broker, a prompter, or any other user callback while
//! holding it, and never block while holding it. Under the lock the registry
//! only mutates its own state and calls [`ResponseSink::try_send_line`], which
//! must never block. The caller-supplied `cancel_line` formatter and the
//! `pre_lock` hook therefore run outside the lock.
//!
//! Poisoned mutexes are recovered with `into_inner`: the state is plain data
//! and every transition leaves it consistent.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Multiple of the base turn timeout that bounds the whole turn, approval
/// pauses included.
const HARD_DEADLINE_TURNS: u32 = 3;

/// Maximum permission requests one run may present to the user (flood guard).
/// Further requests in the same run are answered Cancelled without prompting.
pub const MAX_PERMISSION_REQUESTS_PER_RUN: u32 = 32;

/// Why a line could not be queued on the writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkError {
    /// The writer queue is full (backpressure or burst).
    Full,
    /// The writer has gone away.
    Disconnected,
}

/// Outcome of committing a response for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitResult {
    /// The line was queued on the current connection for the live run.
    Delivered,
    /// The binding is stale or cancelled, or no connection exists. Nothing was sent.
    Dropped,
    /// The writer refused the line. The run's delivery-failure flag is set.
    DeliveryFailed,
}

/// Why a run could not begin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeginError {
    /// Another run is still unfinished.
    Busy,
    /// This handle's token already began once. Tokens are never reused.
    Consumed,
}

/// Result of recording the agent's session id for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSet {
    /// The session id was recorded.
    Ok,
    /// The run was cancelled first. The session id was not recorded.
    Cancelled,
    /// The handle is not the live run (never began, finished, or superseded).
    Stale,
}

/// Non-blocking destination for JSON-RPC lines (the agent writer).
///
/// Implementations must never block: the registry calls this under its lock.
pub trait ResponseSink: Send + Sync {
    /// Queues one line without blocking. Returns an error instead of waiting.
    fn try_send_line(&self, line: String) -> Result<(), SinkError>;
}

/// Opaque identity of one prompt turn. Copyable and carries only its token.
///
/// Holding a handle grants no authority by itself: cancel, commit and finish
/// compare the token with the live run under the registry lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunHandle {
    token: u64,
}

impl RunHandle {
    /// Unique, monotonic token of this run. Never reused within one registry.
    pub fn token(&self) -> u64 {
        self.token
    }
}

/// Deadline arithmetic for one ACP turn.
///
/// All time is passed in by the caller, so the type is deterministic under
/// test. The effective deadline is `start + turn_timeout + paused_total`,
/// extended by the open pause while approvals wait, and never later than the
/// hard deadline.
#[derive(Debug, Clone)]
pub struct TurnTiming {
    start: Instant,
    turn_timeout: Duration,
    hard_deadline: Instant,
    pending: u32,
    pause_started: Option<Instant>,
    paused_total: Duration,
}

impl TurnTiming {
    /// Starts timing a turn at `start` with base budget `turn_timeout`.
    ///
    /// The hard deadline is `start + 3 * turn_timeout`. If that overflows
    /// `Instant`, it collapses to `start` (fail closed: no extra time).
    pub fn new(start: Instant, turn_timeout: Duration) -> Self {
        let hard_budget = turn_timeout.saturating_mul(HARD_DEADLINE_TURNS);
        Self {
            start,
            turn_timeout,
            hard_deadline: start.checked_add(hard_budget).unwrap_or(start),
            pending: 0,
            pause_started: None,
            paused_total: Duration::ZERO,
        }
    }

    /// Absolute cap for the whole turn, approval pauses included.
    pub fn hard_deadline(&self) -> Instant {
        self.hard_deadline
    }

    /// Number of approvals currently waiting on the user.
    pub fn pending(&self) -> u32 {
        self.pending
    }

    /// Marks the start of an approval wait at `now`. Only the first of
    /// overlapping waits starts the pause clock.
    pub fn enter_approval(&mut self, now: Instant) {
        if self.pending == 0 {
            self.pause_started = Some(now);
        }
        self.pending = self.pending.saturating_add(1);
    }

    /// Marks the end of an approval wait at `now`. When the last overlapping
    /// wait ends, the elapsed union is added to the paused total. An exit with
    /// no matching enter is ignored.
    pub fn exit_approval(&mut self, now: Instant) {
        if self.pending == 0 {
            return;
        }
        self.pending -= 1;
        if self.pending == 0 {
            if let Some(started) = self.pause_started.take() {
                let waited = now.saturating_duration_since(started);
                self.paused_total = self.paused_total.saturating_add(waited);
            }
        }
    }

    /// Effective deadline as seen at `now`:
    /// `min(hard_deadline, start + turn_timeout + paused)`, where `paused`
    /// includes the open pause while approvals are pending.
    pub fn effective_deadline(&self, now: Instant) -> Instant {
        let open_pause = match (self.pending, self.pause_started) {
            (0, _) | (_, None) => Duration::ZERO,
            (_, Some(started)) => now.saturating_duration_since(started),
        };
        let paused = self.paused_total.saturating_add(open_pause);
        let budget = self.turn_timeout.saturating_add(paused);
        let extended = self.start.checked_add(budget).unwrap_or(self.hard_deadline);
        extended.min(self.hard_deadline)
    }

    /// True once `now` has reached the effective deadline.
    pub fn expired(&self, now: Instant) -> bool {
        now >= self.effective_deadline(now)
    }

    /// Time a new approval may wait at `now`: the configured timeout, shrunk to
    /// the remaining hard budget. Zero means no prompt may be shown.
    pub fn approval_budget(&self, now: Instant, configured: Duration) -> Duration {
        configured.min(self.hard_deadline.saturating_duration_since(now))
    }
}

/// The single active run. `finished` keeps the record readable (for the
/// delivery-failure flag) after the run ends, while the slot is free again.
struct ActiveRun {
    token: u64,
    session: Option<String>,
    cancel_latched: bool,
    delivery_failed: bool,
    finished: bool,
    permission_requests: u32,
}

/// Current writer connection: its generation plus its non-blocking sink.
struct Connection {
    conn_gen: u64,
    sink: Arc<dyn ResponseSink>,
}

/// Everything the registry guards with its single lock.
#[derive(Default)]
struct State {
    connection: Option<Connection>,
    run: Option<ActiveRun>,
    next_token: u64,
    last_begun: u64,
}

/// Returns the active run if it is `token` and has not finished.
fn live_run(state: &mut State, token: u64) -> Option<&mut ActiveRun> {
    state
        .run
        .as_mut()
        .filter(|r| r.token == token && !r.finished)
}

/// Registry of the connection and the one active ACP run.
///
/// See the module docs for the lock-order rule. A handle that is not the live
/// run can never cancel, commit or set a session.
pub struct RunRegistry {
    state: Mutex<State>,
}

impl RunRegistry {
    /// Creates an empty registry with no connection and no active run.
    pub fn new() -> Arc<Self> {
        let state = State {
            next_token: 1,
            ..State::default()
        };
        Arc::new(Self {
            state: Mutex::new(state),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Registers writer generation `conn_gen` and its sink, replacing any previous one.
    pub fn set_connection(&self, conn_gen: u64, sink: Arc<dyn ResponseSink>) {
        self.lock().connection = Some(Connection { conn_gen, sink });
    }

    /// Clears the connection only if it is still `conn_gen`. Commits made
    /// afterwards are dropped.
    pub fn clear_connection(&self, conn_gen: u64) {
        let mut state = self.lock();
        if state
            .connection
            .as_ref()
            .is_some_and(|c| c.conn_gen == conn_gen)
        {
            state.connection = None;
        }
    }

    /// Mints an inactive handle with a fresh, unique, monotonic token.
    pub fn new_handle(&self) -> RunHandle {
        let mut state = self.lock();
        let token = state.next_token;
        state.next_token = state.next_token.saturating_add(1);
        RunHandle { token }
    }

    /// Makes `handle` the active run.
    ///
    /// Fails with [`BeginError::Busy`] while another run is unfinished, and with
    /// [`BeginError::Consumed`] for a token that has already begun. A Busy handle
    /// never becomes active and so can never cancel.
    pub fn begin(&self, handle: &RunHandle) -> Result<(), BeginError> {
        let mut state = self.lock();
        if state.run.as_ref().is_some_and(|r| !r.finished) {
            return Err(BeginError::Busy);
        }
        if handle.token <= state.last_begun {
            return Err(BeginError::Consumed);
        }
        state.last_begun = handle.token;
        state.run = Some(ActiveRun {
            token: handle.token,
            session: None,
            cancel_latched: false,
            delivery_failed: false,
            finished: false,
            permission_requests: 0,
        });
        Ok(())
    }

    /// Records the agent's session id for the live run `handle`.
    ///
    /// Returns [`SessionSet::Cancelled`] if the run was already cancelled, and
    /// [`SessionSet::Stale`] if `handle` is not the live run.
    pub fn set_session(&self, handle: &RunHandle, session_id: &str) -> SessionSet {
        let mut state = self.lock();
        let Some(run) = live_run(&mut state, handle.token) else {
            return SessionSet::Stale;
        };
        if run.cancel_latched {
            return SessionSet::Cancelled;
        }
        run.session = Some(session_id.to_owned());
        SessionSet::Ok
    }

    /// Commits one response line for the run bound to `(conn_gen, run_token,
    /// session_id)`. This is the linearization point: validation and the
    /// non-blocking send happen in one critical section, so a commit that was
    /// sent before a cancel stays delivered and any later one is dropped.
    pub fn commit(
        &self,
        conn_gen: u64,
        run_token: u64,
        session_id: &str,
        line: String,
    ) -> CommitResult {
        self.commit_with_hook(conn_gen, run_token, session_id, line, &|| {})
    }

    /// Same as [`Self::commit`], but runs `pre_lock` first, before the registry
    /// lock is taken and never while it is held. Tests use it to pause a commit
    /// at the exact point before it linearizes.
    pub fn commit_with_hook(
        &self,
        conn_gen: u64,
        run_token: u64,
        session_id: &str,
        line: String,
        pre_lock: &dyn Fn(),
    ) -> CommitResult {
        pre_lock();
        let mut guard = self.lock();
        let State {
            connection, run, ..
        } = &mut *guard;
        let (Some(conn), Some(active)) = (connection.as_ref(), run.as_mut()) else {
            return CommitResult::Dropped;
        };
        let valid = conn.conn_gen == conn_gen
            && active.token == run_token
            && !active.finished
            && !active.cancel_latched
            && active.session.as_deref() == Some(session_id);
        if !valid {
            return CommitResult::Dropped;
        }
        match conn.sink.try_send_line(line) {
            Ok(()) => CommitResult::Delivered,
            Err(_) => {
                active.delivery_failed = true;
                CommitResult::DeliveryFailed
            }
        }
    }

    /// Cancels the live run `handle`. Returns true only for the call that
    /// latched the cancel; repeats and stale or Busy handles return false.
    ///
    /// The latch is set under the lock. If the session is known, `cancel_line`
    /// formats the session/cancel line outside the lock (it is a user callback),
    /// and the line is then queued only if the same run and connection are still
    /// live. A refused cancel line is not retried: the latch already stops the turn.
    pub fn cancel(&self, handle: &RunHandle, cancel_line: &dyn Fn(&str) -> String) -> bool {
        let (session, conn_gen) = {
            let mut state = self.lock();
            let conn_gen = state.connection.as_ref().map(|c| c.conn_gen);
            let Some(run) = live_run(&mut state, handle.token) else {
                return false;
            };
            if run.cancel_latched {
                return false;
            }
            run.cancel_latched = true;
            (run.session.clone(), conn_gen)
        };
        if let (Some(session_id), Some(conn_gen)) = (session, conn_gen) {
            let line = cancel_line(&session_id);
            self.send_cancel_if_live(handle.token, conn_gen, line);
        }
        true
    }

    /// Queues a cancel line only if run `run_token` is still live on connection
    /// `conn_gen`. Takes the lock itself: it is never called with the lock held.
    fn send_cancel_if_live(&self, run_token: u64, conn_gen: u64, line: String) {
        let mut guard = self.lock();
        let State {
            connection, run, ..
        } = &mut *guard;
        let (Some(conn), Some(active)) = (connection.as_ref(), run.as_ref()) else {
            return;
        };
        if conn.conn_gen == conn_gen && active.token == run_token && !active.finished {
            let _ = conn.sink.try_send_line(line);
        }
    }

    /// Marks the live run `handle` as finished, freeing the slot for a new run.
    /// Idempotent. A stale handle does nothing.
    pub fn finish(&self, handle: &RunHandle) {
        let mut state = self.lock();
        if let Some(run) = live_run(&mut state, handle.token) {
            run.finished = true;
        }
    }

    /// True if a permission response for run `handle` was refused by the writer.
    /// The record stays readable after finish until the next run begins.
    pub fn delivery_failed(&self, handle: &RunHandle) -> bool {
        let state = self.lock();
        state
            .run
            .as_ref()
            .is_some_and(|r| r.token == handle.token && r.delivery_failed)
    }

    /// True once run `handle` has been cancelled (latched), whether or not the
    /// cancel line was sent.
    pub fn is_cancelled(&self, handle: &RunHandle) -> bool {
        let state = self.lock();
        state
            .run
            .as_ref()
            .is_some_and(|r| r.token == handle.token && r.cancel_latched)
    }

    /// Counts one permission request for the live run `handle`. Returns false
    /// (do not prompt) once the run is cancelled, finished, stale, or has
    /// reached [`MAX_PERMISSION_REQUESTS_PER_RUN`].
    pub fn note_permission_request(&self, handle: &RunHandle) -> bool {
        let mut state = self.lock();
        let Some(run) = live_run(&mut state, handle.token) else {
            return false;
        };
        if run.cancel_latched || run.permission_requests >= MAX_PERMISSION_REQUESTS_PER_RUN {
            return false;
        }
        run.permission_requests += 1;
        true
    }
}
