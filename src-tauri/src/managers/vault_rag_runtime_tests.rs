//! T2 runtime and coordinator tests (DESIGN Rev 5, 3.2 and 3.6, C4 PROTOCOL). Step S5: written
//! first and red until S6. Races are made deterministic by a fake `IndexBackend` whose configure
//! and refresh can be held at a barrier. No real filesystem timing is involved.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use crate::commands::vault_rag::{preview_inner, reindex_inner, status_inner};
use crate::managers::vault_index::{Hit, IndexStatus, RagError, SearchOpts};
use crate::managers::vault_rag::BlockingGate;
use crate::managers::vault_rag_runtime::{IndexBackend, VaultRagRuntime};
use crate::settings::{get_default_settings, AppSettings};

const ALPHA: &str = "/vault/alpha";
const BRAVO: &str = "/vault/bravo";
const CHARLIE: &str = "/vault/charlie";
const FOLDERS: &str = "Projects,Daily";
const UNAVAILABLE: &str = "vault search is unavailable";
const NONE: &str = "-";
const WAIT: Duration = Duration::from_secs(5);
/// Fake credential, built with concat! so no token-shaped literal exists in this file.
const SECRET: &str = concat!("sk-", "abcdefghijklmnopqrstuvwxyz0123");

/// A one-shot signal: `open` releases every waiter.
#[derive(Clone, Default)]
struct Latch(Arc<(Mutex<bool>, Condvar)>);

impl Latch {
    fn open(&self) {
        let (flag, cv) = &*self.0;
        *flag.lock().unwrap() = true;
        cv.notify_all();
    }

    fn wait(&self) {
        let (flag, cv) = &*self.0;
        let mut open = flag.lock().unwrap();
        while !*open {
            open = cv.wait(open).unwrap();
        }
    }

    fn wait_open(&self, timeout: Duration) -> bool {
        let (flag, cv) = &*self.0;
        let deadline = Instant::now() + timeout;
        let mut open = flag.lock().unwrap();
        while !*open {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (guard, _) = cv.wait_timeout(open, deadline - now).unwrap();
            open = guard;
        }
        true
    }
}

/// A point where the fake reports it was reached (`entered`) and waits for `release`.
#[derive(Clone, Default)]
struct Hold {
    entered: Latch,
    release: Latch,
}

/// Fake index. Serves the content of the last completed configure, rejects stale generations,
/// records every configure and refresh call, and can hold any of them at a barrier.
#[derive(Default)]
struct FakeIndex {
    generation: AtomicU64,
    served: Mutex<Option<String>>,
    configures: Mutex<Vec<String>>,
    refreshes: AtomicUsize,
    threads: Mutex<HashSet<ThreadId>>,
    /// Configure for this root holds after its generation bump, before content is served.
    pause_configure: Mutex<Option<(String, Hold)>>,
    /// The next refresh holds (an IO job that never returns while held).
    hold_refresh: Mutex<Option<Hold>>,
    /// The next search holds while it owns `index_lock` (an index mutex held by a query).
    hold_search: Mutex<Option<Hold>>,
    index_lock: Mutex<()>,
}

impl FakeIndex {
    fn arm_pause(&self, root: &str) -> Hold {
        let hold = Hold::default();
        *self.pause_configure.lock().unwrap() = Some((root.to_string(), hold.clone()));
        hold
    }

    fn arm_refresh_hold(&self) -> Hold {
        let hold = Hold::default();
        *self.hold_refresh.lock().unwrap() = Some(hold.clone());
        hold
    }

    fn arm_search_hold(&self) -> Hold {
        let hold = Hold::default();
        *self.hold_search.lock().unwrap() = Some(hold.clone());
        hold
    }

    fn configure_calls(&self) -> Vec<String> {
        self.configures.lock().unwrap().clone()
    }

    fn refresh_calls(&self) -> usize {
        self.refreshes.load(Ordering::SeqCst)
    }

    fn distinct_threads(&self) -> usize {
        self.threads.lock().unwrap().len()
    }

    fn note_thread(&self) {
        self.threads.lock().unwrap().insert(thread::current().id());
    }
}

fn root_name(root: Option<&Path>) -> String {
    root.map(|p| p.display().to_string())
        .unwrap_or_else(|| NONE.to_string())
}

/// Hits served for a root. Twelve per query (the preview must cap at 8). Each snippet carries a
/// newline and the fake secret, so single-line and redaction are observable.
fn hits_for(root: &str) -> Vec<Hit> {
    let marker = root.rsplit('/').next().unwrap_or("x").to_uppercase();
    (0..12)
        .map(|i| Hit {
            rel: format!("{marker}-{i}.md"),
            heading: "Status".to_string(),
            snippet: format!("{marker} note {i}\n{SECRET}"),
            note_ts: None,
            mtime_ms: 0,
            score: 1.0,
        })
        .collect()
}

impl IndexBackend for FakeIndex {
    fn configure(&self, root: Option<PathBuf>, folders: Vec<String>) -> u64 {
        let _ = folders;
        let name = root_name(root.as_deref());
        let gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.note_thread();
        self.configures.lock().unwrap().push(name.clone());
        let armed = {
            let mut slot = self.pause_configure.lock().unwrap();
            let matches = slot.as_ref().is_some_and(|(root, _)| *root == name);
            if matches {
                slot.take()
            } else {
                None
            }
        };
        if let Some((_, hold)) = armed {
            hold.entered.open();
            hold.release.wait();
        }
        *self.served.lock().unwrap() = if name == NONE { None } else { Some(name) };
        gen
    }

    fn request_refresh(&self, budget: Duration) {
        let _ = budget;
        self.note_thread();
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        let armed = self.hold_refresh.lock().unwrap().take();
        if let Some(hold) = armed {
            hold.entered.open();
            hold.release.wait();
        }
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn status(&self) -> IndexStatus {
        IndexStatus {
            notes: 3,
            chunks: 9,
            skipped: 0,
            sensitive_names: 0,
            last_refresh_ms: Some(1),
            refreshing: false,
            last_error_kind: None,
        }
    }

    fn search(&self, gen: u64, query: &str, opts: &SearchOpts) -> Result<Vec<Hit>, RagError> {
        let _ = (query, opts);
        if gen != self.generation() {
            return Err(RagError::GenerationMismatch);
        }
        let _index = self.index_lock.lock().unwrap();
        let armed = self.hold_search.lock().unwrap().take();
        if let Some(hold) = armed {
            hold.entered.open();
            hold.release.wait();
        }
        let served = self.served.lock().unwrap().clone();
        match served {
            None => Err(RagError::Unavailable),
            Some(name) => Ok(hits_for(&name)),
        }
    }
}

/// Settings for a fully possible RAG configuration over `root` (direct route, realtime on).
fn rag_settings(root: &str) -> AppSettings {
    let mut s = get_default_settings();
    s.exp_rt_enabled = true;
    s.exp_rt_direct_endpoint = true;
    s.vault_rag_enabled = true;
    s.vault_path = root.to_string();
    s.vault_rag_folders = FOLDERS.to_string();
    s
}

fn disabled_settings(root: &str) -> AppSettings {
    let mut s = rag_settings(root);
    s.vault_rag_enabled = false;
    s
}

fn opts() -> SearchOpts {
    SearchOpts {
        folder: None,
        since_days: None,
        limit: 5,
    }
}

fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

fn runtime(fake: &Arc<FakeIndex>, gate: BlockingGate) -> Arc<VaultRagRuntime> {
    let backend: Arc<dyn IndexBackend> = fake.clone();
    VaultRagRuntime::new(backend, gate)
}

fn fresh() -> (Arc<FakeIndex>, Arc<VaultRagRuntime>) {
    let fake = Arc::new(FakeIndex::default());
    let rt = runtime(&fake, BlockingGate::new(2));
    (fake, rt)
}

#[test]
fn rt_revision_bumps_synchronously_on_apply() {
    let (_fake, rt) = fresh();
    let before = rt.revision();
    rt.apply(&rag_settings(ALPHA));
    assert_eq!(
        rt.revision(),
        before + 1,
        "apply bumps the revision before returning"
    );
    rt.apply(&rag_settings(BRAVO));
    assert_eq!(rt.revision(), before + 2);
    rt.shutdown();
}

#[test]
fn rt_not_ready_until_applied() {
    let (fake, rt) = fresh();
    rt.apply(&rag_settings(ALPHA));
    wait_until("alpha applied", || rt.is_ready());
    let rev_a = rt.revision();

    // Hold the configure for B so B stays unapplied while we inspect the runtime.
    let hold = fake.arm_pause(BRAVO);
    let bravo = rag_settings(BRAVO);
    rt.apply(&bravo);
    let rev_b = rt.revision();
    assert!(
        hold.entered.wait_open(WAIT),
        "coordinator reached configure B"
    );
    assert!(!rt.is_ready(), "not ready while B is unapplied");
    assert_eq!(rt.applied_revision(), rev_a);
    let early = rt.session(&bravo);
    assert!(
        !early.still_allowed(&bravo),
        "session started before B applies is not allowed"
    );

    hold.release.open();
    wait_until("bravo applied", || rt.applied_revision() == rev_b);
    assert!(rt.is_ready());
    assert!(rt.session(&bravo).still_allowed(&bravo));
    assert!(
        !early.still_allowed(&bravo),
        "the early session stays revoked"
    );
    rt.shutdown();
}

#[test]
fn rt_stale_configure_never_published() {
    let (fake, rt) = fresh();
    let alpha = rag_settings(ALPHA);
    let bravo = rag_settings(BRAVO);
    let hold = fake.arm_pause(ALPHA);
    rt.apply(&alpha);
    assert!(
        hold.entered.wait_open(WAIT),
        "configure A is past its generation bump"
    );
    let rev_a = rt.revision();

    rt.apply(&bravo);
    let rev_b = rt.revision();
    let started_now = rt.session(&bravo);
    assert!(!started_now.still_allowed(&bravo));
    assert_eq!(rt.preview("status"), Err(UNAVAILABLE.to_string()));
    assert!(!rt.is_ready());

    hold.release.open();
    while rt.applied_revision() != rev_b {
        assert_ne!(
            rt.applied_revision(),
            rev_a,
            "A's revision must never be published"
        );
        match rt.preview("status") {
            Ok(hits) => assert!(
                hits.iter().all(|h| !h.snippet.contains("ALPHA")),
                "no A content may be returned"
            ),
            Err(message) => assert_eq!(message, UNAVAILABLE),
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert!(rt.is_ready());
    assert!(
        !started_now.still_allowed(&bravo),
        "session pinned to A stays revoked"
    );
    rt.shutdown();
}

#[test]
fn rt_pending_slot_keeps_highest_revision() {
    let (fake, rt) = fresh();
    let hold = fake.arm_pause(ALPHA);
    rt.apply(&rag_settings(ALPHA));
    assert!(hold.entered.wait_open(WAIT));
    rt.apply(&rag_settings(BRAVO));
    rt.apply(&rag_settings(CHARLIE));
    let rev_c = rt.revision();
    // A lower revision arrives after a higher one: it must not overwrite the pending slot.
    rt.offer_pending_for_test(rev_c - 1, &rag_settings(BRAVO), false);
    hold.release.open();
    wait_until("charlie applied", || rt.applied_revision() == rev_c);
    assert_eq!(
        fake.configure_calls(),
        vec![ALPHA.to_string(), CHARLIE.to_string()],
        "B was superseded and a lower revision never ran"
    );
    rt.shutdown();
}

#[test]
fn rt_config_beats_pending_reindex() {
    let (fake, rt) = fresh();
    rt.apply(&rag_settings(ALPHA));
    wait_until("alpha ready", || rt.is_ready());
    // Hold the refresh of the first reindex so the next reindex waits in the pending slot.
    let refresh = fake.arm_refresh_hold();
    rt.request_reindex();
    assert!(refresh.entered.wait_open(WAIT), "first reindex is running");
    rt.request_reindex();
    rt.apply(&rag_settings(BRAVO));
    let rev_b = rt.revision();
    refresh.release.open();
    wait_until("bravo applied", || rt.applied_revision() == rev_b);
    assert_eq!(
        fake.configure_calls(),
        vec![ALPHA.to_string(), BRAVO.to_string()],
        "the configuration ran; the pending reindex did not replace it"
    );
    rt.shutdown();
}

#[test]
fn rt_reindex_is_refresh_only() {
    let (fake, rt) = fresh();
    rt.apply(&rag_settings(ALPHA));
    wait_until("alpha ready", || rt.is_ready());
    let rev = rt.revision();
    let gen = fake.generation();
    let configs = fake.configure_calls().len();
    let session = rt.session(&rag_settings(ALPHA));
    assert!(session.still_allowed(&rag_settings(ALPHA)));

    let refreshes = fake.refresh_calls();
    rt.request_reindex();
    wait_until("refresh ran", || fake.refresh_calls() > refreshes);
    assert_eq!(rt.revision(), rev, "reindex never bumps the revision");
    assert_eq!(fake.generation(), gen, "reindex never bumps the generation");
    assert_eq!(
        fake.configure_calls().len(),
        configs,
        "reindex never configures"
    );
    assert!(
        session.still_allowed(&rag_settings(ALPHA)),
        "the active session stays valid"
    );
    let hits = session
        .index
        .search(session.generation, "status", &opts())
        .expect("later retrieval still works");
    assert!(!hits.is_empty());
    rt.shutdown();
}

#[test]
fn rt_reindex_ignored_when_not_ready_or_disabled() {
    let (fake, rt) = fresh();
    let before = rt.revision();
    rt.request_reindex();
    thread::sleep(Duration::from_millis(150));
    assert_eq!(fake.refresh_calls(), 0, "never applied: reindex ignored");
    assert!(fake.configure_calls().is_empty());
    assert_eq!(rt.revision(), before, "reindex never bumps the revision");

    rt.apply(&disabled_settings(ALPHA));
    wait_until("disabled applied", || {
        rt.applied_revision() == rt.revision()
    });
    let refreshes = fake.refresh_calls();
    rt.request_reindex();
    thread::sleep(Duration::from_millis(150));
    assert_eq!(fake.refresh_calls(), refreshes, "disabled: reindex ignored");
    rt.shutdown();
}

#[test]
fn rt_coalescing() {
    let (fake, rt) = fresh();
    let hold = fake.arm_pause(ALPHA);
    rt.apply(&rag_settings(ALPHA));
    assert!(hold.entered.wait_open(WAIT));
    let rev_a = rt.revision();
    rt.apply(&rag_settings(BRAVO));
    let rev_b = rt.revision();
    hold.release.open();

    let mut published_a = false;
    while rt.applied_revision() != rev_b {
        published_a |= rt.applied_revision() == rev_a;
        match rt.preview("status") {
            Ok(hits) => assert!(
                hits.iter().all(|h| !h.snippet.contains("ALPHA")),
                "no A evidence after B was accepted"
            ),
            Err(message) => assert_eq!(message, UNAVAILABLE),
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert!(!published_a, "A never publishes");
    assert_eq!(
        fake.configure_calls(),
        vec![ALPHA.to_string(), BRAVO.to_string()],
        "only B publishes after A"
    );
    rt.shutdown();
}

#[test]
fn rt_fixed_thread_count_under_100_repeated_apply_and_reindex() {
    let (fake, rt) = fresh();
    for i in 0..100 {
        let root = if i % 2 == 0 { ALPHA } else { BRAVO };
        rt.apply(&rag_settings(root));
        rt.request_reindex();
    }
    let final_rev = rt.revision();
    wait_until("final revision applied", || {
        rt.applied_revision() == final_rev
    });
    assert_eq!(rt.thread_count(), 1, "exactly one coordinator thread");
    assert!(
        fake.distinct_threads() <= 1,
        "configure and refresh ran on one thread only"
    );
    rt.shutdown();
}

#[test]
fn rt_shutdown_returns_fast_in_three_held_situations() {
    // (i) configure's filesystem step is held.
    {
        let (fake, rt) = fresh();
        let hold = fake.arm_pause(ALPHA);
        rt.apply(&rag_settings(ALPHA));
        assert!(hold.entered.wait_open(WAIT));
        let started = Instant::now();
        rt.shutdown();
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "(i) configure held"
        );
        hold.release.open();
        thread::sleep(Duration::from_millis(200));
        assert!(!rt.is_ready(), "(i) no publish after shutdown");
        assert_ne!(
            rt.applied_revision(),
            rt.revision(),
            "(i) no publish after shutdown"
        );
    }
    // (ii) the index mutex is held by a query.
    {
        let (fake, rt) = fresh();
        rt.apply(&rag_settings(ALPHA));
        wait_until("alpha ready", || rt.is_ready());
        let hold = fake.arm_search_hold();
        let querier = rt.clone();
        let query = thread::spawn(move || querier.preview("status"));
        assert!(
            hold.entered.wait_open(WAIT),
            "(ii) query holds the index mutex"
        );
        let started = Instant::now();
        rt.shutdown();
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "(ii) index mutex held"
        );
        hold.release.open();
        let _ = query.join();
        thread::sleep(Duration::from_millis(100));
        assert!(!rt.is_ready(), "(ii) no publish after shutdown");
        assert_ne!(
            rt.applied_revision(),
            rt.revision(),
            "(ii) no publish after shutdown"
        );
    }
    // (iii) an IO job is held forever.
    {
        let (fake, rt) = fresh();
        let hold = fake.arm_refresh_hold();
        rt.apply(&rag_settings(ALPHA));
        assert!(hold.entered.wait_open(WAIT), "(iii) refresh job is running");
        let started = Instant::now();
        rt.shutdown();
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "(iii) IO job held"
        );
        hold.release.open();
        thread::sleep(Duration::from_millis(200));
        assert!(!rt.is_ready(), "(iii) no publish after shutdown");
        assert_ne!(
            rt.applied_revision(),
            rt.revision(),
            "(iii) no publish after shutdown"
        );
    }
}

#[test]
fn rt_no_publish_after_closed() {
    let (fake, rt) = fresh();
    rt.apply(&rag_settings(ALPHA));
    wait_until("alpha ready", || rt.is_ready());
    rt.shutdown();
    let configs_at_close = fake.configure_calls().len();
    rt.apply(&rag_settings(BRAVO));
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        fake.configure_calls().len(),
        configs_at_close,
        "no configure runs after close"
    );
    assert!(!rt.is_ready());
    assert_ne!(rt.applied_revision(), rt.revision());
}

#[test]
fn rt_init_failure_none_state_outcomes() {
    let status = status_inner(None);
    assert_eq!(status.notes, 0);
    assert_eq!(status.chunks, 0);
    assert_eq!(status.skipped, 0);
    assert_eq!(status.last_refresh_ms, None);
    assert!(!status.refreshing);
    assert_eq!(status.last_error_kind.as_deref(), Some("unavailable"));

    let settings = rag_settings(ALPHA);
    assert_eq!(
        preview_inner(&settings, None, "status"),
        Err(UNAVAILABLE.to_string())
    );
    assert_eq!(reindex_inner(&settings, None), Err(UNAVAILABLE.to_string()));
}

#[test]
fn rt_status_busy_when_gate_saturated() {
    let gate = BlockingGate::new(2);
    let fake = Arc::new(FakeIndex::default());
    let rt = runtime(&fake, gate.clone());
    let release = Latch::default();
    let mut entered = Vec::new();
    let mut holders = Vec::new();
    for _ in 0..2 {
        let g = gate.clone();
        let e = Latch::default();
        entered.push(e.clone());
        let r = release.clone();
        holders.push(thread::spawn(move || {
            tauri::async_runtime::block_on(g.run(Duration::from_secs(30), move || {
                e.open();
                r.wait();
            }))
        }));
    }
    for e in &entered {
        assert!(e.wait_open(WAIT), "both permits are held");
    }
    let started = Instant::now();
    let dto = rt.status_dto();
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "status returns promptly"
    );
    assert_eq!(dto.last_error_kind.as_deref(), Some("busy"));
    assert!(dto.refreshing);
    release.open();
    for holder in holders {
        holder
            .join()
            .expect("holder thread")
            .expect("gated work completed");
    }
}

#[test]
fn rt_status_applying_when_not_ready() {
    let (fake, rt) = fresh();
    let hold = fake.arm_pause(ALPHA);
    rt.apply(&rag_settings(ALPHA));
    assert!(hold.entered.wait_open(WAIT));
    let dto = rt.status_dto();
    assert_eq!(dto.last_error_kind.as_deref(), Some("applying"));
    assert!(dto.refreshing);
    hold.release.open();
    rt.shutdown();
}

#[test]
fn rt_preview_unavailable_when_not_ready() {
    let (fake, rt) = fresh();
    let hold = fake.arm_pause(ALPHA);
    rt.apply(&rag_settings(ALPHA));
    assert!(hold.entered.wait_open(WAIT));
    assert_eq!(rt.preview("status"), Err(UNAVAILABLE.to_string()));
    hold.release.open();
    rt.shutdown();
}

#[test]
fn rt_preview_requires_enabled_and_is_capped_at_8_and_sanitised() {
    let (_fake, rt) = fresh();
    rt.apply(&rag_settings(ALPHA));
    wait_until("alpha ready", || rt.is_ready());
    let hits = rt.preview("status").expect("enabled and ready");
    assert!(!hits.is_empty());
    assert!(hits.len() <= 8, "at most 8 hits, got {}", hits.len());
    for hit in &hits {
        assert!(!hit.snippet.contains(SECRET), "secret is redacted");
        assert!(!hit.snippet.contains('\n'), "snippet is single-line");
    }

    rt.apply(&disabled_settings(ALPHA));
    wait_until("disabled applied", || {
        rt.applied_revision() == rt.revision()
    });
    assert!(rt.preview("status").is_err(), "preview requires enabled");
    rt.shutdown();
}

/// Releases every held barrier and stops the runtime when a test ends, including on panic.
struct ReleaseAll {
    rt: Arc<VaultRagRuntime>,
    holds: Vec<Hold>,
}

impl Drop for ReleaseAll {
    fn drop(&mut self) {
        for hold in &self.holds {
            hold.release.open();
        }
        self.rt.shutdown();
    }
}

#[test]
fn rt_stale_configure_is_not_published_even_transiently() {
    let (fake, rt) = fresh();
    let mut releases = ReleaseAll {
        rt: Arc::clone(&rt),
        holds: Vec::new(),
    };

    // Configure A pauses after its generation bump, before it could publish.
    let hold_a = fake.arm_pause(ALPHA);
    releases.holds.push(hold_a.clone());
    rt.apply(&rag_settings(ALPHA));
    assert!(
        hold_a.entered.wait_open(WAIT),
        "configure A is past its generation bump"
    );
    let rev_a = rt.revision();

    // B is accepted while A is in flight, and B's configure is held as well.
    let hold_b = fake.arm_pause(BRAVO);
    releases.holds.push(hold_b.clone());
    let bravo = rag_settings(BRAVO);
    rt.apply(&bravo);
    let rev_b = rt.revision();
    assert_ne!(rev_a, rev_b, "B has a newer revision than A");

    // Release only A. The coordinator must drop A's result and start B's configure.
    hold_a.release.open();
    assert!(
        hold_b.entered.wait_open(WAIT),
        "coordinator started configure B after A returned"
    );

    // While B is still held, A's result must be invisible.
    assert_ne!(
        rt.applied_revision(),
        rev_a,
        "A's revision must not be published while B is held"
    );
    assert!(!rt.is_ready(), "not ready while B is unapplied");
    let session = rt.session(&bravo);
    assert!(
        !session.still_allowed(&bravo),
        "a session started while B is unapplied is not allowed"
    );
    assert_eq!(
        fake.refresh_calls(),
        0,
        "no refresh was requested for stale A"
    );

    // Release B and wait for it to apply.
    hold_b.release.open();
    wait_until("bravo applied", || rt.applied_revision() == rev_b);
    assert_eq!(rt.applied_revision(), rt.revision());
    assert!(rt.is_ready());

    releases.holds.clear();
    rt.shutdown();
}
