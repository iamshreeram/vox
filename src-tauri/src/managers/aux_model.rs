//! Verified downloader and store for the optional ONNX models (design D2, A1).
//!
//! STUB (step T3): types are real, bodies are `todo!()`. Spec = `aux_model_tests.rs`.
//! Signatures and doc contracts are binding.
//!
//! Install layout: `<root>/<kind snake_case>/<file>`; staging: `<root>/<kind>/.staging-*/<file>.partial`.
//! `DownloadPolicy`/`policy_allows` live here (not in `model/download.rs`) because
//! `model::download` is a private module; `model/download.rs` imports them from here.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The optional model kinds. Directory name = snake_case serde name.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum AuxKind {
    SmartTurn,
    SpeakerResnet34,
}

/// One optional model: ordered mirror list, pinned size and SHA-256 (lowercase hex).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuxSpec {
    pub kind: AuxKind,
    pub file: String,
    pub urls: Vec<String>,
    pub size: u64,
    pub sha256: String,
}

/// Redirect/host policy for the downloader.
///
/// `allowed_hosts` entries: an exact host (e.g. `github.com`), or a suffix rule when the entry
/// starts with `.` (e.g. `.hf.co` matches `cdn.hf.co` but NOT `hf.co` and NOT `hf.co.evil.example`).
/// Every URL (the start URL and every redirect hop) must pass [`policy_allows`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadPolicy {
    pub allowed_hosts: Vec<String>,
    pub max_redirects: u8,
    pub require_https: bool,
    /// Test-only escape hatch: permits `http` to a loopback host literal.
    pub allow_loopback_http: bool,
}

/// True when `scheme` + `host` may be fetched under `policy` (host rule and scheme rule).
pub fn policy_allows(_policy: &DownloadPolicy, _scheme: &str, _host: &str) -> bool {
    todo!("policy_allows is not implemented yet (T3 stub)")
}

/// The production policy (A1): GitHub/Hugging Face hosts only, https only, at most 4 redirects.
pub fn production_policy() -> DownloadPolicy {
    todo!("production_policy is not implemented yet (T3 stub)")
}

/// The pinned production spec for `kind` (file, size, sha256, ordered urls).
pub fn aux_spec(_kind: AuxKind) -> AuxSpec {
    todo!("aux_spec is not implemented yet (T3 stub)")
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum AuxError {
    Busy,
    Cancelled,
    NotInstalled,
    Corrupt,
    UrlNotAllowed,
    Download,
    Io,
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum AuxState {
    NotInstalled,
    Downloading,
    Verifying,
    Ready,
    Error,
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq, specta::Type)]
pub struct AuxProgress {
    pub kind: AuxKind,
    pub downloaded: u64,
    pub total: u64,
}

/// One optional model on disk under `root`. Single-flight downloads; verified paths only.
pub struct AuxModelManager {
    root: PathBuf,
    spec: AuxSpec,
    policy: DownloadPolicy,
}

impl AuxModelManager {
    /// Purges stale `.staging-*` directories inside this kind's install dir.
    pub fn new(root: PathBuf, spec: AuxSpec, policy: DownloadPolicy) -> Self {
        let _ = (root, spec, policy);
        todo!("AuxModelManager::new is not implemented yet (T3 stub)")
    }

    /// Tries `spec.urls` in order (each hop checked by the policy); verifies size + sha256;
    /// publishes atomically. `Busy` while another op runs; `UrlNotAllowed` before any I/O.
    pub async fn download(
        &self,
        on_progress: &(dyn Fn(AuxProgress) + Send + Sync),
    ) -> Result<(), AuxError> {
        let _ = on_progress;
        todo!("AuxModelManager::download is not implemented yet (T3 stub)")
    }

    /// Cancels the running download (no-op otherwise).
    pub fn cancel(&self) {
        todo!("AuxModelManager::cancel is not implemented yet (T3 stub)")
    }

    /// Removes the install dir. Idempotent: `Ok` when nothing is installed.
    pub fn delete(&self) -> Result<(), AuxError> {
        todo!("AuxModelManager::delete is not implemented yet (T3 stub)")
    }

    pub fn state(&self) -> AuxState {
        todo!("AuxModelManager::state is not implemented yet (T3 stub)")
    }

    /// The installed file path, ONLY after size + sha256 verified (full hash on the first call
    /// per instance, cached). Corrupt/changed/truncated file => `None` and state `NotInstalled`.
    pub fn path_if_ready(&self) -> Option<PathBuf> {
        todo!("AuxModelManager::path_if_ready is not implemented yet (T3 stub)")
    }
}

#[cfg(test)]
#[path = "aux_model_tests.rs"]
mod tests;
