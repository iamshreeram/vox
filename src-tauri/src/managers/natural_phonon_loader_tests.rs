//! Spec for the platform-neutral Phonon engine glue (DESIGN.md Rev 4, P3):
//! `synthesize_with` (speakable-text gate, sanitising, tail trim) and `SnapshotLoader`
//! (verified snapshot -> builder -> drop). Fakes only: no ptts, no real model, no network.
//! Authoritative: do not weaken an assertion; if a test is wrong, report it instead.

use crate::managers::natural_engine::{EngineLoader, SynthEngine};
use crate::managers::natural_model::{
    ModelCoordinator, PHONON_DIR_NAME, PHONON_URL_PREFIX, READY_MARKER,
};
use crate::managers::natural_phonon_core::{synthesize_with, SnapshotLoader};
use crate::managers::natural_snapshot::Snapshot;
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

const SR: usize = 24_000;

fn samples(ms: usize) -> usize {
    SR * ms / 1000
}

// ---- synthesize_with -----------------------------------------------------------------------------

#[test]
fn unspeakable_text_returns_empty_without_calling_say() {
    for text in ["", "   ", "\t\n", "...!?", "\u{1F600}", "--- *** ---"] {
        let calls = Cell::new(0usize);
        let mut say = |_: &str| -> Result<Vec<f32>, String> {
            calls.set(calls.get() + 1);
            Ok(vec![0.5; 100])
        };
        let out = synthesize_with(text, &mut say).unwrap();
        assert!(out.is_empty(), "{text:?} must give no audio");
        assert_eq!(calls.get(), 0, "{text:?} must never reach the engine");
    }
}

#[test]
fn speakable_text_calls_say_exactly_once_with_the_exact_text() {
    let mut seen: Vec<String> = Vec::new();
    let mut say = |text: &str| -> Result<Vec<f32>, String> {
        seen.push(text.to_string());
        Ok(vec![0.5; samples(200)])
    };
    let out = synthesize_with("Hello from Phonon.", &mut say).unwrap();
    assert!(!out.is_empty());
    assert_eq!(seen, vec!["Hello from Phonon.".to_string()]);
}

#[test]
fn say_error_passes_through_as_err() {
    let mut say = |_: &str| -> Result<Vec<f32>, String> { Err("boom".to_string()) };
    let err = synthesize_with("Hello.", &mut say).unwrap_err();
    assert!(err.contains("boom"), "got {err:?}");
}

#[test]
fn non_finite_and_out_of_range_samples_are_sanitised() {
    let raw = vec![f32::NAN, 2.0, -3.0, f32::INFINITY, 0.5, f32::NEG_INFINITY];
    let mut say = |_: &str| -> Result<Vec<f32>, String> { Ok(raw.clone()) };
    let out = synthesize_with("Hello.", &mut say).unwrap();
    assert!(!out.is_empty());
    assert!(out.iter().all(|x| x.is_finite()), "{out:?}");
    assert!(out.iter().all(|x| (-1.0..=1.0).contains(x)), "{out:?}");
    assert_eq!(out[0], 0.0, "NaN becomes silence");
    assert_eq!(out[1], 1.0, "2.0 is clamped to 1.0");
    assert_eq!(out[2], -1.0, "-3.0 is clamped to -1.0");
}

#[test]
fn loud_burst_followed_by_silence_is_trimmed_to_burst_plus_150_ms() {
    let mut pcm = vec![0.5f32; samples(500)];
    pcm.extend(vec![0.0f32; samples(800)]);
    let mut say = move |_: &str| -> Result<Vec<f32>, String> { Ok(pcm.clone()) };
    let out = synthesize_with("Hello.", &mut say).unwrap();
    assert_eq!(out.len(), samples(500) + samples(150));
    assert!(out[..samples(500)].iter().all(|&x| x == 0.5));
}

#[test]
fn all_zero_audio_for_speakable_text_is_an_error() {
    let mut say = |_: &str| -> Result<Vec<f32>, String> { Ok(vec![0.0f32; samples(1000)]) };
    let err = synthesize_with("Hello.", &mut say).unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn empty_audio_for_speakable_text_is_an_error() {
    let mut say = |_: &str| -> Result<Vec<f32>, String> { Ok(Vec::new()) };
    assert!(synthesize_with("Hello.", &mut say).is_err());
}

#[test]
fn all_non_finite_audio_for_speakable_text_is_an_error() {
    // Sanitising turns NaN into silence, and silence trims to nothing: no audible speech.
    let mut say = |_: &str| -> Result<Vec<f32>, String> { Ok(vec![f32::NAN; samples(300)]) };
    assert!(synthesize_with("Hello.", &mut say).is_err());
}

// ---- SnapshotLoader fixtures ----------------------------------------------------------------------

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Tiny fake assets with the real file names (no download, no real model).
const TINY: [(&str, &str); 4] = [
    ("model.q8.gguf", "tiny-model-weights-0001"),
    ("default-voice.safetensors", "tiny-voice-0002-abcdef"),
    ("tokenizer.json", "{\"tiny\":\"tokenizer\"}"),
    ("config.json", "{\"tiny\":true}"),
];

fn tiny_assets() -> Vec<crate::managers::natural_model::AssetSpec> {
    TINY.iter()
        .map(|(file, body)| crate::managers::natural_model::AssetSpec {
            file: file.to_string(),
            url: format!("{PHONON_URL_PREFIX}{file}"),
            size: body.len() as u64,
            sha256: sha(body.as_bytes()),
        })
        .collect()
}

struct Tiny {
    dir: TempDir,
}

impl Tiny {
    fn root(&self) -> PathBuf {
        self.dir.path().join("phonon_models")
    }
    fn install(&self) -> PathBuf {
        self.root().join(PHONON_DIR_NAME)
    }
    fn scratch(&self) -> PathBuf {
        self.dir.path().join("scratch")
    }
    fn coord(&self) -> Arc<ModelCoordinator> {
        Arc::new(ModelCoordinator::new_named(
            self.root(),
            PHONON_DIR_NAME,
            tiny_assets(),
            Vec::new(),
        ))
    }
}

/// A root with a complete tiny install (files + READY written the way the installer does:
/// an empty READY file), plus an empty scratch directory.
fn tiny_installed() -> Tiny {
    let t = Tiny {
        dir: TempDir::new().unwrap(),
    };
    let install = t.install();
    fs::create_dir_all(&install).unwrap();
    for (file, body) in TINY {
        fs::write(install.join(file), body).unwrap();
    }
    fs::File::create(install.join(READY_MARKER)).unwrap();
    fs::create_dir_all(t.scratch()).unwrap();
    t
}

fn scratch_entries(scratch: &Path) -> usize {
    fs::read_dir(scratch).unwrap().count()
}

fn load_err(r: Result<Box<dyn SynthEngine>, String>) -> String {
    match r {
        Ok(_) => panic!("expected the load to fail"),
        Err(e) => e,
    }
}

/// What the builder observed while the snapshot was alive.
#[derive(Default, Debug)]
struct Seen {
    calls: usize,
    dir: Option<PathBuf>,
    dir_existed: bool,
    files: Vec<(String, Vec<u8>)>,
}

struct MarkerEngine {
    marker: f32,
}

impl SynthEngine for MarkerEngine {
    fn synth(&mut self, _text: &str, _voice: &str, _speed: f32) -> Result<Vec<f32>, String> {
        Ok(vec![self.marker])
    }
}

type Builder = Box<dyn Fn(&Snapshot) -> Result<Box<dyn SynthEngine>, String> + Send + Sync>;

/// Builder that records what it saw and returns an engine whose marker sample is 0.25.
fn recording_builder(seen: Arc<Mutex<Seen>>) -> Builder {
    Box::new(move |snap: &Snapshot| {
        let mut s = seen.lock().unwrap();
        s.calls += 1;
        s.dir = Some(snap.dir().to_path_buf());
        s.dir_existed = snap.dir().is_dir();
        for (file, _) in TINY {
            let bytes = fs::read(snap.path(file)).unwrap_or_default();
            s.files.push((file.to_string(), bytes));
        }
        let engine: Box<dyn SynthEngine> = Box::new(MarkerEngine { marker: 0.25 });
        Ok(engine)
    })
}

/// Builder that only counts its calls and succeeds.
fn counting_ok_builder(calls: Arc<AtomicUsize>) -> Builder {
    Box::new(move |_snap: &Snapshot| {
        calls.fetch_add(1, Ordering::SeqCst);
        let engine: Box<dyn SynthEngine> = Box::new(MarkerEngine { marker: 0.25 });
        Ok(engine)
    })
}

// ---- SnapshotLoader -------------------------------------------------------------------------------

#[test]
fn happy_path_builder_sees_verified_files_and_snapshot_is_gone_afterwards() {
    let t = tiny_installed();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut loader = SnapshotLoader::new(t.coord(), t.scratch(), recording_builder(seen.clone()));

    let mut engine = match loader.load() {
        Ok(engine) => engine,
        Err(e) => panic!("load must succeed, got: {e}"),
    };

    let s = seen.lock().unwrap();
    assert_eq!(s.calls, 1, "the builder runs exactly once");
    assert!(
        s.dir_existed,
        "the snapshot directory exists while building"
    );
    let dir = s.dir.clone().expect("the builder saw a snapshot directory");
    assert!(
        dir.starts_with(t.scratch()),
        "snapshot lives under scratch_root"
    );
    assert_eq!(s.files.len(), TINY.len());
    for ((file, bytes), (want_file, want_body)) in s.files.iter().zip(TINY) {
        assert_eq!(file, want_file);
        assert_eq!(bytes.as_slice(), want_body.as_bytes(), "{file} bytes");
    }
    assert!(!dir.exists(), "the snapshot directory is gone after load()");
    assert_eq!(scratch_entries(&t.scratch()), 0);

    let out = engine.synth("anything", "default", 1.0).unwrap();
    assert_eq!(
        out,
        vec![0.25],
        "the returned engine is the builder's engine"
    );
}

#[test]
fn builder_error_passes_through_and_snapshot_is_still_removed() {
    let t = tiny_installed();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let builder: Builder = {
        let seen = seen.clone();
        Box::new(move |snap: &Snapshot| {
            seen.lock().unwrap().dir = Some(snap.dir().to_path_buf());
            Err("bad model".to_string())
        })
    };
    let mut loader = SnapshotLoader::new(t.coord(), t.scratch(), builder);

    let err = load_err(loader.load());
    assert!(err.contains("bad model"), "got {err:?}");
    let dir = seen
        .lock()
        .unwrap()
        .dir
        .clone()
        .expect("builder was called");
    assert!(
        !dir.exists(),
        "the snapshot is dropped even when the builder fails"
    );
    assert_eq!(scratch_entries(&t.scratch()), 0);
}

#[test]
fn tampered_install_file_is_refused_before_the_builder_runs() {
    let t = tiny_installed();
    let model = t.install().join("model.q8.gguf");
    let original = TINY[0].1;
    let tampered = original.replace("0001", "9999");
    assert_eq!(tampered.len(), original.len(), "same size, different bytes");
    fs::write(&model, &tampered).unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let mut loader =
        SnapshotLoader::new(t.coord(), t.scratch(), counting_ok_builder(calls.clone()));

    let err = load_err(loader.load());
    assert!(err.contains("reinstall"), "got {err:?}");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the builder is never called"
    );
    assert_eq!(scratch_entries(&t.scratch()), 0, "no leftover snapshot");
}

#[test]
fn model_not_installed_asks_for_reinstall_and_never_builds() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("phonon_models");
    let scratch = dir.path().join("scratch");
    fs::create_dir_all(&scratch).unwrap();
    let coord = Arc::new(ModelCoordinator::new_named(
        root,
        PHONON_DIR_NAME,
        tiny_assets(),
        Vec::new(),
    ));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut loader =
        SnapshotLoader::new(coord, scratch.clone(), counting_ok_builder(calls.clone()));

    let err = load_err(loader.load());
    assert!(err.contains("reinstall"), "got {err:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(scratch_entries(&scratch), 0);
}

#[test]
fn construction_removes_stale_snapshot_dirs_and_keeps_other_entries() {
    let t = tiny_installed();
    let stale = t.scratch().join("snapshot-deadbeef");
    fs::create_dir_all(&stale).unwrap();
    fs::write(stale.join("leftover.bin"), b"old").unwrap();
    fs::write(t.scratch().join("keep-me.txt"), b"keep").unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let _loader = SnapshotLoader::new(t.coord(), t.scratch(), counting_ok_builder(calls));

    assert!(!stale.exists(), "stale snapshot-* directory is removed");
    assert!(
        t.scratch().join("keep-me.txt").is_file(),
        "entries that are not snapshot-* are never touched"
    );
}

#[test]
fn two_loads_in_a_row_both_verify_and_leave_scratch_empty() {
    let t = tiny_installed();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut loader =
        SnapshotLoader::new(t.coord(), t.scratch(), counting_ok_builder(calls.clone()));

    assert!(loader.load().is_ok(), "first load");
    assert_eq!(scratch_entries(&t.scratch()), 0, "after first load");
    assert!(loader.load().is_ok(), "second load");
    assert_eq!(scratch_entries(&t.scratch()), 0, "after second load");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn second_load_reverifies_and_fails_after_tampering_between_loads() {
    let t = tiny_installed();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut loader =
        SnapshotLoader::new(t.coord(), t.scratch(), counting_ok_builder(calls.clone()));

    assert!(loader.load().is_ok(), "first load verifies and succeeds");

    let config = t.install().join("config.json");
    let tampered = TINY[3].1.replace("true", "tru3");
    assert_eq!(
        tampered.len(),
        TINY[3].1.len(),
        "same size, different bytes"
    );
    fs::write(&config, tampered).unwrap();

    let err = load_err(loader.load());
    assert!(err.contains("reinstall"), "got {err:?}");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "no cached verdict: the second builder call never happens"
    );
    assert_eq!(scratch_entries(&t.scratch()), 0);
}
