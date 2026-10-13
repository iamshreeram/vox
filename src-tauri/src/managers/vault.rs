//! Confined access to the user notes vault (handle-relative file IO, no Tauri types).
//!
//! Contract (DESIGN Rev 3, section 3.2). Every filesystem operation after
//! `Vault::open` goes through a root directory handle pinned once at open time.
//! Each path component is a single name (never a multi-component path) opened
//! relative to its parent handle with symlink following disabled
//! (`cap_fs_ext::DirExt::open_dir_nofollow` for directories and
//! `OpenOptionsFollowExt::follow(FollowSymlinks::No)` for files). The no-follow
//! behaviour is the platform's own: `O_NOFOLLOW` on unix, and refusal of
//! symlinks and reparse points on Windows (`ERROR_STOPPED_ON_SYMLINK`). A
//! symlink found at any component yields `VaultError::Unsafe`.
//!
//! The error classifier runs only after a failed open, to choose which error to
//! return. It never grants access. Opened handles are re-checked with their own
//! metadata.
//!
//! The vault is not yet called from the rest of the crate, so the items here
//! are dead code in the non-test build until the conversation wiring lands.

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
#[cfg(unix)]
use cap_std::fs::OpenOptionsExt;
use cap_std::fs::{Dir, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

/// Relative path of the optional context pack read by `read_context_pack`.
pub const CONTEXT_PACK_REL: &str = "Memory/Code-Puppy/CONTEXT-PACK.md";

/// Directory (relative to the vault root) that all notes are forced under.
pub const NOTES_DIR_REL: &str = "Memory/Vox-Conversations";

/// Maximum accepted size of the context pack. Larger packs are REJECTED
/// (`VaultError::TooLarge`), never truncated.
pub const MAX_PACK_BYTES: usize = 64 * 1024;

/// Maximum accepted size of a note body passed to `write_note`.
pub const MAX_NOTE_WRITE_BYTES: usize = 256 * 1024;

/// Error kinds returned by the vault. `Display` prints fixed words only and
/// never contains note contents, pack contents or the vault root path.
///
/// Variant meaning (applies to every function in this module):
/// - `Invalid`: the caller supplied a malformed value (empty, relative or
///   non-directory root, a malformed `rel`, or a directory or other non-regular
///   entry where the context pack file is expected).
/// - `Missing`: the requested path does not exist.
/// - `TooLarge`: content exceeds the byte limit; nothing is returned or written.
/// - `Unsafe`: a symlink, a reparse point, or a non-directory where a directory
///   is required was found on the path.
/// - `Exists`: `write_note` refused to overwrite an existing file.
/// - `Io(kind)`: any other filesystem failure, carrying only the `ErrorKind`.
#[derive(Debug)]
pub enum VaultError {
    /// Malformed input (see the type-level docs).
    Invalid,
    /// The requested path does not exist.
    Missing,
    /// Content exceeds the configured byte limit.
    TooLarge,
    /// A symlink or other unsafe entry was found on the path.
    Unsafe,
    /// The target note already exists.
    Exists,
    /// Other filesystem failure, carrying only the error kind.
    Io(std::io::ErrorKind),
}

impl std::fmt::Display for VaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            VaultError::Invalid => "invalid vault path",
            VaultError::Missing => "vault path missing",
            VaultError::TooLarge => "vault content too large",
            VaultError::Unsafe => "unsafe vault path",
            VaultError::Exists => "vault note already exists",
            VaultError::Io(_) => "vault I/O error",
        };
        f.write_str(text)
    }
}

impl std::error::Error for VaultError {}

/// A handle pinned to a canonical vault root directory.
///
/// Every later operation goes through the pinned root handle with symlink
/// following disabled on each component.
pub struct Vault {
    root: Dir,
    root_path: PathBuf,
}

/// Bounded retries for a no-follow directory open that reports `NotFound`
/// while the entry is a real directory (see `open_component_dir`).
const NOFOLLOW_RETRIES: u32 = 64;

/// What a path component must be, used when classifying a failed open.
#[derive(Clone, Copy)]
enum Want {
    Dir,
    File,
}

/// Maps a standard IO error: `NotFound` becomes `Missing`, anything else
/// becomes `Io(kind)`.
fn map_io(err: std::io::Error) -> VaultError {
    match err.kind() {
        ErrorKind::NotFound => VaultError::Missing,
        kind => VaultError::Io(kind),
    }
}

/// Chooses the error for a failed no-follow open of `name` under `parent`.
/// Runs only after the open has already been refused, so it cannot grant
/// access.
fn classify(parent: &Dir, name: &str, err: std::io::Error, want: Want) -> VaultError {
    match parent.symlink_metadata(name) {
        Ok(meta) if meta.is_symlink() => VaultError::Unsafe,
        Ok(meta) => match want {
            Want::Dir if !meta.is_dir() => VaultError::Unsafe,
            Want::File if meta.is_dir() || !meta.is_file() => VaultError::Invalid,
            _ => VaultError::Io(err.kind()),
        },
        Err(lstat_err) if lstat_err.kind() == ErrorKind::NotFound => VaultError::Missing,
        Err(_) => VaultError::Io(err.kind()),
    }
}

/// Opens the existing directory `name` under `parent` without following
/// symlinks, then re-checks the opened handle is a directory.
fn open_component_dir(parent: &Dir, name: &str) -> Result<Dir, VaultError> {
    let mut attempt = 0;
    let child = loop {
        match parent.open_dir_nofollow(name) {
            Ok(child) => break child,
            Err(err) => {
                let classified = classify(parent, name, err, Want::Dir);
                // A concurrent mkdir can make the no-follow open report NotFound
                // while the entry is already a real directory. Retry that case a
                // bounded number of times. Retrying never grants access: every
                // successful open is still checked below.
                let transient = matches!(classified, VaultError::Io(ErrorKind::NotFound));
                if !transient || attempt >= NOFOLLOW_RETRIES {
                    return Err(classified);
                }
                attempt += 1;
                std::thread::yield_now();
            }
        }
    };
    let handle = child.dir_metadata().map_err(map_io)?;
    if !handle.is_dir() {
        return Err(VaultError::Unsafe);
    }
    Ok(child)
}

/// Returns the directory `name` under `parent`, creating it when absent.
/// Symlinks and non-directories yield `Unsafe`.
fn ensure_dir(parent: &Dir, name: &str) -> Result<Dir, VaultError> {
    match open_component_dir(parent, name) {
        Err(VaultError::Missing) => {
            match parent.create_dir(name) {
                Ok(()) => {}
                // Another writer created it between the open and the mkdir.
                // A symlink created there is caught by the reopen below.
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
                Err(err) => return Err(VaultError::Io(err.kind())),
            }
            open_component_dir(parent, name)
        }
        other => other,
    }
}

/// Opens each directory name in turn from `root`, one component at a time.
fn open_dir_chain(root: &Dir, names: &[&str]) -> Result<Dir, VaultError> {
    let mut current = root.try_clone().map_err(map_io)?;
    for name in names {
        current = open_component_dir(&current, name)?;
    }
    Ok(current)
}

/// Validates a note-relative path. Only non-empty normal components are
/// accepted; empty, `.`, `..`, absolute, NUL, backslash and colon (drive
/// prefixes) are rejected.
fn split_rel(rel: &str) -> Result<Vec<&str>, VaultError> {
    if rel.is_empty() || rel.contains('\0') || rel.contains('\\') || rel.contains(':') {
        return Err(VaultError::Invalid);
    }
    let parts: Vec<&str> = rel.split('/').collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return Err(VaultError::Invalid);
    }
    Ok(parts)
}

impl Vault {
    /// Opens the vault rooted at `root`.
    ///
    /// Returns `Invalid` for an empty, relative, or non-directory (for example
    /// a regular file) `root`; returns `Missing` if `root` does not exist.
    /// On success the root is canonicalized once and pinned as a handle.
    pub fn open(root: &str) -> Result<Vault, VaultError> {
        if root.is_empty() || !Path::new(root).is_absolute() {
            return Err(VaultError::Invalid);
        }
        let canonical = std::fs::canonicalize(root).map_err(map_io)?;
        let meta = std::fs::metadata(&canonical).map_err(map_io)?;
        if !meta.is_dir() {
            return Err(VaultError::Invalid);
        }
        let dir =
            Dir::open_ambient_dir(&canonical, cap_std::ambient_authority()).map_err(map_io)?;
        Ok(Vault {
            root: dir,
            root_path: canonical,
        })
    }

    /// Reads `CONTEXT_PACK_REL` through the pinned root handle.
    ///
    /// - `Missing` if the pack (or one of its parent directories) does not exist.
    /// - `Unsafe` if any component on the path is a symlink or reparse point
    ///   (including a symlinked `Memory` or `Code-Puppy` directory or a
    ///   symlinked pack file), or a parent component is not a directory.
    /// - `Invalid` if the final entry is a directory or other non-regular file.
    /// - `TooLarge` if the file is longer than `MAX_PACK_BYTES`; nothing is returned.
    /// - Invalid UTF-8 is decoded lossily (U+FFFD), not reported as an error.
    pub fn read_context_pack(&self) -> Result<String, VaultError> {
        let parts: Vec<&str> = CONTEXT_PACK_REL.split('/').collect();
        let (file_name, dirs) = parts.split_last().ok_or(VaultError::Invalid)?;
        let dir = open_dir_chain(&self.root, dirs)?;

        // Pre-check so that a FIFO or device at the pack path is refused
        // before an open could block on it. The no-follow open below remains
        // the actual guard.
        match dir.symlink_metadata(file_name) {
            Ok(meta) if meta.is_symlink() => return Err(VaultError::Unsafe),
            Ok(meta) if !meta.is_file() => return Err(VaultError::Invalid),
            Ok(_) => {}
            Err(err) => return Err(map_io(err)),
        }

        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let mut file = dir
            .open_with(file_name, &options)
            .map_err(|err| classify(&dir, file_name, err, Want::File))?;
        let handle = file.metadata().map_err(map_io)?;
        if !handle.is_file() {
            return Err(VaultError::Invalid);
        }

        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_PACK_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(map_io)?;
        if bytes.len() > MAX_PACK_BYTES {
            return Err(VaultError::TooLarge);
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Writes a new note under `NOTES_DIR_REL` at the relative path `rel`.
    ///
    /// `rel` must be relative with normal components only. It is rejected with
    /// `Invalid` if it is empty, absolute, contains `..`, `.`, empty components,
    /// NUL, a backslash, or a colon (so drive prefixes are refused). Intermediate
    /// directories are created one component at a time through the pinned handle.
    ///
    /// - `Unsafe` if an existing component is a symlink, reparse point or
    ///   non-directory.
    /// - `Exists` if the target name already exists (a file, or a symlink, which
    ///   is never followed); existing bytes are left untouched.
    /// - `TooLarge` if `contents` is longer than `MAX_NOTE_WRITE_BYTES`; no file is created.
    /// - On failure after creation begins, the partial file is removed.
    /// - On unix the file is created with mode 0600.
    ///
    /// Returns the path of the written note, under the vault root.
    pub fn write_note(&self, rel: &str, contents: &str) -> Result<std::path::PathBuf, VaultError> {
        let parts = split_rel(rel)?;
        if contents.len() > MAX_NOTE_WRITE_BYTES {
            return Err(VaultError::TooLarge);
        }
        let (file_name, rel_dirs) = parts.split_last().ok_or(VaultError::Invalid)?;

        let mut dir = self.root.try_clone().map_err(map_io)?;
        for name in NOTES_DIR_REL.split('/').chain(rel_dirs.iter().copied()) {
            dir = ensure_dir(&dir, name)?;
        }

        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        options.mode(0o600);

        let mut file = match dir.open_with(file_name, &options) {
            Ok(file) => file,
            Err(err) if err.kind() == ErrorKind::AlreadyExists => return Err(VaultError::Exists),
            Err(err) => return Err(VaultError::Io(err.kind())),
        };
        let written = file
            .write_all(contents.as_bytes())
            .and_then(|()| file.sync_all());
        if let Err(err) = written {
            drop(file);
            // Best effort: remove only the entry this call created.
            let _ = dir.remove_file(file_name);
            return Err(VaultError::Io(err.kind()));
        }
        Ok(self.root_path.join(NOTES_DIR_REL).join(rel))
    }
}
