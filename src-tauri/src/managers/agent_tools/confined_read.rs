// M2 stub: bodies arrive in M2-1..M2-3, this allow is removed in M2-3
#![allow(dead_code)]

//! Confined no-follow reads inside a project (DESIGN R2-2, R3-3). Every filesystem touch
//! goes through `FsGate::run`, which caps live blocking jobs at 4.

use std::path::Path;
use std::sync::Arc;

use tokio::sync::Semaphore;

/// Filesystem types the tools refuse (network or virtual drives; R3-3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsKind {
    Local,
    NetworkOrVirtual(String),
}

/// Global admission gate: 4 permits, fail-fast `busy` when saturated (R3-3).
#[derive(Debug, Clone)]
pub struct FsGate {
    permits: Arc<Semaphore>,
}

impl FsGate {
    pub fn new() -> Self {
        todo!()
    }

    /// Runs `job` on the blocking pool. The owned permit moves into the closure, so it is
    /// released only when the job finishes.
    pub async fn run<T, F>(&self, job: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let _ = job;
        todo!()
    }
}

/// Classifies the filesystem under an open root from its statfs type name or magic number.
pub fn classify_fs(type_name_or_magic: &str) -> FsKind {
    let _ = type_name_or_magic;
    todo!()
}

/// Reads one project file (relative path) through the project's pinned directory handle.
/// Returns redacted, neutralised text capped at 32 KiB.
pub async fn read_project_file(
    gate: &FsGate,
    project_dir: &Path,
    relative_path: &str,
) -> Result<String, String> {
    let _ = (gate, project_dir, relative_path);
    todo!()
}
