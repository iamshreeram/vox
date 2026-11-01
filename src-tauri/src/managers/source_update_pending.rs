//! Pending-run record `<state>/pending.json` for the two-phase auto-update
//! (R7-1, R7-3, R8-1, R8-2).
//!
//! The build job writes state `built`; Vox itself moves the record to state
//! `committing` right before it starts the install job (R7-2). Parsing is
//! strict: exact key set per state, typed validation of every field, and no
//! unknown keys. Writes serialise typed fields with serde_json, so untrusted
//! text is never spliced into JSON. Nothing in this file touches launchd.

#![cfg_attr(not(test), allow(dead_code))]

use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use super::source_update_runner::{
    is_lower_hex, is_valid_run_id, job_label, plist_path, write_private_atomic, JOB_LABEL_PREFIX,
};

/// File name of the pending record inside the state directory.
pub const PENDING_FILE_NAME: &str = "pending.json";
/// Largest pending record accepted, in bytes.
pub const PENDING_MAX_BYTES: usize = 4096;
/// The only schema version this build reads and writes.
pub const PENDING_VERSION: u64 = 1;
/// Directory under the state directory that holds staged app copies.
pub const STAGED_DIR_NAME: &str = "staged";
/// Name of the staged app bundle inside `staged/<run_id>/`.
pub const STAGED_APP_NAME: &str = "Vox.app";
/// Longest accepted executable file name, in characters.
pub const MAX_EXE_NAME_CHARS: usize = 64;

const BUILT_KEYS: [&str; 10] = [
    "v",
    "run_id",
    "target_full",
    "staged_app",
    "exe_name",
    "exe_sha256",
    "info_plist_sha256",
    "state",
    "origin",
    "ts",
];

const COMMITTING_EXTRA_KEYS: [&str; 4] = [
    "install_run_id",
    "install_label",
    "install_plist",
    "committing_ts",
];

/// Who asked for the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingOrigin {
    /// Started by the automatic policy.
    Auto,
    /// Started by the owner clicking the update button.
    Manual,
}

impl PendingOrigin {
    /// Wire name used in the file.
    pub fn as_str(self) -> &'static str {
        match self {
            PendingOrigin::Auto => "auto",
            PendingOrigin::Manual => "manual",
        }
    }
}

/// Facts recorded at commit time, before the install job is spawned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    /// Run id of the install job (16 lowercase hex, never equal to the build run id).
    pub install_run_id: String,
    /// Launchd label of the install job (`com.iamshreeram.vox.update.<install_run_id>`).
    pub install_label: String,
    /// Plist of the install job (`<state>/run-<install_run_id>.plist`).
    pub install_plist: PathBuf,
    /// Unix seconds when the commit was recorded.
    pub committing_ts: u64,
}

/// Lifecycle state stored in the record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingState {
    /// Staged and resting; nothing is installed yet.
    Built,
    /// The install job is about to be (or has been) spawned.
    Committing(CommitInfo),
}

/// One validated pending record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRun {
    /// Build run id (16 lowercase hex).
    pub run_id: String,
    /// Full target commit (40 lowercase hex).
    pub target_full: String,
    /// Staged app bundle; must equal `<state>/staged/<run_id>/Vox.app`.
    pub staged_app: PathBuf,
    /// File name of the executable inside `Contents/MacOS`.
    pub exe_name: String,
    /// SHA-256 of the executable (64 lowercase hex).
    pub exe_sha256: String,
    /// SHA-256 of `Contents/Info.plist` (64 lowercase hex).
    pub info_plist_sha256: String,
    /// Who asked for the run.
    pub origin: PendingOrigin,
    /// Unix seconds when the build finished.
    pub ts: u64,
    /// Built or committing.
    pub state: PendingState,
}

/// Reasons a pending record is rejected. Display never echoes the input.
#[derive(Debug)]
pub enum PendingError {
    /// Larger than `PENDING_MAX_BYTES`.
    TooLarge,
    /// Not valid UTF-8.
    InvalidUtf8,
    /// Not parseable JSON.
    InvalidJson,
    /// Parseable JSON that is not an object.
    NotAnObject,
    /// `v` is not `PENDING_VERSION`.
    BadVersion,
    /// `state` is neither `built` nor `committing`.
    BadState,
    /// A key is missing or unexpected for the state; the name is the key.
    BadField(&'static str),
    /// `run_id` (or `install_run_id`) is malformed or equal to the other id.
    BadRunId,
    /// `target_full` is not 40 lowercase hex.
    BadTarget,
    /// A SHA-256 field is not 64 lowercase hex; the name is the field.
    BadSha256(&'static str),
    /// `exe_name` is empty, too long, or has characters outside `[A-Za-z0-9._-]`.
    BadExeName,
    /// `origin` is neither `auto` nor `manual`.
    BadOrigin,
    /// A path field does not match the layout the state directory dictates.
    BadPath(&'static str),
    /// The path is a symlink, a directory, or otherwise not a regular file.
    NotRegularFile,
    /// Filesystem failure.
    Io(io::Error),
}

impl fmt::Display for PendingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PendingError::TooLarge => {
                write!(f, "pending record is larger than {PENDING_MAX_BYTES} bytes")
            }
            PendingError::InvalidUtf8 => write!(f, "pending record is not valid UTF-8"),
            PendingError::InvalidJson => write!(f, "pending record is not valid JSON"),
            PendingError::NotAnObject => write!(f, "pending record is not a JSON object"),
            PendingError::BadVersion => write!(f, "unsupported pending record version"),
            PendingError::BadState => write!(f, "unknown pending record state"),
            PendingError::BadField(name) => write!(f, "invalid or missing pending field: {name}"),
            PendingError::BadRunId => write!(f, "invalid pending run id"),
            PendingError::BadTarget => write!(f, "invalid pending target"),
            PendingError::BadSha256(name) => write!(f, "invalid pending digest: {name}"),
            PendingError::BadExeName => write!(f, "invalid pending executable name"),
            PendingError::BadOrigin => write!(f, "invalid pending origin"),
            PendingError::BadPath(name) => write!(f, "pending path is not allowed: {name}"),
            PendingError::NotRegularFile => write!(f, "pending record is not a regular file"),
            PendingError::Io(error) => write!(f, "pending record I/O error: {error}"),
        }
    }
}

impl std::error::Error for PendingError {}

impl From<io::Error> for PendingError {
    fn from(error: io::Error) -> Self {
        PendingError::Io(error)
    }
}

/// Parses and validates one pending record. Fail closed: any violation is an error.
///
/// `state_dir` fixes the only allowed `staged_app` and `install_plist` paths.
pub fn parse_pending(bytes: &[u8], state_dir: &Path) -> Result<PendingRun, PendingError> {
    if bytes.len() > PENDING_MAX_BYTES {
        return Err(PendingError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| PendingError::InvalidUtf8)?;
    let value: Value = serde_json::from_str(text).map_err(|_| PendingError::InvalidJson)?;
    let object = value.as_object().ok_or(PendingError::NotAnObject)?;
    if uint_field(object, "v")? != PENDING_VERSION {
        return Err(PendingError::BadVersion);
    }
    let committing = match text_field(object, "state")? {
        "built" => false,
        "committing" => true,
        _ => return Err(PendingError::BadState),
    };
    check_keys(object, committing)?;

    let run_id = text_field(object, "run_id")?;
    if !is_valid_run_id(run_id) {
        return Err(PendingError::BadRunId);
    }
    let target_full = text_field(object, "target_full")?;
    if !is_lower_hex(target_full, 40) {
        return Err(PendingError::BadTarget);
    }
    let staged_expected = staged_app_path(state_dir, run_id);
    if Some(text_field(object, "staged_app")?) != staged_expected.to_str() {
        return Err(PendingError::BadPath("staged_app"));
    }
    let exe_name = text_field(object, "exe_name")?;
    if !is_valid_exe_name(exe_name) {
        return Err(PendingError::BadExeName);
    }
    let exe_sha256 = sha_field(object, "exe_sha256")?;
    let info_plist_sha256 = sha_field(object, "info_plist_sha256")?;
    let origin = match text_field(object, "origin")? {
        "auto" => PendingOrigin::Auto,
        "manual" => PendingOrigin::Manual,
        _ => return Err(PendingError::BadOrigin),
    };
    let ts = uint_field(object, "ts")?;

    let state = if committing {
        PendingState::Committing(parse_commit_info(object, state_dir, run_id)?)
    } else {
        PendingState::Built
    };
    Ok(PendingRun {
        run_id: run_id.to_string(),
        target_full: target_full.to_string(),
        staged_app: staged_expected,
        exe_name: exe_name.to_string(),
        exe_sha256: exe_sha256.to_string(),
        info_plist_sha256: info_plist_sha256.to_string(),
        origin,
        ts,
        state,
    })
}

/// Serialises a validated record to the wire format. The result is parsed
/// again before it is returned, so an invalid record can never reach disk.
pub fn encode_pending(run: &PendingRun, state_dir: &Path) -> Result<Vec<u8>, PendingError> {
    let staged = path_text(&run.staged_app, "staged_app")?;
    let mut map = Map::new();
    map.insert("v".into(), json!(PENDING_VERSION));
    map.insert("run_id".into(), json!(run.run_id));
    map.insert("target_full".into(), json!(run.target_full));
    map.insert("staged_app".into(), json!(staged));
    map.insert("exe_name".into(), json!(run.exe_name));
    map.insert("exe_sha256".into(), json!(run.exe_sha256));
    map.insert("info_plist_sha256".into(), json!(run.info_plist_sha256));
    map.insert("origin".into(), json!(run.origin.as_str()));
    map.insert("ts".into(), json!(run.ts));
    match &run.state {
        PendingState::Built => {
            map.insert("state".into(), json!("built"));
        }
        PendingState::Committing(info) => {
            map.insert("state".into(), json!("committing"));
            map.insert("install_run_id".into(), json!(info.install_run_id));
            map.insert("install_label".into(), json!(info.install_label));
            let plist = path_text(&info.install_plist, "install_plist")?;
            map.insert("install_plist".into(), json!(plist));
            map.insert("committing_ts".into(), json!(info.committing_ts));
        }
    }
    let bytes = serde_json::to_vec(&Value::Object(map)).map_err(|_| PendingError::InvalidJson)?;
    parse_pending(&bytes, state_dir)?;
    Ok(bytes)
}

/// `<state>/staged/<run_id>/Vox.app`, the only allowed staged location.
pub fn staged_app_path(state_dir: &Path, run_id: &str) -> PathBuf {
    state_dir
        .join(STAGED_DIR_NAME)
        .join(run_id)
        .join(STAGED_APP_NAME)
}

/// True when `pending.json` holds a valid record in state `built` for `run_id`.
/// Any read or parse problem counts as false.
pub fn pending_built_for(state_dir: &Path, run_id: &str) -> bool {
    matches!(
        PendingStore::new(state_dir).load(),
        Ok(Some(run)) if run.run_id == run_id && run.state == PendingState::Built
    )
}

/// Owner of `<state>/pending.json` and of the staged directories.
#[derive(Debug, Clone)]
pub struct PendingStore {
    state_dir: PathBuf,
}

impl PendingStore {
    /// Store rooted at `state_dir`. Creates nothing.
    pub fn new(state_dir: &Path) -> Self {
        Self {
            state_dir: state_dir.to_path_buf(),
        }
    }

    /// The state directory this store is rooted at.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Full path of `pending.json`.
    pub fn path(&self) -> PathBuf {
        self.state_dir.join(PENDING_FILE_NAME)
    }

    /// Reads and validates the record. `Ok(None)` when there is no file;
    /// `Err` for symlinks, non-regular files, oversize or invalid content.
    pub fn load(&self) -> Result<Option<PendingRun>, PendingError> {
        let path = self.path();
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(PendingError::Io(error)),
        };
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err(PendingError::NotRegularFile);
        }
        if meta.len() > PENDING_MAX_BYTES as u64 {
            return Err(PendingError::TooLarge);
        }
        let mut bytes = Vec::new();
        open_no_follow(&path)?
            .take(PENDING_MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        parse_pending(&bytes, &self.state_dir).map(Some)
    }

    /// Moves a `built` record to `committing` and writes it atomically (mode 0600).
    /// Refuses unless the file on disk still equals `built`.
    pub fn begin_commit(
        &self,
        built: &PendingRun,
        install_run_id: &str,
        now: u64,
    ) -> Result<PendingRun, PendingError> {
        if built.state != PendingState::Built {
            return Err(PendingError::BadState);
        }
        self.require_current(built)?;
        if !is_valid_run_id(install_run_id) || install_run_id == built.run_id {
            return Err(PendingError::BadRunId);
        }
        let committing = PendingRun {
            state: PendingState::Committing(CommitInfo {
                install_run_id: install_run_id.to_string(),
                install_label: job_label(install_run_id),
                install_plist: plist_path(&self.state_dir, install_run_id),
                committing_ts: now,
            }),
            ..built.clone()
        };
        self.write(&committing)?;
        Ok(committing)
    }

    /// Moves a `committing` record back to `built` (used when the spawn fails).
    /// Refuses unless the file on disk still equals `committing`.
    pub fn revert(&self, committing: &PendingRun) -> Result<PendingRun, PendingError> {
        if !matches!(committing.state, PendingState::Committing(_)) {
            return Err(PendingError::BadState);
        }
        self.require_current(committing)?;
        let built = PendingRun {
            state: PendingState::Built,
            ..committing.clone()
        };
        self.write(&built)?;
        Ok(built)
    }

    /// Removes `pending.json` and then the staged directory of its run.
    /// Idempotent. Refuses symlinks and never follows them. An unreadable
    /// record is removed without touching a staged directory (see
    /// [`PendingStore::sweep_orphans`]).
    pub fn remove_all(&self) -> Result<(), PendingError> {
        let path = self.path();
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(PendingError::Io(error)),
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => {
                return Err(PendingError::NotRegularFile);
            }
            Ok(_) => {}
        }
        let run_id = match self.load() {
            Ok(Some(run)) => Some(run.run_id),
            _ => None,
        };
        remove_file_if_present(&path)?;
        match run_id {
            Some(run_id) => self.remove_staged(&run_id),
            None => Ok(()),
        }
    }

    /// Removes `staged/<run_id>`. Idempotent. Refuses symlinks and non-directories.
    pub fn remove_staged(&self, run_id: &str) -> Result<(), PendingError> {
        if !is_valid_run_id(run_id) {
            return Err(PendingError::BadRunId);
        }
        let dir = self.state_dir.join(STAGED_DIR_NAME).join(run_id);
        match fs::symlink_metadata(&dir) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(PendingError::Io(error)),
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                Err(PendingError::BadPath("staged directory"))
            }
            Ok(_) => fs::remove_dir_all(&dir).map_err(PendingError::Io),
        }
    }

    /// Best-effort removal of `staged/<id>` directories whose names are valid
    /// run ids and are not `keep_run_id`. Symlinks and other entries are
    /// skipped and never followed. Returns how many directories were removed.
    pub fn sweep_orphans(&self, keep_run_id: Option<&str>) -> usize {
        let Ok(entries) = fs::read_dir(self.state_dir.join(STAGED_DIR_NAME)) else {
            return 0;
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !is_valid_run_id(name) || keep_run_id == Some(name) {
                continue;
            }
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() || !meta.is_dir() {
                continue;
            }
            if fs::remove_dir_all(&path).is_ok() {
                removed += 1;
            }
        }
        removed
    }

    /// Fails unless the record on disk equals `expected`.
    fn require_current(&self, expected: &PendingRun) -> Result<(), PendingError> {
        match self.load()? {
            Some(current) if current == *expected => Ok(()),
            _ => Err(PendingError::BadState),
        }
    }

    /// Validates then writes the record atomically with mode 0600.
    fn write(&self, run: &PendingRun) -> Result<(), PendingError> {
        let bytes = encode_pending(run, &self.state_dir)?;
        write_private_atomic(&self.path(), &bytes)?;
        Ok(())
    }
}

fn check_keys(object: &Map<String, Value>, committing: bool) -> Result<(), PendingError> {
    let extra: &[&str] = if committing {
        &COMMITTING_EXTRA_KEYS
    } else {
        &[]
    };
    let allowed = |key: &str| BUILT_KEYS.contains(&key) || extra.contains(&key);
    if object.keys().any(|key| !allowed(key)) {
        return Err(PendingError::BadField("unexpected field"));
    }
    for key in BUILT_KEYS.iter().chain(extra.iter()) {
        if !object.contains_key(*key) {
            return Err(PendingError::BadField(static_key(key)));
        }
    }
    Ok(())
}

/// Maps a known key back to its `'static` name for error reporting.
fn static_key(key: &str) -> &'static str {
    BUILT_KEYS
        .iter()
        .chain(COMMITTING_EXTRA_KEYS.iter())
        .find(|known| **known == key)
        .copied()
        .unwrap_or("unknown")
}

fn parse_commit_info(
    object: &Map<String, Value>,
    state_dir: &Path,
    run_id: &str,
) -> Result<CommitInfo, PendingError> {
    let install_run_id = text_field(object, "install_run_id")?;
    if !is_valid_run_id(install_run_id) || install_run_id == run_id {
        return Err(PendingError::BadRunId);
    }
    let install_label = text_field(object, "install_label")?;
    if install_label != format!("{JOB_LABEL_PREFIX}{install_run_id}") {
        return Err(PendingError::BadField("install_label"));
    }
    let install_plist = plist_path(state_dir, install_run_id);
    if Some(text_field(object, "install_plist")?) != install_plist.to_str() {
        return Err(PendingError::BadPath("install_plist"));
    }
    Ok(CommitInfo {
        install_run_id: install_run_id.to_string(),
        install_label: install_label.to_string(),
        install_plist,
        committing_ts: uint_field(object, "committing_ts")?,
    })
}

fn field<'a>(
    object: &'a Map<String, Value>,
    name: &'static str,
) -> Result<&'a Value, PendingError> {
    object.get(name).ok_or(PendingError::BadField(name))
}

fn text_field<'a>(
    object: &'a Map<String, Value>,
    name: &'static str,
) -> Result<&'a str, PendingError> {
    field(object, name)?
        .as_str()
        .ok_or(PendingError::BadField(name))
}

fn uint_field(object: &Map<String, Value>, name: &'static str) -> Result<u64, PendingError> {
    field(object, name)?
        .as_u64()
        .ok_or(PendingError::BadField(name))
}

fn sha_field<'a>(
    object: &'a Map<String, Value>,
    name: &'static str,
) -> Result<&'a str, PendingError> {
    let text = text_field(object, name)?;
    if is_lower_hex(text, 64) {
        Ok(text)
    } else {
        Err(PendingError::BadSha256(name))
    }
}

/// True for `[A-Za-z0-9._-]{1,64}` other than `.` and `..`.
fn is_valid_exe_name(name: &str) -> bool {
    let length = name.chars().count();
    (1..=MAX_EXE_NAME_CHARS).contains(&length)
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn path_text(path: &Path, name: &'static str) -> Result<String, PendingError> {
    path.to_str()
        .map(str::to_string)
        .ok_or(PendingError::BadPath(name))
}

fn remove_file_if_present(path: &Path) -> Result<(), PendingError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(PendingError::Io(error)),
    }
}

/// Opens for reading without following a final symlink (`O_NOFOLLOW`).
fn open_no_follow(path: &Path) -> io::Result<fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
    }
    #[cfg(not(unix))]
    {
        fs::File::open(path)
    }
}

#[cfg(test)]
#[path = "source_update_pending_tests.rs"]
mod tests;
