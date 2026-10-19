//! Model coordinator for the natural voice (Kokoro files): single-flight download with
//! staging + atomic publish + READY marker, cancellation, verified in-memory loading.
//!
//! Spec = `natural_model_tests.rs`. Signatures and doc contracts are binding. DESIGN.md D8
//! describes the intent. The download transport is the existing resumable downloader in
//! `model/download.rs` (reached through its narrow `pub(crate)` entry point
//! `ModelManager::download_verified_file`).
#![allow(dead_code)]

use crate::managers::model::ModelManager;
use crate::managers::natural_voices::ModelBytes;
use hf_hub::api::tokio::CancellationToken;
use log::{info, warn};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

const STAGING_PREFIX: &str = ".staging-";

pub const KOKORO_DIR_NAME: &str = "kokoro-v1.0";
pub const KOKORO_MODEL_FILE: &str = "kokoro-v1.0.int8.onnx";
pub const KOKORO_VOICES_FILE: &str = "voices.bin";
pub const READY_MARKER: &str = "READY";
/// The only host path production downloads may start from (redirects to the release asset hosts
/// are followed, but integrity rests on the pinned size + SHA-256, not the host).
pub const KOKORO_URL_PREFIX: &str = "https://github.com/mzdk100/kokoro/releases/download/V1.0/";
pub const KOKORO_MODEL_SIZE: u64 = 92_361_271;
pub const KOKORO_MODEL_SHA256: &str =
    "6e742170d309016e5891a994e1ce1559c702a2ccd0075e67ef7157974f6406cb";
pub const KOKORO_VOICES_SIZE: u64 = 28_311_781;
pub const KOKORO_VOICES_SHA256: &str =
    "988549ccf1d68a80a31de1b9d00ac59d5dceb69743cbc607d1de17c7c4cdccf4";

/// One pinned file to install into `<root>/kokoro-v1.0/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetSpec {
    pub file: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

/// The two production assets (model first, then voices), URLs under `KOKORO_URL_PREFIX`.
pub fn kokoro_assets() -> Vec<AssetSpec> {
    [
        (KOKORO_MODEL_FILE, KOKORO_MODEL_SIZE, KOKORO_MODEL_SHA256),
        (KOKORO_VOICES_FILE, KOKORO_VOICES_SIZE, KOKORO_VOICES_SHA256),
    ]
    .iter()
    .map(|(file, size, sha256)| AssetSpec {
        file: (*file).to_string(),
        url: format!("{KOKORO_URL_PREFIX}{file}"),
        size: *size,
        sha256: (*sha256).to_string(),
    })
    .collect()
}

/// True when `url` may be fetched: it starts with one of `prefixes`, has no userinfo (`@`),
/// no backslash, and no `..` path segment. Everything else (other host, `http` instead of
/// `https`, lookalike hosts such as `github.com.evil.example`) is refused.
pub fn url_allowed(url: &str, prefixes: &[String]) -> bool {
    let prefix_ok = prefixes
        .iter()
        .any(|p| p.ends_with('/') && url.starts_with(p.as_str()));
    prefix_ok
        && !url.contains('@')
        && !url.contains('\\')
        && !url.split('/').any(|segment| segment == "..")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    /// Another operation (download/delete/load) is running.
    Busy,
    Cancelled,
    NotInstalled,
    /// Files present but wrong (size/hash/symlink): the READY marker is removed.
    Corrupt(String),
    UrlNotAllowed,
    /// Transport / verification failure while downloading (no text from the network is logged).
    Download(String),
    Io(String),
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelError::Busy => write!(f, "another model operation is running"),
            ModelError::Cancelled => write!(f, "cancelled"),
            ModelError::NotInstalled => write!(f, "model is not installed"),
            ModelError::Corrupt(why) => write!(f, "model files are invalid: {why}"),
            ModelError::UrlNotAllowed => write!(f, "download address is not allowed"),
            ModelError::Download(why) => write!(f, "download failed: {why}"),
            ModelError::Io(why) => write!(f, "file error: {why}"),
        }
    }
}

impl std::error::Error for ModelError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelState {
    NotInstalled,
    Downloading,
    Verifying,
    Installing,
    Deleting,
    Loading,
    Ready,
}

/// Cumulative over all assets of one download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: u64,
}

/// Contract (DESIGN.md D8):
/// * `new(root, assets, allowed_prefixes)`: `root` is `<app_data>/tts_models`; removes every
///   stale `.staging-*` directory directly under `root` (it does not create `root`).
/// * `state()`: a busy state while an operation runs; otherwise `Ready` iff `<root>/kokoro-v1.0`
///   and each asset file are REGULAR files (checked with `symlink_metadata`, symlinks refused)
///   of exactly the pinned size AND the `READY` marker exists; else `NotInstalled`.
/// * `download` (async, single-flight; a concurrent download/delete/load gets `Busy`): refuses a
///   URL outside the allowlist with `UrlNotAllowed` BEFORE any network I/O; downloads each asset
///   into `<root>/.staging-<random>/` through the existing resumable downloader with the pinned
///   size and SHA-256; progress callbacks are cumulative and monotonic and end at `total`;
///   checks cancellation (and that no delete happened meanwhile) before publishing; publishes by
///   renaming the staging directory to `<root>/kokoro-v1.0` (replacing a stale/unready one) and
///   writes the `READY` marker LAST. On every error or cancel the staging directory is removed
///   and nothing is published. A cancel returns `Cancelled`; a later download works normally.
/// * `cancel()`: cancels the running download (no-op otherwise; never poisons the next one).
/// * `delete()`: `Busy` while an operation runs, otherwise removes the install directory
///   (idempotent: `Ok` when nothing is installed).
/// * `load_verified_bytes()` (blocking; called on the engine worker thread): `NotInstalled` unless
///   `state()` would be `Ready`; reads both files fully into memory, hashes THOSE buffers against
///   the pinned SHA-256; on mismatch removes the `READY` marker and returns `Corrupt`; refuses
///   symlinks. While reading the state is `Loading` (delete/download get `Busy`).
pub struct ModelCoordinator {
    root: PathBuf,
    assets: Vec<AssetSpec>,
    allowed_prefixes: Vec<String>,
    shared: Shared,
}

/// The operation currently holding the single-flight slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Idle,
    Downloading,
    Verifying,
    Installing,
    Deleting,
    Loading,
}

struct Shared {
    op: Mutex<Op>,
    progress: Mutex<DownloadProgress>,
    cancel: Mutex<Option<CancellationToken>>,
    /// Bumped by every delete so a download can tell that the install changed under it.
    generation: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// RAII ownership of the single-flight slot; dropping it returns the slot to `Idle`.
struct OpGuard<'a> {
    shared: &'a Shared,
}

impl<'a> OpGuard<'a> {
    fn acquire(shared: &'a Shared, op: Op) -> Result<Self, ModelError> {
        let mut current = lock(&shared.op);
        if *current != Op::Idle {
            return Err(ModelError::Busy);
        }
        *current = op;
        Ok(OpGuard { shared })
    }

    fn set(&self, op: Op) {
        *lock(&self.shared.op) = op;
    }
}

impl Drop for OpGuard<'_> {
    fn drop(&mut self) {
        *lock(&self.shared.op) = Op::Idle;
    }
}

fn io_err(e: std::io::Error) -> ModelError {
    ModelError::Io(e.to_string())
}

fn is_regular_file_of_size(path: &Path, size: u64) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_file() && m.len() == size)
        .unwrap_or(false)
}

fn staging_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mixed = nanos ^ u64::from(std::process::id());
    format!("{STAGING_PREFIX}{:08x}", mixed as u32)
}

impl ModelCoordinator {
    pub fn new(root: PathBuf, assets: Vec<AssetSpec>, allowed_prefixes: Vec<String>) -> Self {
        if let Ok(entries) = fs::read_dir(&root) {
            let mut removed = 0usize;
            for entry in entries.flatten() {
                let stale = entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(STAGING_PREFIX)
                    && entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if stale && fs::remove_dir_all(entry.path()).is_ok() {
                    removed += 1;
                }
            }
            if removed > 0 {
                info!("natural model: removed {removed} stale staging dir(s)");
            }
        }
        ModelCoordinator {
            root,
            assets,
            allowed_prefixes,
            shared: Shared {
                op: Mutex::new(Op::Idle),
                progress: Mutex::new(DownloadProgress::default()),
                cancel: Mutex::new(None),
                generation: AtomicU64::new(0),
            },
        }
    }

    pub fn state(&self) -> ModelState {
        match *lock(&self.shared.op) {
            Op::Downloading => return ModelState::Downloading,
            Op::Verifying => return ModelState::Verifying,
            Op::Installing => return ModelState::Installing,
            Op::Deleting => return ModelState::Deleting,
            Op::Loading => return ModelState::Loading,
            Op::Idle => {}
        }
        if self.installed_on_disk() {
            ModelState::Ready
        } else {
            ModelState::NotInstalled
        }
    }

    pub fn progress(&self) -> DownloadProgress {
        *lock(&self.shared.progress)
    }

    pub fn install_dir(&self) -> PathBuf {
        self.root.join(KOKORO_DIR_NAME)
    }

    /// Install dir + every asset are regular files of the pinned size and READY exists.
    /// Never hashes.
    fn installed_on_disk(&self) -> bool {
        let dir = self.install_dir();
        let dir_ok = fs::symlink_metadata(&dir)
            .map(|m| m.file_type().is_dir())
            .unwrap_or(false);
        dir_ok
            && self
                .assets
                .iter()
                .all(|a| is_regular_file_of_size(&dir.join(&a.file), a.size))
            && fs::symlink_metadata(dir.join(READY_MARKER))
                .map(|m| m.file_type().is_file())
                .unwrap_or(false)
    }

    pub async fn download(
        &self,
        on_progress: &(dyn Fn(DownloadProgress) + Send + Sync),
    ) -> Result<(), ModelError> {
        if !self
            .assets
            .iter()
            .all(|a| url_allowed(&a.url, &self.allowed_prefixes))
        {
            warn!("natural model: download refused (address not allowed)");
            return Err(ModelError::UrlNotAllowed);
        }
        let guard = OpGuard::acquire(&self.shared, Op::Downloading)?;
        let token = CancellationToken::new();
        *lock(&self.shared.cancel) = Some(token.clone());
        let generation = self.shared.generation.load(Ordering::SeqCst);
        let total: u64 = self.assets.iter().map(|a| a.size).sum();
        *lock(&self.shared.progress) = DownloadProgress {
            downloaded: 0,
            total,
        };
        info!(
            "natural model: download started ({} assets)",
            self.assets.len()
        );

        let staging = self.root.join(staging_name());
        let result = self
            .download_into(&guard, &staging, &token, generation, total, on_progress)
            .await;
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        *lock(&self.shared.cancel) = None;
        match &result {
            Ok(()) => info!("natural model: installed"),
            Err(ModelError::Cancelled) => info!("natural model: download cancelled"),
            Err(_) => warn!("natural model: download failed"),
        }
        drop(guard);
        result
    }

    /// Body of [`Self::download`]; the caller removes `staging` on any error.
    async fn download_into(
        &self,
        guard: &OpGuard<'_>,
        staging: &Path,
        token: &CancellationToken,
        generation: u64,
        total: u64,
        on_progress: &(dyn Fn(DownloadProgress) + Send + Sync),
    ) -> Result<(), ModelError> {
        fs::create_dir_all(&self.root).map_err(io_err)?;
        fs::create_dir_all(staging).map_err(io_err)?;

        let shared = &self.shared;
        let mut base = 0u64;
        for asset in &self.assets {
            if token.is_cancelled() {
                return Err(ModelError::Cancelled);
            }
            let partial = staging.join(format!("{}.partial", asset.file));
            let outcome = ModelManager::download_verified_file(
                &asset.file,
                &asset.url,
                &partial,
                asset.size,
                &asset.sha256,
                token,
                &|event| {
                    if let Some((downloaded, _)) = event.progress() {
                        let cumulative = base.saturating_add(downloaded.min(asset.size)).min(total);
                        let snapshot = {
                            let mut progress = lock(&shared.progress);
                            if cumulative > progress.downloaded {
                                progress.downloaded = cumulative;
                            }
                            *progress
                        };
                        on_progress(snapshot);
                    }
                },
                None,
            )
            .await
            .map_err(|e| ModelError::Download(e.to_string()))?;
            if outcome.is_cancelled() {
                return Err(ModelError::Cancelled);
            }
            fs::rename(&partial, staging.join(&asset.file)).map_err(io_err)?;
            base = base.saturating_add(asset.size);
        }

        if token.is_cancelled() || shared.generation.load(Ordering::SeqCst) != generation {
            return Err(ModelError::Cancelled);
        }
        guard.set(Op::Installing);
        let install = self.install_dir();
        if fs::symlink_metadata(&install).is_ok() {
            fs::remove_dir_all(&install).map_err(io_err)?;
        }
        fs::rename(staging, &install).map_err(io_err)?;
        let marker = fs::File::create(install.join(READY_MARKER)).map_err(io_err)?;
        let _ = marker.sync_all();
        drop(marker);

        let done = DownloadProgress {
            downloaded: total,
            total,
        };
        *lock(&shared.progress) = done;
        on_progress(done);
        Ok(())
    }

    pub fn cancel(&self) {
        if let Some(token) = lock(&self.shared.cancel).as_ref() {
            token.cancel();
        }
    }

    pub fn delete(&self) -> Result<(), ModelError> {
        let _guard = OpGuard::acquire(&self.shared, Op::Deleting)?;
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        let install = self.install_dir();
        if fs::symlink_metadata(&install).is_ok() {
            fs::remove_dir_all(&install).map_err(io_err)?;
            info!("natural model: deleted");
        }
        Ok(())
    }

    pub fn load_verified_bytes(&self) -> Result<ModelBytes, ModelError> {
        let _guard = OpGuard::acquire(&self.shared, Op::Loading)?;
        if !self.installed_on_disk() {
            return Err(ModelError::NotInstalled);
        }
        let dir = self.install_dir();
        let mut buffers = Vec::with_capacity(self.assets.len());
        for asset in &self.assets {
            let bytes = fs::read(dir.join(&asset.file)).map_err(io_err)?;
            let actual = format!("{:x}", Sha256::digest(&bytes));
            if bytes.len() as u64 != asset.size || actual != asset.sha256 {
                let _ = fs::remove_file(dir.join(READY_MARKER));
                warn!("natural model: installed file failed verification");
                return Err(ModelError::Corrupt(format!(
                    "{} does not match its pinned checksum",
                    asset.file
                )));
            }
            buffers.push(bytes);
        }
        let mut it = buffers.into_iter();
        match (it.next(), it.next(), it.next()) {
            (Some(model), Some(voices), None) => Ok(ModelBytes { model, voices }),
            _ => Err(ModelError::Corrupt(
                "expected exactly two model files".into(),
            )),
        }
    }
}
