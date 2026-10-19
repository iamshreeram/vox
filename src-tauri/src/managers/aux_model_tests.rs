//! Spec for `aux_model` (step T3). Authoritative: do not weaken an assertion; if a test is
//! wrong, report it instead of editing it. Uses local HTTP servers only; no real network.
//!
//! Non-ignored tests never touch the network: the test policy allows only `127.0.0.1`
//! (plus `http` to loopback), and `localhost` is used as the "forbidden" host.
use crate::managers::aux_model::*;
use crate::managers::model::ModelManager;
use hf_hub::api::tokio::CancellationToken;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

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

/// Local HTTP server: one route per path. Every accepted connection is counted.
#[derive(Clone)]
enum Route {
    Body(Vec<u8>),
    Redirect(String),
    Status(u16),
    /// Sends `prefix` bytes of `body`, then holds the connection open (a slow server).
    Stall {
        body: Vec<u8>,
        prefix: usize,
    },
}

#[derive(Default)]
struct Book {
    routes: HashMap<String, Route>,
    hits: Vec<String>,
}

struct LocalServer {
    port: u16,
    conns: Arc<AtomicUsize>,
    book: Arc<Mutex<Book>>,
}

impl LocalServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let conns = Arc::new(AtomicUsize::new(0));
        let book = Arc::new(Mutex::new(Book::default()));
        let (c, b) = (conns.clone(), book.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                c.fetch_add(1, Ordering::SeqCst);
                let b = b.clone();
                tokio::spawn(async move { serve(&mut sock, &b).await });
            }
        });
        LocalServer { port, conns, book }
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Same server, reached through a host name that is NOT on the test allow-list.
    fn localhost_base(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base(), path)
    }

    fn route(&self, path: &str, route: Route) {
        self.book
            .lock()
            .unwrap()
            .routes
            .insert(path.to_string(), route);
    }

    fn hits_for(&self, path: &str) -> usize {
        self.book
            .lock()
            .unwrap()
            .hits
            .iter()
            .filter(|p| p.as_str() == path)
            .count()
    }

    fn conns(&self) -> usize {
        self.conns.load(Ordering::SeqCst)
    }
}

async fn serve(sock: &mut TcpStream, book: &Mutex<Book>) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match sock.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let head = String::from_utf8_lossy(&buf).to_string();
    let path = head
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .trim_start_matches('/')
        .to_string();
    let route = {
        let mut b = book.lock().unwrap();
        b.hits.push(path.clone());
        b.routes.get(&path).cloned()
    };
    match route {
        None => reply(sock, "404 Not Found", "", b"").await,
        Some(Route::Body(body)) => reply(sock, "200 OK", "", &body).await,
        Some(Route::Redirect(to)) => {
            reply(sock, "302 Found", &format!("Location: {to}\r\n"), b"").await
        }
        Some(Route::Status(code)) => reply(sock, &format!("{code} Error"), "", b"").await,
        Some(Route::Stall { body, prefix }) => {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(head.as_bytes()).await;
            let _ = sock.write_all(&body[..prefix.min(body.len())]).await;
            let _ = sock.flush().await;
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    }
}

async fn reply(sock: &mut TcpStream, status: &str, extra: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if sock.write_all(head.as_bytes()).await.is_ok() {
        let _ = sock.write_all(body).await;
    }
    let _ = sock.shutdown().await;
}

/// Test-only policy: loopback IP only, http allowed for loopback literals.
fn test_policy() -> DownloadPolicy {
    DownloadPolicy {
        allowed_hosts: vec!["127.0.0.1".into()],
        max_redirects: 4,
        require_https: true,
        allow_loopback_http: true,
    }
}

fn spec(urls: Vec<String>, bytes: &[u8]) -> AuxSpec {
    AuxSpec {
        kind: AuxKind::SmartTurn,
        file: "test-model.onnx".into(),
        urls,
        size: bytes.len() as u64,
        sha256: sha(bytes),
    }
}

fn manager(root: &Path, spec: AuxSpec) -> AuxModelManager {
    AuxModelManager::new(root.to_path_buf(), spec, test_policy())
}

fn new_root() -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("aux_models");
    (dir, root)
}

fn kind_dir(root: &Path) -> PathBuf {
    root.join("smart_turn")
}

fn installed_file(root: &Path) -> PathBuf {
    kind_dir(root).join("test-model.onnx")
}

/// Every `.staging-*` directory and every `*.partial` file below `root`.
fn leftovers(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".staging-") || name.ends_with(".partial") {
                out.push(path.clone());
            }
            if path.is_dir() {
                walk(&path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out
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

/// Installs `bytes` at `<root>/smart_turn/test-model.onnx` through a fresh local server.
async fn install_fixture(root: &Path, bytes: &[u8]) -> LocalServer {
    let server = LocalServer::start().await;
    server.route("f", Route::Body(bytes.to_vec()));
    manager(root, spec(vec![server.url("f")], bytes))
        .download(&|_| {})
        .await
        .expect("fixture install must succeed");
    server
}

/// `hops` redirects (`c0` -> `c1` -> ... -> `c{hops}` serves the body).
fn redirect_chain(server: &LocalServer, hops: usize, bytes: &[u8]) -> String {
    for i in 0..hops {
        server.route(
            &format!("c{i}"),
            Route::Redirect(server.url(&format!("c{}", i + 1))),
        );
    }
    server.route(&format!("c{hops}"), Route::Body(bytes.to_vec()));
    server.url("c0")
}

/// (scheme, host) of a absolute URL; test helper only.
fn scheme_host(url: &str) -> (&str, &str) {
    let (scheme, rest) = url.split_once("://").unwrap();
    let host = rest.split('/').next().unwrap();
    (scheme, host)
}

// ---------------------------------------------------------------- downloads

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn success_installs_verified_file_with_monotonic_progress() {
    let (_dir, root) = new_root();
    let server = LocalServer::start().await;
    let bytes = pseudo(300_000, 7);
    server.route("f", Route::Body(bytes.clone()));
    let m = manager(&root, spec(vec![server.url("f")], &bytes));

    let seen = Mutex::new(Vec::new());
    m.download(&|p| seen.lock().unwrap().push(p))
        .await
        .expect("download should succeed");

    let events = seen.into_inner().unwrap();
    assert!(!events.is_empty(), "progress callback must fire");
    let mut last = 0u64;
    for p in &events {
        assert_eq!(p.kind, AuxKind::SmartTurn);
        assert_eq!(p.total, bytes.len() as u64);
        assert!(p.downloaded <= p.total, "downloaded must not exceed total");
        assert!(p.downloaded >= last, "progress must be monotonic");
        last = p.downloaded;
    }
    assert_eq!(m.state(), AuxState::Ready);
    let path = m.path_if_ready().expect("verified path after install");
    assert_eq!(
        path.canonicalize().unwrap(),
        installed_file(&root).canonicalize().unwrap()
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(leftovers(&root).is_empty(), "no staging left after success");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sha_mismatch_on_every_url_is_corrupt_and_leaves_nothing() {
    let (_dir, root) = new_root();
    let server = LocalServer::start().await;
    let good = pseudo(200_000, 3);
    let wrong = pseudo(200_000, 4);
    server.route("a.onnx", Route::Body(wrong.clone()));
    server.route("b.onnx", Route::Body(wrong));
    let m = manager(
        &root,
        spec(vec![server.url("a.onnx"), server.url("b.onnx")], &good),
    );

    assert_eq!(m.download(&|_| {}).await, Err(AuxError::Corrupt));
    assert!(m.path_if_ready().is_none());
    assert!(!installed_file(&root).exists());
    assert!(leftovers(&root).is_empty(), "no partial/staging may remain");
    assert_eq!(m.state(), AuxState::NotInstalled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn size_mismatch_fails_and_leaves_nothing() {
    let (_dir, root) = new_root();
    let server = LocalServer::start().await;
    let good = pseudo(150_000, 5);
    server.route("f", Route::Body(pseudo(150_001, 5)));
    let m = manager(&root, spec(vec![server.url("f")], &good));

    let result = m.download(&|_| {}).await;
    assert!(
        matches!(result, Err(AuxError::Corrupt) | Err(AuxError::Download)),
        "size mismatch must fail, got {result:?}"
    );
    assert!(m.path_if_ready().is_none());
    assert!(!installed_file(&root).exists());
    assert!(leftovers(&root).is_empty());
    assert_eq!(m.state(), AuxState::NotInstalled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_allow_listed_url_is_refused_before_any_request() {
    let (_dir, root) = new_root();
    let server = LocalServer::start().await;
    let bytes = pseudo(10_000, 6);
    server.route("f", Route::Body(bytes.clone()));
    // Same loopback server, but `localhost` is not on the allow-list.
    let url = format!("{}/f", server.localhost_base());
    let m = manager(&root, spec(vec![url], &bytes));

    assert_eq!(m.download(&|_| {}).await, Err(AuxError::UrlNotAllowed));
    assert_eq!(server.conns(), 0, "a refused URL must never be contacted");
    assert!(leftovers(&root).is_empty());
    assert_eq!(m.state(), AuxState::NotInstalled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_url_404_falls_back_to_second_url() {
    let (_dir, root) = new_root();
    let server = LocalServer::start().await;
    let bytes = pseudo(180_000, 8);
    server.route("ok", Route::Body(bytes.clone()));
    // "missing" is not routed: the server answers 404.
    let m = manager(
        &root,
        spec(vec![server.url("missing"), server.url("ok")], &bytes),
    );

    m.download(&|_| {}).await.expect("second url must succeed");
    assert_eq!(server.hits_for("ok"), 1, "second url must be the one used");
    let path = m.path_if_ready().expect("verified path");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_url_wrong_bytes_falls_back_to_second_url() {
    let (_dir, root) = new_root();
    let server = LocalServer::start().await;
    let bytes = pseudo(180_000, 9);
    server.route("bad", Route::Body(pseudo(180_000, 10)));
    server.route("ok", Route::Body(bytes.clone()));
    let m = manager(
        &root,
        spec(vec![server.url("bad"), server.url("ok")], &bytes),
    );

    m.download(&|_| {}).await.expect("second url must succeed");
    assert_eq!(server.hits_for("ok"), 1);
    let path = m.path_if_ready().expect("verified path");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

// ------------------------------------------------------------ redirect safety (A1)

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redirect_to_forbidden_host_is_refused_and_forbidden_host_untouched() {
    let (_dir, root) = new_root();
    let allowed = LocalServer::start().await;
    let forbidden = LocalServer::start().await;
    let bytes = pseudo(100_000, 12);
    forbidden.route("f", Route::Body(bytes.clone()));
    allowed.route(
        "f",
        Route::Redirect(format!("{}/f", forbidden.localhost_base())),
    );
    let m = manager(&root, spec(vec![allowed.url("f")], &bytes));

    assert_eq!(m.download(&|_| {}).await, Err(AuxError::UrlNotAllowed));
    assert_eq!(
        forbidden.conns(),
        0,
        "a redirect into a forbidden host must never be followed"
    );
    assert!(m.path_if_ready().is_none());
    assert!(leftovers(&root).is_empty());
    assert_eq!(m.state(), AuxState::NotInstalled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn allowed_redirect_chains_within_limit_succeed() {
    // max_redirects is 4: 2 hops and the boundary of 4 hops must both succeed.
    for hops in [2usize, 4] {
        let (_dir, root) = new_root();
        let server = LocalServer::start().await;
        let bytes = pseudo(90_000, 13 + hops as u64);
        let url = redirect_chain(&server, hops, &bytes);
        let m = manager(&root, spec(vec![url], &bytes));

        m.download(&|_| {})
            .await
            .unwrap_or_else(|e| panic!("{hops}-hop chain must succeed, got {e:?}"));
        assert_eq!(m.state(), AuxState::Ready, "{hops}-hop chain");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redirect_chain_over_max_redirects_fails() {
    let (_dir, root) = new_root();
    let server = LocalServer::start().await;
    let bytes = pseudo(90_000, 21);
    let url = redirect_chain(&server, 5, &bytes); // 5 > max_redirects (4)
    let m = manager(&root, spec(vec![url], &bytes));

    assert!(m.download(&|_| {}).await.is_err(), "5 hops must fail");
    assert!(m.path_if_ready().is_none());
    assert!(!installed_file(&root).exists());
    assert!(leftovers(&root).is_empty());
    assert_eq!(m.state(), AuxState::NotInstalled);
}

#[test]
fn policy_allows_matches_production_host_rules() {
    let p = production_policy();
    assert!(policy_allows(&p, "https", "github.com"));
    assert!(policy_allows(&p, "https", "raw.githubusercontent.com"));
    assert!(policy_allows(&p, "https", "huggingface.co"));
    assert!(
        policy_allows(&p, "https", "cdn.hf.co"),
        "suffix rule .hf.co"
    );
    assert!(!policy_allows(&p, "https", "evil.example"));
    assert!(!policy_allows(&p, "https", "hf.co.evil.example"));
    assert!(!policy_allows(&p, "https", "github.com.evil.example"));
    // https -> http downgrade (and any plain http) is rejected when require_https is set.
    assert!(!policy_allows(&p, "http", "github.com"));
    assert!(!policy_allows(&p, "http", "huggingface.co"));
}

#[test]
fn policy_allows_test_policy_and_require_https() {
    let t = test_policy();
    assert!(
        policy_allows(&t, "http", "127.0.0.1"),
        "loopback http allowed in tests"
    );
    assert!(policy_allows(&t, "https", "127.0.0.1"));
    assert!(!policy_allows(&t, "http", "localhost"));
    assert!(!policy_allows(&t, "https", "localhost"));

    let strict = DownloadPolicy {
        allowed_hosts: vec!["example.test".into()],
        max_redirects: 4,
        require_https: true,
        allow_loopback_http: true,
    };
    assert!(policy_allows(&strict, "https", "example.test"));
    assert!(
        !policy_allows(&strict, "http", "example.test"),
        "https->http downgrade"
    );
}

// ------------------------------------------------------------ cancel / single-flight

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_mid_download_cleans_up_and_next_download_succeeds() {
    let (_dir, root) = new_root();
    let server = LocalServer::start().await;
    let bytes = pseudo(400_000, 31);
    server.route(
        "f",
        Route::Stall {
            body: bytes.clone(),
            prefix: 64 * 1024,
        },
    );
    let m = Arc::new(manager(&root, spec(vec![server.url("f")], &bytes)));

    let task = {
        let m = m.clone();
        tokio::spawn(async move { m.download(&|_| {}).await })
    };
    assert!(
        wait_for(Duration::from_secs(10), || server.hits_for("f") >= 1).await,
        "download never reached the server"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    m.cancel();

    let result = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("cancel must return promptly")
        .expect("download task must not panic");
    assert_eq!(result, Err(AuxError::Cancelled));
    assert!(
        leftovers(&root).is_empty(),
        "no partial or staging dir may survive a cancel"
    );
    assert!(!installed_file(&root).exists());
    assert_eq!(m.state(), AuxState::NotInstalled);
    assert!(m.path_if_ready().is_none());

    server.route("f", Route::Body(bytes.clone()));
    m.download(&|_| {})
        .await
        .expect("a download after a cancel must succeed");
    assert_eq!(m.state(), AuxState::Ready);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_concurrent_download_is_busy() {
    let (_dir, root) = new_root();
    let server = LocalServer::start().await;
    let bytes = pseudo(400_000, 41);
    server.route(
        "f",
        Route::Stall {
            body: bytes.clone(),
            prefix: 32 * 1024,
        },
    );
    let m = Arc::new(manager(&root, spec(vec![server.url("f")], &bytes)));

    let first = {
        let m = m.clone();
        tokio::spawn(async move { m.download(&|_| {}).await })
    };
    assert!(
        wait_for(Duration::from_secs(10), || server.hits_for("f") >= 1).await,
        "first download never started"
    );

    assert_eq!(m.download(&|_| {}).await, Err(AuxError::Busy));
    assert_eq!(m.state(), AuxState::Downloading);

    m.cancel();
    let result = tokio::time::timeout(Duration::from_secs(10), first)
        .await
        .expect("cancel must return promptly")
        .expect("download task must not panic");
    assert_eq!(result, Err(AuxError::Cancelled));
}

// ------------------------------------------------------------ delete / restart / on-disk

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_removes_installed_file_and_is_idempotent() {
    let (_dir, root) = new_root();
    let bytes = pseudo(64_000, 51);
    let _server = install_fixture(&root, &bytes).await;
    let m = manager(
        &root,
        spec(vec!["http://127.0.0.1:9/unused".into()], &bytes),
    );
    assert_eq!(m.state(), AuxState::Ready);

    m.delete()
        .expect("delete of an installed model must succeed");
    assert_eq!(m.state(), AuxState::NotInstalled);
    assert!(m.path_if_ready().is_none());
    assert!(!installed_file(&root).exists());

    // Contract choice: delete when nothing is installed is Ok (idempotent), state stays NotInstalled.
    assert_eq!(m.delete(), Ok(()));
    assert_eq!(m.state(), AuxState::NotInstalled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_purges_stale_staging_dirs() {
    let (_dir, root) = new_root();
    let stale = kind_dir(&root).join(".staging-old");
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::write(stale.join("test-model.onnx.partial"), b"junk").unwrap();

    let _m = manager(&root, spec(vec!["http://127.0.0.1:9/unused".into()], b"x"));

    assert!(!stale.exists(), "new() must purge stale .staging-* dirs");
    assert!(leftovers(&root).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flipped_byte_on_disk_is_not_ready_for_a_new_instance() {
    let (_dir, root) = new_root();
    let bytes = pseudo(64_000, 61);
    let _server = install_fixture(&root, &bytes).await;

    let file = installed_file(&root);
    let mut data = std::fs::read(&file).unwrap();
    data[0] ^= 0xFF;
    std::fs::write(&file, &data).unwrap();

    let fresh = manager(
        &root,
        spec(vec!["http://127.0.0.1:9/unused".into()], &bytes),
    );
    assert!(
        fresh.path_if_ready().is_none(),
        "flipped byte must not verify"
    );
    assert_eq!(fresh.state(), AuxState::NotInstalled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn truncated_file_on_disk_is_not_ready_for_a_new_instance() {
    let (_dir, root) = new_root();
    let bytes = pseudo(64_000, 71);
    let _server = install_fixture(&root, &bytes).await;

    let file = installed_file(&root);
    let data = std::fs::read(&file).unwrap();
    std::fs::write(&file, &data[..data.len() - 1]).unwrap();

    let fresh = manager(
        &root,
        spec(vec!["http://127.0.0.1:9/unused".into()], &bytes),
    );
    assert!(
        fresh.path_if_ready().is_none(),
        "truncated file must not verify"
    );
    assert_eq!(fresh.state(), AuxState::NotInstalled);
}

// ------------------------------------------------------------ production pins

#[test]
fn aux_spec_pins_match_the_design_constants() {
    let st = aux_spec(AuxKind::SmartTurn);
    assert_eq!(st.kind, AuxKind::SmartTurn);
    assert_eq!(st.file, "smart-turn-v3.2-cpu.onnx");
    assert_eq!(st.size, 8_679_182);
    assert_eq!(
        st.sha256,
        "2bb026316b14a660486a75b1733cd3fbab8c2fd0314dc9af7be49f8cca967e4f"
    );
    assert_eq!(
        st.urls,
        vec![
            "https://raw.githubusercontent.com/pipecat-ai/pipecat/main/src/pipecat/audio/turn/smart_turn/data/smart-turn-v3.2-cpu.onnx".to_string(),
            "https://huggingface.co/pipecat-ai/smart-turn-v3/resolve/main/smart-turn-v3.2-cpu.onnx".to_string(),
        ]
    );

    let sr = aux_spec(AuxKind::SpeakerResnet34);
    assert_eq!(sr.kind, AuxKind::SpeakerResnet34);
    assert_eq!(sr.file, "wespeaker_en_voxceleb_resnet34.onnx");
    assert_eq!(sr.size, 26_534_365);
    assert_eq!(
        sr.sha256,
        "5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94"
    );
    assert_eq!(
        sr.urls,
        vec![
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_en_voxceleb_resnet34.onnx".to_string(),
        ]
    );
}

#[test]
fn production_policy_and_every_production_url_pass_it() {
    let prod = production_policy();
    assert_eq!(prod.max_redirects, 4);
    assert!(prod.require_https);
    assert!(!prod.allow_loopback_http);
    assert_eq!(
        prod.allowed_hosts,
        vec![
            "raw.githubusercontent.com".to_string(),
            "github.com".to_string(),
            "objects.githubusercontent.com".to_string(),
            "release-assets.githubusercontent.com".to_string(),
            "huggingface.co".to_string(),
            ".hf.co".to_string(),
            ".huggingface.co".to_string(),
        ]
    );
    for kind in [AuxKind::SmartTurn, AuxKind::SpeakerResnet34] {
        for url in aux_spec(kind).urls {
            let (scheme, host) = scheme_host(&url);
            assert!(
                policy_allows(&prod, scheme, host),
                "production url refused: {url}"
            );
        }
    }
}

#[test]
fn serde_names_are_snake_case() {
    assert_eq!(
        serde_json::to_string(&AuxKind::SmartTurn).unwrap(),
        "\"smart_turn\""
    );
    assert_eq!(
        serde_json::to_string(&AuxKind::SpeakerResnet34).unwrap(),
        "\"speaker_resnet34\""
    );

    assert_eq!(
        serde_json::to_string(&AuxState::NotInstalled).unwrap(),
        "\"not_installed\""
    );
    assert_eq!(
        serde_json::to_string(&AuxState::Downloading).unwrap(),
        "\"downloading\""
    );
    assert_eq!(
        serde_json::to_string(&AuxState::Verifying).unwrap(),
        "\"verifying\""
    );
    assert_eq!(
        serde_json::to_string(&AuxState::Ready).unwrap(),
        "\"ready\""
    );
    assert_eq!(
        serde_json::to_string(&AuxState::Error).unwrap(),
        "\"error\""
    );

    assert_eq!(serde_json::to_string(&AuxError::Busy).unwrap(), "\"busy\"");
    assert_eq!(
        serde_json::to_string(&AuxError::Cancelled).unwrap(),
        "\"cancelled\""
    );
    assert_eq!(
        serde_json::to_string(&AuxError::NotInstalled).unwrap(),
        "\"not_installed\""
    );
    assert_eq!(
        serde_json::to_string(&AuxError::Corrupt).unwrap(),
        "\"corrupt\""
    );
    assert_eq!(
        serde_json::to_string(&AuxError::UrlNotAllowed).unwrap(),
        "\"url_not_allowed\""
    );
    assert_eq!(
        serde_json::to_string(&AuxError::Download).unwrap(),
        "\"download\""
    );
    assert_eq!(serde_json::to_string(&AuxError::Io).unwrap(), "\"io\"");
}

// ------------------------------------------------------------ legacy path (A1: unchanged)

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_download_verified_file_with_none_policy_still_follows_redirects() {
    let dir = TempDir::new().unwrap();
    let server = LocalServer::start().await;
    let bytes = pseudo(120_000, 81);
    server.route("final.bin", Route::Body(bytes.clone()));
    server.route("hop.bin", Route::Redirect(server.url("final.bin")));
    let partial = dir.path().join("legacy.partial");

    let outcome = ModelManager::download_verified_file(
        "legacy",
        &server.url("hop.bin"),
        &partial,
        bytes.len() as u64,
        &sha(&bytes),
        &CancellationToken::new(),
        &|_| {},
        None,
    )
    .await
    .expect("legacy download must succeed");

    assert!(!outcome.is_cancelled());
    assert_eq!(std::fs::read(&partial).unwrap(), bytes);
}
