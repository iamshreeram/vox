//! Verified private snapshots of an installed model (DESIGN.md Rev 4 P2 d).
//!
//! ptts can only load weights from a file path, and xn memory-maps them. Opening the install
//! files directly would leave a gap between verifying them and the loader opening them. So
//! Vox copies the verified bytes into a private directory and the loader reads only from that
//! copy. The copy is hashed AS WRITTEN, so a file changed after inspection is caught.
//!
//! Platform-neutral. Unix adds mode bits (0700 directory, 0600 files) and `O_NOFOLLOW`.
//! `Snapshot` removes its directory on drop.

use crate::managers::natural_model::{AssetSpec, ModelError, READY_MARKER};
use sha2::{Digest, Sha256};
use std::collections::hash_map::RandomState;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::hash::{BuildHasher, Hasher};
use std::io::{ErrorKind, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const COPY_CHUNK: usize = 64 * 1024;
const NAME_ATTEMPTS: u32 = 16;

/// A private, verified copy of a model install. The directory is removed on drop.
#[derive(Debug)]
pub struct Snapshot {
    dir: PathBuf,
}

impl Snapshot {
    /// The private directory holding the copied files.
    // Consumed by P3 (PhononLoader), which builds the engine from these paths.
    #[allow(dead_code)]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path of one copied file inside the snapshot.
    // Consumed by P3 (PhononLoader), which builds the engine from these paths.
    #[allow(dead_code)]
    pub fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// [`snapshot_verified_with`] with no hook.
// Consumed by P3 (PhononLoader); until then only the tests call it.
#[allow(dead_code)]
pub fn snapshot_verified(
    install_dir: &Path,
    assets: &[AssetSpec],
    scratch_root: &Path,
) -> Result<Snapshot, ModelError> {
    snapshot_verified_with(install_dir, assets, scratch_root, &|_: &Path| {})
}

/// Verifies `install_dir` and copies its files into a fresh private directory under
/// `scratch_root`.
///
/// * `install_dir` must hold EXACTLY the asset files plus the READY marker: no directories, no
///   symlinks, no extra entries. Otherwise the error is returned before `before_copy` runs.
/// * Per asset: the path must be a regular file (`symlink_metadata`), the file is opened without
///   following links, and the size taken from the OPEN handle must equal the pin before any byte
///   is read.
/// * `before_copy` is called with the source path after inspection and before its copy.
/// * The bytes are copied with a bounded stream into `create_new` files, and SHA-256 is
///   computed over the bytes AS WRITTEN and compared to the pin.
///
/// On any error the half-built snapshot is removed, so `scratch_root` gets no entries.
pub fn snapshot_verified_with(
    install_dir: &Path,
    assets: &[AssetSpec],
    scratch_root: &Path,
    before_copy: &dyn Fn(&Path),
) -> Result<Snapshot, ModelError> {
    check_install_layout(install_dir, assets)?;
    fs::create_dir_all(scratch_root).map_err(io_err)?;
    // Owning the directory from here on: any early return removes it through Drop.
    let snapshot = Snapshot {
        dir: create_private_dir(scratch_root)?,
    };
    for asset in assets {
        let source = install_dir.join(&asset.file);
        let mut input = open_verified_source(&source, asset)?;
        before_copy(&source);
        copy_and_verify(&mut input, &snapshot.dir.join(&asset.file), asset)?;
    }
    Ok(snapshot)
}

fn io_err(e: std::io::Error) -> ModelError {
    ModelError::Io(e.to_string())
}

fn corrupt(why: impl Into<String>) -> ModelError {
    ModelError::Corrupt(why.into())
}

/// The install directory must be a real directory holding exactly the asset files and the
/// READY marker, each a regular file. Nothing else is allowed.
fn check_install_layout(install_dir: &Path, assets: &[AssetSpec]) -> Result<(), ModelError> {
    let dir_meta = match fs::symlink_metadata(install_dir) {
        Ok(meta) => meta,
        Err(e) if e.kind() == ErrorKind::NotFound => return Err(ModelError::NotInstalled),
        Err(e) => return Err(io_err(e)),
    };
    if !dir_meta.file_type().is_dir() {
        return Err(corrupt("the install location is not a directory"));
    }

    let expected: HashSet<&str> = assets
        .iter()
        .map(|asset| asset.file.as_str())
        .chain(std::iter::once(READY_MARKER))
        .collect();
    let mut seen: HashSet<String> = HashSet::new();
    for entry in fs::read_dir(install_dir).map_err(io_err)? {
        let entry = entry.map_err(io_err)?;
        let name_os = entry.file_name();
        let name = name_os
            .to_str()
            .ok_or_else(|| corrupt("an unexpected file name is in the install directory"))?;
        if !expected.contains(name) {
            return Err(corrupt(format!(
                "unexpected entry {name} in the install directory"
            )));
        }
        let meta = fs::symlink_metadata(entry.path()).map_err(io_err)?;
        if !meta.file_type().is_file() {
            return Err(corrupt(format!("{name} is not a regular file")));
        }
        seen.insert(name.to_string());
    }
    if let Some(missing) = expected.iter().find(|name| !seen.contains(**name)) {
        return Err(corrupt(format!("{missing} is missing")));
    }
    Ok(())
}

/// Opens one source for copying. Refuses symlinks and any size other than the pin, measured on
/// the open handle, before a single byte is read.
pub(crate) fn open_verified_source(source: &Path, asset: &AssetSpec) -> Result<File, ModelError> {
    let meta = fs::symlink_metadata(source).map_err(io_err)?;
    if !meta.file_type().is_file() {
        return Err(corrupt(format!("{} is not a regular file", asset.file)));
    }
    let file = open_no_follow(source).map_err(io_err)?;
    let opened = file.metadata().map_err(io_err)?;
    if !opened.is_file() || opened.len() != asset.size {
        return Err(corrupt(format!(
            "{} does not have its pinned size",
            asset.file
        )));
    }
    Ok(file)
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_no_follow(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

/// Streams at most `asset.size` bytes from `input` into a new file at `destination`, hashing
/// what was written. Errors if the size or the digest does not match the pin.
pub(crate) fn copy_and_verify(
    input: &mut File,
    destination: &Path,
    asset: &AssetSpec,
) -> Result<(), ModelError> {
    let mut output = create_private_file(destination)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; COPY_CHUNK];
    // One byte past the pin is read so a file that grew is detected rather than truncated.
    let mut limited = Read::by_ref(input).take(asset.size.saturating_add(1));
    let mut copied: u64 = 0;
    loop {
        let read = match limited.read(&mut buffer) {
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_err(e)),
        };
        if read == 0 {
            break;
        }
        copied = copied.saturating_add(read as u64);
        if copied > asset.size {
            return Err(corrupt(format!("{} grew while it was copied", asset.file)));
        }
        output.write_all(&buffer[..read]).map_err(io_err)?;
        hasher.update(&buffer[..read]);
    }
    if copied != asset.size {
        return Err(corrupt(format!(
            "{} changed size while it was copied",
            asset.file
        )));
    }
    let digest = format!("{:x}", hasher.finalize());
    if digest != asset.sha256 {
        return Err(corrupt(format!(
            "{} does not match its pinned checksum",
            asset.file
        )));
    }
    Ok(())
}

/// Creates a new directory under `scratch_root` with an unpredictable name and mode 0700.
/// `create` refuses to reuse an existing name, so a retry never adopts someone else's directory.
fn create_private_dir(scratch_root: &Path) -> Result<PathBuf, ModelError> {
    for _ in 0..NAME_ATTEMPTS {
        let candidate = scratch_root.join(format!("snapshot-{}", random_token()));
        let mut builder = fs::DirBuilder::new();
        builder.recursive(false);
        #[cfg(unix)]
        builder.mode(0o700);
        match builder.create(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io_err(e)),
        }
    }
    Err(ModelError::Io(
        "could not create a private snapshot directory".into(),
    ))
}

/// Creates a new file (never truncates an existing one) with mode 0600 on unix.
fn create_private_file(destination: &Path) -> Result<File, ModelError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(destination).map_err(io_err)
}

/// A name token. It is keyed from per-process OS randomness and a counter, and the directory
/// is created exclusively. It is not a cryptographic nonce: the protection is the 0700
/// directory under app-private storage plus exclusive creation.
fn random_token() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    hasher.write_u32(std::process::id());
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    hasher.write_u128(nanos);
    format!("{:016x}", hasher.finish())
}
