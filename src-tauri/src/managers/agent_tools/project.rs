// M2 stub: bodies arrive in M2-1..M2-3, this allow is removed in M2-3
#![allow(dead_code)]

//! Project root and name resolution (DESIGN R2-3). Roots are pinned once as directory
//! handles; a project is a bare name opened relative to its root handle.

use std::fmt;
use std::path::PathBuf;

use cap_std::fs::Dir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectError {
    InvalidName,
    NotFound,
    /// Same name found in more than one configured root.
    Ambiguous(usize),
    /// Symlinked child, not a plain folder.
    NotPlainFolder,
    RootInvalid,
    NoRoots,
}

/// A resolved project: the pinned directory handle (opened no-follow relative to the
/// pinned root handle) plus the project NAME. No path string is kept for display.
pub struct ProjectHandle {
    name: String,
    dir: Dir,
}

impl ProjectHandle {
    /// The project name (the only identity exposed for display).
    pub fn name(&self) -> &str {
        todo!()
    }

    /// The pinned directory handle every file read goes through.
    pub fn dir(&self) -> &Dir {
        todo!()
    }
}

impl fmt::Debug for ProjectHandle {
    /// Shows the name only; never the handle internals or any path.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectHandle")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProjectResolver {
    roots: Vec<PathBuf>,
}

impl ProjectResolver {
    /// Validates and canonicalises each configured root once (must be absolute, existing,
    /// a directory). Invalid roots are skipped.
    pub fn new(configured_roots: &[String]) -> Self {
        let _ = configured_roots;
        todo!()
    }

    /// Resolves a bare project name to a pinned project handle.
    pub fn resolve(&self, name: &str) -> Result<ProjectHandle, ProjectError> {
        let _ = name;
        todo!()
    }

    /// Immediate plain-directory names across roots: sorted, at most 50, at most 500
    /// entries read per root.
    pub fn list_projects(&self) -> Vec<String> {
        todo!()
    }
}

#[cfg(test)]
#[path = "project_tests.rs"]
mod project_tests;
