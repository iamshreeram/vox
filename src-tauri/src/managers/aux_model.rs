//! Verified downloader and store for the optional ONNX models (design D2, A1).
//!
//! Install layout: `<root>/<kind snake_case>/<file>`; staging: `<root>/<kind>/.staging-*/<file>.partial`.
//! `DownloadPolicy`/`policy_allows` live here (not in `model/download.rs`) because
//! `model::download` is a private module; `model/download.rs` imports them from here.
//! Spec = `aux_model_tests.rs`. Signatures and doc contracts are binding.
#![allow(dead_code)]

use crate::managers::model::ModelManager;
use hf_hub::api::tokio::CancellationToken;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs;
use std::io::Read;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

const STAGING_PREFIX: &str = ".staging-";

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

/// True when `host` is a loopback IP literal (brackets allowed for IPv6).
fn is_loopback_literal(host: &str) -> bool {
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// One allow-list entry against a lowercase host. A `.suffix` entry needs at least one extra
/// label in front of it.
fn host_matches(entry: &str, host: &str) -> bool {
    if entry.is_empty() {
        return false;
    }
    if entry.starts_with('.') {
        entry.len() > 1 && host.len() > entry.len() && host.ends_with(entry)
    } else {
        host == entry
    }
}

/// True when `scheme` + `host` may be fetched under `policy` (host rule and scheme rule).
pub fn policy_allows(policy: &DownloadPolicy, scheme: &str, host: &str) -> bool {
    let scheme = scheme.to_ascii_lowercase();
    let host = host.to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    let scheme_ok = match scheme.as_str() {
        "https" => true,
        "http" => {
            !policy.require_https || (policy.allow_loopback_http && is_loopback_literal(&host))
        }
        _ => false,
    };
    scheme_ok
        && policy
            .allowed_hosts
            .iter()
            .any(|entry| host_matches(&entry.to_ascii_lowercase(), &host))
}

/// True when the absolute `url` may be fetched under `policy`: scheme and host pass
/// [`policy_allows`], and the URL carries no userinfo. Unparseable URLs are refused.
pub(crate) fn url_allowed_by(policy: &DownloadPolicy, url: &str) -> bool {
    match reqwest::Url::parse(url) {
        Ok(parsed) => {
            parsed.username().is_empty()
                && parsed.password().is_none()
                && policy_allows(policy, parsed.scheme(), parsed.host_str().unwrap_or(""))
        }
        Err(_) => false,
    }
}

/// Marker error: a URL (start or redirect hop) was refused by the policy. Carried as the source
/// of the reqwest redirect error (or as the root error for a refused start URL).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RedirectRefused;

impl fmt::Display for RedirectRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "redirect target not allowed by download policy")
    }
}

impl std::error::Error for RedirectRefused {}

/// True when `err` was caused by a [`RedirectRefused`] anywhere in its source chain.
pub(crate) fn is_redirect_refused(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.downcast_ref::<RedirectRefused>().is_some())
}

/// Redirect policy for the reqwest client: every hop must pass [`policy_allows`], and at most
/// `max_redirects` hops are followed. A refused hop aborts with [`RedirectRefused`] before any
/// request to that host is made.
pub(crate) fn redirect_policy(policy: DownloadPolicy) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        // `previous()` holds the URLs visited before this hop: the n-th redirect sees n entries.
        let hops = attempt.previous().len();
        let url = attempt.url();
        let within_limit = hops <= policy.max_redirects as usize;
        if within_limit && policy_allows(&policy, url.scheme(), url.host_str().unwrap_or("")) {
            attempt.follow()
        } else {
            attempt.error(RedirectRefused)
        }
    })
}

/// The production policy (A1): GitHub/Hugging Face hosts only, https only, at most 4 redirects.
pub fn production_policy() -> DownloadPolicy {
    DownloadPolicy {
        allowed_hosts: vec![
            "raw.githubusercontent.com".into(),
            "github.com".into(),
            "objects.githubusercontent.com".into(),
            "release-assets.githubusercontent.com".into(),
            "huggingface.co".into(),
            ".hf.co".into(),
            ".huggingface.co".into(),
        ],
        max_redirects: 4,
        require_https: true,
        allow_loopback_http: false,
    }
}

/// The pinned production spec for `kind` (file, size, sha256, ordered urls).
pub fn aux_spec(kind: AuxKind) -> AuxSpec {
    match kind {
        AuxKind::SmartTurn => AuxSpec {
            kind,
            file: "smart-turn-v3.2-cpu.onnx".into(),
            urls: vec![
                "https://raw.githubusercontent.com/pipecat-ai/pipecat/main/src/pipecat/audio/turn/smart_turn/data/smart-turn-v3.2-cpu.onnx".into(),
                "https://huggingface.co/pipecat-ai/smart-turn-v3/resolve/main/smart-turn-v3.2-cpu.onnx".into(),
            ],
            size: 8_679_182,
            sha256: "2bb026316b14a660486a75b1733cd3fbab8c2fd0314dc9af7be49f8cca967e4f".into(),
        },
        AuxKind::SpeakerResnet34 => AuxSpec {
            kind,
            file: "wespeaker_en_voxceleb_resnet34.onnx".into(),
            urls: vec![
                "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_en_voxceleb_resnet34.onnx".into(),
            ],
            size: 26_534_365,
            sha256: "5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94".into(),
        },
    }
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

/// The operation currently holding the single-flight slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Idle,
    Downloading,
    Verifying,
    Deleting,
}

struct Shared {
    op: Mutex<Op>,
    cancel: Mutex<Option<CancellationToken>>,
    /// Cached verdict on the installed file: `None` = not checked yet by this instance.
    /// Held across publish/delete/check so a verdict never describes a replaced file.
    verdict: Mutex<Option<bool>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// RAII ownership of the single-flight slot; dropping it returns the slot to `Idle`.
struct OpGuard<'a> {
    shared: &'a Shared,
}

impl<'a> OpGuard<'a> {
    fn acquire(shared: &'a Shared, op: Op) -> Result<Self, AuxError> {
        let mut current = lock(&shared.op);
        if *current != Op::Idle {
            return Err(AuxError::Busy);
        }
        *current = op;
        Ok(OpGuard { shared })
    }
}

impl Drop for OpGuard<'_> {
    fn drop(&mut self) {
        *lock(&self.shared.op) = Op::Idle;
    }
}

fn io_err(_e: std::io::Error) -> AuxError {
    // The OS error text can contain local paths; log nothing beyond the kind of failure.
    warn!("aux model: local file operation failed");
    AuxError::Io
}

fn kind_dir_name(kind: AuxKind) -> &'static str {
    match kind {
        AuxKind::SmartTurn => "smart_turn",
        AuxKind::SpeakerResnet34 => "speaker_resnet34",
    }
}

fn is_regular_file_of_size(path: &Path, size: u64) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_file() && m.len() == size)
        .unwrap_or(false)
}

/// True when the file at `path` hashes to `expected` (lowercase hex). Any read error is a mismatch.
fn sha_matches(path: &Path, expected: &str) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buffer[..n]),
            Err(_) => return false,
        }
    }
    format!("{:x}", hasher.finalize()) == expected
}

static STAGING_SEQ: AtomicU64 = AtomicU64::new(0);

/// Unique per download attempt within and across processes.
fn staging_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = STAGING_SEQ.fetch_add(1, Ordering::SeqCst);
    format!("{:x}-{:x}-{:x}", std::process::id(), nanos, seq)
}

/// Removes every `.staging-*` directory directly inside `dir` (never follows symlinks).
fn purge_stale_staging(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGING_PREFIX)
            && entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if stale {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Failure classification across all URLs, folded into the final error.
#[derive(Default)]
struct Failures {
    refused: bool,
    mismatched: usize,
}

/// One optional model on disk under `root`. Single-flight downloads; verified paths only.
pub struct AuxModelManager {
    root: PathBuf,
    spec: AuxSpec,
    policy: DownloadPolicy,
    shared: Shared,
}

impl AuxModelManager {
    /// Purges stale `.staging-*` directories inside this kind's install dir.
    pub fn new(root: PathBuf, spec: AuxSpec, policy: DownloadPolicy) -> Self {
        let manager = AuxModelManager {
            root,
            spec,
            policy,
            shared: Shared {
                op: Mutex::new(Op::Idle),
                cancel: Mutex::new(None),
                verdict: Mutex::new(None),
            },
        };
        purge_stale_staging(&manager.kind_dir());
        manager
    }

    fn kind_dir(&self) -> PathBuf {
        self.root.join(kind_dir_name(self.spec.kind))
    }

    fn installed_path(&self) -> PathBuf {
        self.kind_dir().join(&self.spec.file)
    }

    /// Tries `spec.urls` in order (each hop checked by the policy); verifies size + sha256;
    /// publishes atomically. `Busy` while another op runs; `UrlNotAllowed` before any I/O.
    pub async fn download(
        &self,
        on_progress: &(dyn Fn(AuxProgress) + Send + Sync),
    ) -> Result<(), AuxError> {
        if self.spec.urls.is_empty()
            || !self
                .spec
                .urls
                .iter()
                .all(|url| url_allowed_by(&self.policy, url))
        {
            warn!("aux model: download refused (address not allowed)");
            return Err(AuxError::UrlNotAllowed);
        }
        let guard = OpGuard::acquire(&self.shared, Op::Downloading)?;
        let token = CancellationToken::new();
        *lock(&self.shared.cancel) = Some(token.clone());
        let result = self.run_download(&token, on_progress).await;
        *lock(&self.shared.cancel) = None;
        drop(guard);
        result
    }

    async fn run_download(
        &self,
        token: &CancellationToken,
        on_progress: &(dyn Fn(AuxProgress) + Send + Sync),
    ) -> Result<(), AuxError> {
        let staging = self
            .kind_dir()
            .join(format!("{STAGING_PREFIX}{}", staging_suffix()));
        let result = self.fetch_verified(&staging, token, on_progress).await;
        let _ = fs::remove_dir_all(&staging);
        match &result {
            Ok(()) => info!("aux model: installed"),
            Err(AuxError::Cancelled) => info!("aux model: download cancelled"),
            Err(_) => warn!("aux model: download failed"),
        }
        result
    }

    /// Body of [`Self::download`]: fetches into `staging`, publishes on the first verified source.
    async fn fetch_verified(
        &self,
        staging: &Path,
        token: &CancellationToken,
        on_progress: &(dyn Fn(AuxProgress) + Send + Sync),
    ) -> Result<(), AuxError> {
        fs::create_dir_all(staging).map_err(io_err)?;
        let partial = staging.join(format!("{}.partial", self.spec.file));
        let (kind, total) = (self.spec.kind, self.spec.size);
        let shared = &self.shared;
        // Each mirror restarts progress at zero; the high-water mark keeps callbacks monotonic.
        let high_water = AtomicU64::new(0);
        let verify_started = AtomicBool::new(false);

        let mut failures = Failures::default();
        for url in &self.spec.urls {
            if token.is_cancelled() {
                return Err(AuxError::Cancelled);
            }
            *lock(&shared.op) = Op::Downloading;
            verify_started.store(false, Ordering::SeqCst);
            let attempt = ModelManager::download_verified_file(
                &self.spec.file,
                url,
                &partial,
                self.spec.size,
                &self.spec.sha256,
                token,
                &|event| match event.progress() {
                    Some((downloaded, _)) => {
                        let downloaded = downloaded.min(total);
                        let previous = high_water.fetch_max(downloaded, Ordering::SeqCst);
                        if downloaded > previous {
                            on_progress(AuxProgress {
                                kind,
                                downloaded,
                                total,
                            });
                        }
                    }
                    // Any non-progress event is a verification event; a verification that
                    // completes always returns Ok, so a failed attempt with this set is a
                    // verification failure (hash or size).
                    None => {
                        verify_started.store(true, Ordering::SeqCst);
                        *lock(&shared.op) = Op::Verifying;
                    }
                },
                Some(&self.policy),
            )
            .await;
            match attempt {
                Ok(outcome) if outcome.is_cancelled() => return Err(AuxError::Cancelled),
                Ok(_) => return self.publish(&partial),
                Err(err) => {
                    let _ = fs::remove_file(&partial);
                    if token.is_cancelled() {
                        return Err(AuxError::Cancelled);
                    }
                    // Logged by kind only: error text and URLs may carry query strings.
                    if is_redirect_refused(&err) {
                        failures.refused = true;
                    } else if verify_started.load(Ordering::SeqCst) {
                        failures.mismatched += 1;
                    }
                    warn!("aux model: source failed, trying next");
                }
            }
        }
        if failures.refused {
            Err(AuxError::UrlNotAllowed)
        } else if failures.mismatched == self.spec.urls.len() {
            Err(AuxError::Corrupt)
        } else {
            Err(AuxError::Download)
        }
    }

    /// Moves a fully verified staged file into place. The downloader only returns `Ok` after the
    /// pinned size and SHA-256 both matched, so this only re-checks the size before renaming.
    fn publish(&self, partial: &Path) -> Result<(), AuxError> {
        if !is_regular_file_of_size(partial, self.spec.size) {
            return Err(AuxError::Corrupt);
        }
        let mut verdict = lock(&self.shared.verdict);
        fs::rename(partial, self.installed_path()).map_err(io_err)?;
        *verdict = Some(true);
        Ok(())
    }

    /// Cancels the running download (no-op otherwise).
    pub fn cancel(&self) {
        if let Some(token) = lock(&self.shared.cancel).as_ref() {
            token.cancel();
        }
    }

    /// Removes the install dir. Idempotent: `Ok` when nothing is installed.
    pub fn delete(&self) -> Result<(), AuxError> {
        let _guard = OpGuard::acquire(&self.shared, Op::Deleting)?;
        let mut verdict = lock(&self.shared.verdict);
        let dir = self.kind_dir();
        if fs::symlink_metadata(&dir).is_ok() {
            fs::remove_dir_all(&dir).map_err(io_err)?;
        }
        *verdict = None;
        Ok(())
    }

    pub fn state(&self) -> AuxState {
        match *lock(&self.shared.op) {
            Op::Downloading => return AuxState::Downloading,
            Op::Verifying => return AuxState::Verifying,
            Op::Deleting => return AuxState::NotInstalled,
            Op::Idle => {}
        }
        if self.path_if_ready().is_some() {
            AuxState::Ready
        } else {
            AuxState::NotInstalled
        }
    }

    /// The installed file path, ONLY after size + sha256 verified (full hash on the first call
    /// per instance, cached). Corrupt/changed/truncated file => `None` and state `NotInstalled`.
    pub fn path_if_ready(&self) -> Option<PathBuf> {
        let mut verdict = lock(&self.shared.verdict);
        let file = self.installed_path();
        let size_ok = is_regular_file_of_size(&file, self.spec.size);
        let valid = match *verdict {
            // A cached verdict still needs the cheap size check: the file may have been removed
            // or truncated since it was verified.
            Some(cached) => cached && size_ok,
            None => size_ok && sha_matches(&file, &self.spec.sha256),
        };
        *verdict = Some(valid);
        if valid {
            Some(file)
        } else {
            None
        }
    }
}

#[cfg(test)]
#[path = "aux_model_tests.rs"]
mod tests;
