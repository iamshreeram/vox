//! Bounded, one-at-a-time broker for ACP permission requests.
//!
//! The broker owns every pending `session/request_permission` call. At most one
//! request is presented at a time, and at most three more wait behind it. Each
//! request is a [`Ticket`] that reaches exactly one terminal state:
//!
//! - answered through [`PermissionBroker::resolve`] (`Done(choice)`),
//! - timed out by the worker at the ticket deadline (`Done(TimedOut)`),
//! - invalidated because its run or connection went away (`Invalidated`).
//!
//! Only a `Done` ticket produces a committed outcome, delivered through the
//! [`CommitFn`] callback. An invalidated ticket never commits. Overflowing
//! requests are committed as `Cancelled` by [`PermissionBroker::submit`] and are
//! never presented.
//!
//! Lock rules:
//!
//! - The broker state lock is never held while calling [`PermissionPrompter`]
//!   methods or the commit callback, and never while waiting on a ticket's
//!   condition variable. Callbacks may therefore re-enter the broker
//!   (`submit`, `resolve`, `invalidate_*`, `pending_len`) without deadlock.
//! - Lock order is broker state first, then a ticket's state. No ticket lock is
//!   held while the broker lock is taken.
//! - Poisoned mutexes are recovered, never propagated as panics.
//! - Panics raised by the prompter or the commit callback are caught and logged
//!   without request details. A panicking `present` rejects its ticket, and the
//!   worker keeps running.
//!
//! [`PermissionPrompter`] is presentation-only. It shows or hides a prompt and
//! never answers it; answers arrive only through [`PermissionBroker::resolve`].

use crate::managers::acp_permission::{
    outcome_for, OperationDetails, Outcome, PermissionRequest, ToolKind, UserChoice,
};
use serde_json::Value;
use std::collections::hash_map::RandomState;
use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, Hasher};
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Maximum live tickets: one presented plus three queued.
const MAX_LIVE_TICKETS: usize = 4;

/// Name of the single worker thread.
const WORKER_NAME: &str = "acp-permission-broker";

/// Identifies the agent request a decision belongs to.
///
/// Two requests are the same only when both `conn_gen` and `rpc_id` match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestBinding {
    /// Connection generation the request arrived on.
    pub conn_gen: u64,
    /// Turn (run) the request belongs to.
    pub run_token: u64,
    /// ACP session the request was made in.
    pub session_id: String,
    /// JSON-RPC id exactly as the agent sent it (typed, not stringified).
    pub rpc_id: Value,
}

/// What a prompter shows for one presented request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionView {
    /// Opaque prompt token. It is never the RPC id.
    pub token: String,
    /// Sanitised title. Untrusted text, display only.
    pub title: String,
    /// Tool kind reported by the agent.
    pub kind: ToolKind,
    /// Bounded operation details, when the agent provided any.
    pub details: Option<OperationDetails>,
    /// Time left before the prompt times out, measured when it was presented.
    pub remaining: Duration,
    /// Requests still waiting behind this one.
    pub queued: usize,
}

/// Presentation-only sink for permission prompts. It never answers a prompt.
pub trait PermissionPrompter: Send + Sync {
    /// Shows a prompt. Must not block waiting for the user.
    fn present(&self, view: &PermissionView);

    /// Hides the prompt identified by `token`. Called exactly once per presented prompt.
    fn close(&self, token: &str);
}

/// Delivers the final outcome for a binding. Called by the worker with no broker lock held.
pub type CommitFn = Arc<dyn Fn(RequestBinding, Outcome) + Send + Sync>;

/// Result of [`PermissionBroker::submit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitResult {
    /// The request was accepted. `token` identifies its prompt.
    Queued {
        /// Opaque prompt token.
        token: String,
    },
    /// A pending request with the same `conn_gen` and `rpc_id` already exists. Nothing changed.
    Duplicate,
    /// The broker is full, or the worker could not start. The request was committed
    /// as `Cancelled` before this returned and was never presented.
    Overflow,
}

/// Lifecycle of one ticket. Only `Pending` may change; every other state is final.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TicketState {
    Pending,
    Done(UserChoice),
    Invalidated,
}

/// One submitted request and its completion state.
struct Ticket {
    token: String,
    binding: RequestBinding,
    req: PermissionRequest,
    deadline: Instant,
    state: Mutex<TicketState>,
    wake: Condvar,
}

/// Locks a mutex, recovering the guard if a previous holder panicked.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl Ticket {
    /// Creates a pending ticket.
    fn new(
        token: String,
        binding: RequestBinding,
        req: PermissionRequest,
        deadline: Instant,
    ) -> Arc<Self> {
        Arc::new(Self {
            token,
            binding,
            req,
            deadline,
            state: Mutex::new(TicketState::Pending),
            wake: Condvar::new(),
        })
    }

    /// Moves a pending ticket to `next` and wakes any waiter.
    ///
    /// Returns `true` only for the call that performed the transition (first caller wins).
    fn complete(&self, next: TicketState) -> bool {
        let mut guard = lock(&self.state);
        if *guard != TicketState::Pending {
            return false;
        }
        *guard = next;
        self.wake.notify_all();
        true
    }

    /// Returns the current state.
    fn current(&self) -> TicketState {
        *lock(&self.state)
    }

    /// Returns `true` while the ticket is still awaiting an answer.
    fn is_pending(&self) -> bool {
        self.current() == TicketState::Pending
    }

    /// Returns `true` if this pending ticket is the same request as `binding`.
    fn is_pending_for(&self, binding: &RequestBinding) -> bool {
        self.binding.conn_gen == binding.conn_gen
            && self.binding.rpc_id == binding.rpc_id
            && self.is_pending()
    }

    /// Blocks until the ticket is terminal or its deadline passes.
    ///
    /// Spurious wakeups are handled by re-reading the state and the clock.
    fn wait_until_settled(&self) {
        let mut guard = lock(&self.state);
        loop {
            if *guard != TicketState::Pending {
                return;
            }
            let now = Instant::now();
            if now >= self.deadline {
                return;
            }
            let (next, _) = self
                .wake
                .wait_timeout(guard, self.deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            guard = next;
        }
    }
}

/// Mutable broker state. Guarded by `Inner::state`; never held during callbacks.
#[derive(Default)]
struct BrokerState {
    /// Tickets waiting to be presented, oldest first.
    queue: VecDeque<Arc<Ticket>>,
    /// Every live ticket (presented and queued), keyed by token.
    pending: HashMap<String, Arc<Ticket>>,
    /// Token of the ticket the worker is currently handling, if any.
    presented: Option<String>,
    /// Whether a worker thread is running and will re-check the queue before exiting.
    worker_running: bool,
}

/// Shared broker internals. The worker thread holds its own `Arc` to this.
struct Inner {
    prompter: Arc<dyn PermissionPrompter>,
    commit: CommitFn,
    state: Mutex<BrokerState>,
    seq: AtomicU64,
    nonce: u64,
}

/// Bounded, one-at-a-time permission broker. See the module docs for the lock rules.
pub struct PermissionBroker {
    inner: Arc<Inner>,
}

impl PermissionBroker {
    /// Creates a broker that presents through `prompter` and delivers outcomes through `commit`.
    pub fn new(prompter: Arc<dyn PermissionPrompter>, commit: CommitFn) -> Arc<Self> {
        let nonce = RandomState::new().build_hasher().finish();
        let inner = Inner {
            prompter,
            commit,
            state: Mutex::new(BrokerState::default()),
            seq: AtomicU64::new(0),
            nonce,
        };
        Arc::new(Self {
            inner: Arc::new(inner),
        })
    }

    /// Accepts a permission request for presentation.
    ///
    /// `deadline` is absolute. Returns `Queued` with the prompt token, `Duplicate`
    /// when the same `(conn_gen, rpc_id)` is already pending (nothing changes), or
    /// `Overflow` when the broker is full. An overflowing request is committed as
    /// `Cancelled` before this returns.
    pub fn submit(
        &self,
        binding: RequestBinding,
        req: PermissionRequest,
        deadline: Instant,
    ) -> SubmitResult {
        let inner = &self.inner;
        let seq = inner.seq.fetch_add(1, Ordering::Relaxed);
        let token = format!("perm-{:016x}-{seq}", inner.nonce);
        let ticket = Ticket::new(token.clone(), binding.clone(), req, deadline);
        let admission = {
            let mut st = lock(&inner.state);
            if st.pending.values().any(|t| t.is_pending_for(&binding)) {
                Admission::Duplicate
            } else if st.pending.len() >= MAX_LIVE_TICKETS {
                Admission::Overflow
            } else {
                st.pending.insert(token.clone(), Arc::clone(&ticket));
                st.queue.push_back(ticket);
                let start_worker = !st.worker_running;
                st.worker_running = true;
                Admission::Queued { start_worker }
            }
        };
        match admission {
            Admission::Duplicate => SubmitResult::Duplicate,
            Admission::Overflow => {
                inner.commit_outcome(binding, Outcome::Cancelled);
                SubmitResult::Overflow
            }
            Admission::Queued { start_worker } => {
                if start_worker && spawn_worker(inner).is_err() {
                    inner.abort_queued();
                    return SubmitResult::Overflow;
                }
                SubmitResult::Queued { token }
            }
        }
    }

    /// Records the user's (or a timer's) choice for the prompt `token`.
    ///
    /// Returns `true` only if this call moved a pending ticket to `Done`. An unknown,
    /// already answered, or invalidated token returns `false` and changes nothing.
    pub fn resolve(&self, token: &str, choice: UserChoice) -> bool {
        let ticket = lock(&self.inner.state).pending.get(token).cloned();
        match ticket {
            Some(ticket) => ticket.complete(TicketState::Done(choice)),
            None => false,
        }
    }

    /// Returns the number of live tickets (presented plus queued).
    pub fn pending_len(&self) -> usize {
        lock(&self.inner.state).pending.len()
    }

    /// Invalidates every live ticket belonging to `run_token`.
    ///
    /// Returns how many pending tickets were invalidated (presented plus queued).
    /// Invalidated tickets never commit an outcome.
    pub fn invalidate_run(&self, run_token: u64) -> usize {
        self.invalidate_matching(|b| b.run_token == run_token)
    }

    /// Invalidates every live ticket that arrived on connection generation `conn_gen`.
    ///
    /// Returns how many pending tickets were invalidated (presented plus queued).
    pub fn invalidate_conn(&self, conn_gen: u64) -> usize {
        self.invalidate_matching(|b| b.conn_gen == conn_gen)
    }

    /// Marks matching pending tickets invalidated and drops matching queued ones.
    ///
    /// The presented ticket stays in the pending map until the worker has closed its
    /// prompt. Callers hold no other lock, and no callback runs under this lock.
    fn invalidate_matching(&self, matches: impl Fn(&RequestBinding) -> bool) -> usize {
        let mut st = lock(&self.inner.state);
        let hits: Vec<Arc<Ticket>> = st
            .pending
            .values()
            .filter(|t| matches(&t.binding))
            .cloned()
            .collect();
        let mut count = 0;
        for ticket in &hits {
            if ticket.complete(TicketState::Invalidated) {
                count += 1;
            }
            if st.presented.as_deref() != Some(ticket.token.as_str()) {
                st.pending.remove(&ticket.token);
                st.queue.retain(|queued| queued.token != ticket.token);
            }
        }
        count
    }
}

/// Outcome of the admission check in [`PermissionBroker::submit`].
enum Admission {
    Duplicate,
    Overflow,
    Queued { start_worker: bool },
}

/// Starts the single worker thread.
fn spawn_worker(inner: &Arc<Inner>) -> std::io::Result<()> {
    let worker = Arc::clone(inner);
    std::thread::Builder::new()
        .name(WORKER_NAME.to_string())
        .spawn(move || worker_loop(worker))
        .map(|_| ())
}

impl Inner {
    /// Cancels every queued ticket after the worker failed to start.
    ///
    /// Each request is committed as `Cancelled`, so nothing is left unanswered.
    fn abort_queued(&self) {
        let drained: Vec<Arc<Ticket>> = {
            let mut st = lock(&self.state);
            st.worker_running = false;
            let drained: Vec<Arc<Ticket>> = st.queue.drain(..).collect();
            for ticket in &drained {
                st.pending.remove(&ticket.token);
            }
            drained
        };
        for ticket in drained {
            ticket.complete(TicketState::Invalidated);
            self.commit_outcome(ticket.binding.clone(), Outcome::Cancelled);
        }
    }

    /// Calls the prompter's `present`, catching a panic. Returns `false` if it panicked.
    ///
    /// The panic is logged without request details. The caller then rejects the ticket.
    fn present_guarded(&self, ticket: &Ticket, queued: usize) -> bool {
        let view = PermissionView {
            token: ticket.token.clone(),
            title: ticket.req.title.clone(),
            kind: ticket.req.kind,
            details: ticket.req.details.clone(),
            remaining: ticket.deadline.saturating_duration_since(Instant::now()),
            queued,
        };
        let call = panic::catch_unwind(AssertUnwindSafe(|| self.prompter.present(&view)));
        if call.is_err() {
            log::warn!("permission prompt panicked while presenting; request will be rejected");
        }
        call.is_ok()
    }

    /// Calls the prompter's `close`, catching and logging any panic.
    fn close_guarded(&self, token: &str) {
        let call = panic::catch_unwind(AssertUnwindSafe(|| self.prompter.close(token)));
        if call.is_err() {
            log::warn!("permission prompt panicked while closing");
        }
    }

    /// Delivers an outcome through the commit callback, catching and logging any panic.
    fn commit_outcome(&self, binding: RequestBinding, outcome: Outcome) {
        let call = panic::catch_unwind(AssertUnwindSafe(|| (self.commit)(binding, outcome)));
        if call.is_err() {
            log::warn!("permission outcome callback panicked");
        }
    }

    /// Presents `ticket` if it is still live, then waits until it settles.
    ///
    /// A ticket that is already answered or whose deadline has passed is never
    /// presented. A deadline that passes while the prompt is up completes the
    /// ticket as `TimedOut`. Returns whether the prompt was shown.
    fn present_and_settle(&self, ticket: &Ticket, queued: usize) -> bool {
        let mut shown = ticket.is_pending() && Instant::now() < ticket.deadline;
        if shown {
            shown = self.present_guarded(ticket, queued);
        }
        if shown {
            ticket.wait_until_settled();
        }
        // Only effective when the deadline passed while still pending.
        ticket.complete(TicketState::Done(UserChoice::TimedOut));
        shown
    }

    /// Closes the prompt (if shown), drops the ticket, then commits a `Done` outcome.
    ///
    /// Invalidated tickets commit nothing. No lock is held during the callbacks.
    fn finish_ticket(&self, ticket: &Ticket, shown: bool) {
        let settled = ticket.current();
        if shown {
            self.close_guarded(&ticket.token);
        }
        {
            let mut st = lock(&self.state);
            st.pending.remove(&ticket.token);
            if st.presented.as_deref() == Some(ticket.token.as_str()) {
                st.presented = None;
            }
        }
        if let TicketState::Done(choice) = settled {
            self.commit_outcome(ticket.binding.clone(), outcome_for(choice, &ticket.req));
        }
    }
}

/// Restores worker bookkeeping if the worker unwinds before its loop ends.
///
/// The loop disarms it on a normal exit. On unwind it rejects the presented ticket
/// (if still pending), clears the running flag, and starts a replacement worker when
/// tickets are still queued. If that replacement cannot start, the queue is cancelled.
struct WorkerGuard {
    inner: Arc<Inner>,
    armed: bool,
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let (orphan, restart) = {
            let mut st = lock(&self.inner.state);
            let orphan = st
                .presented
                .take()
                .and_then(|token| st.pending.remove(&token));
            let restart = !st.queue.is_empty();
            st.worker_running = restart;
            (orphan, restart)
        };
        if let Some(ticket) = orphan {
            self.inner.close_guarded(&ticket.token);
            if ticket.complete(TicketState::Done(UserChoice::TimedOut)) {
                let outcome = outcome_for(UserChoice::TimedOut, &ticket.req);
                self.inner.commit_outcome(ticket.binding.clone(), outcome);
            }
        }
        if restart && spawn_worker(&self.inner).is_err() {
            self.inner.abort_queued();
        }
    }
}

/// Worker loop: handles queued tickets one at a time, then exits when the queue is empty.
fn worker_loop(inner: Arc<Inner>) {
    let mut guard = WorkerGuard {
        inner: Arc::clone(&inner),
        armed: true,
    };
    while let Some((ticket, queued)) = next_ticket(&inner) {
        let shown = inner.present_and_settle(&ticket, queued);
        inner.finish_ticket(&ticket, shown);
    }
    guard.armed = false;
}

/// Pops the next ticket and records it as presented. When the queue is empty, clears
/// the running flag under the same lock, so a concurrent `submit` either sees the
/// flag set (and this loop picks its ticket up) or starts a new worker.
fn next_ticket(inner: &Inner) -> Option<(Arc<Ticket>, usize)> {
    let mut st = lock(&inner.state);
    match st.queue.pop_front() {
        Some(ticket) => {
            st.presented = Some(ticket.token.clone());
            let queued = st.queue.len();
            Some((ticket, queued))
        }
        None => {
            st.worker_running = false;
            None
        }
    }
}
