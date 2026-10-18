//! Model coordinator for the natural voice (Kokoro files): single-flight download with
//! staging + atomic publish + READY marker, cancellation, verified in-memory loading.
//!
//! Spec = `natural_model_tests.rs`. STUB: bodies are `todo!()` until step S4. Signatures and
//! doc contracts are binding. DESIGN.md D8 describes the intent. The download transport is the
//! existing resumable downloader in `model/download.rs` (S4 adds a narrow `pub(crate)` entry
//! point there; existing callers and tests stay unchanged).
#![allow(dead_code)]

use crate::managers::natural_voices::ModelBytes;
use std::fmt;
use std::path::PathBuf;

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
    todo!("S4")
}

/// True when `url` may be fetched: it starts with one of `prefixes`, has no userinfo (`@`),
/// no backslash, and no `..` path segment. Everything else (other host, `http` instead of
/// `https`, lookalike hosts such as `github.com.evil.example`) is refused.
pub fn url_allowed(url: &str, prefixes: &[String]) -> bool {
    let _ = (url, prefixes);
    todo!("S4")
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
    _private: (),
}

impl ModelCoordinator {
    pub fn new(root: PathBuf, assets: Vec<AssetSpec>, allowed_prefixes: Vec<String>) -> Self {
        let _ = (root, assets, allowed_prefixes);
        todo!("S4")
    }

    pub fn state(&self) -> ModelState {
        todo!("S4")
    }

    pub fn progress(&self) -> DownloadProgress {
        todo!("S4")
    }

    pub fn install_dir(&self) -> PathBuf {
        todo!("S4")
    }

    pub async fn download(
        &self,
        on_progress: &(dyn Fn(DownloadProgress) + Send + Sync),
    ) -> Result<(), ModelError> {
        let _ = on_progress;
        todo!("S4")
    }

    pub fn cancel(&self) {
        todo!("S4")
    }

    pub fn delete(&self) -> Result<(), ModelError> {
        todo!("S4")
    }

    pub fn load_verified_bytes(&self) -> Result<ModelBytes, ModelError> {
        todo!("S4")
    }
}
