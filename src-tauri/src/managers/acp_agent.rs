//! Persistent ACP (Agent Client Protocol) client.
//!
//! Implements DESIGN.md rev 3.1 and is specified by the tests in `acp_agent_tests.rs`.
//! Signatures, constants and type shapes here are part of the contract: do not
//! change them without updating the design and the tests.

use std::collections::HashMap;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::thread;
use std::time::{Duration, Instant};

use crate::managers::acp_permission::{
    decide_auto, outcome_for, outcome_json, parse_permission_request, AutoDecision, Outcome,
    PermissionMode, PermissionRequest, UserChoice,
};
use crate::managers::acp_permission_broker::{
    CommitFn, PermissionBroker, PermissionPrompter, RequestBinding, SubmitResult,
};
use crate::managers::acp_run::{
    BeginError, ResponseSink, RunHandle, RunRegistry, SessionSet, SinkError, TurnTiming,
};
use crate::managers::agent_settings_gate::AgentSettingsGate;

#[cfg(unix)]
use std::io::{ErrorKind, Read, Write};
#[cfg(unix)]
use std::os::unix::io::{AsRawFd, RawFd};
#[cfg(unix)]
use std::process::{ChildStderr, ChildStdin, ChildStdout};
#[cfg(unix)]
use std::sync::mpsc::Receiver;

use serde_json::{json, Value};

/// Longest accepted stdout line. Enforced while reading (limit + 1), never allocates beyond it.
pub const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
/// Aggregate chunk text per turn. Over => `Protocol("reply too large")` and the child is retired.
pub const MAX_RAW_REPLY_BYTES: usize = 256 * 1024;
/// Reply cap after sanitising, truncated at a char boundary.
pub const MAX_REPLY_CHARS: usize = 16_000;
pub const STDERR_TAIL_CHARS: usize = 2_000;
pub const MAX_ERR_DETAIL_CHARS: usize = 200;
/// Helper-thread poll interval.
pub const TICK: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq)]
pub struct AcpAgentConfig {
    pub command: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub turn_timeout: Duration,
    pub handshake_timeout: Duration,
    pub cancel_grace: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptOptions {
    /// true: reuse one ACP session across turns. false: a fresh session per turn.
    pub reuse_session: bool,
    /// Settings generation the launch was prepared under (`AgentSettingsGate`). `None` means
    /// the caller is not gated. `Some(e)` is admitted only while the gate is stable at `e`.
    pub expected_epoch: Option<u64>,
}

/// Admission rule for a prepared launch (A3-R3). `None` is never refused. `Some(expected)`
/// is admitted only when `expected` is even and equals the current generation, so a launch
/// from before a settings change, or one prepared during an update, is refused.
pub(crate) fn admission_allowed(expected: Option<u64>, current: u64) -> bool {
    match expected {
        None => true,
        Some(expected) => expected % 2 == 0 && expected == current,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpReply {
    pub text: String,
    pub agent_name: Option<String>,
}

/// Permission policy read fresh for each agent request.
#[derive(Debug, Clone)]
pub struct PermissionSettings {
    /// How requests are decided: auto-allow reads, ask, or deny.
    pub mode: PermissionMode,
    /// How long one approval may wait for the user before it is rejected.
    pub approval_timeout: Duration,
}

/// Timing of the active run, keyed by its run token. Read by the broker's commit callback
/// so that an approval wait ends when its response is committed.
type RunTimingSlot = Mutex<Option<(u64, Arc<Mutex<TurnTiming>>)>>;

/// Broker and settings source. Present only after `with_permissions`.
pub(crate) struct PermissionWiring {
    broker: Arc<PermissionBroker>,
    settings: Arc<dyn Fn() -> PermissionSettings + Send + Sync>,
}

/// What one connection needs to answer agent requests and to commit responses.
pub(crate) struct ConnLink {
    conn_gen: u64,
    registry: Arc<RunRegistry>,
    permissions: Option<Arc<PermissionWiring>>,
    force_failure: Arc<AtomicBool>,
}

/// Non-blocking writer sink registered with the run registry for one connection.
struct ConnSink {
    outbound: SyncSender<String>,
    #[cfg_attr(not(test), allow(dead_code))]
    force_failure: Arc<AtomicBool>,
}

impl ResponseSink for ConnSink {
    fn try_send_line(&self, line: String) -> Result<(), SinkError> {
        #[cfg(test)]
        if self.force_failure.load(Ordering::SeqCst) {
            return Err(SinkError::Full);
        }
        match self.outbound.try_send(line) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(SinkError::Full),
            Err(TrySendError::Disconnected(_)) => Err(SinkError::Disconnected),
        }
    }
}

/// Permission context of the turn running on a connection. The reader uses it to answer
/// agent requests for exactly this run and session.
#[derive(Clone)]
pub(crate) struct PermTurn {
    handle: RunHandle,
    session: String,
    timing: Arc<Mutex<TurnTiming>>,
}

/// The run a prompt executes under: its handle and its shared turn timing.
pub(crate) struct TurnRun {
    handle: RunHandle,
    timing: Arc<Mutex<TurnTiming>>,
}

/// Ends a run on every exit path: clears its timing slot, finishes the registry run and
/// invalidates its prompts. Dropped before the manager reservation is released.
struct RunGuard {
    inner: Arc<Inner>,
    handle: RunHandle,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        {
            let mut slot = lock(&self.inner.run_timing);
            if slot
                .as_ref()
                .is_some_and(|(t, _)| *t == self.handle.token())
            {
                *slot = None;
            }
        }
        self.inner.registry.finish(&self.handle);
        let broker = lock(&self.inner.permissions)
            .as_ref()
            .map(|w| Arc::clone(&w.broker));
        if let Some(broker) = broker {
            broker.invalidate_run(self.handle.token());
        }
    }
}

/// Builds the broker's commit callback. It runs on the broker worker with no broker lock
/// held. The response is committed first and the approval wait ends after, so the turn
/// cannot time out between the two.
fn build_commit(registry: Arc<RunRegistry>, run_timing: Arc<RunTimingSlot>) -> CommitFn {
    Arc::new(move |binding: RequestBinding, outcome: Outcome| {
        let line = response_ok_json(&binding.rpc_id, outcome_json(&outcome));
        let _ = registry.commit(
            binding.conn_gen,
            binding.run_token,
            &binding.session_id,
            line,
        );
        let timing = lock(&run_timing)
            .as_ref()
            .filter(|(t, _)| *t == binding.run_token)
            .map(|(_, timing)| Arc::clone(timing));
        if let Some(timing) = timing {
            lock(&timing).exit_approval(Instant::now());
        }
    })
}

#[derive(Debug)]
pub enum AcpAgentError {
    NotConfigured,
    CommandNotFound,
    // Why: only the ACP tests construct this today.
    #[cfg_attr(not(test), allow(dead_code))]
    Unsupported,
    SpawnFailed(String),
    HandshakeFailed(String),
    Timeout,
    Busy,
    Stale,
    Closed,
    AgentExited {
        code: Option<i32>,
        stderr: String,
    },
    EmptyReply,
    Refused,
    /// The agent settings changed (or are changing) after the launch was prepared. Nothing
    /// was spawned.
    SettingsChanged,
    /// The caller cancelled the turn. The child stays usable.
    Cancelled,
    /// A permission response could not be queued for the agent. The turn was stopped.
    PermissionDelivery,
    Protocol(String),
    // Why: only the ACP tests construct this today.
    #[cfg_attr(not(test), allow(dead_code))]
    Io(std::io::Error),
}

/// One short, user-safe line per error. Agent-provided text is always passed through
/// `cap_detail` so a hostile agent cannot flood or spoof the message.
impl std::fmt::Display for AcpAgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured => write!(f, "ACP agent is not configured"),
            Self::CommandNotFound => write!(f, "ACP agent command was not found"),
            Self::Unsupported => write!(f, "ACP agent is not supported on this platform"),
            Self::SpawnFailed(msg) => write!(f, "ACP agent could not start: {}", cap_detail(msg)),
            Self::HandshakeFailed(msg) => {
                write!(f, "ACP agent handshake failed: {}", cap_detail(msg))
            }
            Self::Timeout => write!(f, "ACP agent timed out"),
            Self::Busy => write!(f, "ACP agent is busy"),
            Self::Stale => write!(f, "ACP agent request was superseded"),
            Self::Closed => write!(f, "ACP agent is closed"),
            Self::AgentExited { code, stderr } => {
                match code {
                    Some(c) => write!(f, "ACP agent exited (code {c})")?,
                    None => write!(f, "ACP agent exited")?,
                }
                let tail = cap_detail(stderr);
                if !tail.is_empty() {
                    write!(f, ": {tail}")?;
                }
                Ok(())
            }
            Self::EmptyReply => write!(f, "ACP agent returned an empty reply"),
            Self::Refused => write!(f, "ACP agent refused the request"),
            Self::SettingsChanged => write!(f, "ACP agent settings changed"),
            Self::Cancelled => write!(f, "ACP agent request was cancelled"),
            Self::PermissionDelivery => {
                write!(f, "ACP permission response could not be delivered")
            }
            Self::Protocol(msg) => write!(f, "ACP protocol error: {}", cap_detail(msg)),
            Self::Io(e) => write!(f, "ACP agent I/O error: {}", cap_detail(&e.to_string())),
        }
    }
}

impl std::error::Error for AcpAgentError {}

/// Session of the current conversation on the current child. `epoch` is the manager epoch
/// when the session was created; a `reset_session` bumps the epoch and drops it.
#[derive(Debug, Clone)]
pub(crate) struct SessionInfo {
    pub(crate) id: String,
    pub(crate) epoch: u64,
}

/// The single operation allowed to hold the manager (DESIGN 5). `token` identifies the
/// reservation; a release or publish only acts when the token still matches.
#[derive(Debug, Clone)]
pub(crate) enum Op {
    Idle,
    Warming { token: u64, cfg: AcpAgentConfig },
    Turn { token: u64 },
}

impl Op {
    pub(crate) fn token(&self) -> Option<u64> {
        match self {
            Self::Idle => None,
            Self::Warming { token, .. } | Self::Turn { token } => Some(*token),
        }
    }
}

/// What a caller reserves the manager for. The config comes in as a separate argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reservation {
    Turn,
    Warm,
}

/// Everything guarded by `Inner::m`. Never held across spawns, I/O, waits or kills.
pub(crate) struct State {
    pub(crate) closed: bool,
    pub(crate) conn: Option<Arc<Conn>>,
    pub(crate) cfg: Option<AcpAgentConfig>,
    pub(crate) session: Option<SessionInfo>,
    pub(crate) epoch: u64,
    pub(crate) op: Op,
    pub(crate) last_activity: Instant,
    pub(crate) agent_name: Option<String>,
}

impl State {
    fn new() -> Self {
        Self {
            closed: false,
            conn: None,
            cfg: None,
            session: None,
            epoch: 0,
            op: Op::Idle,
            last_activity: Instant::now(),
            agent_name: None,
        }
    }
}

/// Shared manager state. The reaper and helper threads hold only `Weak` or `Conn` handles,
/// so `Arc::strong_count` of this value stays 1 for a lone manager (DESIGN 4).
pub(crate) struct Inner {
    pub(crate) m: Mutex<State>,
    pub(crate) cv: Condvar,
    pub(crate) next_token: AtomicU64,
    pub(crate) helpers: Arc<AtomicUsize>,
    /// Held by the reaper while it upgrades its `Weak` and inspects the state, and by the
    /// test accessor while it reads the strong count. Why: without it a reaper tick could
    /// make the count read 2 for an instant even though no handle leaked.
    pub(crate) reaper_gate: Arc<Mutex<()>>,
    /// Run registry: connection generation, writer sink and the single active run.
    pub(crate) registry: Arc<RunRegistry>,
    /// Broker and settings source. Set once by `with_permissions`.
    pub(crate) permissions: Mutex<Option<Arc<PermissionWiring>>>,
    /// Timing of the active run, read by the broker's commit callback.
    pub(crate) run_timing: Arc<RunTimingSlot>,
    /// Source of connection generations. Each spawned child takes the next one.
    pub(crate) next_conn_gen: AtomicU64,
    /// Test hook: makes the writer refuse registry-routed lines.
    pub(crate) force_failure: Arc<AtomicBool>,
    /// Settings generation gate. Admission of gated launches is checked under `m`.
    pub(crate) gate: Arc<AgentSettingsGate>,
}

/// One long-lived ACP child process behind a small state machine. `Send + Sync`.
pub struct AcpAgentManager {
    inner: Arc<Inner>,
}

impl Default for AcpAgentManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AcpAgentManager {
    /// Idle shutdown of 30 minutes.
    pub fn new() -> Self {
        Self::with_idle_shutdown(Duration::from_secs(30 * 60))
    }

    /// Like `new`, but gated launches are admitted against `gate`, which the settings writers
    /// share with this manager.
    pub fn new_with_gate(gate: Arc<AgentSettingsGate>) -> Self {
        Self::with_idle_shutdown_and_gate(Duration::from_secs(30 * 60), gate)
    }

    /// Starts the manager and its single reaper thread (DESIGN 5.7). The reaper holds only
    /// a `Weak`, so it exits as soon as the last handle is dropped.
    pub fn with_idle_shutdown(idle: Duration) -> Self {
        Self::with_idle_shutdown_and_gate(idle, Arc::new(AgentSettingsGate::new()))
    }

    fn with_idle_shutdown_and_gate(idle: Duration, gate: Arc<AgentSettingsGate>) -> Self {
        let inner = Arc::new(Inner {
            gate,
            m: Mutex::new(State::new()),
            cv: Condvar::new(),
            next_token: AtomicU64::new(1),
            helpers: Arc::new(AtomicUsize::new(0)),
            reaper_gate: Arc::new(Mutex::new(())),
            registry: RunRegistry::new(),
            permissions: Mutex::new(None),
            run_timing: Arc::new(Mutex::new(None)),
            next_conn_gen: AtomicU64::new(1),
            force_failure: Arc::new(AtomicBool::new(false)),
        });
        let weak = Arc::downgrade(&inner);
        let gate = Arc::clone(&inner.reaper_gate);
        // Why ignore the error: without a reaper the child is still retired on drop, close
        // or shutdown; only the idle timeout is lost.
        let _ = thread::Builder::new()
            .name("acp-reaper".to_owned())
            .spawn(move || reaper_loop(weak, idle, gate));
        Self { inner }
    }

    /// Installs the permission broker. Agent permission requests are then decided by
    /// `settings` (read for each request) and approvals are shown through `prompter`.
    pub fn with_permissions(
        self,
        prompter: Arc<dyn PermissionPrompter>,
        settings: Arc<dyn Fn() -> PermissionSettings + Send + Sync>,
    ) -> Self {
        let commit = build_commit(
            Arc::clone(&self.inner.registry),
            Arc::clone(&self.inner.run_timing),
        );
        let broker = PermissionBroker::new(prompter, commit);
        *lock(&self.inner.permissions) = Some(Arc::new(PermissionWiring { broker, settings }));
        self
    }

    /// The permission broker, when permissions are configured.
    pub fn permission_broker(&self) -> Option<Arc<PermissionBroker>> {
        lock(&self.inner.permissions)
            .as_ref()
            .map(|w| Arc::clone(&w.broker))
    }

    /// Mints the handle for one prompt. Pass it to `prompt_with_handle` and `cancel`.
    pub fn new_run_handle(&self) -> RunHandle {
        self.inner.registry.new_handle()
    }

    /// Cancels the run `handle`. Returns true only for the call that latched the cancel.
    /// A stale, finished, or never-begun handle returns false. Pending approvals of the
    /// run are invalidated, so none can be granted after this returns.
    pub fn cancel(&self, handle: &RunHandle) -> bool {
        let cancelled = self.inner.registry.cancel(handle, &|session_id| {
            notification_json("session/cancel", cancel_params(session_id))
        });
        if let Some(broker) = self.permission_broker() {
            broker.invalidate_run(handle.token());
        }
        cancelled
    }

    /// Test hook: while on, the writer refuses every registry-routed line, as if full.
    #[cfg(test)]
    pub(crate) fn force_permission_delivery_failure(&self, on: bool) {
        self.inner.force_failure.store(on, Ordering::SeqCst);
    }

    /// Blocking. See DESIGN.md sections 4-6.
    pub fn prompt(
        &self,
        cfg: &AcpAgentConfig,
        text: &str,
        opts: PromptOptions,
    ) -> Result<AcpReply, AcpAgentError> {
        let handle = self.new_run_handle();
        self.prompt_with_handle(cfg, text, opts, &handle)
    }

    /// Like `prompt`, but the run is identified by `handle`, so `cancel` can stop it. A
    /// handle begins at most one run.
    pub fn prompt_with_handle(
        &self,
        cfg: &AcpAgentConfig,
        text: &str,
        opts: PromptOptions,
        handle: &RunHandle,
    ) -> Result<AcpReply, AcpAgentError> {
        // Why before reserve: a blank prompt must never start, reuse or disturb the child.
        if text.trim().is_empty() {
            return Err(AcpAgentError::Protocol("blank prompt".to_owned()));
        }
        check_config(cfg)?;
        let started = Instant::now();
        let turn_deadline = started + cfg.turn_timeout;
        let handshake_deadline = Instant::now() + cfg.handshake_timeout;
        let (token, epoch, stale) = reserve(
            &self.inner,
            Reservation::Turn,
            cfg,
            handshake_deadline,
            opts.expected_epoch,
        )?;
        // Created first so that every later path, including a panic, releases the op.
        let _reservation = ReservationGuard::new(&self.inner, token);
        if let Some(old) = stale {
            retire(&old, RetireReason::Replaced);
        }
        // Why after reserve: a caller that is Busy never begins, so it can never cancel.
        // A cancelled-before-begin handle is refused here, before any connect or prompt.
        self.inner.registry.begin(handle).map_err(|e| match e {
            BeginError::Busy | BeginError::Consumed => AcpAgentError::Busy,
            BeginError::Cancelled => AcpAgentError::Cancelled,
        })?;
        let timing = Arc::new(Mutex::new(TurnTiming::new(started, cfg.turn_timeout)));
        *lock(&self.inner.run_timing) = Some((handle.token(), Arc::clone(&timing)));
        // Why after the reservation guard: the run finishes before the op is released.
        let _run = RunGuard {
            inner: Arc::clone(&self.inner),
            handle: *handle,
        };
        let run = TurnRun {
            handle: *handle,
            timing,
        };
        let cancelled = || self.inner.registry.is_cancelled(handle);
        let conn = connect(&self.inner, cfg, token, handshake_deadline, &cancelled)?;
        let ticket = TurnTicket {
            token,
            epoch,
            turn_deadline,
            handshake_deadline,
        };
        let outcome = run_turn(&self.inner, &conn, cfg, text, opts, ticket, &run);
        // Why: a retired child must not stay the stored connection; the next call respawns.
        if conn.is_retired() {
            forget_conn(&self.inner, &conn);
        }
        let mut reply = outcome?;
        reply.agent_name = lock_state(&self.inner).agent_name.clone();
        Ok(reply)
    }

    /// Spawn + initialize only. Returns the agent name when the agent reports one.
    pub fn warm_up(&self, cfg: &AcpAgentConfig) -> Result<Option<String>, AcpAgentError> {
        check_config(cfg)?;
        let deadline = Instant::now() + cfg.handshake_timeout;
        let (token, _epoch, stale) = reserve(&self.inner, Reservation::Warm, cfg, deadline, None)?;
        // Created first so that every later path, including a panic, releases the op.
        let _guard = ReservationGuard::new(&self.inner, token);
        if let Some(old) = stale {
            retire(&old, RetireReason::Replaced);
        }
        connect(&self.inner, cfg, token, deadline, &|| false)?;
        let name = lock_state(&self.inner).agent_name.clone();
        Ok(name)
    }

    /// Drops the conversation (DESIGN 5.5): the epoch moves on, so an in-flight turn returns
    /// `Stale`; the agent's session is closed best-effort, outside the lock.
    pub fn reset_session(&self) {
        let (conn, session) = {
            let mut s = lock_state(&self.inner);
            s.epoch = s.epoch.wrapping_add(1);
            let session = s.session.take();
            (s.conn.clone(), session)
        };
        if let (Some(conn), Some(session)) = (conn, session) {
            conn.try_notify("session/close", close_params(&session.id));
        }
    }

    // Why: only the ACP tests call this today.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_running(&self) -> bool {
        lock_state(&self.inner)
            .conn
            .as_ref()
            .is_some_and(|c| !c.is_retired())
    }

    /// Retires the current child; the manager stays usable.
    pub fn shutdown(&self) {
        stop(&self.inner, false);
    }

    /// Irreversible (app exit): later prompt/warm_up return `Closed`; nothing can spawn afterwards.
    pub fn close(&self) {
        stop(&self.inner, true);
    }

    /// Live helper threads (reader, writer, stderr) of this manager's children.
    #[cfg(test)]
    pub fn helper_threads_alive(&self) -> usize {
        self.inner.helpers.load(Ordering::SeqCst)
    }

    /// Strong count of the shared inner state. Must stay 1: the reaper and helper
    /// threads hold only weak references.
    #[cfg(test)]
    pub fn inner_strong_count(&self) -> usize {
        // Why the gate: the reaper only holds an upgraded `Arc` while it holds this lock,
        // so the count is read between reaper ticks, never mid-upgrade.
        let _gate = lock(&self.inner.reaper_gate);
        Arc::strong_count(&self.inner)
    }
}

impl Drop for AcpAgentManager {
    /// Dropping the last handle behaves like `close()`: the child is retired and the
    /// reaper exits on its next tick.
    fn drop(&mut self) {
        // Why unconditional: this handle is the only strong owner of the operations (the
        // reaper and helpers hold Weak or Conn only). A count check could miss the child
        // while the reaper is mid-tick and let it outlive the manager.
        stop(&self.inner, true);
    }
}

// ---------------------------------------------------------------- lifecycle (S1b-3a)

/// Locks the manager state, ignoring poisoning (a panic elsewhere must not panic callers).
fn lock_state(inner: &Inner) -> MutexGuard<'_, State> {
    lock(&inner.m)
}

/// Rejects configurations that can never work, before any reservation (DESIGN 4).
fn check_config(cfg: &AcpAgentConfig) -> Result<(), AcpAgentError> {
    if cfg.command.as_os_str().to_string_lossy().trim().is_empty() {
        return Err(AcpAgentError::NotConfigured);
    }
    Ok(())
}

/// Reserves the manager for one operation (DESIGN 5 rule 1). Returns the reservation token
/// and the connection taken out of State (the caller retires it after unlocking). Waits are
/// bounded by `deadline`; after every wake the decision restarts from the top.
fn reserve(
    inner: &Arc<Inner>,
    kind: Reservation,
    cfg: &AcpAgentConfig,
    deadline: Instant,
    expected_epoch: Option<u64>,
) -> Result<(u64, u64, Option<Arc<Conn>>), AcpAgentError> {
    loop {
        let mut s = lock_state(inner);
        if s.closed {
            return Err(AcpAgentError::Closed);
        }
        // Why here, under the manager lock and before any spawn: a launch prepared under an
        // older settings generation (or during an update) never starts an agent.
        if !admission_allowed(expected_epoch, inner.gate.generation()) {
            return Err(AcpAgentError::SettingsChanged);
        }
        let warming = match &s.op {
            Op::Turn { .. } => return Err(AcpAgentError::Busy),
            Op::Warming { cfg: warming, .. } => Some(warming.clone()),
            Op::Idle => None,
        };
        if let Some(warming) = warming {
            // Why join: a prompt waits for the warm-up it overlaps; a warm-up joins only the
            // same config, since a different one would replace the child under it.
            let joinable = match kind {
                Reservation::Turn => true,
                Reservation::Warm => warming == *cfg,
            };
            if !joinable || Instant::now() >= deadline {
                return Err(AcpAgentError::Busy);
            }
            let _woken = wait_deadline(&inner.cv, s, deadline, |st| {
                st.closed || !matches!(st.op, Op::Warming { .. })
            });
            continue;
        }
        let token = inner.next_token.fetch_add(1, Ordering::SeqCst);
        s.op = match kind {
            Reservation::Turn => Op::Turn { token },
            Reservation::Warm => Op::Warming {
                token,
                cfg: cfg.clone(),
            },
        };
        // A different config or a dead child invalidates the session and the child.
        let config_changed = s.cfg.as_ref() != Some(cfg);
        if config_changed {
            s.cfg = Some(cfg.clone());
        }
        let dead = s.conn.as_ref().is_some_and(|c| c.is_retired());
        let stale = if config_changed || dead {
            s.session = None;
            s.conn.take()
        } else {
            None
        };
        return Ok((token, s.epoch, stale));
    }
}

/// Ends the reservation `token` if it is still the current one (DESIGN 5 rule 4). A token
/// mismatch means a newer operation owns the manager; leave it alone.
fn release(inner: &Inner, token: u64) {
    let mut s = lock_state(inner);
    if s.op.token() == Some(token) {
        s.op = Op::Idle;
        s.last_activity = Instant::now();
        drop(s);
        inner.cv.notify_all();
    }
}

/// Releases its reservation on drop, so no path (including a panic) leaves `op` stuck.
pub(crate) struct ReservationGuard {
    inner: Arc<Inner>,
    token: u64,
}

impl ReservationGuard {
    pub(crate) fn new(inner: &Arc<Inner>, token: u64) -> Self {
        Self {
            inner: Arc::clone(inner),
            token,
        }
    }
}

impl Drop for ReservationGuard {
    fn drop(&mut self) {
        release(&self.inner, self.token);
    }
}

/// Ends a manager's life or its child (DESIGN 5 rule 6). Invalidates any reservation by
/// setting `op` to Idle, takes the child out under the lock and retires it after unlocking.
fn stop(inner: &Inner, closing: bool) {
    let conn = {
        let mut s = lock_state(inner);
        if closing {
            s.closed = true;
        }
        s.op = Op::Idle;
        s.session = None;
        let conn = s.conn.take();
        drop(s);
        conn
    };
    inner.cv.notify_all();
    if let Some(conn) = conn {
        retire(&conn, RetireReason::Shutdown);
    }
}

/// Installs a freshly spawned, handshaken connection (DESIGN 5 rule 3). Only the current
/// reservation may publish, and never over a live connection; otherwise the connection is
/// handed back so the caller retires it.
fn publish_conn(
    inner: &Inner,
    token: u64,
    conn: Arc<Conn>,
    agent_name: Option<String>,
) -> Result<(), Arc<Conn>> {
    let mut s = lock_state(inner);
    let live = s.conn.as_ref().is_some_and(|c| !c.is_retired());
    if s.closed || s.op.token() != Some(token) || live {
        return Err(conn);
    }
    s.conn = Some(conn);
    s.agent_name = agent_name;
    Ok(())
}

/// Maps a handshake-deadline timeout to `HandshakeFailed`; other errors pass through.
fn handshake_error(err: AcpAgentError) -> AcpAgentError {
    match err {
        AcpAgentError::Timeout => {
            AcpAgentError::HandshakeFailed("no response before the handshake deadline".to_owned())
        }
        other => other,
    }
}

/// `initialize` (DESIGN 6c step 1). Returns the agent name when reported.
fn handshake(
    conn: &Conn,
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<String>, AcpAgentError> {
    let outcome = conn
        .request_until("initialize", initialize_params(), deadline, cancelled)
        .map_err(handshake_error)?;
    let result = match outcome {
        Ok(result) => result,
        Err(err) => return Err(AcpAgentError::HandshakeFailed(error_detail(&err))),
    };
    // Why fail closed: we only speak protocol version 1.
    if result.get("protocolVersion").and_then(Value::as_u64) != Some(1) {
        return Err(AcpAgentError::HandshakeFailed(
            "unsupported protocol version".to_owned(),
        ));
    }
    let name = result
        .pointer("/agentInfo/name")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(name)
}

/// Returns a live connection for this reservation: the stored one when present (no second
/// spawn), otherwise a fresh child that completes `initialize` and is published (rule 3).
/// Every failure retires the new child before returning.
fn connect(
    inner: &Arc<Inner>,
    cfg: &AcpAgentConfig,
    token: u64,
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
) -> Result<Arc<Conn>, AcpAgentError> {
    {
        let s = lock_state(inner);
        if s.closed {
            return Err(AcpAgentError::Closed);
        }
        if s.op.token() != Some(token) {
            return Err(AcpAgentError::Stale);
        }
        if let Some(conn) = s.conn.as_ref().filter(|c| !c.is_retired()) {
            return Ok(Arc::clone(conn));
        }
    }
    // Why check here: a missing folder would otherwise surface as a misleading command error.
    if !cfg.cwd.is_dir() {
        return Err(AcpAgentError::SpawnFailed(
            "workspace folder missing".to_owned(),
        ));
    }
    let link = ConnLink {
        conn_gen: inner.next_conn_gen.fetch_add(1, Ordering::SeqCst),
        registry: Arc::clone(&inner.registry),
        permissions: lock(&inner.permissions).as_ref().map(Arc::clone),
        force_failure: Arc::clone(&inner.force_failure),
    };
    let conn = Conn::spawn(cfg, Arc::clone(&inner.helpers), link)?;
    let name = match handshake(&conn, deadline, cancelled) {
        Ok(name) => name,
        Err(err) => {
            retire(&conn, RetireReason::Protocol("handshake failed".to_owned()));
            return Err(err);
        }
    };
    let handle = Arc::clone(&conn);
    match publish_conn(inner, token, conn, name) {
        Ok(()) => Ok(handle),
        Err(unpublished) => {
            retire(&unpublished, RetireReason::Shutdown);
            let closed = lock_state(inner).closed;
            Err(if closed {
                AcpAgentError::Closed
            } else {
                AcpAgentError::Stale
            })
        }
    }
}

// ---------------------------------------------------------------- turn (S1b-3b)

/// What one turn carries through its helpers: its reservation and its absolute deadlines.
#[derive(Clone, Copy)]
struct TurnTicket {
    token: u64,
    /// Manager epoch at reservation. A `reset_session` moves it, which makes the turn stale.
    epoch: u64,
    /// Covers queue, write and response (DESIGN 6c).
    turn_deadline: Instant,
    /// Covers spawn..session/new (DESIGN 6c).
    handshake_deadline: Instant,
}

fn epoch_moved(inner: &Inner, epoch: u64) -> bool {
    lock_state(inner).epoch != epoch
}

/// Drops the stored connection and its session when they still point at `conn`.
/// Why: a retired child must never be handed to the next reservation as a live one.
fn forget_conn(inner: &Inner, conn: &Arc<Conn>) {
    let mut s = lock_state(inner);
    if s.conn.as_ref().is_some_and(|c| Arc::ptr_eq(c, conn)) {
        s.conn = None;
        s.session = None;
    }
}

/// Returns the session for this turn: the stored one when reusable, otherwise a fresh
/// `session/new`. A stored session is kept only when `reuse_session` is set (DESIGN 3b).
fn ensure_session(
    inner: &Inner,
    conn: &Arc<Conn>,
    cfg: &AcpAgentConfig,
    opts: PromptOptions,
    ticket: TurnTicket,
    run: &TurnRun,
) -> Result<String, AcpAgentError> {
    if opts.reuse_session {
        let s = lock_state(inner);
        let same_conn = s.conn.as_ref().is_some_and(|c| Arc::ptr_eq(c, conn));
        if let Some(session) = s
            .session
            .as_ref()
            .filter(|x| same_conn && x.epoch == ticket.epoch)
        {
            return Ok(session.id.clone());
        }
    }
    let cancelled = || inner.registry.is_cancelled(&run.handle);
    let outcome = conn.request_until(
        "session/new",
        session_new_params(&cfg.cwd),
        ticket.handshake_deadline,
        &cancelled,
    );
    let result = match outcome {
        Ok(Ok(result)) => result,
        Ok(Err(err)) => {
            retire(
                conn,
                RetireReason::Protocol("session/new failed".to_owned()),
            );
            return Err(AcpAgentError::HandshakeFailed(error_detail(&err)));
        }
        Err(err) => {
            retire(
                conn,
                RetireReason::Protocol("session/new failed".to_owned()),
            );
            return Err(handshake_error(err));
        }
    };
    let id = match result.get("sessionId").and_then(Value::as_str) {
        Some(id) if !id.trim().is_empty() => id.to_owned(),
        _ => {
            retire(
                conn,
                RetireReason::Protocol("session/new without id".to_owned()),
            );
            return Err(AcpAgentError::HandshakeFailed(
                "session/new returned no session id".to_owned(),
            ));
        }
    };
    if opts.reuse_session {
        // Why the checks: a reset, a shutdown or a newer reservation must never receive a
        // session that belongs to a conversation it did not start (DESIGN 5 rule 3).
        let mut s = lock_state(inner);
        let owned = !s.closed
            && s.op.token() == Some(ticket.token)
            && s.epoch == ticket.epoch
            && s.conn.as_ref().is_some_and(|c| Arc::ptr_eq(c, conn));
        if owned {
            s.session = Some(SessionInfo {
                id: id.clone(),
                epoch: ticket.epoch,
            });
        }
    }
    Ok(id)
}

/// Maps the prompt response to the reply (DESIGN 6c). `raw` is the accumulated text of
/// this turn only; it is sanitised and capped here.
fn map_turn_result(
    result: &Value,
    raw: &str,
    run_cancelled: bool,
) -> Result<AcpReply, AcpAgentError> {
    let text = sanitize_reply(raw);
    let reply = |text: String| AcpReply {
        text,
        agent_name: None,
    };
    let mut stop = map_stop_reason(result.get("stopReason").and_then(Value::as_str));
    // Why: an agent may echo "cancelled" on a turn that was never cancelled, for example after
    // a stale cancel from an earlier turn on the same child. That turn is a normal reply.
    if stop == StopOutcome::Cancelled && !run_cancelled {
        stop = StopOutcome::Reply;
    }
    match stop {
        StopOutcome::Reply if text.is_empty() => Err(AcpAgentError::EmptyReply),
        StopOutcome::Reply => Ok(reply(text)),
        StopOutcome::ReplyIfText if text.is_empty() => Err(AcpAgentError::Protocol(
            "max_turn_requests without text".to_owned(),
        )),
        StopOutcome::ReplyIfText => Ok(reply(text)),
        StopOutcome::Refused => Err(AcpAgentError::Refused),
        // Why: the agent confirmed our cancel. Its child is healthy, so it is kept.
        StopOutcome::Cancelled => Err(AcpAgentError::Cancelled),
        StopOutcome::Unknown => Err(AcpAgentError::Protocol("unknown stopReason".to_owned())),
    }
}

/// Timeout path (DESIGN 5 rule 8): cancel best-effort, give the agent `grace` to answer,
/// then ALWAYS retire the child so the next call starts clean. The caller has already
/// discarded the turn text.
fn abandon_turn(conn: &Arc<Conn>, session_id: &str, rx: Option<&ResponseRx>, grace: Duration) {
    conn.try_notify("session/cancel", cancel_params(session_id));
    if let Some(rx) = rx {
        let _ = conn.await_response(rx, Instant::now() + grace);
    }
    retire(conn, RetireReason::Timeout);
}

/// How the wait for a turn's response ended.
enum TurnWait {
    Response(Result<Value, Value>),
    /// The turn deadline passed with no approval in flight.
    Timeout,
    /// The run was cancelled and the agent did not answer within the grace period.
    Cancelled,
    /// The writer refused a permission response.
    PermissionDelivery,
}

/// True once the turn has run out of time at `now`. While an approval is pending, the
/// deadline is only trusted at the hard cap plus the grace period: the approval itself
/// ends at that cap, so the turn waits for its commit instead of racing it.
fn turn_expired(timing: &Mutex<TurnTiming>, now: Instant, grace: Duration) -> bool {
    let t = lock(timing);
    if t.pending() == 0 {
        return t.expired(now);
    }
    match t.hard_deadline().checked_add(grace) {
        Some(limit) => now >= limit,
        None => true,
    }
}

/// Maps one channel message to the wait result.
fn wait_from(conn: &Conn, msg: Result<Value, ConnFail>) -> Result<TurnWait, AcpAgentError> {
    match msg {
        Ok(result) => Ok(TurnWait::Response(Ok(result))),
        Err(ConnFail::Rpc(err)) => Ok(TurnWait::Response(Err(err))),
        Err(ConnFail::Retired) => Err(conn.retire_error()),
    }
}

/// Waits for the prompt's response in slices of at most `TICK`. A response already queued
/// wins over the deadline, because the channel is polled before the clock is read. The
/// deadline is re-read on every slice, so approval pauses extend it. Cancel and delivery
/// failure are noticed within one slice. Holds no lock while it waits.
fn await_turn_response(
    inner: &Inner,
    conn: &Conn,
    rx: &ResponseRx,
    run: &TurnRun,
    grace: Duration,
) -> Result<TurnWait, AcpAgentError> {
    let mut cancelled_at: Option<Instant> = None;
    loop {
        match rx.recv_timeout(TICK) {
            Ok(msg) => return wait_from(conn, msg),
            Err(RecvTimeoutError::Disconnected) => return Err(conn.retire_error()),
            Err(RecvTimeoutError::Timeout) => {}
        }
        if inner.registry.delivery_failed(&run.handle) {
            return Ok(TurnWait::PermissionDelivery);
        }
        let now = Instant::now();
        if inner.registry.is_cancelled(&run.handle) {
            // Why: a cancel ignores the turn deadline and allows only the grace period.
            let since = *cancelled_at.get_or_insert(now);
            if now.saturating_duration_since(since) >= grace {
                return Ok(TurnWait::Cancelled);
            }
        } else if turn_expired(&run.timing, now, grace) {
            return match rx.try_recv() {
                Ok(msg) => wait_from(conn, msg),
                Err(TryRecvError::Disconnected) => Err(conn.retire_error()),
                Err(TryRecvError::Empty) => Ok(TurnWait::Timeout),
            };
        }
    }
}

/// The turn itself on a live connection: session, prompt, reply mapping, stale check.
/// Every path ends the turn's accumulation and closes a turn-local or stale session.
fn run_turn(
    inner: &Inner,
    conn: &Arc<Conn>,
    cfg: &AcpAgentConfig,
    text: &str,
    opts: PromptOptions,
    ticket: TurnTicket,
    run: &TurnRun,
) -> Result<AcpReply, AcpAgentError> {
    let session_id = ensure_session(inner, conn, cfg, opts, ticket, run)?;
    // Why: the registry cancels only sessions it knows. A cancel that latched before the
    // session was recorded is answered here, and session/prompt is never sent.
    match inner.registry.set_session(&run.handle, &session_id) {
        SessionSet::Ok => {}
        SessionSet::Cancelled => {
            conn.try_notify("session/cancel", cancel_params(&session_id));
            return Err(AcpAgentError::Cancelled);
        }
        SessionSet::Stale => return Err(AcpAgentError::Stale),
    }
    if epoch_moved(inner, ticket.epoch) {
        conn.try_notify("session/close", close_params(&session_id));
        return Err(AcpAgentError::Stale);
    }
    if inner.registry.is_cancelled(&run.handle) {
        return Err(AcpAgentError::Cancelled);
    }
    conn.begin_turn(&session_id);
    conn.attach_turn(PermTurn {
        handle: run.handle,
        session: session_id.clone(),
        timing: Arc::clone(&run.timing),
    });
    let started = conn.start_request(
        "session/prompt",
        prompt_params(&session_id, text),
        ticket.turn_deadline,
    );
    let rx = match started {
        Ok((_id, rx)) => rx,
        Err(AcpAgentError::Timeout) => {
            conn.end_turn();
            abandon_turn(conn, &session_id, None, cfg.cancel_grace);
            return Err(AcpAgentError::Timeout);
        }
        Err(other) => {
            conn.end_turn();
            return Err(other);
        }
    };
    let waited = await_turn_response(inner, conn, &rx, run, cfg.cancel_grace);
    // Why before the match: the text is final now. Updates that arrive later (during a
    // cancel, or after the response) must not reach this turn.
    let raw = conn.end_turn();
    let outcome = match waited? {
        TurnWait::Timeout => {
            abandon_turn(conn, &session_id, Some(&rx), cfg.cancel_grace);
            return Err(AcpAgentError::Timeout);
        }
        TurnWait::Cancelled => {
            retire(conn, RetireReason::Timeout);
            return Err(AcpAgentError::Cancelled);
        }
        TurnWait::PermissionDelivery => {
            abandon_turn(conn, &session_id, None, cfg.cancel_grace);
            return Err(AcpAgentError::PermissionDelivery);
        }
        TurnWait::Response(Err(err)) => Err(AcpAgentError::Protocol(error_detail(&err))),
        TurnWait::Response(Ok(result)) => {
            // Why: the deadline wins even when the response arrives just after it.
            let cancelled = inner.registry.is_cancelled(&run.handle);
            let late = !cancelled && turn_expired(&run.timing, Instant::now(), cfg.cancel_grace);
            if late {
                abandon_turn(conn, &session_id, None, cfg.cancel_grace);
                return Err(AcpAgentError::Timeout);
            }
            map_turn_result(&result, &raw, cancelled)
        }
    };
    let stale = epoch_moved(inner, ticket.epoch);
    if stale || !opts.reuse_session {
        conn.try_notify("session/close", close_params(&session_id));
    }
    if stale {
        // Why: a reset during the turn makes its reply belong to a dropped conversation.
        return Err(AcpAgentError::Stale);
    }
    outcome
}

/// What the reaper decided for one tick (DESIGN 5 rule 7).
enum ReapStep {
    /// The manager is gone or closed: the reaper must exit.
    Gone,
    Keep,
    /// Retire this idle child after the state lock is released.
    Retire(Arc<Conn>),
}

/// One reaper decision. The `Arc<Inner>` is dropped before returning, so the caller never
/// sleeps or retires while holding a strong reference.
fn reap_once(weak: &Weak<Inner>, idle: Duration) -> ReapStep {
    let Some(inner) = weak.upgrade() else {
        return ReapStep::Gone;
    };
    let mut s = lock_state(&inner);
    if s.closed {
        return ReapStep::Gone;
    }
    let is_idle = matches!(s.op, Op::Idle) && s.conn.is_some() && s.last_activity.elapsed() >= idle;
    if !is_idle {
        return ReapStep::Keep;
    }
    s.session = None;
    match s.conn.take() {
        Some(conn) => ReapStep::Retire(conn),
        None => ReapStep::Keep,
    }
}

/// The single reaper thread of a manager (DESIGN 5 rule 7). It wakes at least every 200 ms
/// so a dropped or closed manager is noticed quickly.
fn reaper_loop(weak: Weak<Inner>, idle: Duration, gate: Arc<Mutex<()>>) {
    let pause = (idle / 4)
        .min(Duration::from_millis(200))
        .max(Duration::from_millis(5));
    loop {
        // Why the gate: keeps the transient strong `Arc` of `reap_once` invisible to
        // `inner_strong_count`, which reads the count under the same lock.
        let step = {
            let _gate = lock(&gate);
            reap_once(&weak, idle)
        };
        match step {
            ReapStep::Gone => return,
            ReapStep::Retire(conn) => retire(&conn, RetireReason::Idle),
            ReapStep::Keep => {}
        }
        thread::sleep(pause);
    }
}

/// Strips ESC/CSI/OSC sequences and control characters except `\n` and `\t`, trims,
/// and caps the result at `MAX_REPLY_CHARS` characters (char boundary).
pub fn sanitize_reply(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(MAX_REPLY_CHARS * 4));
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Why: terminal escape sequences would move the cursor or change terminal state
            // when the reply is shown or pasted.
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    // CSI: parameter and intermediate bytes, then one final byte in @..=~.
                    for n in chars.by_ref() {
                        if ('@'..='~').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    // OSC: ends at BEL or at ESC followed by a backslash (string terminator).
                    let mut prev_esc = false;
                    for n in chars.by_ref() {
                        if n == '\u{7}' || (prev_esc && n == '\\') {
                            break;
                        }
                        prev_esc = n == '\u{1b}';
                    }
                }
                // Other two-character ESC pairs: drop the introducer and the next char.
                Some(next) if next != '\u{1b}' => {
                    chars.next();
                }
                _ => {}
            }
            continue;
        }
        if c == '\n' || c == '\t' || !c.is_control() {
            out.push(c);
        }
    }
    out.trim().chars().take(MAX_REPLY_CHARS).collect()
}

/// Makes agent-provided text safe for one error line: controls removed, whitespace
/// collapsed, capped. Why: agent text is untrusted and must not break the log or UI line.
pub(crate) fn cap_detail(s: &str) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect();
    let collapsed = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .chars()
        .take(MAX_ERR_DETAIL_CHARS)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Outcome of one bounded line read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LineRead {
    Line,
    Eof,
    TooLong,
}

/// Reads one '\n'-terminated line into `buf`, never storing more than `max + 1` bytes.
/// Why: an agent that never sends a newline must not grow our memory without bound.
pub(crate) fn read_bounded_line<R: BufRead>(
    r: &mut R,
    buf: &mut Vec<u8>,
    max: usize,
) -> std::io::Result<LineRead> {
    buf.clear();
    loop {
        let (consumed, found) = {
            let available = r.fill_buf()?;
            if available.is_empty() {
                return Ok(LineRead::Eof);
            }
            let room = (max + 1).saturating_sub(buf.len());
            let window = available
                .get(..available.len().min(room))
                .unwrap_or_default();
            match window.iter().position(|&b| b == b'\n') {
                Some(i) => {
                    buf.extend_from_slice(window.get(..=i).unwrap_or_default());
                    (i + 1, true)
                }
                None => {
                    buf.extend_from_slice(window);
                    (window.len(), false)
                }
            }
        };
        r.consume(consumed);
        if found {
            return Ok(LineRead::Line);
        }
        if buf.len() > max {
            return Ok(LineRead::TooLong);
        }
    }
}

/// One classified message from the agent.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Incoming {
    /// Agent-to-client request; `id` is echoed back verbatim (any JSON type).
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    /// Response to one of our requests (ids are the integers we generated).
    Response {
        id: u64,
        result: Option<Value>,
        error: Option<Value>,
    },
}

/// Parses one stdout line. Why: the agent may print junk (escape bytes) before its JSON,
/// so parsing starts at the first '{'; anything else is dropped rather than trusted.
pub(crate) fn classify_line(raw: &[u8]) -> Option<Incoming> {
    let start = raw.iter().position(|&b| b == b'{')?;
    let value: Value = serde_json::from_slice(raw.get(start..)?).ok()?;
    let obj = value.as_object()?;
    let method = obj.get("method").and_then(Value::as_str).map(str::to_owned);
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    match (method, obj.get("id")) {
        (Some(method), Some(id)) => Some(Incoming::Request {
            id: id.clone(),
            method,
            params,
        }),
        (Some(method), None) => Some(Incoming::Notification { method, params }),
        (None, Some(id)) if obj.contains_key("result") || obj.contains_key("error") => {
            Some(Incoming::Response {
                id: id.as_u64()?,
                result: obj.get("result").cloned(),
                error: obj.get("error").cloned(),
            })
        }
        _ => None,
    }
}

/// Client request as one JSON line (the writer adds the newline).
pub(crate) fn request_json(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

/// Client notification (no id) as one JSON line.
pub(crate) fn notification_json(method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string()
}

/// Success response to an agent request, echoing its id verbatim.
pub(crate) fn response_ok_json(id: &Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

/// Error response to an agent request (e.g. -32601 for unsupported methods).
pub(crate) fn response_err_json(id: &Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

/// Answer to `session/request_permission`: always denied. Why: Vox grants no client-side
/// capability in this version; the agent's own tools are not controlled by this answer.
pub(crate) fn permission_denied_result() -> Value {
    json!({"outcome": {"outcome": "cancelled"}})
}

/// Code and message of a JSON-RPC error object, capped for display.
pub(crate) fn error_detail(err: &Value) -> String {
    let code = err
        .get("code")
        .and_then(Value::as_i64)
        .map_or_else(|| "unknown".to_string(), |c| c.to_string());
    let message = err.get("message").and_then(Value::as_str).unwrap_or("");
    cap_detail(&format!("code {code}: {message}"))
}

/// What a `session/prompt` stop reason means for the turn (DESIGN 6c).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopOutcome {
    Reply,
    ReplyIfText,
    Refused,
    Cancelled,
    Unknown,
}

pub(crate) fn map_stop_reason(v: Option<&str>) -> StopOutcome {
    match v {
        Some("end_turn" | "max_tokens") => StopOutcome::Reply,
        Some("max_turn_requests") => StopOutcome::ReplyIfText,
        Some("refusal") => StopOutcome::Refused,
        Some("cancelled") => StopOutcome::Cancelled,
        _ => StopOutcome::Unknown,
    }
}

pub(crate) fn initialize_params() -> Value {
    json!({
        "protocolVersion": 1,
        "clientCapabilities": {
            "fs": {"readTextFile": false, "writeTextFile": false},
            "terminal": false
        },
        "clientInfo": {"name": "vox", "version": env!("CARGO_PKG_VERSION")}
    })
}

pub(crate) fn session_new_params(cwd: &std::path::Path) -> Value {
    json!({"cwd": cwd.to_string_lossy().into_owned(), "mcpServers": []})
}

pub(crate) fn prompt_params(session_id: &str, text: &str) -> Value {
    json!({"sessionId": session_id, "prompt": [{"type": "text", "text": text}]})
}

pub(crate) fn cancel_params(session_id: &str) -> Value {
    json!({"sessionId": session_id})
}

pub(crate) fn close_params(session_id: &str) -> Value {
    json!({"sessionId": session_id})
}

// ---------------------------------------------------------------- connection layer (S1b-2)

/// Why a connection was retired. Stored once by `retire`; waiters map it to an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RetireReason {
    Shutdown,
    Idle,
    Timeout,
    Replaced,
    Eof,
    Protocol(String),
    Io(String),
}

/// What a waiter receives: the JSON-RPC `result`, a JSON-RPC `error` object, or the
/// news that the connection was retired (the reason is read from the connection).
pub(crate) enum ConnFail {
    Rpc(Value),
    Retired,
}

/// Delivery slot of one in-flight request.
type Waiter = mpsc::Sender<Result<Value, ConnFail>>;

/// Receiving end of one request's waiter.
type ResponseRx = mpsc::Receiver<Result<Value, ConnFail>>;

/// Bounded state of the turn currently running on a connection.
#[derive(Default)]
pub(crate) struct TurnAcc {
    /// Session whose `session/update` notifications are accepted (None = no turn).
    active_session: Option<String>,
    text: String,
    /// Set when the aggregate would exceed `MAX_RAW_REPLY_BYTES`.
    overflow: bool,
    /// Permission context of the same turn. Cleared with `active_session`.
    perm: Option<PermTurn>,
}

/// Locks ignoring poisoning. Why: a panicking helper must not turn every later call into
/// a panic; the protected data is plain state that stays consistent per update.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Waits on `cv` until `done` holds or `deadline` passes; returns the guard either way.
fn wait_deadline<'a, T>(
    cv: &Condvar,
    mut guard: MutexGuard<'a, T>,
    deadline: Instant,
    done: impl Fn(&T) -> bool,
) -> MutexGuard<'a, T> {
    while !done(&guard) {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        guard = cv
            .wait_timeout(guard, deadline - now)
            .map(|(g, _)| g)
            .unwrap_or_else(|e| e.into_inner().0);
    }
    guard
}

/// Counts one live helper thread. Created before the thread starts and dropped when the
/// thread's closure ends, so `helper_threads_alive()` never reports a thread that is gone.
struct HelperGuard(Arc<AtomicUsize>);

impl HelperGuard {
    fn new(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(counter))
    }
}

impl Drop for HelperGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// One spawned ACP child with its pipes, helper threads and in-flight requests.
///
/// Lock order (never hold two of these at once, except where noted in `retire`):
/// `pending` / `reason` / `acc` / `exit` / `stderr_tail` / `stderr_done` / `child` are leaves.
/// No lock is held while calling `retire`, `submit`, or any blocking wait.
pub(crate) struct Conn {
    /// The leader. Locked only briefly for `try_wait`/`wait`; never held across a sleep.
    child: Mutex<Child>,
    /// Process-group id (== leader pid). Only signalled when > 1 (never 0 or -1).
    pgid: i32,
    /// Set exactly once by the winning `retire` call.
    retired: AtomicBool,
    reason: Mutex<Option<RetireReason>>,
    /// Request id -> waiter. Drained by `retire`, so every waiter fails exactly once.
    pending: Mutex<HashMap<u64, Waiter>>,
    /// Bounded outbound queue drained by the writer thread (capacity 8).
    outbound: SyncSender<String>,
    next_id: AtomicU64,
    acc: Mutex<TurnAcc>,
    acc_cv: Condvar,
    /// Last `STDERR_TAIL_CHARS` characters of the child's stderr.
    stderr_tail: Mutex<String>,
    stderr_done: Mutex<bool>,
    stderr_cv: Condvar,
    /// `None` until the leader is reaped; then `Some(exit code or None on signal)`.
    exit: Mutex<Option<Option<i32>>>,
    exit_cv: Condvar,
    helpers: Arc<AtomicUsize>,
    /// Generation and permission wiring of this child. See `ConnLink`.
    link: Arc<ConnLink>,
}

impl Conn {
    pub(crate) fn is_retired(&self) -> bool {
        self.retired.load(Ordering::SeqCst)
    }

    /// Error a waiter reports when the connection was retired for `self.reason`.
    pub(crate) fn retire_error(&self) -> AcpAgentError {
        let reason = lock(&self.reason).clone();
        match reason {
            None | Some(RetireReason::Shutdown | RetireReason::Idle | RetireReason::Replaced) => {
                AcpAgentError::Closed
            }
            Some(RetireReason::Timeout) => AcpAgentError::Timeout,
            Some(RetireReason::Eof | RetireReason::Io(_)) => self.exit_error(),
            Some(RetireReason::Protocol(msg)) => AcpAgentError::Protocol(msg),
        }
    }

    /// `AgentExited` with the exit code and stderr tail. Waits at most 1 s for the leader
    /// to be reaped and for the stderr thread to reach EOF, so the tail is complete when
    /// the child died on its own. Why: the caller otherwise races the helper threads.
    pub(crate) fn exit_error(&self) -> AcpAgentError {
        let deadline = Instant::now() + Duration::from_secs(1);
        let exit = wait_deadline(&self.exit_cv, lock(&self.exit), deadline, Option::is_some);
        let code = (*exit).flatten();
        drop(exit);
        let done = wait_deadline(&self.stderr_cv, lock(&self.stderr_done), deadline, |d| *d);
        drop(done);
        let stderr = lock(&self.stderr_tail).clone();
        AcpAgentError::AgentExited { code, stderr }
    }

    /// Records the leader's exit (first writer wins) and wakes `exit_error` waiters.
    fn record_exit(&self, code: Option<i32>) {
        {
            let mut exit = lock(&self.exit);
            if exit.is_none() {
                *exit = Some(code);
            }
        }
        self.exit_cv.notify_all();
    }

    /// Non-blocking reap attempt. Returns true once the leader is reaped.
    fn try_reap(&self) -> bool {
        let status = lock(&self.child).try_wait();
        match status {
            Ok(Some(s)) => {
                self.record_exit(s.code());
                true
            }
            Ok(None) => false,
            Err(_) => {
                self.record_exit(None);
                true
            }
        }
    }

    /// Blocking reap. Used only after SIGKILL, which guarantees the leader dies.
    fn reap_blocking(&self) {
        let status = lock(&self.child).wait();
        self.record_exit(status.ok().and_then(|s| s.code()));
    }

    /// Kills the whole process group: SIGTERM, up to 2 s for the leader, then SIGKILL and
    /// a blocking reap. Why the group: the agent's own helper processes must not survive it.
    #[cfg(unix)]
    fn terminate(&self) {
        if self.pgid > 1 {
            // SAFETY: kill(2) only sends a signal; pgid is validated above (> 1).
            unsafe { libc::kill(-self.pgid, libc::SIGTERM) };
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.try_reap() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        if self.pgid > 1 {
            // Sweeps descendants even when the leader already exited on SIGTERM.
            // SAFETY: as above.
            unsafe { libc::kill(-self.pgid, libc::SIGKILL) };
        }
        self.reap_blocking();
    }

    #[cfg(not(unix))]
    fn terminate(&self) {
        let _ = lock(&self.child).kill();
        self.reap_blocking();
    }

    /// Registers and sends one request; the response is read with `await_response`.
    pub(crate) fn start_request(
        &self,
        method: &str,
        params: Value,
        deadline: Instant,
    ) -> Result<(u64, ResponseRx), AcpAgentError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel::<Result<Value, ConnFail>>();
        lock(&self.pending).insert(id, tx);
        // Why: retire sets the flag before draining `pending`; re-checking after the insert
        // closes the window where the drain already happened and our waiter would hang.
        if self.is_retired() {
            lock(&self.pending).remove(&id);
            return Err(self.retire_error());
        }
        if let Err(e) = self.submit(request_json(id, method, params), deadline) {
            lock(&self.pending).remove(&id);
            return Err(e);
        }
        Ok((id, rx))
    }

    /// Waits until `deadline` for the response on `rx`. Retirement fails the wait at once.
    /// The waiter stays registered on timeout, so a caller can still wait for the answer.
    pub(crate) fn await_response(
        &self,
        rx: &ResponseRx,
        deadline: Instant,
    ) -> Result<Result<Value, Value>, AcpAgentError> {
        let wait = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok(Ok(result)) => Ok(Ok(result)),
            Ok(Err(ConnFail::Rpc(err))) => Ok(Err(err)),
            Ok(Err(ConnFail::Retired)) | Err(RecvTimeoutError::Disconnected) => {
                Err(self.retire_error())
            }
            Err(RecvTimeoutError::Timeout) => Err(AcpAgentError::Timeout),
        }
    }

    /// Registers, sends and waits for one request. Returns the raw JSON-RPC outcome so the
    /// manager can map initialize/session errors to `HandshakeFailed` itself. Gives up at
    /// once with `Cancelled` when `cancelled` turns true.
    /// Polls in slices of at most `TICK`, so a cancel never waits for the agent's reply.
    pub(crate) fn request_until(
        &self,
        method: &str,
        params: Value,
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Result<Value, Value>, AcpAgentError> {
        let (id, rx) = self.start_request(method, params, deadline)?;
        let outcome = loop {
            if cancelled() {
                break Err(AcpAgentError::Cancelled);
            }
            let wait = deadline.saturating_duration_since(Instant::now()).min(TICK);
            match rx.recv_timeout(wait) {
                Ok(Ok(result)) => break Ok(Ok(result)),
                Ok(Err(ConnFail::Rpc(err))) => break Ok(Err(err)),
                Ok(Err(ConnFail::Retired)) | Err(RecvTimeoutError::Disconnected) => {
                    break Err(self.retire_error());
                }
                Err(RecvTimeoutError::Timeout) => {
                    if Instant::now() >= deadline {
                        break Err(AcpAgentError::Timeout);
                    }
                }
            }
        };
        // A late response may still be in the channel; the caller treats the request as
        // not answered (DESIGN 5.8: timeout precedence).
        if matches!(
            outcome,
            Err(AcpAgentError::Timeout | AcpAgentError::Cancelled)
        ) {
            lock(&self.pending).remove(&id);
        }
        outcome
    }

    /// Best-effort notification: never blocks, errors ignored (used for session/close).
    pub(crate) fn try_notify(&self, method: &str, params: Value) {
        let _ = self.outbound.try_send(notification_json(method, params));
    }

    /// Queues one line for the writer, polling until the deadline or retirement.
    /// Why polling: a blocking send could outlive the absolute deadline.
    fn submit(&self, line: String, deadline: Instant) -> Result<(), AcpAgentError> {
        let mut line = line;
        loop {
            if self.is_retired() {
                return Err(self.retire_error());
            }
            match self.outbound.try_send(line) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Full(back)) => {
                    if Instant::now() >= deadline {
                        return Err(AcpAgentError::Timeout);
                    }
                    line = back;
                    thread::sleep(Duration::from_millis(5));
                }
                Err(TrySendError::Disconnected(_)) => return Err(self.retire_error()),
            }
        }
    }

    /// Starts collecting agent text for the session `session_id` (the reader ignores every
    /// other session's updates). Must be called before the prompt request is sent.
    pub(crate) fn begin_turn(&self, session_id: &str) {
        let mut acc = lock(&self.acc);
        acc.active_session = Some(session_id.to_owned());
        acc.text.clear();
        acc.overflow = false;
    }

    /// Stops collecting and takes the accumulated text. Ends the turn's permission context too.
    pub(crate) fn end_turn(&self) -> String {
        let mut acc = lock(&self.acc);
        acc.active_session = None;
        acc.perm = None;
        std::mem::take(&mut acc.text)
    }

    /// Attaches the permission context of the turn that `begin_turn` started. Must be called
    /// before the prompt is sent, so that no agent request can arrive without it.
    pub(crate) fn attach_turn(&self, turn: PermTurn) {
        lock(&self.acc).perm = Some(turn);
    }
}

/// Tears a connection down. Idempotent: only the first caller proceeds (DESIGN 6a).
/// Order matters: the reason is stored before `pending` is drained, so a woken waiter
/// always finds the reason; the helper threads are never joined (they exit on their own
/// within one tick once `retired` is set).
pub(crate) fn retire(conn: &Arc<Conn>, reason: RetireReason) {
    if conn
        .retired
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    // Why: a retired child must receive no more committed responses or cancels.
    conn.link.registry.clear_connection(conn.link.conn_gen);
    *lock(&conn.reason) = Some(reason);
    let waiters: Vec<Waiter> = lock(&conn.pending).drain().map(|(_, w)| w).collect();
    for waiter in waiters {
        let _ = waiter.send(Err(ConnFail::Retired));
    }
    // Why: taking the lock before notifying closes the race with a turn that checked
    // `retired` under the lock and is about to wait on the condvar.
    drop(lock(&conn.acc));
    conn.acc_cv.notify_all();
    conn.terminate();
}

/// Result of applying one `session/update` to the turn accumulator.
#[derive(Debug, PartialEq, Eq)]
enum UpdateEffect {
    Ignored,
    Appended,
    Overflow,
}

/// Text of an `agent_message_chunk` whose content is text. Every other update is None.
fn message_chunk_text(params: &Value) -> Option<&str> {
    let update = params.get("update")?;
    if update.get("sessionUpdate")?.as_str()? != "agent_message_chunk" {
        return None;
    }
    let content = update.get("content")?;
    if content.get("type")?.as_str()? != "text" {
        return None;
    }
    content.get("text")?.as_str()
}

/// Applies one notification to the active turn. Only the active session counts (DESIGN 6b):
/// a stray or late update must never leak into another turn's reply.
fn apply_update(conn: &Conn, params: &Value) -> UpdateEffect {
    let mut acc = lock(&conn.acc);
    let session_matches = acc.active_session.is_some()
        && params.get("sessionId").and_then(Value::as_str) == acc.active_session.as_deref();
    if !session_matches {
        return UpdateEffect::Ignored;
    }
    let Some(text) = message_chunk_text(params) else {
        return UpdateEffect::Ignored;
    };
    if text.is_empty() {
        return UpdateEffect::Ignored;
    }
    if acc.text.len().saturating_add(text.len()) > MAX_RAW_REPLY_BYTES {
        acc.overflow = true;
        return UpdateEffect::Overflow;
    }
    acc.text.push_str(text);
    UpdateEffect::Appended
}

/// Routes one stdout line (reader thread only). Dispatch rules: DESIGN 6b.
fn handle_line(conn: &Arc<Conn>, raw: &[u8]) {
    match classify_line(raw) {
        // Junk, invalid JSON and non-objects are dropped without echoing their content.
        None => {}
        Some(Incoming::Response { id, result, error }) => {
            let waiter = lock(&conn.pending).remove(&id);
            if let Some(waiter) = waiter {
                // Why: updates after a response belong to no turn (DESIGN 6b). Closing the turn
                // here, before the waiter wakes, makes that cut exact; the text stays for the caller.
                let mut acc = lock(&conn.acc);
                acc.active_session = None;
                acc.perm = None;
                drop(acc);
                let outcome = match error.filter(|e| !e.is_null()) {
                    Some(err) => Err(ConnFail::Rpc(err)),
                    None => Ok(result.unwrap_or(Value::Null)),
                };
                let _ = waiter.send(outcome);
            }
        }
        Some(Incoming::Request { id, method, params }) => {
            if method == "session/request_permission" {
                answer_permission_request(conn, &id, &params);
            } else {
                write_direct(conn, response_err_json(&id, -32601, "method not found"));
            }
        }
        Some(Incoming::Notification { method, params }) => {
            if method == "session/update" {
                match apply_update(conn, &params) {
                    UpdateEffect::Ignored => {}
                    UpdateEffect::Appended => conn.acc_cv.notify_all(),
                    UpdateEffect::Overflow => {
                        retire(conn, RetireReason::Protocol("reply too large".to_owned()));
                    }
                }
            }
        }
    }
}

/// Queues a line the reader owes the agent, without blocking. A full queue means the agent
/// floods requests while not reading, which is a protocol failure.
fn write_direct(conn: &Arc<Conn>, line: String) {
    if let Err(TrySendError::Full(_)) = conn.outbound.try_send(line) {
        retire(
            conn,
            RetireReason::Protocol("outbound queue full".to_owned()),
        );
    }
}

/// Answers a request that no run decides: cancelled, echoing the id exactly as received.
fn write_cancelled(conn: &Arc<Conn>, id: &Value) {
    write_direct(conn, response_ok_json(id, permission_denied_result()));
}

/// Commits a turn-bound answer through the registry, the linearization point. A refused
/// commit is recorded by the registry, so nothing more is done here.
fn commit_decision(conn: &Arc<Conn>, turn: &PermTurn, id: &Value, outcome: &Outcome) {
    let line = response_ok_json(id, outcome_json(outcome));
    let _ = conn
        .link
        .registry
        .commit(conn.link.conn_gen, turn.handle.token(), &turn.session, line);
}

/// Starts an approval wait. The turn's clock pauses and the broker owns the prompt until it
/// commits. Never blocks: the broker submit is a bounded, non-blocking queue operation.
fn ask_user(
    conn: &Arc<Conn>,
    turn: &PermTurn,
    wiring: &PermissionWiring,
    id: &Value,
    req: PermissionRequest,
    configured: Duration,
) {
    let now = Instant::now();
    let budget = lock(&turn.timing).approval_budget(now, configured);
    if budget.is_zero() {
        return commit_decision(conn, turn, id, &Outcome::Cancelled);
    }
    lock(&turn.timing).enter_approval(now);
    let binding = RequestBinding {
        conn_gen: conn.link.conn_gen,
        run_token: turn.handle.token(),
        session_id: turn.session.clone(),
        rpc_id: id.clone(),
    };
    match wiring.broker.submit(binding, req, now + budget) {
        SubmitResult::Queued { .. } => {}
        // Why: a duplicate is dropped without a commit, so its approval wait ends here.
        SubmitResult::Duplicate => lock(&turn.timing).exit_approval(Instant::now()),
        // Why: an overflowing request was already committed as Cancelled, which ended its wait.
        SubmitResult::Overflow => {}
    }
}

/// Answers one `session/request_permission` (DESIGN Rev 3, M2). The reader never blocks:
/// every path either commits through the registry, queues to the broker, or answers
/// cancelled. Requests outside an active turn, from another session, past the flood
/// limit, or unparsable are cancelled.
fn answer_permission_request(conn: &Arc<Conn>, id: &Value, params: &Value) {
    let turn = lock(&conn.acc).perm.clone();
    let Some(turn) = turn else {
        return write_cancelled(conn, id);
    };
    if !conn.link.registry.note_permission_request(&turn.handle) {
        return write_cancelled(conn, id);
    }
    let Ok(req) = parse_permission_request(params) else {
        return write_cancelled(conn, id);
    };
    if req.session_id != turn.session {
        return write_cancelled(conn, id);
    }
    let Some(wiring) = conn.link.permissions.clone() else {
        return write_cancelled(conn, id);
    };
    let settings = (wiring.settings)();
    match decide_auto(settings.mode, &req) {
        AutoDecision::Allow { option_id } => {
            commit_decision(conn, &turn, id, &Outcome::Selected(option_id));
        }
        AutoDecision::Reject { .. } => {
            commit_decision(conn, &turn, id, &outcome_for(UserChoice::Reject, &req));
        }
        AutoDecision::Ask => {
            ask_user(conn, &turn, &wiring, id, req, settings.approval_timeout);
        }
    }
}

/// Blocks until `fd` is ready for `events` or one TICK passes (EINTR included). The result
/// is ignored on purpose: callers re-check `retired` and retry after every return.
#[cfg(unix)]
fn wait_fd(fd: RawFd, events: libc::c_short) {
    let mut pfd = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    let ms = libc::c_int::try_from(TICK.as_millis()).unwrap_or(100);
    // SAFETY: one valid, initialised pollfd lives for the whole call.
    unsafe { libc::poll(&mut pfd, 1, ms) };
}

/// Puts a pipe end into non-blocking mode so no helper can block past a tick.
#[cfg(unix)]
fn set_nonblocking(fd: RawFd) -> std::io::Result<()> {
    // SAFETY: F_GETFL/F_SETFL only read and update the flags of an fd we own.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: as above.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Reader adapter: turns WouldBlock into a tick-bounded poll and gives up once the
/// connection is retired, so a blocked read never outlives the retirement by more than
/// one TICK.
#[cfg(unix)]
struct PollRead<R> {
    src: R,
    fd: RawFd,
    conn: Arc<Conn>,
}

#[cfg(unix)]
impl<R: Read> Read for PollRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            match self.src.read(buf) {
                Ok(n) => return Ok(n),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if self.conn.is_retired() {
                        return Err(std::io::Error::other("connection retired"));
                    }
                    wait_fd(self.fd, libc::POLLIN);
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// Writes all of `line`, waiting for POLLOUT on partial writes. Gives up once retired.
#[cfg(unix)]
fn write_line(conn: &Conn, stdin: &mut ChildStdin, fd: RawFd, line: &[u8]) -> std::io::Result<()> {
    let mut sent = 0;
    while sent < line.len() {
        let rest = line.get(sent..).unwrap_or_default();
        match stdin.write(rest) {
            Ok(0) => return Err(ErrorKind::WriteZero.into()),
            Ok(n) => sent += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                if conn.is_retired() {
                    return Err(std::io::Error::other("connection retired"));
                }
                wait_fd(fd, libc::POLLOUT);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// WRITER thread: sole owner of stdin. Exits when retired or when every sender is gone.
#[cfg(unix)]
fn writer_loop(conn: Arc<Conn>, rx: Receiver<String>, mut stdin: ChildStdin, fd: RawFd) {
    loop {
        if conn.is_retired() {
            return;
        }
        match rx.recv_timeout(TICK) {
            Ok(mut line) => {
                line.push('\n');
                if let Err(e) = write_line(&conn, &mut stdin, fd, line.as_bytes()) {
                    retire(&conn, RetireReason::Io(e.to_string()));
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// READER thread: the dispatcher and the only consumer of stdout.
#[cfg(unix)]
fn reader_loop(conn: Arc<Conn>, stdout: ChildStdout) {
    let fd = stdout.as_raw_fd();
    let src = PollRead {
        src: stdout,
        fd,
        conn: Arc::clone(&conn),
    };
    let mut reader = std::io::BufReader::new(src);
    let mut buf = Vec::new();
    loop {
        if conn.is_retired() {
            return;
        }
        match read_bounded_line(&mut reader, &mut buf, MAX_LINE_BYTES) {
            Ok(LineRead::Line) => handle_line(&conn, &buf),
            Ok(LineRead::Eof) => {
                retire(&conn, RetireReason::Eof);
                return;
            }
            Ok(LineRead::TooLong) => {
                retire(&conn, RetireReason::Protocol("line too long".to_owned()));
                return;
            }
            Err(e) => {
                retire(&conn, RetireReason::Io(e.to_string()));
                return;
            }
        }
    }
}

/// Keeps only the last `STDERR_TAIL_CHARS` characters. Cuts on a char boundary.
fn push_tail(tail: &mut String, chunk: &str) {
    tail.push_str(chunk);
    let total = tail.chars().count();
    if total > STDERR_TAIL_CHARS {
        let cut = tail
            .char_indices()
            .nth(total - STDERR_TAIL_CHARS)
            .map_or(0, |(i, _)| i);
        tail.drain(..cut);
    }
}

/// STDERR thread: drains continuously so a chatty agent can never block on a full pipe.
#[cfg(unix)]
fn stderr_loop(conn: Arc<Conn>, stderr: ChildStderr) {
    let fd = stderr.as_raw_fd();
    let mut src = PollRead {
        src: stderr,
        fd,
        conn: Arc::clone(&conn),
    };
    let mut chunk = [0u8; 4096];
    // Why check on every pass: a descendant that keeps writing stderr must not keep this
    // helper alive after retirement, because a read that never blocks never reaches the
    // wait inside `PollRead`.
    while !conn.is_retired() {
        let Ok(n) = src.read(&mut chunk) else {
            break;
        };
        if n == 0 {
            break;
        }
        // Why lossy per chunk: a split multi-byte char shows as U+FFFD, never as a panic.
        let text = String::from_utf8_lossy(chunk.get(..n).unwrap_or_default());
        push_tail(&mut lock(&conn.stderr_tail), &text);
    }
    *lock(&conn.stderr_done) = true;
    conn.stderr_cv.notify_all();
}

/// Kills and reaps a child that never became a connection, then reports `SpawnFailed`.
#[cfg(unix)]
fn abort_spawn(mut child: Child, msg: &str) -> AcpAgentError {
    let _ = child.kill();
    let _ = child.wait();
    AcpAgentError::SpawnFailed(cap_detail(msg))
}

/// Starts one named helper thread that owns a `Conn` handle and a live-count guard.
/// On failure the connection is retired (the child is killed) and `SpawnFailed` returned.
#[cfg(unix)]
fn start_helper<F>(conn: &Arc<Conn>, name: &str, body: F) -> Result<(), AcpAgentError>
where
    F: FnOnce(Arc<Conn>) + Send + 'static,
{
    let guard = HelperGuard::new(&conn.helpers);
    let handle_conn = Arc::clone(conn);
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let _alive = guard;
            body(handle_conn);
        })
        .map(|_handle| ())
        .map_err(|e| {
            retire(conn, RetireReason::Io(e.to_string()));
            AcpAgentError::SpawnFailed(cap_detail(&e.to_string()))
        })
}

impl Conn {
    /// Spawns the child (no shell, own process group, piped stdio, cwd = `cfg.cwd`) and its
    /// three helper threads. A missing program is `CommandNotFound`; any other spawn error
    /// is `SpawnFailed`.
    #[cfg(unix)]
    pub(crate) fn spawn(
        cfg: &AcpAgentConfig,
        helpers: Arc<AtomicUsize>,
        link: ConnLink,
    ) -> Result<Arc<Self>, AcpAgentError> {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        let mut cmd = Command::new(&cfg.command);
        cmd.args(&cfg.args)
            .current_dir(&cfg.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Why a new group: retirement must reach the agent's own helper processes.
            .process_group(0);
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                AcpAgentError::CommandNotFound
            } else {
                AcpAgentError::SpawnFailed(cap_detail(&e.to_string()))
            }
        })?;
        let pgid = match i32::try_from(child.id()) {
            Ok(pid) if pid > 1 => pid,
            _ => return Err(abort_spawn(child, "invalid child pid")),
        };
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(abort_spawn(child, "missing stdio pipe"));
        };
        // Why O_NONBLOCK: every helper polls with a tick, so none can block past a retirement.
        let (fd_in, fd_out, fd_err) = (stdin.as_raw_fd(), stdout.as_raw_fd(), stderr.as_raw_fd());
        for fd in [fd_in, fd_out, fd_err] {
            if let Err(e) = set_nonblocking(fd) {
                return Err(abort_spawn(child, &e.to_string()));
            }
        }

        // Why bounded(8): a stuck agent must hit `Full` (=> retire), never grow memory.
        let (outbound, rx) = mpsc::sync_channel::<String>(8);
        let sink = Arc::new(ConnSink {
            outbound: outbound.clone(),
            force_failure: Arc::clone(&link.force_failure),
        });
        let conn = Arc::new(Conn {
            child: Mutex::new(child),
            pgid,
            retired: AtomicBool::new(false),
            reason: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            outbound,
            next_id: AtomicU64::new(1),
            acc: Mutex::new(TurnAcc::default()),
            acc_cv: Condvar::new(),
            stderr_tail: Mutex::new(String::new()),
            stderr_done: Mutex::new(false),
            stderr_cv: Condvar::new(),
            exit: Mutex::new(None),
            exit_cv: Condvar::new(),
            helpers,
            link: Arc::new(link),
        });
        start_helper(&conn, "acp-writer", move |c| {
            writer_loop(c, rx, stdin, fd_in);
        })?;
        start_helper(&conn, "acp-reader", move |c| reader_loop(c, stdout))?;
        start_helper(&conn, "acp-stderr", move |c| stderr_loop(c, stderr))?;
        // Why last: a sink is registered only for a child that fully started.
        conn.link.registry.set_connection(conn.link.conn_gen, sink);
        Ok(conn)
    }

    /// Non-unix platforms have no process-group or poll support: report it plainly.
    #[cfg(not(unix))]
    pub(crate) fn spawn(
        _cfg: &AcpAgentConfig,
        _helpers: Arc<AtomicUsize>,
        _link: ConnLink,
    ) -> Result<Arc<Self>, AcpAgentError> {
        Err(AcpAgentError::Unsupported)
    }
}
