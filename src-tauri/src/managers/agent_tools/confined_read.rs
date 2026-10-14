//! Confined no-follow reads inside a project (DESIGN R2-2, R3-3). Every filesystem touch
//! goes through `FsGate::run`, which caps live blocking jobs at 4.

use std::sync::Arc;

use tokio::sync::Semaphore;

#[cfg(unix)]
use std::time::Duration;

use super::project::ProjectHandle;

/// Filesystem types the tools refuse (network or virtual drives; R3-3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsKind {
    Local,
    NetworkOrVirtual(String),
}

/// Live blocking filesystem jobs allowed at once (R3-3 (b)).
pub const FS_GATE_PERMITS: usize = 4;
/// Message returned when the gate is saturated (fail fast, never queue).
pub const BUSY_MESSAGE: &str = "busy, try again";
/// Most bytes returned from one file (DESIGN D4).
pub const READ_CAP_BYTES: usize = 32 * 1024;
/// Longest accepted relative path in bytes (DESIGN D4).
#[cfg(unix)]
const MAX_REL_BYTES: usize = 512;
/// Longest single path component in bytes (DESIGN R2-2).
#[cfg(unix)]
const MAX_COMPONENT_BYTES: usize = 255;
/// Upper bound on the admitted open-and-read job as seen by the caller (R2-2).
#[cfg(unix)]
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Appended after kept bytes when a file is longer than the cap.
#[cfg(unix)]
const TRUNCATED_MARKER: &str = "\n(truncated)";
/// Linux `f_type` magic numbers of network and virtual filesystems (R3-3 (a)).
const NETWORK_MAGICS: [u64; 8] = [
    0x6969,      // NFS
    0x517B,      // SMB
    0xFF53_4D42, // CIFS
    0xFE53_4D42, // SMB2
    0x6573_5546, // FUSE
    0x0102_1997, // 9P
    0x5346_414F, // AFS
    0x00C3_6400, // CEPH
];
/// macOS `f_fstypename` values of network and virtual filesystems (R3-3 (a)).
const NETWORK_NAMES: [&str; 6] = ["nfs", "smbfs", "afpfs", "webdav", "macfuse", "osxfuse"];

/// Global admission gate: 4 permits, fail-fast `busy` when saturated (R3-3).
#[derive(Debug, Clone)]
pub struct FsGate {
    permits: Arc<Semaphore>,
}

impl Default for FsGate {
    fn default() -> Self {
        Self::new()
    }
}

impl FsGate {
    /// Creates a gate with `FS_GATE_PERMITS` permits.
    pub fn new() -> Self {
        Self {
            permits: Arc::new(Semaphore::new(FS_GATE_PERMITS)),
        }
    }

    /// Runs `job` on the blocking pool. The owned permit moves into the closure, so it is
    /// released only when the job finishes.
    pub async fn run<T, F>(&self, job: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| BUSY_MESSAGE.to_string())?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            job()
        })
        .await
        .map_err(|_| "the file worker failed; try again".to_string())
    }
}

/// Classifies the filesystem under an open root from its statfs type name or magic number.
/// Hex magic (`0x...`, any case) is matched against the Linux list; other text against the
/// macOS names and the `fuse*` prefix. Anything else is local.
pub fn classify_fs(type_name_or_magic: &str) -> FsKind {
    let raw = type_name_or_magic.trim();
    let lower = raw.to_ascii_lowercase();
    let network = match lower.strip_prefix("0x") {
        Some(hex) => classify_magic(hex),
        None => NETWORK_NAMES.contains(&lower.as_str()) || lower.starts_with("fuse"),
    };
    if network {
        FsKind::NetworkOrVirtual(raw.to_string())
    } else {
        FsKind::Local
    }
}

/// True when the hex magic (without the `0x` prefix) is a known network or FUSE type.
fn classify_magic(hex: &str) -> bool {
    u64::from_str_radix(hex, 16).is_ok_and(|magic| NETWORK_MAGICS.contains(&magic))
}

/// Reads the filesystem type of an open directory with `fstatfs`. macOS and Linux only;
/// other unix platforms report `Unsupported`, so their roots are not used.
#[cfg(unix)]
pub fn fs_kind_of(dir: &cap_std::fs::Dir) -> std::io::Result<FsKind> {
    #[cfg(target_os = "macos")]
    {
        let buf = fstatfs_of(dir)?;
        let name: Vec<u8> = buf
            .f_fstypename
            .iter()
            .take_while(|c| **c != 0)
            .map(|c| *c as u8)
            .collect();
        Ok(classify_fs(&String::from_utf8_lossy(&name)))
    }

    #[cfg(target_os = "linux")]
    {
        let buf = fstatfs_of(dir)?;
        let magic = buf.f_type as u64;
        Ok(classify_fs(&format!("0x{magic:X}")))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = dir;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "filesystem policy is not implemented on this platform",
        ))
    }
}

/// Calls `fstatfs` on the directory handle (macOS and Linux only).
#[cfg(all(unix, any(target_os = "macos", target_os = "linux")))]
fn fstatfs_of(dir: &cap_std::fs::Dir) -> std::io::Result<libc::statfs> {
    use std::os::fd::AsRawFd;

    // SAFETY: `libc::statfs` is a plain C struct of integers and byte arrays, for which the
    // all-zero bit pattern is a valid value.
    let mut buf: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: the descriptor is owned by `dir` and valid for the call; `buf` is a valid
    // out-pointer to a zeroed `statfs`.
    let rc = unsafe { libc::fstatfs(dir.as_raw_fd(), &mut buf) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(buf)
}

/// Denylisted names, compared case-insensitively against every path component (R2-2).
const DENY_EXACT: [&str; 10] = [
    ".git", ".ssh", ".aws", ".gnupg", ".kube", ".docker", ".env", ".npmrc", ".netrc", ".pgpass",
];
const DENY_PREFIX: [&str; 5] = [".env.", "id_rsa", "id_ed25519", "credentials", "secrets"];
const DENY_SUFFIX: [&str; 8] = [
    ".pem",
    ".key",
    ".p12",
    ".pfx",
    ".keystore",
    ".jks",
    ".ovpn",
    ".kdbx",
];

/// True when `component` names something Vox must never read, in any case.
#[cfg(unix)]
fn is_denied(component: &str) -> bool {
    let lower = component.to_ascii_lowercase();
    DENY_EXACT.contains(&lower.as_str())
        || DENY_PREFIX.iter().any(|prefix| lower.starts_with(prefix))
        || DENY_SUFFIX.iter().any(|suffix| lower.ends_with(suffix))
}

/// Refusal text for a path that is malformed or not a plain relative path.
#[cfg(unix)]
const INVALID_PATH: &str = "that is not a valid file path inside the project";
/// Refusal text for a denylisted name (names only, never contents).
#[cfg(unix)]
const DENIED_NAME: &str = "Vox does not read files with that kind of name";

/// Validates a project-relative path BEFORE any filesystem access (R2-2). Returns the
/// components on success.
#[cfg(unix)]
fn validate_relative(rel: &str) -> Result<Vec<&str>, String> {
    if rel.is_empty() || rel.len() > MAX_REL_BYTES || rel.contains(['\0', '\\', ':']) {
        return Err(INVALID_PATH.to_string());
    }
    let parts: Vec<&str> = rel.split('/').collect();
    for part in &parts {
        validate_component(part)?;
    }
    Ok(parts)
}

/// One component must be a normal name: not empty, `.` or `..`, no trailing dot or space,
/// at most 255 bytes, and not denylisted.
#[cfg(unix)]
fn validate_component(part: &str) -> Result<(), String> {
    let shape_ok = !part.is_empty()
        && part != "."
        && part != ".."
        && part.len() <= MAX_COMPONENT_BYTES
        && !part.ends_with('.')
        && !part.ends_with(' ');
    if !shape_ok {
        return Err(INVALID_PATH.to_string());
    }
    if is_denied(part) {
        return Err(DENIED_NAME.to_string());
    }
    Ok(())
}

/// Reads one project file (relative path) through the project's pinned directory handle.
/// Returns redacted text capped at 32 KiB (plus a truncation marker).
pub async fn read_project_file(
    gate: &FsGate,
    project: &ProjectHandle,
    relative_path: &str,
) -> Result<String, String> {
    read_project_file_impl(gate, project, relative_path).await
}

#[cfg(unix)]
async fn read_project_file_impl(
    gate: &FsGate,
    project: &ProjectHandle,
    relative_path: &str,
) -> Result<String, String> {
    let parts: Vec<String> = validate_relative(relative_path)?
        .into_iter()
        .map(str::to_owned)
        .collect();
    let project_dir = project
        .dir()
        .try_clone()
        .map_err(|_| "could not open the project folder".to_string())?;
    let job = move || read_confined(&project_dir, &parts);
    match tokio::time::timeout(READ_TIMEOUT, gate.run(job)).await {
        Err(_) => Err("reading that file took too long; try again".to_string()),
        Ok(outcome) => outcome?,
    }
}

#[cfg(not(unix))]
async fn read_project_file_impl(
    gate: &FsGate,
    project: &ProjectHandle,
    relative_path: &str,
) -> Result<String, String> {
    let _ = (gate, project, relative_path);
    Err(super::project::UNSUPPORTED_MESSAGE.to_string())
}

/// Opens each directory component no-follow, then the final file no-follow with
/// `O_NONBLOCK|O_NOCTTY`, checks the OPENED handle (regular file, one link), and reads at
/// most `READ_CAP_BYTES + 1` bytes. Runs on the blocking pool.
#[cfg(unix)]
fn read_confined(project: &cap_std::fs::Dir, parts: &[String]) -> Result<String, String> {
    use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::fs::{OpenOptions, OpenOptionsExt};
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;

    const OPEN_FAILED: &str = "could not read that file in the project";
    const NOT_REGULAR: &str = "that is not a regular file";
    let (file_name, dirs) = parts.split_last().ok_or_else(|| INVALID_PATH.to_string())?;

    let mut current = project.try_clone().map_err(|_| OPEN_FAILED.to_string())?;
    for name in dirs {
        current = current
            .open_dir_nofollow(name)
            .map_err(|_| OPEN_FAILED.to_string())?;
    }

    // Pre-check so that a FIFO, socket or device node is never opened. The no-follow open
    // below remains the actual guard against symlinks.
    let entry = current
        .symlink_metadata(file_name)
        .map_err(|_| OPEN_FAILED.to_string())?;
    if !entry.is_file() {
        return Err(NOT_REGULAR.to_string());
    }

    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    let file = current
        .open_with(file_name, &options)
        .map_err(|_| OPEN_FAILED.to_string())?;
    let mut file = file.into_std();

    let meta = file.metadata().map_err(|_| OPEN_FAILED.to_string())?;
    if !meta.file_type().is_file() {
        return Err(NOT_REGULAR.to_string());
    }
    if meta.nlink() != 1 {
        return Err("that file has more than one link; Vox does not read it".to_string());
    }

    let mut bytes = Vec::new();
    (&mut file)
        .take(READ_CAP_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| OPEN_FAILED.to_string())?;
    let truncated = bytes.len() > READ_CAP_BYTES;
    bytes.truncate(READ_CAP_BYTES);
    if bytes.contains(&0) {
        return Err(BINARY_FILE.to_string());
    }
    let mut text = decode_text(&bytes, truncated)?;
    if truncated {
        text.push_str(TRUNCATED_MARKER);
    }
    Ok(crate::managers::redact::redact(&text))
}

/// Refusal text for bytes that are not text (NUL or invalid UTF-8).
#[cfg(unix)]
const BINARY_FILE: &str = "binary file; Vox reads text files only";

/// Decodes strict UTF-8. When the read was truncated at the cap, an incomplete multibyte
/// sequence at the very end is trimmed first, so a character split by the cap is kept out
/// rather than misreported as binary. Any other invalid byte is refused.
#[cfg(unix)]
fn decode_text(bytes: &[u8], truncated: bool) -> Result<String, String> {
    let valid_len = match std::str::from_utf8(bytes) {
        Ok(_) => bytes.len(),
        Err(err) if truncated && err.error_len().is_none() => err.valid_up_to(),
        Err(_) => return Err(BINARY_FILE.to_string()),
    };
    std::str::from_utf8(&bytes[..valid_len])
        .map(str::to_owned)
        .map_err(|_| BINARY_FILE.to_string())
}

#[cfg(test)]
#[path = "confined_read_tests.rs"]
mod confined_read_tests;
