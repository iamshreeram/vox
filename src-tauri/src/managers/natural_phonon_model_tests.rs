//! Spec for the P2 Phonon model layer (DESIGN.md Rev 4 section 3 P2 a-e, section 5, A.2/A.6).
//! Covers pins, URL allowlist, `new_named`, sibling roots, verified snapshots, the coordinator
//! op guard and the Kokoro behaviour guard. Local HTTP only; no real network.
//! Authoritative: do not weaken an assertion; if a test is wrong, report it instead.
use crate::managers::natural_model::{
    kokoro_assets, phonon_allowed_prefixes, phonon_assets, url_allowed, AssetSpec,
    ModelCoordinator, ModelError, ModelState, KOKORO_DIR_NAME, KOKORO_MODEL_FILE,
    KOKORO_MODEL_SIZE, KOKORO_URL_PREFIX, KOKORO_VOICES_FILE, KOKORO_VOICES_SIZE, PHONON_COMMIT,
    PHONON_DIR_NAME, PHONON_URL_PREFIX, READY_MARKER,
};
use crate::managers::natural_snapshot::{snapshot_verified, snapshot_verified_with, Snapshot};
use crate::managers::tts_engines::PHONON_TOTAL_BYTES;
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::ffi::OsStr;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The real pins (DESIGN.md section 3 P2 b). Order = `phonon_assets()` order.
const PHONON_PINS: [(&str, u64, &str); 4] = [
    (
        "model.q8.gguf",
        64_512_352,
        "a7d9aad2cd6dc66473f24a08bc6d2939030ef4d751ef82349361fbef3272dbae",
    ),
    (
        "default-voice.safetensors",
        256_136,
        "f739e8c4c76ee7f92e7c94a82002748c8cd22e99582ac49f7b9b94cd63786e18",
    ),
    (
        "tokenizer.json",
        277_967,
        "691a00068fa7b989f2f3e5ae5daa2d1a5bb319f2d5360deafcfb19a8bb6e7936",
    ),
    (
        "config.json",
        4_634,
        "b5d0f88353638370d94035b7d73d7d6577c0554d78f8d7ecfd40490aae39ae85",
    ),
];

/// Tiny fake assets with the real file names (snapshot tests never download).
const TINY: [(&str, &str); 4] = [
    ("model.q8.gguf", "tiny-model-weights-0001"),
    ("default-voice.safetensors", "tiny-voice-0002-abcdef"),
    ("tokenizer.json", "{\"tiny\":\"tokenizer\"}"),
    ("config.json", "{\"tiny\":true}"),
];

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

/// A root with a complete tiny install (files + READY written the way the installer does),
/// plus an empty scratch directory.
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
    fn coord(&self) -> ModelCoordinator {
        ModelCoordinator::new_named(
            self.root(),
            PHONON_DIR_NAME,
            tiny_assets(PHONON_URL_PREFIX),
            Vec::new(),
        )
    }
}

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

fn err_of(r: Result<Snapshot, ModelError>) -> ModelError {
    match r {
        Ok(_) => panic!("expected an error, got Ok(snapshot)"),
        Err(e) => e,
    }
}

// ---- (a) pins ------------------------------------------------------------------------------------

#[test]
fn phonon_pins_are_exact_and_in_order() {
    let assets = phonon_assets();
    assert_eq!(assets.len(), 4, "exactly four assets");
    for (asset, (file, size, sha256)) in assets.iter().zip(PHONON_PINS) {
        assert_eq!(asset.file, file);
        assert_eq!(asset.size, size, "{file} size");
        assert_eq!(asset.sha256, sha256, "{file} sha256");
    }
}

#[test]
fn phonon_sizes_sum_to_the_catalog_total() {
    let total: u64 = phonon_assets().iter().map(|a| a.size).sum();
    assert_eq!(total, PHONON_TOTAL_BYTES);
    assert_eq!(total, 65_051_089);
}

#[test]
fn phonon_urls_are_pinned_to_the_commit_and_never_main() {
    assert_eq!(PHONON_DIR_NAME, "phonon-31m-en");
    assert_eq!(PHONON_COMMIT, "541f5752bc3c79583068bb205bfda5eb39aaa08a");
    assert_eq!(
        PHONON_URL_PREFIX,
        format!("https://huggingface.co/gradium/phonon-31m-en/resolve/{PHONON_COMMIT}/")
    );
    for asset in phonon_assets() {
        assert!(asset.url.starts_with(PHONON_URL_PREFIX), "{}", asset.url);
        assert!(asset.url.ends_with(&asset.file), "{}", asset.url);
        assert!(asset.url.contains(PHONON_COMMIT), "{}", asset.url);
        assert!(!asset.url.contains("/main/"), "{}", asset.url);
    }
}

#[test]
fn phonon_sha256_values_are_64_lowercase_hex() {
    for asset in phonon_assets() {
        assert_eq!(asset.sha256.len(), 64, "{}", asset.file);
        assert!(
            asset
                .sha256
                .chars()
                .all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "{} must be lowercase hex",
            asset.file
        );
    }
}

// ---- (b) allowlist -------------------------------------------------------------------------------

#[test]
fn phonon_real_file_urls_are_allowed_by_the_phonon_prefix() {
    let prefixes = phonon_allowed_prefixes();
    for asset in phonon_assets() {
        assert!(url_allowed(&asset.url, &prefixes), "{}", asset.url);
    }
}

#[test]
fn phonon_allowlist_refuses_downgrade_userinfo_lookalike_and_other_repos() {
    let prefixes = phonon_allowed_prefixes();
    let file = "model.q8.gguf";
    for bad in [
        format!("http://huggingface.co/gradium/phonon-31m-en/resolve/{PHONON_COMMIT}/{file}"),
        format!(
            "https://user:pw@huggingface.co/gradium/phonon-31m-en/resolve/{PHONON_COMMIT}/{file}"
        ),
        format!(
            "https://huggingface.co.evil.example/gradium/phonon-31m-en/resolve/{PHONON_COMMIT}/{file}"
        ),
        "https://huggingface.co/other/repo/resolve/main/x".to_string(),
        format!("https://huggingface.co/gradium/phonon-31m-en/resolve/main/{file}"),
    ] {
        assert!(!url_allowed(&bad, &prefixes), "{bad} must be refused");
    }
}

#[test]
fn phonon_and_kokoro_allowlists_do_not_cross() {
    assert!(!url_allowed(
        &format!("{KOKORO_URL_PREFIX}voices.bin"),
        &phonon_allowed_prefixes()
    ));
    assert!(!url_allowed(
        &format!("{PHONON_URL_PREFIX}config.json"),
        &[KOKORO_URL_PREFIX.to_string()]
    ));
}

// ---- (c) named roots -----------------------------------------------------------------------------

#[test]
fn new_named_installs_under_the_phonon_directory() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("phonon_models");
    let coord = ModelCoordinator::new_named(
        root.clone(),
        PHONON_DIR_NAME,
        phonon_assets(),
        phonon_allowed_prefixes(),
    );
    assert_eq!(coord.install_dir(), root.join(PHONON_DIR_NAME));
    assert_eq!(coord.state(), ModelState::NotInstalled);
}

#[test]
fn new_keeps_the_kokoro_directory_and_a_fresh_root_is_not_installed() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("tts_models");
    fs::create_dir_all(&root).unwrap();
    let coord = ModelCoordinator::new(
        root.clone(),
        kokoro_assets(),
        vec![KOKORO_URL_PREFIX.to_string()],
    );
    assert_eq!(coord.install_dir(), root.join(KOKORO_DIR_NAME));
    assert_eq!(coord.state(), ModelState::NotInstalled);
}

// ---- (d) sibling roots ---------------------------------------------------------------------------

#[test]
fn sibling_roots_do_not_purge_each_other() {
    let tmp = TempDir::new().unwrap();
    let tts = tmp.path().join("tts_models");
    let phonon = tmp.path().join("phonon_models");
    fs::create_dir_all(&tts).unwrap();
    fs::create_dir_all(&phonon).unwrap();
    let staging = tts.join(".staging-aaaaaaaa");
    fs::create_dir_all(&staging).unwrap();
    fs::write(staging.join("voices.bin.partial"), b"half-written").unwrap();

    let _phonon_coord = ModelCoordinator::new_named(
        phonon.clone(),
        PHONON_DIR_NAME,
        phonon_assets(),
        phonon_allowed_prefixes(),
    );
    assert!(
        staging.join("voices.bin.partial").is_file(),
        "constructing the Phonon coordinator must not touch Kokoro's staging"
    );

    let _kokoro = ModelCoordinator::new(
        tts.clone(),
        kokoro_assets(),
        vec![KOKORO_URL_PREFIX.to_string()],
    );
    assert!(
        !staging.exists(),
        "constructing a coordinator on the Kokoro root purges its own stale staging"
    );
}

// ---- (e) verified snapshots (tiny fake assets) ---------------------------------------------------

#[test]
fn snapshot_copies_exactly_the_verified_bytes_and_drop_removes_it() {
    let t = tiny_installed();
    let snap = snapshot_verified(&t.install(), &tiny_assets(PHONON_URL_PREFIX), &t.scratch())
        .expect("a complete, verified install snapshots");
    let dir = snap.dir().to_path_buf();
    assert!(dir.is_dir());
    assert!(dir.starts_with(t.scratch()), "snapshot lives under scratch");
    assert_ne!(dir, t.install(), "never the install directory itself");
    for (file, body) in TINY {
        assert_eq!(
            fs::read(snap.path(file)).unwrap(),
            body.as_bytes(),
            "{file} content"
        );
    }
    drop(snap);
    assert!(!dir.exists(), "Drop removes the snapshot directory");
    assert_eq!(scratch_entries(&t.scratch()), 0, "scratch left empty");
}

#[cfg(unix)]
#[test]
fn snapshot_directory_is_mode_0700() {
    use std::os::unix::fs::PermissionsExt;
    let t = tiny_installed();
    let snap = snapshot_verified(&t.install(), &tiny_assets(PHONON_URL_PREFIX), &t.scratch())
        .expect("snapshot");
    let mode = fs::metadata(snap.dir()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
}

#[test]
fn swap_after_inspect_is_refused_and_leaves_scratch_empty() {
    let t = tiny_installed();
    let hook = |src: &Path| {
        if src.file_name() == Some(OsStr::new("tokenizer.json")) {
            // Same length, different bytes, after the source was inspected, before its copy.
            let mut bytes = fs::read(src).unwrap();
            bytes[0] ^= 0x01;
            fs::write(src, &bytes).unwrap();
        }
    };
    let err = err_of(snapshot_verified_with(
        &t.install(),
        &tiny_assets(PHONON_URL_PREFIX),
        &t.scratch(),
        &hook,
    ));
    assert!(matches!(err, ModelError::Corrupt(_)), "{err:?}");
    assert_eq!(
        scratch_entries(&t.scratch()),
        0,
        "half-built snapshot removed"
    );
}

#[test]
fn truncated_file_is_refused() {
    let t = tiny_installed();
    let path = t.install().join("model.q8.gguf");
    let body = TINY[0].1.as_bytes();
    fs::write(&path, &body[..body.len() - 1]).unwrap();
    let err = err_of(snapshot_verified(
        &t.install(),
        &tiny_assets(PHONON_URL_PREFIX),
        &t.scratch(),
    ));
    assert!(matches!(err, ModelError::Corrupt(_)), "{err:?}");
    assert_eq!(scratch_entries(&t.scratch()), 0);
}

#[test]
fn an_extra_file_in_the_install_dir_is_refused_before_any_copy() {
    let t = tiny_installed();
    fs::write(t.install().join("extra.bin"), b"unpinned").unwrap();
    let calls = Cell::new(0usize);
    let hook = |_: &Path| calls.set(calls.get() + 1);
    let err = err_of(snapshot_verified_with(
        &t.install(),
        &tiny_assets(PHONON_URL_PREFIX),
        &t.scratch(),
        &hook,
    ));
    assert!(matches!(err, ModelError::Corrupt(_)), "{err:?}");
    assert_eq!(calls.get(), 0, "the loader callback is never reached");
    assert_eq!(scratch_entries(&t.scratch()), 0);
}

#[test]
fn an_extra_voices_directory_is_refused_before_any_copy() {
    let t = tiny_installed();
    fs::create_dir_all(t.install().join("voices")).unwrap();
    let calls = Cell::new(0usize);
    let hook = |_: &Path| calls.set(calls.get() + 1);
    let err = err_of(snapshot_verified_with(
        &t.install(),
        &tiny_assets(PHONON_URL_PREFIX),
        &t.scratch(),
        &hook,
    ));
    assert!(matches!(err, ModelError::Corrupt(_)), "{err:?}");
    assert_eq!(calls.get(), 0, "the loader callback is never reached");
    assert_eq!(scratch_entries(&t.scratch()), 0);
}

#[cfg(unix)]
#[test]
fn a_symlinked_asset_is_refused() {
    let t = tiny_installed();
    let elsewhere = t.dir.path().join("elsewhere.gguf");
    fs::write(&elsewhere, TINY[0].1).unwrap(); // identical content: only the link is wrong
    let path = t.install().join("model.q8.gguf");
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
    let r = snapshot_verified(&t.install(), &tiny_assets(PHONON_URL_PREFIX), &t.scratch());
    assert!(r.is_err(), "a symlinked asset must be refused");
    assert_eq!(scratch_entries(&t.scratch()), 0);
}

#[test]
fn a_missing_ready_marker_is_refused_before_any_copy() {
    let t = tiny_installed();
    fs::remove_file(t.install().join(READY_MARKER)).unwrap();
    let calls = Cell::new(0usize);
    let hook = |_: &Path| calls.set(calls.get() + 1);
    let r = snapshot_verified_with(
        &t.install(),
        &tiny_assets(PHONON_URL_PREFIX),
        &t.scratch(),
        &hook,
    );
    assert!(r.is_err(), "no READY marker means not verified");
    assert_eq!(calls.get(), 0);
    assert_eq!(scratch_entries(&t.scratch()), 0);
}

#[test]
fn a_missing_asset_is_refused_before_any_copy() {
    let t = tiny_installed();
    fs::remove_file(t.install().join("config.json")).unwrap();
    let calls = Cell::new(0usize);
    let hook = |_: &Path| calls.set(calls.get() + 1);
    let r = snapshot_verified_with(
        &t.install(),
        &tiny_assets(PHONON_URL_PREFIX),
        &t.scratch(),
        &hook,
    );
    assert!(r.is_err(), "a missing asset must be refused");
    assert_eq!(calls.get(), 0);
    assert_eq!(scratch_entries(&t.scratch()), 0);
}

#[test]
fn snapshots_never_use_a_cached_verdict() {
    let t = tiny_installed();
    let assets = tiny_assets(PHONON_URL_PREFIX);
    snapshot_verified(&t.install(), &assets, &t.scratch()).expect("first snapshot verifies");
    let path = t.install().join("config.json");
    let mut bytes = fs::read(&path).unwrap();
    bytes[0] ^= 0x01; // same size, different content
    fs::write(&path, &bytes).unwrap();
    let err = err_of(snapshot_verified(&t.install(), &assets, &t.scratch()));
    assert!(matches!(err, ModelError::Corrupt(_)), "{err:?}");
    assert_eq!(scratch_entries(&t.scratch()), 0);
}

#[test]
fn coordinator_snapshot_rehashes_on_every_call() {
    let t = tiny_installed();
    let coord = t.coord();
    coord
        .snapshot(&t.scratch())
        .expect("first coordinator snapshot verifies");
    let path = t.install().join("config.json");
    let mut bytes = fs::read(&path).unwrap();
    bytes[0] ^= 0x01;
    fs::write(&path, &bytes).unwrap();
    let err = err_of(coord.snapshot(&t.scratch()));
    assert!(matches!(err, ModelError::Corrupt(_)), "{err:?}");
    assert_eq!(scratch_entries(&t.scratch()), 0);
}

// ---- (f) coordinator op guard --------------------------------------------------------------------

#[test]
fn coordinator_snapshot_holds_the_op_guard_against_delete() {
    let t = tiny_installed();
    let coord = t.coord();
    let scratch = t.scratch();

    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let entered_tx = Mutex::new(entered_tx);
    let release_rx = Mutex::new(release_rx);
    let first = AtomicBool::new(true);
    // Blocks the first asset's copy until the test releases it. Bounded wait: never hangs.
    let hook = |_: &Path| {
        if first.swap(false, Ordering::SeqCst) {
            let _ = entered_tx.lock().unwrap().send(());
            let _ = release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10));
        }
    };

    thread::scope(|s| {
        let a = s.spawn(|| coord.snapshot_with(&scratch, &hook));
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("thread A reached before_copy");

        // Thread B races a delete while A is blocked inside the snapshot.
        let b = s.spawn(|| coord.delete());
        thread::sleep(Duration::from_millis(250));
        for (file, _) in TINY {
            assert!(
                t.install().join(file).is_file(),
                "{file} must survive a delete that races a snapshot"
            );
        }
        assert!(t.install().join(READY_MARKER).is_file());

        let _ = release_tx.send(());
        let snap = a
            .join()
            .expect("thread A must not panic")
            .expect("thread A's snapshot returns Ok");
        let _b_result = b.join().expect("thread B must not panic"); // Ok or Err both accepted

        for (file, body) in TINY {
            assert_eq!(
                fs::read(snap.path(file)).unwrap(),
                body.as_bytes(),
                "{file}"
            );
        }
        let dir = snap.dir().to_path_buf();
        drop(snap);
        assert!(!dir.exists(), "no leftover snapshot directory");
    });
    assert_eq!(scratch_entries(&scratch), 0, "scratch left empty");
}

// ---- download: HTTP 407 is a distinct error ------------------------------------------------------

async fn spawn_proxy_407_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_407_is_reported_as_proxy_auth_required_and_nothing_is_published() {
    let addr = spawn_proxy_407_server().await;
    let prefix = format!("http://{addr}/");
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("phonon_models");
    let coord = ModelCoordinator::new_named(
        root.clone(),
        PHONON_DIR_NAME,
        tiny_assets(&prefix),
        vec![prefix.clone()],
    );
    let err = coord.download(&|_| {}).await.unwrap_err();
    assert_eq!(err, ModelError::ProxyAuthRequired);
    assert_eq!(coord.state(), ModelState::NotInstalled);
    assert!(!root.join(PHONON_DIR_NAME).exists(), "nothing published");
}

// ---- (g) Kokoro behaviour guard ------------------------------------------------------------------

#[test]
fn kokoro_assets_are_unchanged_by_the_phonon_split() {
    let a = kokoro_assets();
    assert_eq!(a.len(), 2);
    assert_eq!(a[0].file, KOKORO_MODEL_FILE);
    assert_eq!(a[0].size, KOKORO_MODEL_SIZE);
    assert_eq!(a[1].file, KOKORO_VOICES_FILE);
    assert_eq!(a[1].size, KOKORO_VOICES_SIZE);
    for asset in &a {
        assert!(asset.url.starts_with(KOKORO_URL_PREFIX), "{}", asset.url);
        assert!(asset.url.ends_with(&asset.file), "{}", asset.url);
    }
}

// ---- correction 1: redirect/host policy (Phonon only; Kokoro stays on the legacy redirects) -------

use crate::managers::aux_model::{url_allowed_by, DownloadPolicy, RedirectRefused};
use crate::managers::natural_model::phonon_download_policy;

#[test]
fn phonon_download_policy_allows_hugging_face_hosts_over_https() {
    let policy = phonon_download_policy();
    for url in [
        "https://huggingface.co/gradium/phonon-31m-en/resolve/x/model.q8.gguf",
        "https://cdn-lfs.huggingface.co/x",
        "https://cdn-lfs-us-1.hf.co/x",
        "https://cas-bridge.xethub.hf.co/x",
    ] {
        assert!(url_allowed_by(&policy, url), "{url} must be allowed");
    }
}

#[test]
fn phonon_download_policy_refuses_downgrades_lookalikes_userinfo_and_github() {
    let policy = phonon_download_policy();
    for url in [
        "http://huggingface.co/x",
        "https://hf.co.evil.example/x",
        "https://evilhuggingface.co/x",
        "https://huggingface.co.evil.example/x",
        "https://hf.co/x",
        "https://user:pw@huggingface.co/x",
        "https://github.com/x",
        "https://objects.githubusercontent.com/x",
    ] {
        assert!(!url_allowed_by(&policy, url), "{url} must be refused");
    }
}

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

/// Local raw-TCP server (same technique as the 407 test) that answers each request with `route`.
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
                let head = String::from_utf8_lossy(&buf).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                let _ = sock.write_all(&route(&path, port)).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    addr
}

/// Every request is redirected to another loopback address that the policy does not allow.
fn route_redirect_to_disallowed_host(path: &str, port: u16) -> Vec<u8> {
    raw_response(
        "302 Found",
        &format!("Location: http://127.0.0.2:{port}{path}\r\n"),
        b"",
    )
}

/// `/<file>` redirects (same host) to `/bytes/<file>`, which serves the tiny asset bytes.
fn route_redirect_to_allowed_target(path: &str, _port: u16) -> Vec<u8> {
    if let Some(file) = path.strip_prefix("/bytes/") {
        return match TINY.iter().find(|(name, _)| *name == file) {
            Some((_, body)) => raw_response("200 OK", "", body.as_bytes()),
            None => raw_response("404 Not Found", "", b""),
        };
    }
    raw_response("302 Found", &format!("Location: /bytes{path}\r\n"), b"")
}

/// The Phonon policy with loopback http allowed and only 127.0.0.1 allowed (test-only copy).
fn loopback_policy() -> DownloadPolicy {
    DownloadPolicy {
        allowed_hosts: vec!["127.0.0.1".into()],
        max_redirects: 4,
        require_https: true,
        allow_loopback_http: true,
    }
}

fn root_entries(root: &Path) -> Vec<String> {
    fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redirect_to_a_non_allowed_host_is_refused_and_nothing_is_published() {
    let addr = spawn_route_server(route_redirect_to_disallowed_host).await;
    let prefix = format!("http://{addr}/");
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("phonon_models");
    let coord = ModelCoordinator::new_named(
        root.clone(),
        PHONON_DIR_NAME,
        tiny_assets(&prefix),
        vec![prefix.clone()],
    )
    .with_download_policy(loopback_policy());
    let err = coord.download(&|_| {}).await.unwrap_err();
    assert_eq!(
        err,
        ModelError::Download(RedirectRefused.to_string()),
        "{err:?}"
    );
    assert_eq!(coord.state(), ModelState::NotInstalled);
    assert!(!root.join(PHONON_DIR_NAME).exists(), "nothing published");
    assert!(root_entries(&root).is_empty(), "no staging left behind");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redirect_to_an_allowed_target_succeeds_and_passes_verification() {
    let addr = spawn_route_server(route_redirect_to_allowed_target).await;
    let prefix = format!("http://{addr}/");
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("phonon_models");
    let coord = ModelCoordinator::new_named(
        root,
        PHONON_DIR_NAME,
        tiny_assets(&prefix),
        vec![prefix.clone()],
    )
    .with_download_policy(loopback_policy());
    coord
        .download(&|_| {})
        .await
        .expect("a redirect within the policy downloads and verifies");
    assert_eq!(coord.state(), ModelState::Ready);
    for (file, body) in TINY {
        assert_eq!(
            fs::read(coord.install_dir().join(file)).unwrap(),
            body.as_bytes(),
            "{file}"
        );
    }
}

#[test]
fn only_the_phonon_coordinator_carries_a_download_policy() {
    let dir = TempDir::new().unwrap();
    let kokoro = ModelCoordinator::new(
        dir.path().join("tts_models"),
        kokoro_assets(),
        vec![KOKORO_URL_PREFIX.to_string()],
    );
    assert!(
        !kokoro.has_download_policy(),
        "Kokoro keeps the legacy redirects"
    );
    let phonon = ModelCoordinator::new_named(
        dir.path().join("phonon_models"),
        PHONON_DIR_NAME,
        phonon_assets(),
        phonon_allowed_prefixes(),
    );
    assert!(!phonon.has_download_policy(), "no policy until configured");
    assert!(phonon
        .with_download_policy(phonon_download_policy())
        .has_download_policy());
}
