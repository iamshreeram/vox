//! Single-owner inference service for the optional ONNX models (design A2').
//!
//! STUB (step S8, red): the contract below compiles; every body is `todo!()`
//! until the implementation step. The authoritative lifecycle tests live in
//! `voice_models_tests.rs`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Optional ONNX model kinds served by the single inference owner.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    SmartTurn,
    Speaker,
}

/// One loaded model session. Owned exclusively by the service worker thread.
pub trait ModelSession: Send {
    /// Runs inference on 16 kHz mono samples. `None` means "no output".
    fn run(&mut self, audio16k: &[f32]) -> Option<Vec<f32>>;
}

/// Loads sessions from a verified model path. Called only on the worker thread.
pub trait SessionLoader: Send + Sync + 'static {
    fn load(&self, kind: ModelKind, path: &Path) -> Option<Box<dyn ModelSession>>;
}

/// Reply delivered for a request id. Panic payloads are never carried here.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Output(Vec<f32>),
    Failed(ErrKind),
}

/// Failure categories for a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrKind {
    Panic,
    NoOutput,
    LoadFailed,
}

/// Returned by `try_submit` when no request can be accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unavailable;

/// Next process-wide request id: one `AtomicU64`, strictly increasing, never 0.
pub fn next_request_id() -> u64 {
    todo!("S8 implementation: process-wide AtomicU64 request ids")
}

/// Single-owner inference service. One worker thread owns every session.
pub struct VoiceModelService {
    _loader: Arc<dyn SessionLoader>,
}

impl VoiceModelService {
    /// Spawns exactly one worker thread that owns every session.
    pub fn new(_loader: Arc<dyn SessionLoader>) -> Self {
        todo!("S8 implementation: spawn the single worker thread")
    }

    /// Records the verified path; resets that kind's reload counter and latch.
    pub fn install(&self, _kind: ModelKind, _path: PathBuf) {
        todo!("S8 implementation: install")
    }

    /// Removes the installed path and drops any reply for that kind's requests.
    pub fn uninstall(&self, _kind: ModelKind) {
        todo!("S8 implementation: uninstall")
    }

    pub fn is_installed(&self, _kind: ModelKind) -> bool {
        todo!("S8 implementation: is_installed")
    }

    /// Single-slot hand-off. Input longer than 128000 samples keeps the LAST 128000.
    pub fn try_submit(&self, _kind: ModelKind, _audio: Vec<f32>) -> Result<u64, Unavailable> {
        todo!("S8 implementation: try_submit")
    }

    /// Non-blocking. Consumes and returns the reply only for exactly `request_id`.
    pub fn poll(&self, _request_id: u64) -> Option<Reply> {
        todo!("S8 implementation: poll")
    }

    /// Never blocks, never joins. Skips the request if not yet started; a running
    /// inference is not interrupted.
    pub fn abandon(&self, _request_id: u64) {
        todo!("S8 implementation: abandon")
    }

    pub fn reload_count(&self, _kind: ModelKind) -> u8 {
        todo!("S8 implementation: reload_count")
    }

    pub fn is_latched_unavailable(&self, _kind: ModelKind) -> bool {
        todo!("S8 implementation: is_latched_unavailable")
    }

    /// True while any request is outstanding (submit until the thread finished it).
    pub fn outstanding(&self) -> bool {
        todo!("S8 implementation: outstanding")
    }

    /// Diagnostic: distinct thread ids that ever ran a session. Must stay 1.
    pub fn worker_thread_ids_seen(&self) -> usize {
        todo!("S8 implementation: worker_thread_ids_seen")
    }

    /// Diagnostic: number of reply slots currently holding a reply (0 or 1).
    pub fn stored_replies(&self) -> usize {
        todo!("S8 implementation: stored_replies")
    }

    /// Test helper: sets shutdown, waits up to `timeout` for the worker to exit.
    pub fn shutdown_and_join(self, _timeout: Duration) -> bool {
        todo!("S8 implementation: shutdown_and_join")
    }
}

impl Drop for VoiceModelService {
    /// Stub: intentionally empty (a `todo!()` here would abort the test binary
    /// when a red test drops a service during unwinding). The implementation
    /// sets shutdown here and never joins while an inference is blocked.
    fn drop(&mut self) {}
}

#[cfg(test)]
#[path = "voice_models_tests.rs"]
mod tests;
