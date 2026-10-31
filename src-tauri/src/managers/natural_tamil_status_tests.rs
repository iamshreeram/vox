//! Spec for the Tamil status and wiring (`natural_tamil.rs`: `tamil_voice_status_for`,
//! `build_tamil_route`). Temp folders with sparse files of the pinned sizes, a fake system
//! backend and the real router. Nothing here loads the Tamil model or needs the real pack.

use crate::managers::emotion::Emotion;
use crate::managers::natural_engine::{FailureReason, NaturalHealth, DEFAULT_AUTO_RETRY_COOLDOWN};
use crate::managers::natural_router::{EngineBackend, RouterChoice, RouterConfig};
use crate::managers::natural_tamil::{
    build_tamil_route, tamil_failure_message, tamil_model_dir, tamil_voice_status_for,
    TamilVoiceStatus, TAMIL_MODEL_DIRNAME, TAMIL_PACK_FILES,
};
use crate::managers::natural_test_support::FakeStreamFactory;
use crate::managers::tts::{SpeechBackend, SpeechHandle};
use std::fs::{self, File};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::TempDir;

const TAMIL_TEXT: &str = "வணக்கம் உலகம்";
const ENGLISH_TEXT: &str = "hello there";

fn write_pack(dir: &Path) {
    fs::create_dir_all(dir).expect("create pack dir");
    for (name, size) in TAMIL_PACK_FILES {
        let file = File::create(dir.join(name)).expect("create pack file");
        file.set_len(size).expect("set size");
    }
}

fn all_names() -> Vec<String> {
    TAMIL_PACK_FILES
        .iter()
        .map(|(name, _)| name.to_string())
        .collect()
}

// ---- tamil_voice_status_for (pure) ----

#[test]
fn status_ready_pack_is_installed_and_ready() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join(TAMIL_MODEL_DIRNAME);
    write_pack(&dir);
    let status = tamil_voice_status_for(&dir, true, None);
    assert_eq!(
        status,
        TamilVoiceStatus {
            installed: true,
            ready: true,
            enabled: true,
            dir: dir.display().to_string(),
            missing: Vec::new(),
            wrong_size: Vec::new(),
            engine_error: None,
        }
    );
}

#[test]
fn status_missing_one_file_names_it_and_is_not_ready() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join(TAMIL_MODEL_DIRNAME);
    write_pack(&dir);
    fs::remove_file(dir.join("tokenizer.model")).unwrap();
    let status = tamil_voice_status_for(&dir, true, None);
    assert!(!status.installed);
    assert!(!status.ready);
    assert_eq!(status.missing, vec!["tokenizer.model".to_string()]);
    assert!(status.wrong_size.is_empty());
}

#[test]
fn status_wrong_size_names_it_and_is_not_installed() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join(TAMIL_MODEL_DIRNAME);
    write_pack(&dir);
    let file = File::create(dir.join("bundle.json")).unwrap();
    file.set_len(24_362).unwrap();
    let status = tamil_voice_status_for(&dir, true, None);
    assert!(!status.installed);
    assert!(!status.ready);
    assert!(status.missing.is_empty());
    assert_eq!(status.wrong_size, vec!["bundle.json".to_string()]);
}

#[test]
fn status_nonexistent_dir_reports_all_nine_missing() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("no-such-folder");
    let status = tamil_voice_status_for(&dir, false, None);
    assert!(!status.installed);
    assert!(!status.ready);
    assert_eq!(status.missing, all_names());
    assert!(status.wrong_size.is_empty());
}

#[test]
fn status_enabled_false_is_reported_and_does_not_change_ready() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join(TAMIL_MODEL_DIRNAME);
    write_pack(&dir);
    let status = tamil_voice_status_for(&dir, false, None);
    assert!(!status.enabled);
    assert!(
        status.ready,
        "ready is the pack and engine health, not the toggle"
    );
}

#[test]
fn status_engine_error_makes_it_not_ready_and_is_passed_through() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join(TAMIL_MODEL_DIRNAME);
    write_pack(&dir);
    let message = tamil_failure_message(FailureReason::Load);
    let status = tamil_voice_status_for(&dir, true, Some(message.clone()));
    assert!(status.installed);
    assert!(!status.ready);
    assert_eq!(status.engine_error, Some(message));
}

#[test]
fn failure_messages_are_user_safe_and_name_the_fallback() {
    for reason in [
        FailureReason::Load,
        FailureReason::Synth,
        FailureReason::Sink,
        FailureReason::WorkerExit,
    ] {
        let text = tamil_failure_message(reason);
        assert!(text.contains("system voice"), "got {text:?}");
        assert!(!text.contains('/'), "no paths in user text: {text:?}");
    }
}

// ---- build_tamil_route (wiring with fakes) ----

/// Records every `spawn` call; its handles are never running.
struct FakeHandle;

impl SpeechHandle for FakeHandle {
    fn kill(&mut self) -> bool {
        true
    }
    fn is_running(&mut self) -> bool {
        false
    }
}

struct FakeSystem {
    calls: Arc<AtomicUsize>,
}

impl SpeechBackend for FakeSystem {
    fn spawn(
        &self,
        _text: &str,
        _voice: Option<&str>,
        _rate_wpm: Option<u32>,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeHandle))
    }
}

fn no_peer_config() -> RouterConfig {
    RouterConfig {
        choice: Arc::new(|| RouterChoice {
            natural: false,
            voice: String::new(),
            phonon: false,
        }),
        model_ready: Arc::new(|| false),
        phonon_ready: Arc::new(|| false),
    }
}

fn always_on() -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(|| true)
}

#[test]
fn route_without_the_pack_is_not_ready() {
    let tmp = TempDir::new().unwrap();
    let route = build_tamil_route(tmp.path(), FakeStreamFactory::new(24_000), always_on());
    assert!((route.enabled)());
    assert!(!(route.ready)(), "no pack: the route must not be ready");
    assert!(route.health.is_ready(), "nothing failed yet");
}

#[test]
fn route_with_the_pack_at_exact_sizes_is_ready() {
    let tmp = TempDir::new().unwrap();
    write_pack(&tamil_model_dir(tmp.path()));
    let route = build_tamil_route(tmp.path(), FakeStreamFactory::new(24_000), always_on());
    assert!((route.ready)(), "the nine files at exact sizes: ready");
}

#[test]
fn route_enabled_reads_the_live_flag_on_every_call() {
    let tmp = TempDir::new().unwrap();
    let flag = Arc::new(AtomicBool::new(true));
    let read = flag.clone();
    let route = build_tamil_route(
        tmp.path(),
        FakeStreamFactory::new(24_000),
        Arc::new(move || read.load(Ordering::SeqCst)),
    );
    assert!((route.enabled)());
    flag.store(false, Ordering::SeqCst);
    assert!(!(route.enabled)(), "a toggle applies without a rebuild");
}

#[test]
fn tamil_text_without_the_pack_goes_to_the_system_voice() {
    let tmp = TempDir::new().unwrap();
    let system_calls = Arc::new(AtomicUsize::new(0));
    let route = build_tamil_route(tmp.path(), FakeStreamFactory::new(24_000), always_on());
    let router = EngineBackend::new(
        Box::new(FakeSystem {
            calls: system_calls.clone(),
        }),
        Box::new(FakeSystem {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        None,
        NaturalHealth::new(DEFAULT_AUTO_RETRY_COOLDOWN),
        no_peer_config(),
    )
    .with_tamil_route(route);
    let mut handle = router
        .spawn_styled(TAMIL_TEXT, None, None, Emotion::Neutral)
        .expect("the system fake accepts the spawn");
    assert!(!handle.is_running());
    assert_eq!(
        system_calls.load(Ordering::SeqCst),
        1,
        "the Tamil text must reach the system fake, not the Tamil route"
    );
}

#[test]
fn english_text_with_the_pack_present_still_goes_to_the_system_voice() {
    let tmp = TempDir::new().unwrap();
    write_pack(&tamil_model_dir(tmp.path()));
    let system_calls = Arc::new(AtomicUsize::new(0));
    let route = build_tamil_route(tmp.path(), FakeStreamFactory::new(24_000), always_on());
    let router = EngineBackend::new(
        Box::new(FakeSystem {
            calls: system_calls.clone(),
        }),
        Box::new(FakeSystem {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        None,
        NaturalHealth::new(DEFAULT_AUTO_RETRY_COOLDOWN),
        no_peer_config(),
    )
    .with_tamil_route(route);
    router
        .spawn(ENGLISH_TEXT, None, None)
        .expect("the system fake accepts the spawn");
    assert_eq!(system_calls.load(Ordering::SeqCst), 1);
}
