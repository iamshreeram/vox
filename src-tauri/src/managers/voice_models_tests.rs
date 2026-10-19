//! Authoritative lifecycle tests for `VoiceModelService` (design A2' / gate G4).
//!
//! Red until the S8 implementation: every test drives the public contract only,
//! using fake loaders/sessions. No test sleeps longer than 300 ms; every wait is
//! a polling loop bounded by a 3 s global timeout.

use super::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Barrier, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const GLOBAL_TIMEOUT: Duration = Duration::from_secs(3);
const CAP: usize = 128_000;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// Gate: a switchable barrier. While armed, `enter` parks the caller until
// `release`; `wait_entered` lets the test know the caller is parked.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct GateState {
    armed: bool,
    released: bool,
    entered: usize,
}

#[derive(Default)]
struct Gate {
    state: Mutex<GateState>,
    cv: Condvar,
}

impl Gate {
    fn arm(&self) {
        let mut s = lock(&self.state);
        s.armed = true;
        s.released = false;
    }

    fn release(&self) {
        lock(&self.state).released = true;
        self.cv.notify_all();
    }

    fn enter(&self) {
        let mut s = lock(&self.state);
        if !s.armed {
            return;
        }
        s.entered += 1;
        self.cv.notify_all();
        while !s.released {
            s = self.cv.wait(s).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn wait_entered(&self, n: usize) {
        let deadline = Instant::now() + GLOBAL_TIMEOUT;
        let mut s = lock(&self.state);
        while s.entered < n {
            let now = Instant::now();
            assert!(now < deadline, "gate: inference never entered (wanted {n})");
            let (next, _) = self
                .cv
                .wait_timeout(s, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            s = next;
        }
    }
}

// ---------------------------------------------------------------------------
// Fake loader + sessions.
// ---------------------------------------------------------------------------

enum Outcome {
    Output(Vec<f32>),
    NoOutput,
    Panic,
}

/// (global run index, kind, input) -> outcome. Runs on the worker thread.
type Plan = Box<dyn Fn(usize, ModelKind, &[f32]) -> Outcome + Send + Sync>;
/// load index (0-based) -> fail?
type FailLoad = Box<dyn Fn(usize) -> bool + Send + Sync>;

struct Fake {
    plan: Plan,
    fail_load: FailLoad,
    load_gate: Arc<Gate>,
    run_gate: Arc<Gate>,
    load_calls: AtomicUsize,
    run_calls: AtomicUsize,
    /// Set when the worker drops a session (i.e. the worker thread has exited).
    exited: AtomicBool,
    loaded: Mutex<Vec<(ModelKind, PathBuf)>>,
    inputs: Mutex<Vec<(ModelKind, Vec<f32>)>>,
}

impl Fake {
    fn with(
        plan: Plan,
        fail_load: FailLoad,
        load_gate: Arc<Gate>,
        run_gate: Arc<Gate>,
    ) -> Arc<Self> {
        Arc::new(Self {
            plan,
            fail_load,
            load_gate,
            run_gate,
            load_calls: AtomicUsize::new(0),
            run_calls: AtomicUsize::new(0),
            exited: AtomicBool::new(false),
            loaded: Mutex::new(Vec::new()),
            inputs: Mutex::new(Vec::new()),
        })
    }

    fn new(plan: Plan) -> Arc<Self> {
        Self::with(plan, no_fail(), no_gate(), no_gate())
    }

    fn echo() -> Plan {
        Box::new(|_, _, audio| Outcome::Output(doubled(audio)))
    }

    fn loads(&self) -> usize {
        self.load_calls.load(SeqCst)
    }

    fn runs(&self) -> usize {
        self.run_calls.load(SeqCst)
    }
}

fn no_fail() -> FailLoad {
    Box::new(|_| false)
}

fn no_gate() -> Arc<Gate> {
    Arc::new(Gate::default())
}

/// Sets `Fake::exited` when the worker drops the session it owns.
struct ExitGuard(Arc<Fake>);

impl Drop for ExitGuard {
    fn drop(&mut self) {
        self.0.exited.store(true, SeqCst);
    }
}

struct FakeSession {
    fake: Arc<Fake>,
    kind: ModelKind,
    _exit: ExitGuard,
}

impl ModelSession for FakeSession {
    fn run(&mut self, audio16k: &[f32]) -> Option<Vec<f32>> {
        let index = self.fake.run_calls.fetch_add(1, SeqCst);
        lock(&self.fake.inputs).push((self.kind, audio16k.to_vec()));
        self.fake.run_gate.enter();
        match (self.fake.plan)(index, self.kind, audio16k) {
            Outcome::Output(v) => Some(v),
            Outcome::NoOutput => None,
            Outcome::Panic => panic!("secret-payload-do-not-log"),
        }
    }
}

/// The loader handed to the service (the service owns it as `Arc<dyn SessionLoader>`).
struct Loader(Arc<Fake>);

impl SessionLoader for Loader {
    fn load(&self, kind: ModelKind, path: &Path) -> Option<Box<dyn ModelSession>> {
        let fake = &self.0;
        let index = fake.load_calls.fetch_add(1, SeqCst);
        lock(&fake.loaded).push((kind, path.to_path_buf()));
        fake.load_gate.enter();
        if (fake.fail_load)(index) {
            return None;
        }
        Some(Box::new(FakeSession {
            fake: Arc::clone(fake),
            kind,
            _exit: ExitGuard(Arc::clone(fake)),
        }))
    }
}

fn service(fake: &Arc<Fake>) -> VoiceModelService {
    VoiceModelService::new(Arc::new(Loader(Arc::clone(fake))))
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

fn path_for(kind: ModelKind) -> PathBuf {
    PathBuf::from(format!("/verified/{kind:?}.onnx"))
}

fn audio(n: usize, seed: f32) -> Vec<f32> {
    (0..n).map(|i| seed + i as f32).collect()
}

fn doubled(v: &[f32]) -> Vec<f32> {
    v.iter().map(|x| x * 2.0).collect()
}

fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + GLOBAL_TIMEOUT;
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn wait_reply(svc: &VoiceModelService, id: u64) -> Reply {
    let mut reply = None;
    wait_until("a reply", || {
        reply = svc.poll(id);
        reply.is_some()
    });
    reply.unwrap_or_else(|| panic!("no reply for request {id}"))
}

fn wait_idle(svc: &VoiceModelService) {
    wait_until("service idle", || !svc.outstanding());
}

fn submit_ok(svc: &VoiceModelService, kind: ModelKind, input: Vec<f32>) -> u64 {
    match svc.try_submit(kind, input) {
        Ok(id) => id,
        Err(e) => panic!("expected Ok from try_submit, got {e:?}"),
    }
}

/// Waits for the previous request to finish, then submits.
fn submit_when_idle(svc: &VoiceModelService, kind: ModelKind, input: Vec<f32>) -> u64 {
    wait_idle(svc);
    submit_ok(svc, kind, input)
}

/// Submit, wait for the Output, and check its payload.
fn round_trip(svc: &VoiceModelService, kind: ModelKind, input: Vec<f32>) -> u64 {
    let expected = doubled(&input);
    let id = submit_when_idle(svc, kind, input);
    assert_eq!(wait_reply(svc, id), Reply::Output(expected));
    id
}

// ---------------------------------------------------------------------------
// (1) happy path
// ---------------------------------------------------------------------------

#[test]
fn happy_path_install_submit_poll_output_loads_session_once() {
    let fake = Fake::new(Fake::echo());
    let svc = service(&fake);
    assert!(!svc.is_installed(ModelKind::SmartTurn));
    svc.install(ModelKind::SmartTurn, path_for(ModelKind::SmartTurn));
    assert!(svc.is_installed(ModelKind::SmartTurn));

    let first = round_trip(&svc, ModelKind::SmartTurn, audio(1000, 1.0));
    assert!(first > 0, "request ids are never 0");
    round_trip(&svc, ModelKind::SmartTurn, audio(500, 2.0));

    assert_eq!(fake.loads(), 1, "session must be loaded exactly once");
    assert_eq!(
        lock(&fake.loaded).clone(),
        vec![(ModelKind::SmartTurn, path_for(ModelKind::SmartTurn))],
        "loader must receive the verified installed path"
    );
    assert_eq!(svc.worker_thread_ids_seen(), 1);
}

// ---------------------------------------------------------------------------
// (2) request ids: unique and strictly increasing
// ---------------------------------------------------------------------------

fn id_block() -> Vec<u64> {
    (0..200).map(|_| next_request_id()).collect()
}

fn cycles_on(svc: &VoiceModelService, kind: ModelKind, n: usize) -> Vec<u64> {
    (0..n)
        .map(|i| {
            let input = audio(8, i as f32);
            let expected = doubled(&input);
            let id = submit_when_idle(svc, kind, input);
            assert_eq!(wait_reply(svc, id), Reply::Output(expected));
            id
        })
        .collect()
}

#[test]
fn request_ids_are_unique_and_strictly_increasing_across_threads_and_services() {
    // 8 threads x 200 calls: per-thread strictly increasing, globally unique, never 0.
    let mut all: Vec<u64> = Vec::new();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..8).map(|_| s.spawn(id_block)).collect();
        for h in handles {
            let ids = h.join().unwrap_or_else(|_| panic!("id thread panicked"));
            assert!(
                ids.windows(2).all(|w| w[0] < w[1]),
                "ids from one thread must be strictly increasing"
            );
            assert!(ids.iter().all(|&id| id != 0), "0 is never a request id");
            all.extend(ids);
        }
    });
    assert_eq!(all.len(), 1600);
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), 1600, "request ids must be unique across threads");

    // Two services, sequential alternation: strictly increasing across services.
    let fake = Fake::new(Fake::echo());
    let a = service(&fake);
    let b = service(&fake);
    a.install(ModelKind::SmartTurn, path_for(ModelKind::SmartTurn));
    b.install(ModelKind::Speaker, path_for(ModelKind::Speaker));
    let mut sequential = Vec::new();
    for _ in 0..50 {
        sequential.push(submit_then_id(&a, ModelKind::SmartTurn));
        sequential.push(submit_then_id(&b, ModelKind::Speaker));
    }
    assert!(
        sequential.windows(2).all(|w| w[0] < w[1]),
        "ids must be strictly increasing across two services"
    );

    // Two services driven concurrently: ids unique across both.
    let (ids_a, ids_b) = std::thread::scope(|s| {
        let ha = s.spawn(|| cycles_on(&a, ModelKind::SmartTurn, 50));
        let hb = s.spawn(|| cycles_on(&b, ModelKind::Speaker, 50));
        (
            ha.join()
                .unwrap_or_else(|_| panic!("service A thread panicked")),
            hb.join()
                .unwrap_or_else(|_| panic!("service B thread panicked")),
        )
    });
    let mut concurrent: Vec<u64> = ids_a.into_iter().chain(ids_b).collect();
    assert!(concurrent.iter().all(|&id| id != 0));
    concurrent.sort_unstable();
    concurrent.dedup();
    assert_eq!(
        concurrent.len(),
        100,
        "ids unique across concurrent services"
    );
}

fn submit_then_id(svc: &VoiceModelService, kind: ModelKind) -> u64 {
    let input = audio(4, 1.0);
    let id = submit_when_idle(svc, kind, input);
    wait_reply(svc, id);
    id
}

// ---------------------------------------------------------------------------
// (3) not installed => Unavailable, no thread work
// ---------------------------------------------------------------------------

#[test]
fn not_installed_is_unavailable_and_does_no_worker_work() {
    let fake = Fake::new(Fake::echo());
    let svc = service(&fake);
    assert!(!svc.is_installed(ModelKind::SmartTurn));
    assert_eq!(
        svc.try_submit(ModelKind::SmartTurn, audio(100, 1.0)),
        Err(Unavailable)
    );

    svc.install(ModelKind::Speaker, path_for(ModelKind::Speaker));
    assert_eq!(
        svc.try_submit(ModelKind::SmartTurn, audio(100, 1.0)),
        Err(Unavailable),
        "a kind that was never installed is Unavailable even if the other kind is"
    );
    assert!(!svc.outstanding());
    assert_eq!(fake.loads(), 0, "no session may be loaded");
    assert_eq!(fake.runs(), 0, "no inference may run");
}

// ---------------------------------------------------------------------------
// (4) single outstanding slot, abandon while blocked
// ---------------------------------------------------------------------------

#[test]
fn single_outstanding_slot_holds_until_thread_finishes_even_after_abandon() {
    let run_gate = no_gate();
    run_gate.arm();
    let fake = Fake::with(Fake::echo(), no_fail(), no_gate(), Arc::clone(&run_gate));
    let svc = service(&fake);
    svc.install(ModelKind::SmartTurn, path_for(ModelKind::SmartTurn));

    let a = submit_ok(&svc, ModelKind::SmartTurn, audio(100, 1.0));
    run_gate.wait_entered(1);
    assert!(svc.outstanding());
    assert_eq!(
        svc.try_submit(ModelKind::SmartTurn, audio(50, 9.0)),
        Err(Unavailable),
        "second submit while A is outstanding must be refused, nothing queued"
    );

    let started = Instant::now();
    svc.abandon(a);
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "abandon must not block while the inference is running"
    );
    assert_eq!(
        svc.try_submit(ModelKind::SmartTurn, audio(50, 9.0)),
        Err(Unavailable),
        "abandoned but still running A keeps the slot"
    );
    assert!(svc.outstanding());

    run_gate.release();
    wait_idle(&svc);

    let b_input = audio(60, 3.0);
    let b = submit_ok(&svc, ModelKind::SmartTurn, b_input.clone());
    assert_eq!(wait_reply(&svc, b), Reply::Output(doubled(&b_input)));
    assert_eq!(svc.poll(a), None, "a new submit clears the old reply slot");
    assert_eq!(
        fake.runs(),
        2,
        "only A and B ran; the refused submit did not"
    );
}

// ---------------------------------------------------------------------------
// (5) A/B isolation with abandoned A
// ---------------------------------------------------------------------------

#[test]
fn abandoned_request_never_leaks_into_the_next_session() {
    let run_gate = no_gate();
    run_gate.arm();
    let fake = Fake::with(Fake::echo(), no_fail(), no_gate(), Arc::clone(&run_gate));
    let svc = service(&fake);
    svc.install(ModelKind::SmartTurn, path_for(ModelKind::SmartTurn));
    svc.install(ModelKind::Speaker, path_for(ModelKind::Speaker));

    let a_input = audio(100, 1.0);
    let a = submit_ok(&svc, ModelKind::SmartTurn, a_input.clone());
    run_gate.wait_entered(1);
    svc.abandon(a);
    run_gate.release();
    wait_idle(&svc);

    let b_input = audio(80, 5.0);
    let b = submit_ok(&svc, ModelKind::Speaker, b_input.clone());
    assert_ne!(a, b, "ids are never reused, even across sessions");
    assert_eq!(wait_reply(&svc, b), Reply::Output(doubled(&b_input)));
    assert_eq!(svc.poll(a), None, "A's reply must never be delivered");

    let seen = lock(&fake.inputs).clone();
    assert_eq!(
        seen,
        vec![
            (ModelKind::SmartTurn, a_input),
            (ModelKind::Speaker, b_input),
        ],
        "the session saw exactly the inferences that actually ran"
    );
    assert_eq!(fake.runs(), 2);
}

// ---------------------------------------------------------------------------
// (6) abandon before the thread starts the request => skipped
// ---------------------------------------------------------------------------

#[test]
fn abandon_before_thread_starts_request_skips_the_inference() {
    let load_gate = no_gate();
    load_gate.arm();
    let fake = Fake::with(Fake::echo(), no_fail(), Arc::clone(&load_gate), no_gate());
    let svc = service(&fake);
    svc.install(ModelKind::SmartTurn, path_for(ModelKind::SmartTurn));

    let a = submit_ok(&svc, ModelKind::SmartTurn, audio(100, 1.0));
    load_gate.wait_entered(1); // the thread is inside load(); request A not started
    svc.abandon(a);
    load_gate.release();
    wait_idle(&svc);

    assert_eq!(
        fake.runs(),
        0,
        "an abandoned, not-yet-started request is skipped"
    );
    assert_eq!(svc.poll(a), None);

    round_trip(&svc, ModelKind::SmartTurn, audio(10, 2.0));
    assert_eq!(fake.runs(), 1, "the service keeps working after a skip");
}

// ---------------------------------------------------------------------------
// (7) panic in run: Failed(Panic), reload, latch after 3, install clears
// ---------------------------------------------------------------------------

#[test]
fn panics_reload_a_fresh_session_and_latch_after_three() {
    // Global run indices 0, 2, 4 panic; 1 and 3 succeed; 5+ succeed.
    let plan: Plan = Box::new(|index, _kind, input| {
        if index % 2 == 0 && index < 5 {
            Outcome::Panic
        } else {
            Outcome::Output(doubled(input))
        }
    });
    let fake = Fake::new(plan);
    let svc = service(&fake);
    svc.install(ModelKind::SmartTurn, path_for(ModelKind::SmartTurn));
    let input = audio(20, 1.0);

    // 1st panic.
    let a = submit_ok(&svc, ModelKind::SmartTurn, input.clone());
    let reply = wait_reply(&svc, a);
    assert_eq!(reply, Reply::Failed(ErrKind::Panic));
    assert!(
        !format!("{reply:?}").contains("secret-payload"),
        "the panic payload must not reach the reply"
    );
    wait_until("reload_count == 1", || {
        svc.reload_count(ModelKind::SmartTurn) == 1
    });
    assert!(!svc.is_latched_unavailable(ModelKind::SmartTurn));

    // Next request: fresh session (second load) succeeds.
    round_trip(&svc, ModelKind::SmartTurn, input.clone());
    assert_eq!(
        fake.loads(),
        2,
        "the session is reloaded from the verified path"
    );

    // 2nd panic.
    let c = submit_when_idle(&svc, ModelKind::SmartTurn, input.clone());
    assert_eq!(wait_reply(&svc, c), Reply::Failed(ErrKind::Panic));
    wait_until("reload_count == 2", || {
        svc.reload_count(ModelKind::SmartTurn) == 2
    });
    round_trip(&svc, ModelKind::SmartTurn, input.clone());

    // 3rd panic => latched.
    let e = submit_when_idle(&svc, ModelKind::SmartTurn, input.clone());
    assert_eq!(wait_reply(&svc, e), Reply::Failed(ErrKind::Panic));
    wait_until("latched after 3 panics", || {
        svc.is_latched_unavailable(ModelKind::SmartTurn)
    });
    assert_eq!(svc.reload_count(ModelKind::SmartTurn), 3);
    assert_eq!(
        svc.try_submit(ModelKind::SmartTurn, input.clone()),
        Err(Unavailable),
        "latched kind is Unavailable"
    );
    wait_idle(&svc);
    assert_eq!(fake.runs(), 5, "nothing runs while latched");

    // install() clears the latch and counters and works again.
    svc.install(ModelKind::SmartTurn, path_for(ModelKind::SmartTurn));
    assert!(!svc.is_latched_unavailable(ModelKind::SmartTurn));
    assert_eq!(svc.reload_count(ModelKind::SmartTurn), 0);
    round_trip(&svc, ModelKind::SmartTurn, input);
}

// ---------------------------------------------------------------------------
// (8) loader failure and run-returns-None
// ---------------------------------------------------------------------------

#[test]
fn loader_failure_yields_failed_load_and_counts_as_a_reload() {
    let fake = Fake::with(
        Fake::echo(),
        Box::new(|index| index == 0),
        no_gate(),
        no_gate(),
    );
    let svc = service(&fake);
    svc.install(ModelKind::Speaker, path_for(ModelKind::Speaker));

    let a = submit_ok(&svc, ModelKind::Speaker, audio(30, 1.0));
    assert_eq!(wait_reply(&svc, a), Reply::Failed(ErrKind::LoadFailed));
    wait_until("reload_count == 1", || {
        svc.reload_count(ModelKind::Speaker) == 1
    });

    round_trip(&svc, ModelKind::Speaker, audio(30, 2.0));
    assert_eq!(fake.loads(), 2, "the next request loads again");
}

#[test]
fn run_returning_none_yields_no_output_without_a_reload() {
    let plan: Plan = Box::new(|index, _kind, input| {
        if index == 0 {
            Outcome::NoOutput
        } else {
            Outcome::Output(doubled(input))
        }
    });
    let fake = Fake::new(plan);
    let svc = service(&fake);
    svc.install(ModelKind::SmartTurn, path_for(ModelKind::SmartTurn));

    let a = submit_ok(&svc, ModelKind::SmartTurn, audio(30, 1.0));
    assert_eq!(wait_reply(&svc, a), Reply::Failed(ErrKind::NoOutput));
    wait_idle(&svc);
    assert_eq!(
        svc.reload_count(ModelKind::SmartTurn),
        0,
        "NoOutput is not a reload"
    );
    assert!(!svc.is_latched_unavailable(ModelKind::SmartTurn));

    round_trip(&svc, ModelKind::SmartTurn, audio(30, 2.0));
    assert_eq!(fake.loads(), 1, "the session is kept after NoOutput");
}

// ---------------------------------------------------------------------------
// (9) both kinds on one thread; abandon/timeout cycles create no threads
// ---------------------------------------------------------------------------

#[test]
fn both_kinds_share_one_worker_thread_across_abandon_cycles() {
    let run_gate = no_gate();
    let fake = Fake::with(Fake::echo(), no_fail(), no_gate(), Arc::clone(&run_gate));
    let svc = service(&fake);
    svc.install(ModelKind::SmartTurn, path_for(ModelKind::SmartTurn));
    svc.install(ModelKind::Speaker, path_for(ModelKind::Speaker));

    for i in 0..6 {
        let kind = if i % 2 == 0 {
            ModelKind::SmartTurn
        } else {
            ModelKind::Speaker
        };
        round_trip(&svc, kind, audio(30, i as f32));
    }
    assert_eq!(
        svc.worker_thread_ids_seen(),
        1,
        "alternating kinds: one thread"
    );
    assert_eq!(fake.loads(), 2, "one session per kind, loaded once each");

    for cycle in 0..5usize {
        let kind = if cycle % 2 == 0 {
            ModelKind::Speaker
        } else {
            ModelKind::SmartTurn
        };
        run_gate.arm();
        let id = submit_when_idle(&svc, kind, audio(40, cycle as f32));
        run_gate.wait_entered(cycle + 1);
        // The caller gives up after a short timeout, then abandons.
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(svc.poll(id), None);
        svc.abandon(id);
        run_gate.release();
        wait_idle(&svc);
    }

    assert_eq!(
        svc.worker_thread_ids_seen(),
        1,
        "timeout/abandon cycles create no threads"
    );
    round_trip(&svc, ModelKind::SmartTurn, audio(30, 9.0));
    assert_eq!(fake.loads(), 2, "no reloads caused by abandon cycles");
}

// ---------------------------------------------------------------------------
// (10) input cap keeps the LAST 128000 samples
// ---------------------------------------------------------------------------

#[test]
fn oversized_input_reaches_the_session_as_its_last_128000_samples() {
    let fake = Fake::new(Fake::echo());
    let svc = service(&fake);
    svc.install(ModelKind::SmartTurn, path_for(ModelKind::SmartTurn));

    let input = audio(200_000, 0.0);
    let id = submit_ok(&svc, ModelKind::SmartTurn, input.clone());
    let expected_tail = &input[input.len() - CAP..];
    assert_eq!(wait_reply(&svc, id), Reply::Output(doubled(expected_tail)));

    let seen = lock(&fake.inputs).clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].1.len(), CAP);
    assert_eq!(seen[0].1.as_slice(), expected_tail);
}

#[test]
fn input_of_exactly_the_cap_is_passed_unchanged() {
    let fake = Fake::new(Fake::echo());
    let svc = service(&fake);
    svc.install(ModelKind::SmartTurn, path_for(ModelKind::SmartTurn));

    let input = audio(CAP, 0.5);
    round_trip(&svc, ModelKind::SmartTurn, input.clone());
    let seen = lock(&fake.inputs).clone();
    assert_eq!(seen[0].1, input);
}

// ---------------------------------------------------------------------------
// (11) shutdown and Drop
// ---------------------------------------------------------------------------

#[test]
fn shutdown_and_join_is_true_when_idle() {
    let fake = Fake::new(Fake::echo());
    let svc = service(&fake);
    svc.install(ModelKind::SmartTurn, path_for(ModelKind::SmartTurn));
    round_trip(&svc, ModelKind::SmartTurn, audio(30, 1.0));
    assert!(svc.shutdown_and_join(Duration::from_secs(1)));
}

#[test]
fn shutdown_and_join_is_true_after_releasing_a_blocked_request() {
    let run_gate = no_gate();
    run_gate.arm();
    let fake = Fake::with(Fake::echo(), no_fail(), no_gate(), Arc::clone(&run_gate));
    let svc = service(&fake);
    svc.install(ModelKind::SmartTurn, path_for(ModelKind::SmartTurn));
    let _ = submit_ok(&svc, ModelKind::SmartTurn, audio(30, 1.0));
    run_gate.wait_entered(1);
    run_gate.release();
    assert!(svc.shutdown_and_join(Duration::from_secs(1)));
}

#[test]
fn drop_while_blocked_does_not_hang_and_the_thread_exits_after_release() {
    let run_gate = no_gate();
    run_gate.arm();
    let fake = Fake::with(Fake::echo(), no_fail(), no_gate(), Arc::clone(&run_gate));
    let svc = service(&fake);
    svc.install(ModelKind::SmartTurn, path_for(ModelKind::SmartTurn));
    let _ = submit_ok(&svc, ModelKind::SmartTurn, audio(30, 1.0));
    run_gate.wait_entered(1);

    let started = Instant::now();
    drop(svc);
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "Drop must not join a blocked inference"
    );
    assert!(
        !fake.exited.load(SeqCst),
        "the worker is still inside the blocked inference"
    );

    run_gate.release();
    wait_until("worker thread exit", || fake.exited.load(SeqCst));
}

// ---------------------------------------------------------------------------
// (12) memory: bounded storage after many cycles
// ---------------------------------------------------------------------------

#[test]
fn fifty_cycles_leave_nothing_outstanding_and_at_most_one_stored_reply() {
    let fake = Fake::new(Fake::echo());
    let svc = service(&fake);
    svc.install(ModelKind::SmartTurn, path_for(ModelKind::SmartTurn));
    for i in 0..50 {
        round_trip(&svc, ModelKind::SmartTurn, audio(16, i as f32));
    }
    wait_idle(&svc);
    assert!(!svc.outstanding());
    assert!(
        svc.stored_replies() <= 1,
        "reply storage is bounded to one slot"
    );
}

// ---------------------------------------------------------------------------
// (13) uninstall during an outstanding request
// ---------------------------------------------------------------------------

#[test]
fn uninstall_during_outstanding_request_drops_reply_and_refuses_submits() {
    let run_gate = no_gate();
    run_gate.arm();
    let fake = Fake::with(Fake::echo(), no_fail(), no_gate(), Arc::clone(&run_gate));
    let svc = service(&fake);
    svc.install(ModelKind::SmartTurn, path_for(ModelKind::SmartTurn));

    let a = submit_ok(&svc, ModelKind::SmartTurn, audio(30, 1.0));
    run_gate.wait_entered(1);
    svc.uninstall(ModelKind::SmartTurn);
    assert!(!svc.is_installed(ModelKind::SmartTurn));
    assert!(
        svc.outstanding(),
        "still outstanding until the thread finishes"
    );
    assert_eq!(
        svc.try_submit(ModelKind::SmartTurn, audio(30, 2.0)),
        Err(Unavailable)
    );

    run_gate.release();
    wait_idle(&svc);
    assert_eq!(
        svc.poll(a),
        None,
        "the reply of an uninstalled model is dropped"
    );
    assert_eq!(
        svc.try_submit(ModelKind::SmartTurn, audio(30, 3.0)),
        Err(Unavailable)
    );
}

// ---------------------------------------------------------------------------
// (14) concurrent try_submit while idle: exactly one Ok
// ---------------------------------------------------------------------------

#[test]
fn concurrent_try_submit_while_idle_admits_exactly_one() {
    let run_gate = no_gate();
    run_gate.arm();
    let fake = Fake::with(Fake::echo(), no_fail(), no_gate(), Arc::clone(&run_gate));
    let svc = service(&fake);
    svc.install(ModelKind::SmartTurn, path_for(ModelKind::SmartTurn));

    let barrier = Barrier::new(8);
    let svc_ref = &svc;
    let barrier_ref = &barrier;
    let results: Vec<Result<u64, Unavailable>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..8)
            .map(|i| {
                s.spawn(move || {
                    barrier_ref.wait();
                    svc_ref.try_submit(ModelKind::SmartTurn, audio(10, i as f32))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| panic!("submit thread panicked"))
            })
            .collect()
    });

    let admitted: Vec<u64> = results
        .iter()
        .filter_map(|r| r.as_ref().ok().copied())
        .collect();
    assert_eq!(
        admitted.len(),
        1,
        "exactly one concurrent submit may be admitted"
    );
    assert_eq!(
        results.iter().filter(|r| r.is_err()).count(),
        7,
        "the other seven are Unavailable"
    );

    run_gate.release();
    wait_idle(&svc);
    assert_eq!(fake.runs(), 1, "only the admitted request ran");
    let seen = lock(&fake.inputs).clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        svc.poll(admitted[0]),
        Some(Reply::Output(doubled(&seen[0].1))),
        "the admitted request's reply is the one delivered"
    );
}
