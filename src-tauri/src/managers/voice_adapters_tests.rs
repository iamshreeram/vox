//! Tests for the service-backed turn detector and speaker gate (design D3b, D6, A2', A8').
//!
//! They drive the real `VoiceModelService` with a fake `SessionLoader`, so no ONNX model is
//! needed. Every wait is bounded by `GLOBAL_TIMEOUT`; the longest deliberate wait is one
//! speaker deadline (`VERIFY_DEADLINE`, 500 ms).

use super::*;
use crate::managers::speaker_id::{enroll, VoiceprintStore, SPEAKER_SPEC, VERIFY_DEADLINE};
use crate::managers::voice_models::{ModelKind, ModelSession, SessionLoader, VoiceModelService};
use crate::settings::SpeakerSensitivity;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tempfile::TempDir;

const GLOBAL_TIMEOUT: Duration = Duration::from_secs(3);
const TURN_DEADLINE: Duration = Duration::from_millis(250);
const TAIL_SAMPLES: usize = 16_000;
const MS_2000_SAMPLES: usize = 32_000;
const MS_1999_SAMPLES: usize = 31_984;
const SHA: &str = SPEAKER_SPEC.sha256;

type Script = Arc<dyn Fn(&[f32]) -> Option<Vec<f32>> + Send + Sync>;
type Sensitivity = Arc<dyn Fn() -> SpeakerSensitivity + Send + Sync>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Polls `cond` until it holds or the global timeout passes.
fn wait_until(cond: impl Fn() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < GLOBAL_TIMEOUT {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    cond()
}

fn script(f: impl Fn(&[f32]) -> Option<Vec<f32>> + Send + Sync + 'static) -> Script {
    Arc::new(f)
}

fn constant(values: Vec<f32>) -> Script {
    script(move |_| Some(values.clone()))
}

fn silent() -> Script {
    script(|_| None)
}

fn fixed(level: SpeakerSensitivity) -> Sensitivity {
    Arc::new(move || level)
}

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

/// A one-shot park point: the session blocks in `wait` until the test calls `open`.
#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    cv: Condvar,
    entered: AtomicUsize,
}

impl Gate {
    fn wait(&self) {
        self.entered.fetch_add(1, SeqCst);
        let mut open = lock(&self.open);
        while !*open {
            open = self.cv.wait(open).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn open(&self) {
        *lock(&self.open) = true;
        self.cv.notify_all();
    }

    fn entered(&self) -> usize {
        self.entered.load(SeqCst)
    }
}

/// A script that parks the session until the gate opens, then answers with `answer`.
fn gated(gate: &Arc<Gate>, answer: Vec<f32>) -> Script {
    let gate = Arc::clone(gate);
    script(move |_| {
        gate.wait();
        Some(answer.clone())
    })
}

#[derive(Default)]
struct Runs {
    turn: AtomicUsize,
    speaker: AtomicUsize,
}

/// Hands out fake sessions. Each session counts its own runs and calls its script.
struct FakeLoader {
    turn: Script,
    speaker: Script,
    runs: Arc<Runs>,
}

impl SessionLoader for FakeLoader {
    fn load(&self, kind: ModelKind, _path: &Path) -> Option<Box<dyn ModelSession>> {
        let script = match kind {
            ModelKind::SmartTurn => Arc::clone(&self.turn),
            ModelKind::Speaker => Arc::clone(&self.speaker),
        };
        Some(Box::new(FakeSession {
            kind,
            script,
            runs: Arc::clone(&self.runs),
        }))
    }
}

struct FakeSession {
    kind: ModelKind,
    script: Script,
    runs: Arc<Runs>,
}

impl ModelSession for FakeSession {
    fn run(&mut self, audio16k: &[f32]) -> Option<Vec<f32>> {
        let counter = match self.kind {
            ModelKind::SmartTurn => &self.runs.turn,
            ModelKind::Speaker => &self.runs.speaker,
        };
        counter.fetch_add(1, SeqCst);
        (self.script)(audio16k)
    }
}

struct Harness {
    service: Arc<VoiceModelService>,
    runs: Arc<Runs>,
}

/// Real service, both models installed (paths are never read by the fake loader).
fn harness(turn: Script, speaker: Script) -> Harness {
    let runs = Arc::new(Runs::default());
    let loader = FakeLoader {
        turn,
        speaker,
        runs: Arc::clone(&runs),
    };
    let service = Arc::new(VoiceModelService::new(Arc::new(loader)));
    service.install(ModelKind::SmartTurn, PathBuf::from("fake-smart-turn.onnx"));
    service.install(ModelKind::Speaker, PathBuf::from("fake-speaker.onnx"));
    Harness { service, runs }
}

// ---------------------------------------------------------------------------------------------
// ServiceTurnDetector
// ---------------------------------------------------------------------------------------------

fn tail() -> Vec<f32> {
    vec![0.01; TAIL_SAMPLES]
}

fn turn_detector(h: &Harness) -> ServiceTurnDetector {
    ServiceTurnDetector::new(Arc::clone(&h.service))
}

fn assert_close(actual: Option<f32>, want: f32) {
    let got = actual.expect("expected a probability");
    assert!((got - want).abs() < 1e-6, "got {got}, want {want}");
}

#[test]
fn complete_probability_passes_through() {
    let h = harness(constant(vec![0.9]), silent());
    let got = turn_detector(&h).done_probability(&tail(), TURN_DEADLINE, &no_cancel());
    assert_close(got, 0.9);
}

#[test]
fn incomplete_probability_passes_through() {
    let h = harness(constant(vec![0.1]), silent());
    let got = turn_detector(&h).done_probability(&tail(), TURN_DEADLINE, &no_cancel());
    assert_close(got, 0.1);
}

#[test]
fn out_of_range_probability_is_clamped() {
    for (raw, want) in [(2.0_f32, 1.0_f32), (-1.0, 0.0)] {
        let h = harness(constant(vec![raw]), silent());
        let got = turn_detector(&h).done_probability(&tail(), TURN_DEADLINE, &no_cancel());
        assert_close(got, want);
    }
}

#[test]
fn blocked_session_gives_none_within_deadline_and_abandons_the_request() {
    let gate = Arc::new(Gate::default());
    let h = harness(gated(&gate, vec![0.9]), silent());
    let deadline = Duration::from_millis(50);

    let started = Instant::now();
    let got = turn_detector(&h).done_probability(&tail(), deadline, &no_cancel());
    let took = started.elapsed();

    assert_eq!(got, None);
    assert!(
        took < deadline + Duration::from_millis(30),
        "returned after {took:?}"
    );
    assert!(wait_until(|| gate.entered() == 1), "session never started");
    assert!(h.service.outstanding(), "blocked request is still running");
    gate.open();
    assert!(
        wait_until(|| !h.service.outstanding()),
        "request must clear after the session is released"
    );
}

#[test]
fn cancel_mid_wait_gives_none_within_40_ms() {
    let gate = Arc::new(Gate::default());
    let h = harness(gated(&gate, vec![0.9]), silent());
    let cancel = Arc::new(AtomicBool::new(false));
    let setter_flag = Arc::clone(&cancel);
    let setter = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        let set_at = Instant::now();
        setter_flag.store(true, SeqCst);
        set_at
    });

    let got = turn_detector(&h).done_probability(&tail(), Duration::from_secs(1), &cancel);
    let returned = Instant::now();
    let set_at = setter.join().expect("setter thread");

    assert_eq!(got, None);
    let lag = returned.duration_since(set_at);
    assert!(
        lag < Duration::from_millis(40),
        "returned {lag:?} after cancel"
    );
    assert!(wait_until(|| gate.entered() == 1));
    gate.open();
    assert!(wait_until(|| !h.service.outstanding()));
}

#[test]
fn failing_sessions_give_none() {
    let cases: Vec<(&str, Script)> = vec![
        ("no output", silent()),
        ("panic", script(|_| panic!("fake session panic"))),
        ("wrong length", constant(vec![0.1, 0.2])),
        ("empty output", constant(vec![])),
        ("nan", constant(vec![f32::NAN])),
        ("infinite", constant(vec![f32::INFINITY])),
    ];
    for (name, turn) in cases {
        let h = harness(turn, silent());
        let got = turn_detector(&h).done_probability(&tail(), TURN_DEADLINE, &no_cancel());
        assert_eq!(got, None, "case: {name}");
    }
}

#[test]
fn missing_model_gives_none_immediately() {
    let h = harness(constant(vec![0.9]), silent());
    h.service.uninstall(ModelKind::SmartTurn);
    let started = Instant::now();
    let got = turn_detector(&h).done_probability(&tail(), TURN_DEADLINE, &no_cancel());
    assert_eq!(got, None);
    assert!(started.elapsed() < Duration::from_millis(50));
    assert_eq!(h.runs.turn.load(SeqCst), 0);
}

#[test]
fn empty_tail_gives_none_without_submission() {
    let h = harness(constant(vec![0.9]), silent());
    let got = turn_detector(&h).done_probability(&[], TURN_DEADLINE, &no_cancel());
    assert_eq!(got, None);
    assert!(!h.service.outstanding());
    assert_eq!(h.runs.turn.load(SeqCst), 0);
}

#[test]
fn repeated_timeouts_reuse_the_single_worker_thread() {
    let slow = script(|_| {
        std::thread::sleep(Duration::from_millis(30));
        Some(vec![0.5])
    });
    let h = harness(slow, silent());
    let det = turn_detector(&h);
    for round in 0..10 {
        let got = det.done_probability(&tail(), Duration::from_millis(5), &no_cancel());
        assert_eq!(got, None, "round {round}");
        assert!(
            wait_until(|| !h.service.outstanding()),
            "round {round} must clear"
        );
    }
    assert_eq!(h.service.worker_thread_ids_seen(), 1);
}

// ---------------------------------------------------------------------------------------------
// ServiceSpeakerGate
// ---------------------------------------------------------------------------------------------

fn base_vector() -> Vec<f32> {
    (0..SPEAKER_SPEC.dim)
        .map(|i| (i * 37 % 101) as f32 - 50.0)
        .collect()
}

/// Enrolls one voiceprint through the real store. Returns the store and its embedding.
fn enrolled(root: &TempDir) -> (Arc<VoiceprintStore>, Vec<f32>) {
    let store = Arc::new(VoiceprintStore::new(root.path().join("voice")));
    let base = base_vector();
    let clips: Vec<Vec<f32>> = (1..=3)
        .map(|k| {
            base.iter()
                .enumerate()
                .map(|(i, b)| b + 3.0 * ((i * (k + 3) * 11 % 7) as f32 - 3.0))
                .collect()
        })
        .collect();
    let vp = enroll(&clips, SHA).expect("enroll");
    store.save(&vp).expect("save");
    let embedding = store.load().expect("load").embedding.clone();
    (store, embedding)
}

fn speech(samples: usize) -> Vec<f32> {
    vec![0.02; samples]
}

/// Unit vector orthogonal to `v` (projection of a fixed probe, then normalised).
fn orthogonal_to(v: &[f32]) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    let unit: Vec<f32> = v.iter().map(|x| x / norm).collect();
    let mut probe = vec![0.0_f32; v.len()];
    probe[3] = 1.0;
    probe[17] = 0.5;
    let dot: f32 = probe.iter().zip(&unit).map(|(p, u)| p * u).sum();
    let mut w: Vec<f32> = probe.iter().zip(&unit).map(|(p, u)| p - dot * u).collect();
    let w_norm = w.iter().map(|x| x * x).sum::<f32>().sqrt();
    for x in &mut w {
        *x /= w_norm;
    }
    w
}

/// Embedding whose cosine with `v` is exactly `c` (up to f32 rounding).
fn at_cosine(v: &[f32], c: f32) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    let unit: Vec<f32> = v.iter().map(|x| x / norm).collect();
    let w = orthogonal_to(v);
    let s = (1.0 - c * c).sqrt();
    unit.iter().zip(&w).map(|(u, w)| c * u + s * w).collect()
}

fn speaker_gate(
    h: &Harness,
    store: Arc<VoiceprintStore>,
    sensitivity: Sensitivity,
) -> ServiceSpeakerGate {
    ServiceSpeakerGate::new(Arc::clone(&h.service), store, sensitivity, SHA.to_string())
}

#[test]
fn not_enrolled_accepts_without_any_inference() {
    let root = TempDir::new().expect("tempdir");
    let store = Arc::new(VoiceprintStore::new(root.path().join("voice")));
    let h = harness(silent(), constant(vec![0.0; SPEAKER_SPEC.dim]));
    let gate = speaker_gate(&h, store, fixed(SpeakerSensitivity::Balanced));
    let result = gate.check(&speech(MS_2000_SAMPLES), &no_cancel());
    assert_eq!(
        result,
        GateResult {
            verdict: Verdict::Accept,
            verify_ms: None
        }
    );
    assert_eq!(h.runs.speaker.load(SeqCst), 0);
    assert!(!h.service.outstanding());
}

#[test]
fn buffer_of_1999_ms_is_skipped_without_inference() {
    let root = TempDir::new().expect("tempdir");
    let (store, owner) = enrolled(&root);
    let h = harness(silent(), constant(owner));
    let gate = speaker_gate(&h, store, fixed(SpeakerSensitivity::Balanced));
    let result = gate.check(&speech(MS_1999_SAMPLES), &no_cancel());
    assert_eq!(
        result,
        GateResult {
            verdict: Verdict::SkippedTooShort,
            verify_ms: None
        }
    );
    assert_eq!(h.runs.speaker.load(SeqCst), 0);
}

#[test]
fn buffer_of_2000_ms_same_voice_is_accepted_with_timing() {
    let root = TempDir::new().expect("tempdir");
    let (store, owner) = enrolled(&root);
    let h = harness(silent(), constant(owner.clone()));
    let gate = speaker_gate(&h, store, fixed(SpeakerSensitivity::Balanced));
    let result = gate.check(&speech(MS_2000_SAMPLES), &no_cancel());
    assert_eq!(result.verdict, Verdict::Accept);
    assert!(result.verify_ms.is_some(), "an embedding was computed");
    assert_eq!(h.runs.speaker.load(SeqCst), 1);
}

#[test]
fn orthogonal_voice_is_rejected_at_balanced() {
    let root = TempDir::new().expect("tempdir");
    let (store, owner) = enrolled(&root);
    let h = harness(silent(), constant(orthogonal_to(&owner)));
    let gate = speaker_gate(&h, store, fixed(SpeakerSensitivity::Balanced));
    let result = gate.check(&speech(MS_2000_SAMPLES), &no_cancel());
    assert_eq!(result.verdict, Verdict::Reject);
    assert!(result.verify_ms.is_some());
}

#[test]
fn owner_accepted_by_lenient_and_rejected_by_strict() {
    let root = TempDir::new().expect("tempdir");
    let (store, owner) = enrolled(&root);
    // 0.66 sits between Lenient (0.59) and Balanced (0.69).
    let h = harness(silent(), constant(at_cosine(&owner, 0.66)));
    let lenient = speaker_gate(&h, Arc::clone(&store), fixed(SpeakerSensitivity::Lenient));
    assert_eq!(
        lenient
            .check(&speech(MS_2000_SAMPLES), &no_cancel())
            .verdict,
        Verdict::Accept
    );
    let balanced = speaker_gate(&h, Arc::clone(&store), fixed(SpeakerSensitivity::Balanced));
    assert_eq!(
        balanced
            .check(&speech(MS_2000_SAMPLES), &no_cancel())
            .verdict,
        Verdict::Reject
    );
    let strict = speaker_gate(&h, store, fixed(SpeakerSensitivity::Strict));
    assert_eq!(
        strict.check(&speech(MS_2000_SAMPLES), &no_cancel()).verdict,
        Verdict::Reject
    );
}

#[test]
fn sensitivity_closure_is_read_on_every_check() {
    let root = TempDir::new().expect("tempdir");
    let (store, owner) = enrolled(&root);
    let h = harness(silent(), constant(at_cosine(&owner, 0.66)));
    let level = Arc::new(Mutex::new(SpeakerSensitivity::Lenient));
    let reader = Arc::clone(&level);
    let sensitivity: Sensitivity = Arc::new(move || *lock(&reader));
    let gate = speaker_gate(&h, store, sensitivity);

    assert_eq!(
        gate.check(&speech(MS_2000_SAMPLES), &no_cancel()).verdict,
        Verdict::Accept
    );
    *lock(&level) = SpeakerSensitivity::Strict;
    assert_eq!(
        gate.check(&speech(MS_2000_SAMPLES), &no_cancel()).verdict,
        Verdict::Reject
    );
}

#[test]
fn voiceprint_deleted_during_inference_gives_accept() {
    let root = TempDir::new().expect("tempdir");
    let (store, owner) = enrolled(&root);
    let park = Arc::new(Gate::default());
    // Orthogonal embedding: without the delete this would be a Reject.
    let h = harness(silent(), gated(&park, orthogonal_to(&owner)));
    let gate = Arc::new(speaker_gate(
        &h,
        Arc::clone(&store),
        fixed(SpeakerSensitivity::Balanced),
    ));
    let checker = {
        let gate = Arc::clone(&gate);
        std::thread::spawn(move || gate.check(&speech(MS_2000_SAMPLES), &no_cancel()))
    };
    assert!(
        wait_until(|| park.entered() == 1),
        "inference never started"
    );
    store.delete().expect("delete during verification");
    park.open();
    let result = checker.join().expect("check thread");
    assert_eq!(result.verdict, Verdict::Accept);
    assert!(result.verify_ms.is_some(), "an embedding was computed");
}

#[test]
fn speaker_timeout_gives_accept_without_verify_ms() {
    let root = TempDir::new().expect("tempdir");
    let (store, owner) = enrolled(&root);
    let park = Arc::new(Gate::default());
    let h = harness(silent(), gated(&park, owner));
    let gate = speaker_gate(&h, store, fixed(SpeakerSensitivity::Balanced));
    let started = Instant::now();
    let result = gate.check(&speech(MS_2000_SAMPLES), &no_cancel());
    let took = started.elapsed();
    assert_eq!(
        result,
        GateResult {
            verdict: Verdict::Accept,
            verify_ms: None
        }
    );
    assert!(
        took < VERIFY_DEADLINE + Duration::from_millis(60),
        "took {took:?}"
    );
    assert!(wait_until(|| park.entered() == 1));
    park.open();
    assert!(wait_until(|| !h.service.outstanding()));
}

#[test]
fn unavailable_speaker_model_accepts_immediately() {
    let root = TempDir::new().expect("tempdir");
    let (store, owner) = enrolled(&root);
    let h = harness(silent(), constant(owner));
    h.service.uninstall(ModelKind::Speaker);
    let gate = speaker_gate(&h, store, fixed(SpeakerSensitivity::Balanced));
    let result = gate.check(&speech(MS_2000_SAMPLES), &no_cancel());
    assert_eq!(
        result,
        GateResult {
            verdict: Verdict::Accept,
            verify_ms: None
        }
    );
    assert_eq!(h.runs.speaker.load(SeqCst), 0);
}

#[test]
fn cancel_during_speaker_check_gives_accept_without_verify_ms() {
    let root = TempDir::new().expect("tempdir");
    let (store, owner) = enrolled(&root);
    let park = Arc::new(Gate::default());
    let h = harness(silent(), gated(&park, owner));
    let gate = speaker_gate(&h, store, fixed(SpeakerSensitivity::Balanced));
    let cancel = Arc::new(AtomicBool::new(false));
    let setter_flag = Arc::clone(&cancel);
    let setter = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        let set_at = Instant::now();
        setter_flag.store(true, SeqCst);
        set_at
    });
    let result = gate.check(&speech(MS_2000_SAMPLES), &cancel);
    let returned = Instant::now();
    let set_at = setter.join().expect("setter thread");
    assert_eq!(
        result,
        GateResult {
            verdict: Verdict::Accept,
            verify_ms: None
        }
    );
    let lag = returned.duration_since(set_at);
    assert!(
        lag < Duration::from_millis(40),
        "returned {lag:?} after cancel"
    );
    assert!(wait_until(|| park.entered() == 1));
    park.open();
    assert!(wait_until(|| !h.service.outstanding()));
}

#[test]
fn failed_speaker_session_gives_accept_without_verify_ms() {
    let root = TempDir::new().expect("tempdir");
    let (store, _owner) = enrolled(&root);
    let failing: Vec<(&str, Script)> = vec![
        ("no output", silent()),
        ("panic", script(|_| panic!("fake speaker panic"))),
    ];
    for (name, speaker) in failing {
        let h = harness(silent(), speaker);
        let gate = speaker_gate(&h, Arc::clone(&store), fixed(SpeakerSensitivity::Balanced));
        let result = gate.check(&speech(MS_2000_SAMPLES), &no_cancel());
        assert_eq!(
            result,
            GateResult {
                verdict: Verdict::Accept,
                verify_ms: None
            },
            "case: {name}"
        );
    }
}

#[test]
fn malformed_embedding_gives_accept_and_counts_as_computed() {
    let root = TempDir::new().expect("tempdir");
    let (store, _owner) = enrolled(&root);
    let h = harness(silent(), constant(vec![0.5; 10]));
    let gate = speaker_gate(&h, store, fixed(SpeakerSensitivity::Balanced));
    let result = gate.check(&speech(MS_2000_SAMPLES), &no_cancel());
    assert_eq!(result.verdict, Verdict::Accept);
    assert!(result.verify_ms.is_some(), "the model did produce output");
}
