//! Managed runtime for the vault RAG (DESIGN Rev 5, 3.2 and 3.6). Contract:
//! - `IndexBackend`: the index operations the runtime needs; implemented for `IndexManager`.
//! - `VaultRagRuntime::new(backend, gate) -> Arc<Self>`: spawns the single coordinator thread.
//! - `apply(&AppSettings)`: non-blocking; bumps `revision` synchronously.
//! - `request_reindex()`: non-blocking, refresh-only.
//! - `session(&AppSettings) -> RagSession`: atomics only, no IO.
//! - `status_dto()`, `preview(&str)`: gated with a timeout; `shutdown()`: non-blocking.
//!
//! Nothing here logs note text, queries, paths, speech or route keys.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};

use crate::managers::agent_tools::tools::neutralize;
use crate::managers::conversation_save::rag_possible;
use crate::managers::redact::redact;
use crate::managers::vault_index::{Hit, IndexManager, IndexStatus, RagError, SearchOpts};
use crate::managers::vault_io::VaultIoWorker;
use crate::managers::vault_rag::BlockingGate;
use crate::settings::{get_settings, parse_rag_folders, AppSettings};

/// Fixed message for every refused call while the index is not ready (DESIGN 3.2).
const UNAVAILABLE_MSG: &str = "vault search is unavailable";
/// Longest wait for a status read before the status reports "busy" (DESIGN 3.6).
const STATUS_TIMEOUT: Duration = Duration::from_secs(1);
/// Longest wait for a preview search (DESIGN 3.5 tool budget).
const SEARCH_TIMEOUT: Duration = Duration::from_secs(4);
/// Budget for one refresh pass on the coordinator thread.
const REFRESH_BUDGET: Duration = Duration::from_secs(30);
/// Most hits a preview returns (DESIGN 3.6).
const PREVIEW_HITS: usize = 8;
/// Name of the single coordinator thread (DESIGN 3.6).
const COORDINATOR_NAME: &str = "vault-rag-coordinator";
/// Tool support is not probed until the probe lands; every status reports it as untested.
const TOOL_SUPPORT_UNTESTED: &str = "untested";

/// The index operations the runtime needs. Implemented for `IndexManager`; tests use a fake.
pub trait IndexBackend: Send + Sync {
    /// Applies a vault root and folder list; returns the new generation (bumped first).
    fn configure(&self, root: Option<PathBuf>, folders: Vec<String>) -> u64;
    /// Starts a refresh pass for the configured vault within `budget`.
    fn request_refresh(&self, budget: Duration);
    /// Current generation, read without waiting on configure.
    fn generation(&self) -> u64;
    /// Snapshot of the index counters and last error kind.
    fn status(&self) -> IndexStatus;
    /// Searches the index at generation `gen`.
    fn search(&self, gen: u64, query: &str, opts: &SearchOpts) -> Result<Vec<Hit>, RagError>;
}

impl IndexBackend for IndexManager {
    fn configure(&self, root: Option<PathBuf>, folders: Vec<String>) -> u64 {
        IndexManager::configure(self, root, folders)
    }

    fn request_refresh(&self, budget: Duration) {
        IndexManager::request_refresh(self, budget);
    }

    fn generation(&self) -> u64 {
        IndexManager::generation(self)
    }

    fn status(&self) -> IndexStatus {
        IndexManager::status(self)
    }

    fn search(&self, gen: u64, query: &str, opts: &SearchOpts) -> Result<Vec<Hit>, RagError> {
        IndexManager::search(self, gen, query, opts)
    }
}

/// Status DTO for the settings UI (DESIGN Rev 5, 3.6). `tool_support` is a plain string here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct VaultIndexStatusDto {
    pub notes: u32,
    pub chunks: u32,
    pub skipped: u32,
    pub last_refresh_ms: Option<i64>,
    pub refreshing: bool,
    pub last_error_kind: Option<String>,
    /// untested | supported | unsupported | unknown, for the current route key.
    pub tool_support: String,
}

/// One preview hit (DESIGN Rev 5, 3.6). Plain text fields only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct VaultHitDto {
    pub rel: String,
    pub heading: String,
    pub snippet: String,
    pub note_date: Option<String>,
}

/// A pending configuration or reindex request (DESIGN Rev 5, 3.6 C4 PROTOCOL).
#[derive(Debug, Clone)]
pub struct ConfigRequest {
    pub revision: u64,
    pub settings: AppSettings,
    pub reindex: bool,
}

/// One RAG session, pinned at start (DESIGN Rev 5, 3.2). Built from atomics only.
#[derive(Clone)]
pub struct RagSession {
    /// True when the feature was on at session start (DESIGN 3.2 latch).
    pub latched: bool,
    /// Revision pinned at session start.
    pub revision: u64,
    /// Index generation pinned at session start.
    pub generation: u64,
    /// Index used for retrieval and tools.
    pub index: Arc<dyn IndexBackend>,
    /// Concrete index for retrieval and tools; `None` means every read is Unavailable.
    pub manager: Option<Arc<IndexManager>>,
    /// Shared admission gate.
    pub gate: BlockingGate,
    runtime: Arc<VaultRagRuntime>,
    /// True only when the index was ready when the session started; a session started during an
    /// unapplied change stays revoked even after that change applies (DESIGN 3.2).
    ready_at_start: bool,
}

impl RagSession {
    /// True while the pinned session may still read the index (DESIGN Rev 5, 3.2).
    /// `settings` are the fresh settings read by the caller.
    pub fn still_allowed(&self, settings: &AppSettings) -> bool {
        self.ready_at_start
            && rag_possible(settings)
            && self.runtime.revision() == self.revision
            && self.runtime.is_ready()
            && self.index.generation() == self.generation
    }

    /// Probe state for `key` as stored by the runtime (DESIGN 3.1, 3.6).
    pub(crate) fn probe_state(
        &self,
        key: &crate::managers::converse_platform::RouteKey,
    ) -> crate::managers::converse_platform::ToolSupport {
        self.runtime.probe_state_for(key)
    }
}

/// Owns the index backend, the revision protocol and the single coordinator thread (DESIGN Rev 5,
/// 3.2 and 3.6). Nothing here logs note text, queries, paths or route keys.
pub struct VaultRagRuntime {
    backend: Arc<dyn IndexBackend>,
    /// The concrete index for per-turn retrieval and tools (DESIGN Rev 5, 3.4-3.5). `None` makes
    /// every retrieval Unavailable (fail safe).
    manager: Option<Arc<IndexManager>>,
    gate: BlockingGate,
    /// Bumped synchronously by every accepted settings change (DESIGN 3.2).
    revision: AtomicU64,
    /// Revision whose configuration is serving the index; 0 = none.
    applied: AtomicU64,
    /// Set by `shutdown`; no later pass publishes.
    closed: AtomicBool,
    /// Latest pending request; the coordinator takes it (DESIGN 3.6 C4 PROTOCOL).
    pending: Mutex<Option<ConfigRequest>>,
    /// Wakes the coordinator when a request arrives or the runtime closes.
    wake: Condvar,
    /// Coordinator threads this runtime started (fixed at one).
    threads: AtomicUsize,
    /// True when the applied configuration is enabled with a vault root (set before `applied`).
    live: AtomicBool,
    /// Snapshot of the applied configuration, used by refresh-only reindex requests.
    applied_settings: Mutex<Option<AppSettings>>,
    /// Last probe result with the route key it was taken for (read via `probe_state_for`).
    probe: Mutex<
        Option<(
            crate::managers::converse_platform::RouteKey,
            crate::managers::converse_platform::ToolSupport,
        )>,
    >,
}

impl VaultRagRuntime {
    /// Creates the runtime and spawns its single coordinator thread (DESIGN 3.6).
    /// Production uses `with_manager`; this constructor serves the tests without a concrete index.
    #[allow(dead_code)] // called by tests only
    pub fn new(backend: Arc<dyn IndexBackend>, gate: BlockingGate) -> Arc<Self> {
        Self::build(backend, None, gate)
    }

    /// Creates the runtime with the concrete index used for retrieval and tools (DESIGN Rev 5,
    /// 3.4-3.5). `backend` and `manager` must be the same index.
    pub fn with_manager(
        backend: Arc<dyn IndexBackend>,
        manager: Arc<IndexManager>,
        gate: BlockingGate,
    ) -> Arc<Self> {
        Self::build(backend, Some(manager), gate)
    }

    fn build(
        backend: Arc<dyn IndexBackend>,
        manager: Option<Arc<IndexManager>>,
        gate: BlockingGate,
    ) -> Arc<Self> {
        let runtime = Arc::new(VaultRagRuntime {
            backend,
            manager,
            gate,
            revision: AtomicU64::new(0),
            applied: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            pending: Mutex::new(None),
            wake: Condvar::new(),
            threads: AtomicUsize::new(0),
            live: AtomicBool::new(false),
            applied_settings: Mutex::new(None),
            probe: Mutex::new(None),
        });
        let worker = Arc::clone(&runtime);
        let spawned = thread::Builder::new()
            .name(COORDINATOR_NAME.to_string())
            .spawn(move || worker.coordinate());
        if spawned.is_ok() {
            runtime.threads.fetch_add(1, Ordering::SeqCst);
        }
        runtime
    }

    /// Accepts a settings change: bumps the revision synchronously and queues the snapshot. Never
    /// blocks (DESIGN 3.2, 3.6).
    pub fn apply(&self, settings: &AppSettings) {
        let revision = self.revision.fetch_add(1, Ordering::SeqCst) + 1;
        self.offer(ConfigRequest {
            revision,
            settings: settings.clone(),
            reindex: false,
        });
    }

    /// Queues a refresh-only pass for the applied configuration. Ignored when not ready or when
    /// disabled. Never blocks and never bumps the revision (DESIGN 3.6 item 5).
    pub fn request_reindex(&self) {
        if !self.is_ready() {
            return;
        }
        let revision = self.revision();
        let applied: Option<AppSettings> = lock(&self.applied_settings).clone();
        let Some(settings) = applied else {
            return;
        };
        self.offer(ConfigRequest {
            revision,
            settings,
            reindex: true,
        });
    }

    /// The current revision.
    pub fn revision(&self) -> u64 {
        self.revision.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The revision whose configuration is serving the index (0 = none).
    #[allow(dead_code)] // read by tests only
    pub fn applied_revision(&self) -> u64 {
        self.applied.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// True when applied == revision, not closed, and the applied configuration is enabled.
    pub fn is_ready(&self) -> bool {
        !self.closed.load(Ordering::SeqCst)
            && self.applied.load(Ordering::SeqCst) == self.revision.load(Ordering::SeqCst)
            && self.live.load(Ordering::SeqCst)
    }

    /// Stops the runtime without waiting (DESIGN 3.6 shutdown). Never calls configure, takes the
    /// index mutex, joins a thread or waits.
    pub fn shutdown(&self) {
        {
            // Closing and bumping under the slot lock: the coordinator checks `closed` under the
            // same lock before it waits, so the wake-up cannot be lost.
            let _slot = lock(&self.pending);
            self.closed.store(true, Ordering::SeqCst);
            self.revision.fetch_add(1, Ordering::SeqCst);
        }
        self.wake.notify_all();
    }

    /// Status for the settings UI, through the gate with a 1 s timeout (DESIGN 3.6). Saturation or
    /// timeout gives `last_error_kind` "busy" and `refreshing` true.
    pub fn status_dto(&self) -> VaultIndexStatusDto {
        let backend = Arc::clone(&self.backend);
        let outcome =
            tauri::async_runtime::block_on(self.gate.run(STATUS_TIMEOUT, move || backend.status()));
        let mut dto = match outcome {
            Ok(status) => dto_from_status(&status),
            Err(_) => return busy_dto(),
        };
        if !self.is_ready() {
            dto.last_error_kind = Some("applying".to_string());
            dto.refreshing = true;
        }
        dto
    }

    /// Searches the applied vault, capped at 8 sanitised hits (DESIGN 3.6). Requires enabled and
    /// ready; otherwise `Err("vault search is unavailable")` or the disabled error.
    pub fn preview(&self, query: &str) -> Result<Vec<VaultHitDto>, String> {
        if !self.is_ready() {
            return Err(UNAVAILABLE_MSG.to_string());
        }
        let revision = self.revision();
        let generation = self.backend.generation();
        let backend = Arc::clone(&self.backend);
        let text = query.to_string();
        let opts = SearchOpts {
            folder: None,
            since_days: None,
            limit: PREVIEW_HITS,
        };
        let outcome = tauri::async_runtime::block_on(self.gate.run(SEARCH_TIMEOUT, move || {
            backend.search(generation, &text, &opts)
        }));
        // Results are kept only if the configuration did not change while the search ran.
        let unchanged = self.is_ready()
            && self.revision() == revision
            && self.backend.generation() == generation;
        if !unchanged {
            return Err(UNAVAILABLE_MSG.to_string());
        }
        match outcome {
            Ok(Ok(hits)) => Ok(hits
                .into_iter()
                .take(PREVIEW_HITS)
                .map(dto_from_hit)
                .collect()),
            _ => Err(UNAVAILABLE_MSG.to_string()),
        }
    }

    /// Builds a session pinned to the current revision and generation (DESIGN 3.2). Atomics only.
    pub fn session(self: &Arc<Self>, settings: &AppSettings) -> RagSession {
        let ready_at_start = self.is_ready();
        let revision = self.revision();
        RagSession {
            latched: rag_possible(settings),
            revision,
            generation: self.backend.generation(),
            index: Arc::clone(&self.backend),
            manager: self.manager.clone(),
            gate: self.gate.clone(),
            runtime: Arc::clone(self),
            ready_at_start,
        }
    }

    /// Test seam: offers a request with an explicit revision, as the pending slot would receive it
    /// (DESIGN 3.6 C4: the slot keeps the highest revision).
    #[cfg(test)]
    pub(crate) fn offer_pending_for_test(
        &self,
        revision: u64,
        settings: &AppSettings,
        reindex: bool,
    ) {
        self.offer(ConfigRequest {
            revision,
            settings: settings.clone(),
            reindex,
        });
    }

    /// Probe state for `key`: the stored classification when the stored route key equals `key`,
    /// otherwise Untested (DESIGN 3.1).
    pub(crate) fn probe_state_for(
        &self,
        key: &crate::managers::converse_platform::RouteKey,
    ) -> crate::managers::converse_platform::ToolSupport {
        use crate::managers::converse_platform::ToolSupport;
        match lock(&self.probe).as_ref() {
            Some((stored, support)) if stored == key => *support,
            _ => ToolSupport::Untested,
        }
    }

    /// Stores a probe result together with the route key it was taken for (DESIGN 3.6).
    pub(crate) fn record_probe(
        &self,
        key: crate::managers::converse_platform::RouteKey,
        support: crate::managers::converse_platform::ToolSupport,
    ) {
        *lock(&self.probe) = Some((key, support));
    }

    /// Number of coordinator threads started by this runtime (test accessor; fixed at one).
    #[cfg(test)]
    pub(crate) fn thread_count(&self) -> usize {
        self.threads.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl VaultRagRuntime {
    /// Publishes a request into the pending slot (DESIGN 3.6 C4). Never blocks on IO.
    fn offer(&self, request: ConfigRequest) {
        let mut slot = lock(&self.pending);
        if beats(slot.as_ref(), &request) {
            *slot = Some(request);
        }
        drop(slot);
        self.wake.notify_all();
    }

    /// True when the runtime is closed or a newer revision exists (DESIGN 3.2).
    fn is_stale(&self, revision: u64) -> bool {
        self.closed.load(Ordering::SeqCst) || self.revision.load(Ordering::SeqCst) != revision
    }

    /// Coordinator thread body: the only place configure and request_refresh run (DESIGN 3.6).
    fn coordinate(&self) {
        while let Some(request) = self.next_request() {
            if request.reindex {
                self.refresh_pass(request.revision);
            } else {
                self.configure_pass(request);
            }
        }
    }

    /// Waits for a pending request; `None` once the runtime is closed. The check and the wait
    /// happen under the slot lock, so a notify cannot be lost.
    fn next_request(&self) -> Option<ConfigRequest> {
        let mut slot = lock(&self.pending);
        loop {
            if self.closed.load(Ordering::SeqCst) {
                return None;
            }
            if let Some(request) = slot.take() {
                return Some(request);
            }
            slot = self.wake.wait(slot).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Configures the index for one revision and publishes it only if it is still current
    /// (DESIGN 3.6 C4 (4)).
    fn configure_pass(&self, request: ConfigRequest) {
        if self.is_stale(request.revision) {
            return;
        }
        let (root, folders) = index_plan(&request.settings);
        let enabled = root.is_some();
        self.backend.configure(root, folders);
        if self.is_stale(request.revision) {
            return;
        }
        *lock(&self.applied_settings) = Some(request.settings);
        self.live.store(enabled, Ordering::SeqCst);
        self.applied.store(request.revision, Ordering::SeqCst);
        if enabled && !self.is_stale(request.revision) {
            self.backend.request_refresh(REFRESH_BUDGET);
        }
    }

    /// Refresh-only pass on the applied configuration (DESIGN 3.6 item 5).
    fn refresh_pass(&self, revision: u64) {
        let applied = self.applied.load(Ordering::SeqCst);
        if self.is_stale(revision) || applied != revision || !self.live.load(Ordering::SeqCst) {
            return;
        }
        self.backend.request_refresh(REFRESH_BUDGET);
    }
}

/// True when `incoming` should replace the pending request (DESIGN 3.6 C4 (2), (3)): a config
/// request beats a reindex, a reindex never beats a config, and the highest revision wins.
fn beats(current: Option<&ConfigRequest>, incoming: &ConfigRequest) -> bool {
    match current {
        None => true,
        Some(current) => match (current.reindex, incoming.reindex) {
            (true, false) => true,
            (false, true) => false,
            _ => incoming.revision >= current.revision,
        },
    }
}

/// Root and folders for the index: a root only when RAG is on, a path is set and the folder list
/// parses. Anything else configures no root, so the runtime is not ready.
fn index_plan(settings: &AppSettings) -> (Option<PathBuf>, Vec<String>) {
    let path = settings.vault_path.trim();
    let folders = parse_rag_folders(&settings.vault_rag_folders).unwrap_or_default();
    if settings.vault_rag_enabled && !path.is_empty() && !folders.is_empty() {
        (Some(PathBuf::from(path)), folders)
    } else {
        (None, Vec::new())
    }
}

/// Locks a mutex, recovering the value if a previous holder panicked.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Status for a refused read: busy and refreshing (DESIGN 3.6).
fn busy_dto() -> VaultIndexStatusDto {
    VaultIndexStatusDto {
        notes: 0,
        chunks: 0,
        skipped: 0,
        last_refresh_ms: None,
        refreshing: true,
        last_error_kind: Some("busy".to_string()),
        tool_support: TOOL_SUPPORT_UNTESTED.to_string(),
    }
}

fn dto_from_status(status: &IndexStatus) -> VaultIndexStatusDto {
    VaultIndexStatusDto {
        notes: status.notes,
        chunks: status.chunks,
        skipped: status.skipped,
        last_refresh_ms: status.last_refresh_ms,
        refreshing: status.refreshing,
        last_error_kind: status.last_error_kind.clone(),
        tool_support: TOOL_SUPPORT_UNTESTED.to_string(),
    }
}

fn dto_from_hit(hit: Hit) -> VaultHitDto {
    VaultHitDto {
        rel: sanitise(&hit.rel),
        heading: sanitise(&hit.heading),
        snippet: sanitise(&hit.snippet),
        note_date: hit.note_ts.and_then(note_date),
    }
}

/// Redacts, then neutralises, one line of text: the same sanitation as the tool search.
fn sanitise(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    neutralize(&redact(&one_line))
}

/// `YYYY-MM-DD` (UTC) for a note timestamp in ms, when it is representable.
fn note_date(ms: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ms).map(|at| at.format("%Y-%m-%d").to_string())
}

/// Builds the managed runtime for the app (DESIGN 3.6 wiring). Returns `None` on any init
/// failure: the data dir, its creation or the coordinator thread. Never blocks: `apply` only
/// queues the persisted settings.
pub fn start_for_app(app: &AppHandle) -> Option<Arc<VaultRagRuntime>> {
    let data_dir = app.path().app_data_dir().ok()?;
    std::fs::create_dir_all(&data_dir).ok()?;
    let index = Arc::new(IndexManager::new(
        data_dir,
        Arc::new(VaultIoWorker::start()),
    ));
    let backend: Arc<dyn IndexBackend> = index.clone();
    let runtime = VaultRagRuntime::with_manager(backend, index, BlockingGate::new(2));
    if runtime.threads.load(Ordering::SeqCst) == 0 {
        return None;
    }
    runtime.apply(&get_settings(app));
    Some(runtime)
}
