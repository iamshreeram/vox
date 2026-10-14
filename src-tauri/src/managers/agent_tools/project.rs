// M2 stub: bodies arrive in M2-1..M2-3, this allow is removed in M2-3
#![allow(dead_code)]

//! Project root and name resolution (DESIGN R2-3). Roots are pinned once as directory
//! handles; a project is a bare name opened relative to its root handle.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    InvalidName,
    NotFound,
    /// Same name found in more than one configured root.
    Ambiguous(usize),
    /// Symlinked child, not a plain folder.
    NotPlainFolder,
    RootInvalid,
    NoRoots,
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

    /// Resolves a bare project name to its canonical folder path.
    pub fn resolve(&self, name: &str) -> Result<PathBuf, ResolveError> {
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
