//! Spec for the platform-neutral Phonon voice command cores (DESIGN.md Rev 4, P4b part A).
//! The thin Tauri wrappers need an AppHandle and are not tested here. Authoritative: do not
//! weaken an assertion; if a test is wrong, report it instead.
//!
//! Event contract under test:
//! * `phonon-voice-state` carries Kokoro's `natural-voice-download-progress` payload for each
//!   progress tick, and Kokoro's `natural-voice-download-finished` payload when the install
//!   finishes OK or is cancelled (a cancel is a state change, not an error).
//! * `phonon-voice-error` carries the same finished payload (`ok: false`) for every other failure.

use crate::commands::natural_voice::{
    error_message, NaturalVoiceDownloadFinished, NaturalVoiceDownloadProgress, NaturalVoiceStatus,
};
use crate::commands::phonon_voice::{
    phonon_voice_cancel_core, phonon_voice_delete_core, phonon_voice_install_core,
    phonon_voice_status_core,
};
use crate::managers::aux_model::{DownloadPolicy, RedirectRefused};
use crate::managers::natural_engine::{FailureReason, NaturalHealth};
use crate::managers::natural_model::{
    phonon_assets, phonon_coordinator, AssetSpec, ModelCoordinator, ModelError, ModelState,
    PHONON_DIR_NAME, PHONON_URL_PREFIX, READY_MARKER,
};
use crate::managers::tts_engines::PHONON_TOTAL_BYTES;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// The event sink the cores take: `(event name, JSON payload)`.
type EventSink = Arc<dyn Fn(&str, Value) + Send + Sync>;

const STATE_EVENT: &str = "phonon-voice-state";
const ERROR_EVENT: &str = "phonon-voice-error";

/// Tiny fake assets: same file names as the Phonon model, real SHA-256 over the bytes.
const TINY: [(&str, &str); 4] = [
    ("model.q8.gguf", "tiny-model-bytes"),
    ("default-voice.safetensors", "tiny-voice"),
    ("tokenizer.json", "{\"tokenizer\":1}"),
    ("config.json", "{\"tiny\":true}"),
];

fn tiny_total() -> u64 {
    TINY.iter().map(|(_, body)| body.len() as u64).sum()
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn tiny_assets(prefix: &str) -> Vec<AssetSpec> {
    TINY.iter()
        .map(|(file, body)| AssetSpec {
            file: file.to_string(),
            url: format!("{prefix}{file}"),
            size: body.len() as u64,
            sha256: sha(body.as_bytes()),
        })
        .collect()
}

/// The real Phonon asset list (pinned sizes and hashes) pointed at a loopback server.
fn pinned_assets_at(prefix: &str) -> Vec<AssetSpec> {
    phonon_assets()
        .into_iter()
        .map(|a| AssetSpec {
            url: format!("{prefix}{}", a.file),
            ..a
        })
        .collect()
}

fn loopback_policy() -> DownloadPolicy {
    DownloadPolicy {
        allowed_hosts: vec!["127.0.0.1".into()],
        max_redirects: 4,
        require_https: true,
        allow_loopback_http: true,
    }
}

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: TempDir::new().unwrap(),
        }
    }
    fn root(&self) -> PathBuf {
        self.dir.path().join("phonon_models")
    }
    fn install(&self) -> PathBuf {
        self.root().join(PHONON_DIR_NAME)
    }
}

/// Tiny coordinator with the Phonon policy copy (loopback http allowed, 127.0.0.1 only).
fn loopback_coord(root: PathBuf, prefix: &str) -> ModelCoordinator {
    ModelCoordinator::new_named(
        root,
        PHONON_DIR_NAME,
        tiny_assets(prefix),
        vec![prefix.to_string()],
    )
    .with_download_policy(loopback_policy())
}

/// Writes a complete tiny install the way the installer does (files + READY).
fn install_tiny(root: &Path) {
    let install = root.join(PHONON_DIR_NAME);
    fs::create_dir_all(&install).unwrap();
    for (file, body) in TINY {
        fs::write(install.join(file), body).unwrap();
    }
    fs::File::create(install.join(READY_MARKER)).unwrap();
}

/// Writes the real pinned file names and sizes as sparse files, plus READY (state() never
/// hashes, so this is a faithful "installed" fixture for status).
fn install_pinned_sizes(root: &Path) {
    let install = root.join(PHONON_DIR_NAME);
    fs::create_dir_all(&install).unwrap();
    for a in phonon_assets() {
        let f = fs::File::create(install.join(&a.file)).unwrap();
        f.set_len(a.size).unwrap();
    }
    fs::File::create(install.join(READY_MARKER)).unwrap();
}

fn staging_entries(root: &Path) -> Vec<String> {
    fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with(".staging-"))
                .collect()
        })
        .unwrap_or_default()
}

/// Collects every emitted event, in order.
#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<(String, Value)>>,
}

impl Recorder {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    fn sink(self: &Arc<Self>) -> EventSink {
        let me = self.clone();
        Arc::new(move |name: &str, payload: Value| {
            me.events.lock().unwrap().push((name.to_string(), payload));
        })
    }
    fn named(&self, name: &str) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, p)| p.clone())
            .collect()
    }
    fn count(&self) -> usize {
        self.events.lock().unwrap().len()
    }
}

/// The payload must not carry a URL, a local path, a loopback address or the hash.
fn assert_no_leak(payloads: &[Value], hash: &str) {
    for payload in payloads {
        let text = payload.to_string();
        for forbidden in ["http", "/Users", "/tmp", "/var", "127.0.0.", hash] {
            assert!(
                !text.contains(forbidden),
                "event payload leaks {forbidden:?}: {text}"
            );
        }
    }
}

fn finished(ok: bool, message: Option<String>) -> Value {
    serde_json::to_value(NaturalVoiceDownloadFinished { ok, message }).unwrap()
}

/// Polls `cond` every 5 ms until it holds or 30 s pass. Returns as soon as it holds, so the
/// happy path stays fast while a saturated CPU still gets a generous bound.
async fn wait_for(mut cond: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    cond()
}

async fn finish(task: JoinHandle<Result<(), String>>) -> Result<(), String> {
    tokio::time::timeout(Duration::from_secs(30), task)
        .await
        .expect("the install must end promptly after a cancel")
        .expect("the install task must not panic")
}

// ---- local raw-TCP servers (same technique as natural_phonon_model_tests.rs) -----------------

/// A raw HTTP/1.1 response with the given status line, extra header lines and body.
fn raw_response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let head = format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    out
}

/// Decides the response for a request path; `port` is the server's own port.
type Route = fn(&str, u16) -> Vec<u8>;

async fn read_head(sock: &mut TcpStream) -> bool {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match sock.read(&mut chunk).await {
            Ok(0) | Err(_) => return false,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    true
}

fn request_path(head: &[u8]) -> String {
    let text = String::from_utf8_lossy(head).to_string();
    text.split_whitespace().nth(1).unwrap_or("/").to_string()
}

async fn spawn_route_server(route: Route) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let path = request_path(&buf);
                let _ = sock.write_all(&route(&path, port)).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    addr
}

/// Serves the tiny bytes for `/<file>`.
fn route_ok(path: &str, _port: u16) -> Vec<u8> {
    let file = path.trim_start_matches('/');
    match TINY.iter().find(|(name, _)| *name == file) {
        Some((_, body)) => raw_response("200 OK", "", body.as_bytes()),
        None => raw_response("404 Not Found", "", b""),
    }
}

/// Same length as the real bytes but one flipped byte: the pinned SHA-256 must refuse it.
fn route_flipped(path: &str, _port: u16) -> Vec<u8> {
    let file = path.trim_start_matches('/');
    match TINY.iter().find(|(name, _)| *name == file) {
        Some((_, body)) => {
            let mut bytes = body.as_bytes().to_vec();
            bytes[0] ^= 1;
            raw_response("200 OK", "", &bytes)
        }
        None => raw_response("404 Not Found", "", b""),
    }
}

/// Every request is redirected to another loopback address the policy does not allow.
fn route_redirect_to_disallowed_host(path: &str, port: u16) -> Vec<u8> {
    raw_response(
        "302 Found",
        &format!("Location: http://127.0.0.2:{port}{path}\r\n"),
        b"",
    )
}

/// Answers 407 to every request, as a proxy that needs sign-in does.
async fn spawn_proxy_407_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                if !read_head(&mut sock).await {
                    return;
                }
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                let _ = sock.shutdown().await;
            });
        }
    });
    addr
}

/// Accepts, answers each request with headers announcing the REQUESTED file's exact size (the
/// downloader rejects a server that advertises another total), then never sends the body: a
/// download that stays in flight until it is cancelled. `sizes` maps file name to size.
async fn spawn_stall_server(sizes: Vec<(String, u64)>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let sizes = Arc::new(sizes);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let sizes = sizes.clone();
            tokio::spawn(async move {
                let Some(path) = read_request_path(&mut sock).await else {
                    return;
                };
                let file = path.trim_start_matches('/');
                let Some((_, size)) = sizes.iter().find(|(name, _)| name == file) else {
                    let _ = sock
                        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                    return;
                };
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n"
                );
                let _ = sock.write_all(head.as_bytes()).await;
                tokio::time::sleep(Duration::from_secs(120)).await;
            });
        }
    });
    addr
}

/// The request path of one HTTP/1.1 request, read up to the blank line.
async fn read_request_path(sock: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match sock.read(&mut chunk).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    Some(request_path(&buf))
}

/// The tiny fake assets' exact sizes, for the stall server.
fn tiny_sizes() -> Vec<(String, u64)> {
    TINY.iter()
        .map(|(file, body)| (file.to_string(), body.len() as u64))
        .collect()
}

/// The real Phonon files' pinned sizes, for the stall server.
fn pinned_sizes() -> Vec<(String, u64)> {
    phonon_assets()
        .into_iter()
        .map(|a| (a.file, a.size))
        .collect()
}

// ---- status ----------------------------------------------------------------------------------

#[test]
fn status_of_an_empty_root_is_not_installed_and_healthy() {
    let fx = Fixture::new();
    let coord = phonon_coordinator(fx.root());
    let health = NaturalHealth::new(Duration::from_secs(300));
    let status = phonon_voice_status_core(true, &coord, &health);
    assert_eq!(
        status,
        NaturalVoiceStatus {
            supported: true,
            model: "not_installed".into(),
            downloaded: 0,
            total: 0,
            health: "ready".into(),
            failure: None,
        }
    );
}

#[test]
fn status_of_an_installed_phonon_model_is_ready_with_the_pinned_names() {
    let fx = Fixture::new();
    install_pinned_sizes(&fx.root());
    let coord = phonon_coordinator(fx.root());
    let health = NaturalHealth::new(Duration::from_secs(300));
    let status = phonon_voice_status_core(true, &coord, &health);
    assert_eq!(
        status.model, "ready",
        "the pinned Phonon files must be recognised"
    );
    assert_eq!(status.health, "ready");
    assert_eq!(status.failure, None);
}

#[test]
fn status_of_a_truncated_phonon_file_is_not_installed() {
    let fx = Fixture::new();
    install_pinned_sizes(&fx.root());
    let model = fx.install().join("model.q8.gguf");
    let size = fs::metadata(&model).unwrap().len();
    fs::OpenOptions::new()
        .write(true)
        .open(&model)
        .unwrap()
        .set_len(size - 1)
        .unwrap();
    let coord = phonon_coordinator(fx.root());
    let health = NaturalHealth::new(Duration::from_secs(300));
    let status = phonon_voice_status_core(true, &coord, &health);
    assert_eq!(
        status.model, "not_installed",
        "a short file is corrupt, not ready"
    );
}

#[test]
fn status_reports_a_failed_load_with_its_reason() {
    let fx = Fixture::new();
    install_pinned_sizes(&fx.root());
    let coord = phonon_coordinator(fx.root());
    let health = NaturalHealth::new(Duration::from_secs(300));
    assert!(health.report_failure(health.revision(), FailureReason::Load));
    let status = phonon_voice_status_core(false, &coord, &health);
    assert_eq!(status.health, "failed");
    assert_eq!(status.failure.as_deref(), Some("load"));
    assert!(!status.supported);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_during_a_phonon_download_reports_the_phonon_total() {
    let addr = spawn_stall_server(pinned_sizes()).await;
    let prefix = format!("http://{addr}/");
    let fx = Fixture::new();
    let coord = Arc::new(
        ModelCoordinator::new_named(
            fx.root(),
            PHONON_DIR_NAME,
            pinned_assets_at(&prefix),
            vec![prefix.clone()],
        )
        .with_download_policy(loopback_policy()),
    );
    let health = NaturalHealth::new(Duration::from_secs(300));
    let task = tokio::spawn(phonon_voice_install_core(
        coord.clone(),
        health.clone(),
        Recorder::new().sink(),
    ));
    assert!(wait_for(|| coord.state() == ModelState::Downloading).await);
    let status = phonon_voice_status_core(true, &coord, &health);
    assert_eq!(status.model, "downloading");
    assert_eq!(
        status.total, PHONON_TOTAL_BYTES,
        "the Phonon status must report the Phonon download total"
    );
    phonon_voice_cancel_core(&coord).expect("cancel");
    let _ = finish(task).await;
}

// ---- install ---------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_downloads_verifies_publishes_and_bumps_health_exactly_once() {
    let addr = spawn_route_server(route_ok).await;
    let prefix = format!("http://{addr}/");
    let fx = Fixture::new();
    let coord = Arc::new(loopback_coord(fx.root(), &prefix));
    let health = NaturalHealth::new(Duration::from_secs(300));
    let rec = Recorder::new();
    let before = health.revision();

    phonon_voice_install_core(coord.clone(), health.clone(), rec.sink())
        .await
        .expect("install must succeed");

    assert_eq!(coord.state(), ModelState::Ready);
    assert_eq!(
        health.revision(),
        before + 1,
        "install invalidates the Phonon health exactly once"
    );
    let states = rec.named(STATE_EVENT);
    let (progress, done): (Vec<Value>, Vec<Value>) =
        states.iter().cloned().partition(|p| p.get("ok").is_none());
    assert!(!progress.is_empty(), "progress ticks are emitted");
    let mut last = 0u64;
    for p in &progress {
        let downloaded = p["downloaded"].as_u64().expect("downloaded");
        assert!(downloaded >= last, "progress is monotonic");
        last = downloaded;
        assert_eq!(p["total"].as_u64(), Some(tiny_total()));
        assert_eq!(p.as_object().unwrap().len(), 2, "progress payload shape");
    }
    assert_eq!(last, tiny_total(), "progress ends at the total");
    assert_eq!(
        done,
        vec![finished(true, None)],
        "exactly one OK finish on the state event"
    );
    assert_eq!(states.last(), Some(&finished(true, None)));
    assert!(rec.named(ERROR_EVENT).is_empty(), "no error on success");
    assert_eq!(
        serde_json::to_value(NaturalVoiceDownloadProgress {
            downloaded: tiny_total(),
            total: tiny_total(),
        })
        .unwrap(),
        *progress.last().unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_over_an_installed_model_is_allowed_like_kokoro_and_ends_ready() {
    let addr = spawn_route_server(route_ok).await;
    let prefix = format!("http://{addr}/");
    let fx = Fixture::new();
    install_tiny(&fx.root());
    let coord = Arc::new(loopback_coord(fx.root(), &prefix));
    let health = NaturalHealth::new(Duration::from_secs(300));
    let before = health.revision();
    phonon_voice_install_core(coord.clone(), health.clone(), Recorder::new().sink())
        .await
        .expect("Ready is not busy: a re-install runs, as Kokoro's does");
    assert_eq!(coord.state(), ModelState::Ready);
    assert_eq!(health.revision(), before + 1);
    assert!(staging_entries(&fx.root()).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_while_another_operation_runs_is_busy_with_the_kokoro_text() {
    let addr = spawn_stall_server(tiny_sizes()).await;
    let prefix = format!("http://{addr}/");
    let fx = Fixture::new();
    let coord = Arc::new(loopback_coord(fx.root(), &prefix));
    let health = NaturalHealth::new(Duration::from_secs(300));
    let first = Recorder::new();
    let task = tokio::spawn(phonon_voice_install_core(
        coord.clone(),
        health.clone(),
        first.sink(),
    ));
    assert!(wait_for(|| coord.state() == ModelState::Downloading).await);
    let revision = health.revision();

    let second = Recorder::new();
    let busy = phonon_voice_install_core(coord.clone(), health.clone(), second.sink())
        .await
        .expect_err("a second install must be refused");
    assert_eq!(busy, error_message(&ModelError::Busy));
    assert_eq!(second.count(), 0, "a refused install emits nothing");

    let delete_busy = phonon_voice_delete_core(&coord, &health, &|| {})
        .expect_err("delete must be refused while a download runs");
    assert_eq!(delete_busy, error_message(&ModelError::Busy));
    assert_eq!(
        health.revision(),
        revision,
        "refused delete does not bump health"
    );

    phonon_voice_cancel_core(&coord).expect("cancel");
    let _ = finish(task).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_mid_download_returns_to_not_installed_and_leaves_no_staging() {
    let addr = spawn_stall_server(tiny_sizes()).await;
    let prefix = format!("http://{addr}/");
    let fx = Fixture::new();
    let coord = Arc::new(loopback_coord(fx.root(), &prefix));
    let health = NaturalHealth::new(Duration::from_secs(300));
    let rec = Recorder::new();
    let revision = health.revision();
    let task = tokio::spawn(phonon_voice_install_core(
        coord.clone(),
        health.clone(),
        rec.sink(),
    ));
    assert!(wait_for(|| coord.state() == ModelState::Downloading).await);

    phonon_voice_cancel_core(&coord).expect("cancel is always Ok");
    let result = finish(task).await;

    let cancelled = error_message(&ModelError::Cancelled);
    assert_eq!(result, Err(cancelled.clone()));
    assert_eq!(coord.state(), ModelState::NotInstalled);
    assert!(
        staging_entries(&fx.root()).is_empty(),
        "partial staging removed"
    );
    assert!(!fx.install().exists(), "nothing published");
    assert_eq!(health.revision(), revision, "a cancel does not bump health");
    assert_eq!(
        rec.named(STATE_EVENT).last(),
        Some(&finished(false, Some(cancelled))),
        "the cancel is reported on phonon-voice-state"
    );
    assert!(rec.named(ERROR_EVENT).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_with_an_address_outside_the_allowlist_is_refused_before_any_network() {
    let fx = Fixture::new();
    let coord = Arc::new(ModelCoordinator::new_named(
        fx.root(),
        PHONON_DIR_NAME,
        tiny_assets("http://127.0.0.1:9/"),
        vec![PHONON_URL_PREFIX.to_string()],
    ));
    let health = NaturalHealth::new(Duration::from_secs(300));
    let rec = Recorder::new();
    let result = phonon_voice_install_core(coord.clone(), health.clone(), rec.sink()).await;
    let refused = error_message(&ModelError::UrlNotAllowed);
    assert_eq!(result, Err(refused.clone()));
    assert_eq!(rec.named(ERROR_EVENT), vec![finished(false, Some(refused))]);
    assert_eq!(coord.state(), ModelState::NotInstalled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_407_gives_the_sign_in_text_on_the_error_event_and_leaks_nothing() {
    let addr = spawn_proxy_407_server().await;
    let prefix = format!("http://{addr}/");
    let fx = Fixture::new();
    let coord = Arc::new(ModelCoordinator::new_named(
        fx.root(),
        PHONON_DIR_NAME,
        tiny_assets(&prefix),
        vec![prefix.clone()],
    ));
    let health = NaturalHealth::new(Duration::from_secs(300));
    let rec = Recorder::new();
    let revision = health.revision();

    let result = phonon_voice_install_core(coord.clone(), health.clone(), rec.sink()).await;

    let text = error_message(&ModelError::ProxyAuthRequired);
    assert_eq!(result, Err(text.clone()));
    assert!(text.contains("sign-in"), "{text}");
    assert!(
        text.contains("folder"),
        "the text must suggest installing from a folder"
    );
    assert_eq!(rec.named(ERROR_EVENT), vec![finished(false, Some(text))]);
    assert_no_leak(&rec.named(ERROR_EVENT), &sha(TINY[0].1.as_bytes()));
    assert_no_leak(&rec.named(STATE_EVENT), &sha(TINY[0].1.as_bytes()));
    assert_eq!(health.revision(), revision);
    assert_eq!(coord.state(), ModelState::NotInstalled);
    assert!(!fx.install().exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redirect_to_a_non_allowed_host_gives_the_generic_download_text_without_the_url() {
    let addr = spawn_route_server(route_redirect_to_disallowed_host).await;
    let prefix = format!("http://{addr}/");
    let fx = Fixture::new();
    let coord = Arc::new(loopback_coord(fx.root(), &prefix));
    let health = NaturalHealth::new(Duration::from_secs(300));
    let rec = Recorder::new();

    let result = phonon_voice_install_core(coord.clone(), health.clone(), rec.sink()).await;

    let generic = error_message(&ModelError::Download(RedirectRefused.to_string()));
    assert_eq!(result, Err(generic.clone()));
    assert_eq!(rec.named(ERROR_EVENT), vec![finished(false, Some(generic))]);
    assert_no_leak(&rec.named(ERROR_EVENT), &sha(TINY[0].1.as_bytes()));
    assert!(staging_entries(&fx.root()).is_empty());
    assert!(!fx.install().exists());
    assert_eq!(coord.state(), ModelState::NotInstalled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hash_mismatch_from_the_server_publishes_nothing_and_reports_the_download_text() {
    let addr = spawn_route_server(route_flipped).await;
    let prefix = format!("http://{addr}/");
    let fx = Fixture::new();
    let coord = Arc::new(loopback_coord(fx.root(), &prefix));
    let health = NaturalHealth::new(Duration::from_secs(300));
    let rec = Recorder::new();
    let revision = health.revision();

    let result = phonon_voice_install_core(coord.clone(), health.clone(), rec.sink()).await;

    let generic = error_message(&ModelError::Download(String::new()));
    assert_eq!(result, Err(generic.clone()));
    assert_eq!(rec.named(ERROR_EVENT), vec![finished(false, Some(generic))]);
    assert_no_leak(&rec.named(ERROR_EVENT), &sha(TINY[0].1.as_bytes()));
    assert_eq!(
        health.revision(),
        revision,
        "a failed install does not bump health"
    );
    assert!(staging_entries(&fx.root()).is_empty(), "staging removed");
    assert!(!fx.install().exists(), "nothing published");
    assert_eq!(coord.state(), ModelState::NotInstalled);
}

// ---- delete ----------------------------------------------------------------------------------

#[test]
fn delete_stops_speech_removes_the_model_and_bumps_health_once() {
    let fx = Fixture::new();
    install_tiny(&fx.root());
    let coord = loopback_coord(fx.root(), "http://127.0.0.1:9/");
    let health = NaturalHealth::new(Duration::from_secs(300));
    let before = health.revision();
    let stops = std::cell::Cell::new(0u32);

    phonon_voice_delete_core(&coord, &health, &|| stops.set(stops.get() + 1))
        .expect("delete must succeed");

    assert_eq!(stops.get(), 1, "speech is stopped once before the delete");
    assert!(!fx.install().exists(), "the model files are gone");
    assert_eq!(coord.state(), ModelState::NotInstalled);
    assert_eq!(
        health.revision(),
        before + 1,
        "delete bumps the Phonon health once"
    );
}

#[test]
fn delete_of_an_absent_model_is_ok_like_kokoro() {
    let fx = Fixture::new();
    let coord = loopback_coord(fx.root(), "http://127.0.0.1:9/");
    let health = NaturalHealth::new(Duration::from_secs(300));
    phonon_voice_delete_core(&coord, &health, &|| {}).expect("idempotent delete");
    assert_eq!(coord.state(), ModelState::NotInstalled);
}

// ---- P7: install the Phonon voice from a folder (phonon_voice_import_core) ---------------------
//
// DESIGN.md Rev 4 section 3 P7. The import core is synchronous and platform-neutral. It returns
// the user text on failure, emits `phonon-voice-state` (finished OK) on success and
// `phonon-voice-error` on failure, bumps Phonon health once on success only, and never puts the
// source folder, a temp path, a URL or a hash into any payload.

/// The four tiny assets written under their real names into `<fixture>/source`.
fn import_source_folder(fx: &Fixture) -> PathBuf {
    let src = fx.dir.path().join("source");
    fs::create_dir_all(&src).unwrap();
    for (file, body) in TINY {
        fs::write(src.join(file), body).unwrap();
    }
    src
}

fn all_payloads(rec: &Recorder) -> Vec<Value> {
    rec.events
        .lock()
        .unwrap()
        .iter()
        .map(|(_, payload)| payload.clone())
        .collect()
}

/// True when `text` holds a run of `len` hex digits (a SHA-256 when len is 64).
fn has_hex_run(text: &str, len: usize) -> bool {
    text.as_bytes()
        .windows(len)
        .any(|w| w.iter().all(|b| b.is_ascii_hexdigit()))
}

/// No payload may carry the source folder, a temp path, a hash or any 64-digit hex run.
fn assert_no_secret_text(payloads: &[Value], src: &Path) {
    let folder = src.display().to_string();
    for payload in payloads {
        let text = payload.to_string();
        assert!(
            !text.contains(&folder),
            "payload carries the source folder: {text}"
        );
        for forbidden in ["/tmp", "/Users", "sha"] {
            assert!(
                !text.contains(forbidden),
                "payload carries {forbidden:?}: {text}"
            );
        }
        assert!(!has_hex_run(&text, 64), "payload carries a hash: {text}");
    }
}

/// Runs the import core on a folder the test has just corrupted, and checks the failure
/// contract: the user text on the error event, no health bump, nothing published, no staging,
/// and no secret in any payload.
fn check_import_failure(corrupt: impl FnOnce(&Path)) {
    let fx = Fixture::new();
    let src = import_source_folder(&fx);
    corrupt(&src);
    let coord = Arc::new(loopback_coord(fx.root(), "http://127.0.0.1:9/"));
    let health = NaturalHealth::new(Duration::from_secs(300));
    let rec = Recorder::new();
    let revision = health.revision();

    let result = crate::commands::phonon_voice::phonon_voice_import_core(
        coord.clone(),
        health.clone(),
        &src,
        rec.sink(),
    );

    let text = error_message(&ModelError::Corrupt(String::new()));
    assert_eq!(result, Err(text.clone()));
    assert_eq!(rec.named(ERROR_EVENT), vec![finished(false, Some(text))]);
    assert!(
        rec.named(STATE_EVENT)
            .iter()
            .all(|p| *p != finished(true, None)),
        "no success event on failure"
    );
    assert_eq!(
        health.revision(),
        revision,
        "a failed import does not bump health"
    );
    assert_eq!(coord.state(), ModelState::NotInstalled);
    assert!(!fx.install().exists(), "nothing published");
    assert!(staging_entries(&fx.root()).is_empty(), "staging removed");
    assert_no_secret_text(&all_payloads(&rec), &src);
}

#[test]
fn phonon_voice_import_core_success_emits_ready_and_bumps_health_once() {
    let fx = Fixture::new();
    let src = import_source_folder(&fx);
    let coord = Arc::new(loopback_coord(fx.root(), "http://127.0.0.1:9/"));
    let health = NaturalHealth::new(Duration::from_secs(300));
    let rec = Recorder::new();
    let revision = health.revision();

    crate::commands::phonon_voice::phonon_voice_import_core(
        coord.clone(),
        health.clone(),
        &src,
        rec.sink(),
    )
    .expect("import must succeed");

    assert_eq!(coord.state(), ModelState::Ready);
    assert_eq!(
        health.revision(),
        revision + 1,
        "import invalidates the Phonon health exactly once"
    );
    let states = rec.named(STATE_EVENT);
    assert_eq!(
        states.last(),
        Some(&finished(true, None)),
        "the final state event is the finished OK shape"
    );
    assert_eq!(
        states
            .iter()
            .filter(|p| **p == finished(true, None))
            .count(),
        1,
        "exactly one OK finish"
    );
    assert!(rec.named(ERROR_EVENT).is_empty(), "no error on success");
    assert_no_secret_text(&all_payloads(&rec), &src);
}

#[test]
fn phonon_voice_import_core_missing_file_fails_with_the_verification_text() {
    check_import_failure(|src| {
        fs::remove_file(src.join("model.q8.gguf")).unwrap();
    });
}

#[test]
fn phonon_voice_import_core_wrong_hash_fails_with_the_verification_text() {
    check_import_failure(|src| {
        let path = src.join("tokenizer.json");
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] ^= 0x01;
        fs::write(&path, &bytes).unwrap();
    });
}

#[cfg(unix)]
#[test]
fn phonon_voice_import_core_symlinked_asset_fails_with_the_verification_text() {
    check_import_failure(|src| {
        let elsewhere = src.parent().unwrap().join("elsewhere.json");
        fs::write(&elsewhere, TINY[3].1).unwrap();
        let link = src.join("config.json");
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn phonon_voice_import_core_while_installing_is_busy_with_the_install_text() {
    let addr = spawn_stall_server(tiny_sizes()).await;
    let prefix = format!("http://{addr}/");
    let fx = Fixture::new();
    let src = import_source_folder(&fx);
    let coord = Arc::new(loopback_coord(fx.root(), &prefix));
    let health = NaturalHealth::new(Duration::from_secs(300));
    let task = tokio::spawn(phonon_voice_install_core(
        coord.clone(),
        health.clone(),
        Recorder::new().sink(),
    ));
    assert!(wait_for(|| coord.state() == ModelState::Downloading).await);
    let revision = health.revision();

    let rec = Recorder::new();
    let busy = crate::commands::phonon_voice::phonon_voice_import_core(
        coord.clone(),
        health.clone(),
        &src,
        rec.sink(),
    )
    .expect_err("an import while installing must be refused");

    assert_eq!(busy, error_message(&ModelError::Busy));
    assert_eq!(
        health.revision(),
        revision,
        "a refused import does not bump health"
    );
    assert!(
        rec.named(STATE_EVENT)
            .iter()
            .all(|p| *p != finished(true, None)),
        "a refused import never reports success"
    );
    assert_no_secret_text(&all_payloads(&rec), &src);

    phonon_voice_cancel_core(&coord).expect("cancel");
    let _ = finish(task).await;
}
