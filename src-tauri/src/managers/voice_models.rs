//! Single-owner inference service for the optional ONNX models (design A2').
//!
//! One worker thread owns every model session (a thread-local map, never
//! shared behind a lock). Callers talk to it through one `Mutex<State>` +
//! `Condvar`: a single-slot hand-off, a single reply slot, and bookkeeping.
//! The lock is never held while a session loads or runs.
//!
//! The authoritative lifecycle tests live in `voice_models_tests.rs`.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{JoinHandle, ThreadId};
use std::time::Duration;

/// Longest input kept for one inference; longer input keeps its last samples.
const MAX_SAMPLES: usize = 128_000;

/// Reload count at which a kind is latched unavailable until reinstalled.
const RELOAD_LATCH: u8 = 3;

/// Process-wide request id source.
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// Optional ONNX model kinds served by the single inference owner.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    SmartTurn,
    Speaker,
}

/// One loaded model session. Owned exclusively by the service worker thread.
pub trait ModelSession: Send {
    /// Runs inference on 16 kHz mono samples. `None` means "no output".
    fn run(&mut self, audio16k: &[f32]) -> Option<Vec<f32>>;
}

/// Loads sessions from a verified model path. Called only on the worker thread.
pub trait SessionLoader: Send + Sync + 'static {
    fn load(&self, kind: ModelKind, path: &Path) -> Option<Box<dyn ModelSession>>;
}

/// Reply delivered for a request id. Panic payloads are never carried here.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Output(Vec<f32>),
    Failed(ErrKind),
}

/// Failure categories for a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrKind {
    Panic,
    NoOutput,
    LoadFailed,
}

/// Returned by `try_submit` when no request can be accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unavailable;

/// Next process-wide request id: one `AtomicU64`, strictly increasing, never 0.
pub fn next_request_id() -> u64 {
    NEXT_ID
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1)
        .max(1)
}

/// The request waiting to be taken by the worker.
struct Job {
    id: u64,
    kind: ModelKind,
    audio: Vec<f32>,
    abandoned: bool,
}

/// The request the worker is currently serving.
struct Running {
    id: u64,
    kind: ModelKind,
    abandoned: bool,
}

/// Everything shared between callers and the worker. Guarded by one mutex.
#[derive(Default)]
struct State {
    installed: HashMap<ModelKind, PathBuf>,
    /// Bumped on every install; a cached session from an older generation is dropped.
    generation: HashMap<ModelKind, u64>,
    pending: Option<Job>,
    running: Option<Running>,
    /// Single reply slot: (request id, kind, reply).
    reply: Option<(u64, ModelKind, Reply)>,
    reloads: HashMap<ModelKind, u8>,
    latched: HashSet<ModelKind>,
    /// Diagnostic: threads that ever loaded or ran a session. Must stay 1.
    worker_ids: HashSet<ThreadId>,
    shutdown: bool,
    /// True from submit until the worker has finished that request.
    outstanding: bool,
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
}

/// A session owned by the worker, tagged with the install generation it came from.
struct Cached {
    generation: u64,
    session: Box<dyn ModelSession>,
}

fn lock_state(mutex: &Mutex<State>) -> MutexGuard<'_, State> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Single-owner inference service. One worker thread owns every session.
pub struct VoiceModelService {
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
    /// Disconnects when the worker thread exits (the worker holds the sender).
    done: Mutex<Receiver<()>>,
}

impl VoiceModelService {
    /// Spawns exactly one worker thread that owns every session.
    pub fn new(loader: Arc<dyn SessionLoader>) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            cv: Condvar::new(),
        });
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let worker_shared = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("voice-models".to_owned())
            .spawn(move || worker_main(worker_shared, loader, done_tx));
        let worker = match spawned {
            Ok(handle) => Some(handle),
            Err(_) => {
                // No worker means no inference: refuse every submit.
                lock_state(&shared.state).shutdown = true;
                None
            }
        };
        Self {
            shared,
            worker: Mutex::new(worker),
            done: Mutex::new(done_rx),
        }
    }

    /// Records the verified path; resets that kind's reload counter and latch.
    pub fn install(&self, kind: ModelKind, path: PathBuf) {
        let mut st = lock_state(&self.shared.state);
        st.installed.insert(kind, path);
        let generation = st.generation.entry(kind).or_insert(0);
        *generation = generation.wrapping_add(1);
        st.reloads.remove(&kind);
        st.latched.remove(&kind);
    }

    /// Removes the installed path and drops any reply for that kind's requests.
    pub fn uninstall(&self, kind: ModelKind) {
        let mut st = lock_state(&self.shared.state);
        st.installed.remove(&kind);
        if matches!(&st.reply, Some((_, k, _)) if *k == kind) {
            st.reply = None;
        }
        if let Some(job) = st.pending.as_mut() {
            if job.kind == kind {
                job.abandoned = true;
            }
        }
        if let Some(running) = st.running.as_mut() {
            if running.kind == kind {
                running.abandoned = true;
            }
        }
    }

    pub fn is_installed(&self, kind: ModelKind) -> bool {
        lock_state(&self.shared.state).installed.contains_key(&kind)
    }

    /// Single-slot hand-off. Input longer than 128000 samples keeps the LAST 128000.
    pub fn try_submit(&self, kind: ModelKind, mut audio: Vec<f32>) -> Result<u64, Unavailable> {
        if audio.len() > MAX_SAMPLES {
            let excess = audio.len() - MAX_SAMPLES;
            audio.drain(..excess);
        }
        let mut st = lock_state(&self.shared.state);
        if st.shutdown
            || st.outstanding
            || st.latched.contains(&kind)
            || !st.installed.contains_key(&kind)
        {
            return Err(Unavailable);
        }
        let id = next_request_id();
        st.reply = None;
        st.pending = Some(Job {
            id,
            kind,
            audio,
            abandoned: false,
        });
        st.outstanding = true;
        self.shared.cv.notify_all();
        Ok(id)
    }

    /// Non-blocking. Consumes and returns the reply only for exactly `request_id`.
    pub fn poll(&self, request_id: u64) -> Option<Reply> {
        let mut st = lock_state(&self.shared.state);
        if matches!(&st.reply, Some((id, _, _)) if *id == request_id) {
            st.reply.take().map(|(_, _, reply)| reply)
        } else {
            None
        }
    }

    /// Never blocks, never joins. Skips the request if not yet started; a running
    /// inference is not interrupted.
    pub fn abandon(&self, request_id: u64) {
        let mut st = lock_state(&self.shared.state);
        if let Some(job) = st.pending.as_mut() {
            if job.id == request_id {
                job.abandoned = true;
                return;
            }
        }
        if let Some(running) = st.running.as_mut() {
            if running.id == request_id {
                running.abandoned = true;
                return;
            }
        }
        if matches!(&st.reply, Some((id, _, _)) if *id == request_id) {
            st.reply = None;
        }
    }

    pub fn reload_count(&self, kind: ModelKind) -> u8 {
        lock_state(&self.shared.state)
            .reloads
            .get(&kind)
            .copied()
            .unwrap_or(0)
    }

    pub fn is_latched_unavailable(&self, kind: ModelKind) -> bool {
        lock_state(&self.shared.state).latched.contains(&kind)
    }

    /// True while any request is outstanding (submit until the thread finished it).
    pub fn outstanding(&self) -> bool {
        lock_state(&self.shared.state).outstanding
    }

    /// Diagnostic: distinct thread ids that ever ran a session. Must stay 1.
    pub fn worker_thread_ids_seen(&self) -> usize {
        lock_state(&self.shared.state).worker_ids.len()
    }

    /// Diagnostic: number of reply slots currently holding a reply (0 or 1).
    pub fn stored_replies(&self) -> usize {
        usize::from(lock_state(&self.shared.state).reply.is_some())
    }

    /// Test helper: sets shutdown, waits up to `timeout` for the worker to exit.
    pub fn shutdown_and_join(self, timeout: Duration) -> bool {
        self.request_shutdown();
        let exited = {
            let done = self.done.lock().unwrap_or_else(|e| e.into_inner());
            matches!(
                done.recv_timeout(timeout),
                Err(RecvTimeoutError::Disconnected)
            )
        };
        if exited {
            let handle = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(handle) = handle {
                // The worker has already signalled exit; joining cannot block.
                let _ = handle.join();
            }
        }
        exited
    }

    fn request_shutdown(&self) {
        lock_state(&self.shared.state).shutdown = true;
        self.shared.cv.notify_all();
    }
}

impl Drop for VoiceModelService {
    /// Sets shutdown and wakes the worker. Never joins: a blocked inference
    /// must not hang the caller. The worker exits after its current request.
    fn drop(&mut self) {
        self.request_shutdown();
    }
}

/// Body of the single worker thread. Owns every session as a local map.
fn worker_main(shared: Arc<Shared>, loader: Arc<dyn SessionLoader>, _done: Sender<()>) {
    let mut sessions: HashMap<ModelKind, Cached> = HashMap::new();
    while let Some((kind, audio)) = take_job(&shared) {
        serve(&shared, loader.as_ref(), &mut sessions, kind, &audio);
        // `audio` is freed here, before the next wait.
    }
}

/// Waits for a pending request or shutdown. Marks the request as running.
fn take_job(shared: &Shared) -> Option<(ModelKind, Vec<f32>)> {
    let mut st = lock_state(&shared.state);
    loop {
        if st.shutdown {
            return None;
        }
        if let Some(job) = st.pending.take() {
            st.running = Some(Running {
                id: job.id,
                kind: job.kind,
                abandoned: job.abandoned,
            });
            return Some((job.kind, job.audio));
        }
        st = shared.cv.wait(st).unwrap_or_else(|e| e.into_inner());
    }
}

/// True while the kind is installed and the running request was not abandoned.
fn still_wanted(st: &State, kind: ModelKind) -> bool {
    st.installed.contains_key(&kind) && matches!(&st.running, Some(r) if !r.abandoned)
}

/// Start-of-request check: returns the install generation and verified path.
fn start_request(shared: &Shared, kind: ModelKind) -> Option<(u64, PathBuf)> {
    let st = lock_state(&shared.state);
    if !still_wanted(&st, kind) {
        return None;
    }
    let path = st.installed.get(&kind).cloned()?;
    let generation = st.generation.get(&kind).copied().unwrap_or(0);
    Some((generation, path))
}

/// Re-checks the request under the lock right before inference and records
/// this thread as the session owner.
fn confirm_still_wanted(shared: &Shared, kind: ModelKind) -> bool {
    let mut st = lock_state(&shared.state);
    st.worker_ids.insert(std::thread::current().id());
    still_wanted(&st, kind)
}

/// Loads a session with the lock released. A loader panic counts as a failed load.
fn load_session(
    loader: &dyn SessionLoader,
    kind: ModelKind,
    path: &Path,
) -> Option<Box<dyn ModelSession>> {
    match catch_unwind(AssertUnwindSafe(|| loader.load(kind, path))) {
        Ok(session) => session,
        Err(_) => None,
    }
}

/// Serves one taken request: start check, lazy load, re-check, run, finish.
fn serve(
    shared: &Shared,
    loader: &dyn SessionLoader,
    sessions: &mut HashMap<ModelKind, Cached>,
    kind: ModelKind,
    audio: &[f32],
) {
    let (generation, path) = match start_request(shared, kind) {
        Some(found) => found,
        None => {
            finish(shared, None, false);
            return;
        }
    };

    if matches!(sessions.get(&kind), Some(c) if c.generation != generation) {
        sessions.remove(&kind);
    }
    if !sessions.contains_key(&kind) {
        match load_session(loader, kind, &path) {
            Some(session) => {
                sessions.insert(
                    kind,
                    Cached {
                        generation,
                        session,
                    },
                );
            }
            None => {
                finish(shared, Some(Reply::Failed(ErrKind::LoadFailed)), true);
                return;
            }
        }
    }

    if !confirm_still_wanted(shared, kind) {
        finish(shared, None, false);
        return;
    }

    let outcome = match sessions.get_mut(&kind) {
        Some(cached) => catch_unwind(AssertUnwindSafe(|| cached.session.run(audio))),
        None => Ok(None),
    };
    match outcome {
        Ok(Some(output)) => finish(shared, Some(Reply::Output(output)), false),
        Ok(None) => finish(shared, Some(Reply::Failed(ErrKind::NoOutput)), false),
        Err(_) => {
            // The payload is dropped unformatted. Discard the session and reload later.
            sessions.remove(&kind);
            finish(shared, Some(Reply::Failed(ErrKind::Panic)), true);
        }
    }
}

/// Locks and completes the running request.
fn finish(shared: &Shared, reply: Option<Reply>, count_reload: bool) {
    let mut st = lock_state(&shared.state);
    finish_locked(shared, &mut st, reply, count_reload);
}

/// Completes the running request: stores the reply unless abandoned or
/// uninstalled, counts a reload when asked, and frees the slot.
fn finish_locked(shared: &Shared, st: &mut State, reply: Option<Reply>, count_reload: bool) {
    if let Some(running) = st.running.take() {
        if count_reload {
            bump_reload(st, running.kind);
        }
        if let Some(reply) = reply {
            if !running.abandoned && st.installed.contains_key(&running.kind) {
                st.reply = Some((running.id, running.kind, reply));
            }
        }
    }
    st.outstanding = false;
    shared.cv.notify_all();
}

fn bump_reload(st: &mut State, kind: ModelKind) {
    let count = st.reloads.entry(kind).or_insert(0);
    *count = count.saturating_add(1);
    if *count >= RELOAD_LATCH {
        st.latched.insert(kind);
    }
}

#[cfg(test)]
#[path = "voice_models_tests.rs"]
mod tests;
