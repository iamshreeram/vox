//! Project root and name resolution (DESIGN R2-3). Roots are pinned once as directory
//! handles; a project is a bare name opened relative to its root handle.

use std::fmt;
use std::sync::Arc;

#[cfg(unix)]
use std::collections::{BTreeSet, HashSet};
#[cfg(unix)]
use std::path::{Path, PathBuf};

#[cfg(unix)]
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;

#[cfg(unix)]
use super::confined_read::{fs_kind_of, FsKind};

/// Longest project name in bytes (DESIGN R2-3).
#[cfg(unix)]
const MAX_NAME_BYTES: usize = 255;
/// Most directory entries read per root when listing (DESIGN R2-3).
#[cfg(unix)]
const LIST_ENTRY_LIMIT: usize = 500;
/// Most project names returned by `list_projects` (DESIGN R2-3).
#[cfg(unix)]
const LIST_NAME_LIMIT: usize = 50;

/// Message for roots on network or virtual filesystems (DESIGN R3-3).
pub const NETWORK_FOLDER_MESSAGE: &str =
    "that folder is on a network or virtual drive; I only read local folders";
/// Message for platforms where the confined tools are not implemented yet.
pub const UNSUPPORTED_MESSAGE: &str = "not supported on this platform yet";

/// Why a project could not be resolved. `Display` never contains root paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectError {
    InvalidName,
    NotFound,
    /// Same name found in more than one configured root (the count of matches).
    Ambiguous(usize),
    /// Symlinked child, not a plain folder.
    NotPlainFolder,
    /// Only constructed by the resolver when a root cannot be opened.
    #[allow(dead_code)]
    RootInvalid,
    NoRoots,
    /// The only configured roots are on a network or virtual filesystem.
    NetworkOrVirtual,
    /// Confined project access is not implemented on this platform yet.
    /// Only constructed on the non-unix stub path.
    #[allow(dead_code)]
    Unsupported,
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            ProjectError::InvalidName => "that is not a valid project name".to_string(),
            ProjectError::NotFound => "no such project in your project folders".to_string(),
            ProjectError::Ambiguous(count) => format!("ambiguous: found in {count} folders"),
            ProjectError::NotPlainFolder => "not a plain folder".to_string(),
            ProjectError::RootInvalid => "no valid project folder".to_string(),
            ProjectError::NoRoots => {
                "No project folders are configured in Vox settings.".to_string()
            }
            ProjectError::NetworkOrVirtual => NETWORK_FOLDER_MESSAGE.to_string(),
            ProjectError::Unsupported => UNSUPPORTED_MESSAGE.to_string(),
        };
        f.write_str(&text)
    }
}

impl std::error::Error for ProjectError {}

/// A resolved project: the pinned directory handle (opened no-follow relative to the
/// pinned root handle) plus the project NAME. No path string is kept for display.
pub struct ProjectHandle {
    name: String,
    dir: Dir,
    /// Canonical project path captured at resolve time. Only used for git invocation via
    /// `git_dir`, never for display or file reads.
    #[cfg(unix)]
    git_path: PathBuf,
}

impl ProjectHandle {
    /// The project name (the only identity exposed for display).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The pinned directory handle every file read goes through.
    pub fn dir(&self) -> &Dir {
        &self.dir
    }

    /// Canonical project path for git. Callers must first check
    /// `path_still_matches_pinned`.
    #[cfg(unix)]
    pub(crate) fn git_dir(&self) -> &Path {
        &self.git_path
    }

    /// `true` only when the path still names the pinned directory: a plain directory (not a
    /// symlink) with the same device and inode as the pinned handle. Unix only.
    #[cfg(unix)]
    pub(crate) fn path_still_matches_pinned(&self) -> bool {
        let (Ok(pinned), Ok(now)) = (
            self.dir.dir_metadata(),
            std::fs::symlink_metadata(&self.git_path),
        ) else {
            return false;
        };
        now.file_type().is_dir()
            && !now.file_type().is_symlink()
            && std::os::unix::fs::MetadataExt::dev(&now) == cap_fs_ext::MetadataExt::dev(&pinned)
            && std::os::unix::fs::MetadataExt::ino(&now) == cap_fs_ext::MetadataExt::ino(&pinned)
    }

    /// Non-unix builds never resolve projects, so there is nothing to match.
    #[cfg(not(unix))]
    pub(crate) fn path_still_matches_pinned(&self) -> bool {
        false
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

/// Pinned, canonical configured roots. Cloning shares the same handles.
#[derive(Debug, Clone, Default)]
pub struct ProjectResolver {
    roots: Arc<Vec<Dir>>,
    /// Canonical root paths, index-aligned with `roots` (used only for git invocation).
    #[cfg(unix)]
    root_paths: Arc<Vec<PathBuf>>,
    /// Count of configured roots refused because they are on a network or virtual
    /// filesystem (used to pick the error when no usable root remains).
    network_roots: usize,
}

impl ProjectResolver {
    /// Validates and canonicalises each configured root once (must be absolute, existing,
    /// a directory, and on a local filesystem). Invalid roots are skipped.
    pub fn new(configured_roots: &[String]) -> Self {
        open_roots(configured_roots)
    }

    /// Resolves a bare project name to a pinned project handle.
    pub fn resolve(&self, name: &str) -> Result<ProjectHandle, ProjectError> {
        resolve_in(&self.roots, self.root_paths(), self.network_roots, name)
    }

    #[cfg(unix)]
    fn root_paths(&self) -> &[PathBuf] {
        &self.root_paths
    }

    /// Non-unix builds keep no root paths.
    #[cfg(not(unix))]
    fn root_paths(&self) -> &[PathBuf] {
        &[]
    }

    /// Immediate plain-directory names across roots: sorted, at most 50, at most 500
    /// entries read per root.
    pub fn list_projects(&self) -> Vec<String> {
        list_in(&self.roots)
    }
}

/// Opens each usable configured root once, skipping invalid, duplicate and network roots.
#[cfg(unix)]
fn open_roots(configured: &[String]) -> ProjectResolver {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut roots = Vec::new();
    let mut root_paths = Vec::new();
    let mut network_roots = 0;
    for raw in configured {
        if raw.is_empty() || !Path::new(raw).is_absolute() {
            continue;
        }
        let Ok(canonical) = std::fs::canonicalize(raw) else {
            continue;
        };
        if !seen.insert(canonical.clone()) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&canonical) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let Ok(dir) = Dir::open_ambient_dir(&canonical, cap_std::ambient_authority()) else {
            continue;
        };
        match fs_kind_of(&dir) {
            Ok(FsKind::Local) => {
                roots.push(dir);
                root_paths.push(canonical.clone());
            }
            Ok(FsKind::NetworkOrVirtual(_)) => network_roots += 1,
            Err(_) => {}
        }
    }
    ProjectResolver {
        roots: Arc::new(roots),
        root_paths: Arc::new(root_paths),
        network_roots,
    }
}

/// Non-unix builds keep no roots: every configured root is ignored.
#[cfg(not(unix))]
fn open_roots(configured: &[String]) -> ProjectResolver {
    let _ = configured;
    ProjectResolver::default()
}

/// Checks that a project name is a bare name: non-empty, no separators, not `.` or `..`,
/// no NUL, at most 255 bytes.
#[cfg(unix)]
fn validate_name(name: &str) -> Result<(), ProjectError> {
    let bare = !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0']);
    if bare {
        Ok(())
    } else {
        Err(ProjectError::InvalidName)
    }
}

/// Finds the single plain-directory project matching `name` (case-insensitive) and opens it
/// relative to its root handle, never following a symlink.
#[cfg(unix)]
fn resolve_in(
    roots: &[Dir],
    roots_paths: &[PathBuf],
    network_roots: usize,
    name: &str,
) -> Result<ProjectHandle, ProjectError> {
    validate_name(name)?;
    if roots.is_empty() {
        return Err(if network_roots > 0 {
            ProjectError::NetworkOrVirtual
        } else {
            ProjectError::NoRoots
        });
    }
    let wanted = name.to_lowercase();
    let mut dir_matches: Vec<(usize, String)> = Vec::new();
    let mut symlink_seen = false;
    for (index, root) in roots.iter().enumerate() {
        let Ok(entries) = root.entries() else {
            continue;
        };
        for entry in entries.flatten() {
            let Some(entry_name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if entry_name.to_lowercase() != wanted {
                continue;
            }
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => dir_matches.push((index, entry_name)),
                Ok(kind) if kind.is_symlink() => symlink_seen = true,
                _ => {}
            }
        }
    }
    match dir_matches.len() {
        0 if symlink_seen => Err(ProjectError::NotPlainFolder),
        0 => Err(ProjectError::NotFound),
        1 => {
            let (index, actual) = dir_matches.remove(0);
            let dir = roots[index]
                .open_dir_nofollow(&actual)
                .map_err(|_| ProjectError::NotPlainFolder)?;
            let git_path = roots_paths[index].join(&actual);
            Ok(ProjectHandle {
                name: actual,
                dir,
                git_path,
            })
        }
        count => Err(ProjectError::Ambiguous(count)),
    }
}

/// Non-unix builds have no confined project access yet.
#[cfg(not(unix))]
fn resolve_in(
    _roots: &[Dir],
    _roots_paths: &[PathBuf],
    _network_roots: usize,
    _name: &str,
) -> Result<ProjectHandle, ProjectError> {
    Err(ProjectError::Unsupported)
}

/// Collects plain-directory names from each root (reading at most 500 entries per root),
/// merged, de-duplicated, sorted, and capped at 50.
#[cfg(unix)]
fn list_in(roots: &[Dir]) -> Vec<String> {
    let mut names = BTreeSet::new();
    for root in roots {
        let Ok(entries) = root.entries() else {
            continue;
        };
        for entry in entries.take(LIST_ENTRY_LIMIT).flatten() {
            let is_plain_dir = matches!(entry.file_type(), Ok(kind) if kind.is_dir());
            if !is_plain_dir {
                continue;
            }
            if let Some(name) = entry.file_name().to_str() {
                names.insert(name.to_string());
            }
        }
    }
    names.into_iter().take(LIST_NAME_LIMIT).collect()
}

/// Non-unix builds list no projects.
#[cfg(not(unix))]
fn list_in(_roots: &[Dir]) -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
#[path = "project_tests.rs"]
mod project_tests;
