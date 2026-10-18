//! Spec for `natural_model` (step S4). Authoritative: do not weaken an assertion; if a test is
//! wrong, report it instead of editing it. Uses a local HTTP server; no real network.
use crate::managers::natural_model::*;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn pseudo(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    /// Send this many body bytes, then stall.
    StallAfter(usize),
    /// Hold every response until `release()`.
    Gated,
}

struct TestServer {
    addr: SocketAddr,
    hits: Arc<AtomicUsize>,
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    mode: Arc<Mutex<Mode>>,
    open: Arc<AtomicBool>,
}

impl TestServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let files: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::new(Mutex::new(HashMap::new()));
        let mode = Arc::new(Mutex::new(Mode::Normal));
        let open = Arc::new(AtomicBool::new(false));
        let (h, f, m, o) = (hits.clone(), files.clone(), mode.clone(), open.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let (h, f, m, o) = (h.clone(), f.clone(), m.clone(), o.clone());
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    h.fetch_add(1, Ordering::SeqCst);
                    let head = String::from_utf8_lossy(&buf).to_string();
                    let path = head
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("/")
                        .trim_start_matches('/')
                        .to_string();
                    let body = f.lock().unwrap().get(&path).cloned();
                    let Some(body) = body else {
                        let _ = sock
                            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                            .await;
                        return;
                    };
                    let mode = *m.lock().unwrap();
                    if mode == Mode::Gated {
                        while !o.load(Ordering::SeqCst) {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    }
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    if sock.write_all(header.as_bytes()).await.is_err() {
                        return;
                    }
                    if let Mode::StallAfter(n) = mode {
                        let n = n.min(body.len());
                        let _ = sock.write_all(&body[..n]).await;
                        let _ = sock.flush().await;
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        return;
                    }
                    for piece in body.chunks(16 * 1024) {
                        if sock.write_all(piece).await.is_err() {
                            return;
                        }
                    }
                    let _ = sock.shutdown().await;
                });
            }
        });
        TestServer {
            addr,
            hits,
            files,
            mode,
            open,
        }
    }
    fn prefix(&self) -> String {
        format!("http://{}/", self.addr)
    }
    fn put(&self, name: &str, bytes: &[u8]) {
        self.files
            .lock()
            .unwrap()
            .insert(name.to_string(), bytes.to_vec());
    }
    fn set_mode(&self, mode: Mode) {
        *self.mode.lock().unwrap() = mode;
    }
    fn release(&self) {
        self.open.store(true, Ordering::SeqCst);
    }
    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

struct Fx {
    dir: TempDir,
    server: TestServer,
    model: Vec<u8>,
    voices: Vec<u8>,
    coord: Arc<ModelCoordinator>,
}

impl Fx {
    fn root(&self) -> std::path::PathBuf {
        self.dir.path().join("tts_models")
    }
    fn install(&self) -> std::path::PathBuf {
        self.root().join(KOKORO_DIR_NAME)
    }
    fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(self.root())
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }
    async fn dl(&self) -> Result<(), ModelError> {
        self.coord.download(&|_| {}).await
    }
}

fn assets(server: &TestServer, model: &[u8], voices: &[u8]) -> Vec<AssetSpec> {
    vec![
        AssetSpec {
            file: KOKORO_MODEL_FILE.into(),
            url: format!("{}{}", server.prefix(), KOKORO_MODEL_FILE),
            size: model.len() as u64,
            sha256: sha(model),
        },
        AssetSpec {
            file: KOKORO_VOICES_FILE.into(),
            url: format!("{}{}", server.prefix(), KOKORO_VOICES_FILE),
            size: voices.len() as u64,
            sha256: sha(voices),
        },
    ]
}

async fn fx() -> Fx {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("tts_models")).unwrap();
    let server = TestServer::start().await;
    let model = pseudo(400_000, 1);
    let voices = pseudo(150_000, 2);
    server.put(KOKORO_MODEL_FILE, &model);
    server.put(KOKORO_VOICES_FILE, &voices);
    let coord = Arc::new(ModelCoordinator::new(
        dir.path().join("tts_models"),
        assets(&server, &model, &voices),
        vec![server.prefix()],
    ));
    Fx {
        dir,
        server,
        model,
        voices,
        coord,
    }
}

async fn wait_for(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    cond()
}

// ---- constants / allowlist -----------------------------------------------------------------------

#[test]
fn production_assets_are_pinned() {
    let a = kokoro_assets();
    assert_eq!(a.len(), 2);
    assert_eq!(a[0].file, KOKORO_MODEL_FILE);
    assert_eq!(
        (a[0].size, a[0].sha256.as_str()),
        (
            92_361_271,
            "6e742170d309016e5891a994e1ce1559c702a2ccd0075e67ef7157974f6406cb"
        )
    );
    assert_eq!(a[1].file, KOKORO_VOICES_FILE);
    assert_eq!(
        (a[1].size, a[1].sha256.as_str()),
        (
            28_311_781,
            "988549ccf1d68a80a31de1b9d00ac59d5dceb69743cbc607d1de17c7c4cdccf4"
        )
    );
    let prefixes = vec![KOKORO_URL_PREFIX.to_string()];
    for asset in &a {
        assert!(asset
            .url
            .starts_with("https://github.com/mzdk100/kokoro/releases/download/V1.0/"));
        assert!(url_allowed(&asset.url, &prefixes), "{}", asset.url);
    }
}

#[test]
fn url_allowlist_rejects_lookalikes_downgrades_and_traversal() {
    let p = vec![KOKORO_URL_PREFIX.to_string()];
    assert!(url_allowed(&format!("{KOKORO_URL_PREFIX}voices.bin"), &p));
    for bad in [
        "",
        "https://github.com.evil.example/mzdk100/kokoro/releases/download/V1.0/voices.bin",
        "http://github.com/mzdk100/kokoro/releases/download/V1.0/voices.bin",
        "https://user@github.com/mzdk100/kokoro/releases/download/V1.0/voices.bin",
        "https://github.com/mzdk100/kokoro/releases/download/V1.0/../../evil",
        "https://github.com/mzdk100/kokoro/releases/download/V1.0/..",
        "https://github.com/mzdk100/kokoro/releases/download/V1.0\\voices.bin",
        "https://github.com/mzdk100/kokoro/releases/download/V1.01/voices.bin",
    ] {
        assert!(!url_allowed(bad, &p), "{bad:?} must be refused");
    }
    assert!(!url_allowed(&format!("{KOKORO_URL_PREFIX}voices.bin"), &[]));
}

// ---- install / verify ------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_root_is_not_installed() {
    let f = fx().await;
    assert_eq!(f.coord.state(), ModelState::NotInstalled);
    assert!(matches!(
        f.coord.load_verified_bytes(),
        Err(ModelError::NotInstalled)
    ));
    assert_eq!(f.coord.delete(), Ok(()), "delete is idempotent");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_installs_both_files_ready_marker_last_with_monotonic_progress() {
    let f = fx().await;
    let seen = Arc::new(Mutex::new(Vec::<DownloadProgress>::new()));
    let s = seen.clone();
    f.coord
        .download(&move |p| s.lock().unwrap().push(p))
        .await
        .unwrap();
    assert_eq!(f.coord.state(), ModelState::Ready);
    assert_eq!(
        f.names(),
        vec![KOKORO_DIR_NAME.to_string()],
        "no staging left"
    );
    assert!(f.install().join(READY_MARKER).is_file());
    assert_eq!(
        std::fs::read(f.install().join(KOKORO_MODEL_FILE)).unwrap(),
        f.model
    );
    assert_eq!(
        std::fs::read(f.install().join(KOKORO_VOICES_FILE)).unwrap(),
        f.voices
    );
    let seen = seen.lock().unwrap();
    assert!(!seen.is_empty());
    let total = (f.model.len() + f.voices.len()) as u64;
    assert!(
        seen.iter().all(|p| p.total == total),
        "total is cumulative over both files"
    );
    assert!(
        seen.windows(2).all(|w| w[0].downloaded <= w[1].downloaded),
        "monotonic"
    );
    assert_eq!(seen.last().unwrap().downloaded, total);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn load_verified_bytes_returns_the_exact_bytes() {
    let f = fx().await;
    f.dl().await.unwrap();
    let bytes = f.coord.load_verified_bytes().unwrap();
    assert_eq!(bytes.model, f.model);
    assert_eq!(bytes.voices, f.voices);
    assert_eq!(
        f.coord.state(),
        ModelState::Ready,
        "loading leaves the state Ready"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hash_mismatch_is_rejected_and_nothing_is_published() {
    let f = fx().await;
    let mut wrong = f.voices.clone();
    wrong[10] ^= 0xFF; // same size, different content
    f.server.put(KOKORO_VOICES_FILE, &wrong);
    let err = f.dl().await.unwrap_err();
    assert!(matches!(err, ModelError::Download(_)), "{err:?}");
    assert_eq!(f.coord.state(), ModelState::NotInstalled);
    assert!(
        f.names().is_empty(),
        "nothing published, staging cleaned: {:?}",
        f.names()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_size_mismatch_is_rejected() {
    let f = fx().await;
    f.server
        .put(KOKORO_MODEL_FILE, &f.model[..f.model.len() - 1000]);
    assert!(f.dl().await.is_err());
    assert_eq!(f.coord.state(), ModelState::NotInstalled);
    assert!(f.names().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_url_outside_the_allowlist_is_refused_before_any_network_io() {
    let f = fx().await;
    let mut a = assets(&f.server, &f.model, &f.voices);
    a[1].url = "http://127.0.0.1:9/voices.bin".into();
    let coord = ModelCoordinator::new(f.root(), a, vec![f.server.prefix()]);
    assert_eq!(
        coord.download(&|_| {}).await,
        Err(ModelError::UrlNotAllowed)
    );
    assert_eq!(
        f.server.hits(),
        0,
        "not even the allowed asset may be fetched first"
    );
    assert!(f.names().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_404_is_a_download_error_and_leaves_nothing_behind() {
    let f = fx().await;
    f.server.files.lock().unwrap().remove(KOKORO_VOICES_FILE);
    assert!(matches!(f.dl().await, Err(ModelError::Download(_))));
    assert!(f.names().is_empty());
}

// ---- single-flight and cancellation -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_operation_while_downloading_is_busy() {
    let f = fx().await;
    f.server.set_mode(Mode::Gated);
    let c = f.coord.clone();
    let first = tokio::spawn(async move { c.download(&|_| {}).await });
    assert!(
        wait_for(Duration::from_secs(3), || f.coord.state()
            == ModelState::Downloading)
        .await
    );
    assert_eq!(f.dl().await, Err(ModelError::Busy));
    assert_eq!(f.coord.delete(), Err(ModelError::Busy));
    assert!(matches!(
        f.coord.load_verified_bytes(),
        Err(ModelError::Busy) | Err(ModelError::NotInstalled)
    ));
    f.server.release();
    first.await.unwrap().unwrap();
    assert_eq!(f.coord.state(), ModelState::Ready);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_stops_a_stalled_download_cleans_staging_and_a_retry_succeeds() {
    let f = fx().await;
    f.server.set_mode(Mode::StallAfter(1000));
    let c = f.coord.clone();
    let task = tokio::spawn(async move { c.download(&|_| {}).await });
    assert!(
        wait_for(Duration::from_secs(3), || f.coord.state()
            == ModelState::Downloading)
        .await
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    let t = Instant::now();
    f.coord.cancel();
    let out = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .expect("cancel must be prompt");
    assert_eq!(out.unwrap(), Err(ModelError::Cancelled));
    assert!(t.elapsed() < Duration::from_secs(3));
    assert_eq!(f.coord.state(), ModelState::NotInstalled);
    assert!(f.names().is_empty(), "staging removed: {:?}", f.names());
    f.server.set_mode(Mode::Normal);
    f.dl().await.expect("a later download works");
    assert_eq!(f.coord.state(), ModelState::Ready);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_with_nothing_running_does_not_poison_the_next_download() {
    let f = fx().await;
    f.coord.cancel();
    f.dl().await.unwrap();
    assert_eq!(f.coord.state(), ModelState::Ready);
}

// ---- delete / corruption / crash leftovers -------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_removes_the_install_and_is_idempotent() {
    let f = fx().await;
    f.dl().await.unwrap();
    assert_eq!(f.coord.delete(), Ok(()));
    assert_eq!(f.coord.state(), ModelState::NotInstalled);
    assert!(!f.install().exists());
    assert_eq!(f.coord.delete(), Ok(()));
    assert!(matches!(
        f.coord.load_verified_bytes(),
        Err(ModelError::NotInstalled)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tampered_file_is_detected_at_load_and_the_marker_removed() {
    let f = fx().await;
    f.dl().await.unwrap();
    let path = f.install().join(KOKORO_MODEL_FILE);
    let mut tampered = f.model.clone();
    tampered[5] ^= 0x01;
    std::fs::write(&path, tampered).unwrap(); // same size: only the hash can tell
    assert_eq!(f.coord.state(), ModelState::Ready, "state() does not hash");
    assert!(matches!(
        f.coord.load_verified_bytes(),
        Err(ModelError::Corrupt(_))
    ));
    assert_eq!(f.coord.state(), ModelState::NotInstalled);
    assert!(!f.install().join(READY_MARKER).exists());
    f.dl().await.expect("re-download repairs it");
    assert!(f.coord.load_verified_bytes().is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_or_resized_file_means_not_installed() {
    let f = fx().await;
    f.dl().await.unwrap();
    std::fs::remove_file(f.install().join(KOKORO_VOICES_FILE)).unwrap();
    assert_eq!(f.coord.state(), ModelState::NotInstalled);
    f.dl().await.unwrap();
    std::fs::write(f.install().join(KOKORO_MODEL_FILE), b"short").unwrap();
    assert_eq!(f.coord.state(), ModelState::NotInstalled);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_symlinked_file_is_refused() {
    let f = fx().await;
    f.dl().await.unwrap();
    let elsewhere = f.dir.path().join("elsewhere.onnx");
    std::fs::write(&elsewhere, &f.model).unwrap(); // identical content
    let path = f.install().join(KOKORO_MODEL_FILE);
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
    assert_eq!(f.coord.state(), ModelState::NotInstalled);
    assert!(f.coord.load_verified_bytes().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_staging_dirs_are_removed_on_start_and_an_unready_install_is_replaced() {
    let f = fx().await;
    let stale = f.root().join(".staging-deadbeef");
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::write(stale.join("x.partial"), b"junk").unwrap();
    // An install directory without READY (crash between rename and marker).
    std::fs::create_dir_all(f.install()).unwrap();
    std::fs::write(f.install().join(KOKORO_MODEL_FILE), &f.model).unwrap();
    std::fs::write(f.install().join(KOKORO_VOICES_FILE), &f.voices).unwrap();
    let coord = Arc::new(ModelCoordinator::new(
        f.root(),
        assets(&f.server, &f.model, &f.voices),
        vec![f.server.prefix()],
    ));
    assert!(!stale.exists(), "stale staging removed by new()");
    assert_eq!(
        coord.state(),
        ModelState::NotInstalled,
        "no READY marker: never Ready"
    );
    coord.download(&|_| {}).await.unwrap();
    assert_eq!(coord.state(), ModelState::Ready);
    assert_eq!(f.names(), vec![KOKORO_DIR_NAME.to_string()]);
}
