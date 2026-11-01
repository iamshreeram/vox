//! Core logic for the detached source-update runner (DESIGN.md Revision 4,
//! D2/D3/R2-1/R2-2/R2-3/R2-5/R2-7/R3-1).
//!
//! Everything here is pure or narrowly scoped I/O: status-file validation,
//! state-directory hygiene, the one-shot launchd job plist, a `launchctl`
//! probe behind a trait, and the reconciliation decision table. The start,
//! adoption and poller logic built on top lives in the `source_update_runner_*`
//! sibling modules.
//!
//! Security notes: the status file is untrusted input (strict parse, fail
//! closed). No user-controlled string reaches argv or a shell: `launchctl` is
//! invoked with fixed argv, and plist text is XML-escaped after validation.

use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use super::source_update_pending::{PendingRun, PendingState, PENDING_FILE_NAME};

/// Largest status file accepted, in bytes.
pub const MAX_STATUS_BYTES: usize = 4096;
/// Longest accepted status `message`, in characters.
pub const MAX_MESSAGE_CHARS: usize = 200;
/// Longest message carried into a failure decision, in characters.
pub const MAX_FAILURE_MESSAGE_CHARS: usize = 120;
/// How far a status timestamp may lie in the future, in seconds.
pub const MAX_FUTURE_SKEW_SECS: u64 = 86_400;
/// A non-terminal run with no live job for this long is interrupted.
pub const INTERRUPTED_AFTER_SECS: u64 = 900;
/// Number of run logs and plists kept by `rotate_logs`.
pub const KEEP_RUN_LOGS: usize = 3;
/// Lowest percent a `waiting` status may report (R6-1 band, R7-1 percent 91).
pub const WAITING_PERCENT_MIN: u64 = 88;
/// Highest percent a `waiting` status may report. Never 100.
pub const WAITING_PERCENT_MAX: u64 = 91;
/// Prefix of every one-shot updater job label.
pub const JOB_LABEL_PREFIX: &str = "com.iamshreeram.vox.update.";
/// Status file name inside the state directory.
pub const STATUS_FILE_NAME: &str = "status.json";
/// The update script, relative to the checkout root.
pub const UPDATE_SCRIPT_RELATIVE: &str = "scripts/update-macos.sh";
/// Absolute path of the launchctl binary.
pub const LAUNCHCTL_PATH: &str = "/bin/launchctl";

/// Phase of an update run, as written by `scripts/lib/update-status.sh`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum RunPhase {
    /// Fast-forward to the pinned target.
    Preparing,
    /// `bun install`.
    Dependencies,
    /// Frontend build and cargo compile.
    Compiling,
    /// Bundling the `.app`.
    Bundling,
    /// Resting: the build is staged and waits for a commit (non-terminal, R7-1).
    Waiting,
    /// Installer staging and swap.
    Installing,
    /// Installer restarting Vox.
    Restarting,
    /// Finished successfully.
    Done,
    /// Finished unsuccessfully.
    Failed,
}

impl RunPhase {
    /// True for `Done` and `Failed`: no further phase may follow.
    pub fn is_terminal(self) -> bool {
        matches!(self, RunPhase::Done | RunPhase::Failed)
    }

    /// Position in the forward phase order used by the monotonic check.
    fn rank(self) -> u8 {
        match self {
            RunPhase::Preparing => 0,
            RunPhase::Dependencies => 1,
            RunPhase::Compiling => 2,
            RunPhase::Bundling => 3,
            RunPhase::Waiting => 4,
            RunPhase::Installing => 5,
            RunPhase::Restarting => 6,
            RunPhase::Done | RunPhase::Failed => 7,
        }
    }
}

/// How the percent in a status was produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum StatusMethod {
    /// Percent taken from the real build output.
    Exact,
    /// Percent is a time-based estimate.
    Estimated,
}

/// One validated status file, as the runner sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RunStatus {
    /// 16 lowercase hex characters identifying the run.
    pub run_id: String,
    /// Current phase.
    pub phase: RunPhase,
    /// Progress, 0..=100.
    pub percent: u8,
    /// 7 lowercase hex characters (short target).
    pub target: String,
    /// 40 lowercase hex characters, or empty when unknown.
    pub target_full: String,
    /// Short human-readable message (at most 200 characters, no control chars).
    pub message: String,
    /// Whether the percent is exact or estimated.
    pub method: StatusMethod,
    /// Unix seconds written by the script.
    pub ts: u64,
}

/// What the UI and tray receive for a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct UpdateProgress {
    /// Run identifier.
    pub run_id: String,
    /// Current phase.
    pub phase: RunPhase,
    /// Progress, 0..=100.
    pub percent: u8,
    /// Short target (7 hex characters).
    pub target: String,
    /// Message from the status file.
    pub message: String,
    /// True when the percent is an estimate.
    pub estimated: bool,
    /// True for an automatic run (its `run-<id>.auto` marker exists). The footer and the tray
    /// ignore silent progress; results appear only in the Settings status.
    #[serde(default)]
    pub silent: bool,
}

impl From<&RunStatus> for UpdateProgress {
    fn from(status: &RunStatus) -> Self {
        UpdateProgress {
            run_id: status.run_id.clone(),
            phase: status.phase,
            percent: status.percent,
            target: status.target.clone(),
            message: status.message.clone(),
            estimated: status.method == StatusMethod::Estimated,
            silent: false,
        }
    }
}

/// Reasons a status file is rejected. Display never echoes the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusError {
    /// More than `MAX_STATUS_BYTES` bytes.
    TooLarge,
    /// Not valid UTF-8.
    InvalidUtf8,
    /// Not parseable JSON.
    InvalidJson,
    /// Parseable JSON that is not an object.
    NotAnObject,
    /// `v` is not 1.
    BadVersion,
    /// A required field is missing, has the wrong type, or is unexpected.
    BadField(&'static str),
    /// `run_id` is malformed or does not match the expected run.
    BadRunId,
    /// `phase` is not one of the eight known phases.
    UnknownPhase,
    /// `percent` is above 100.
    PercentOutOfRange,
    /// A non-terminal phase reports 100, or `done` reports other than 100.
    PercentNotAllowed,
    /// `target` is malformed or disagrees with `target_full`.
    BadTarget,
    /// `target_full` is neither empty nor 40 lowercase hex.
    BadTargetFull,
    /// `message` is longer than `MAX_MESSAGE_CHARS`.
    MessageTooLong,
    /// `message` contains a control character.
    MessageControlChar,
    /// `ts` is more than `MAX_FUTURE_SKEW_SECS` in the future.
    FutureTimestamp,
    /// Percent went down within the same run.
    Regress,
    /// Phase went backwards, or left a terminal phase.
    PhaseRegress,
}

impl fmt::Display for StatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StatusError::TooLarge => {
                write!(f, "status file is larger than {MAX_STATUS_BYTES} bytes")
            }
            StatusError::InvalidUtf8 => write!(f, "status file is not valid UTF-8"),
            StatusError::InvalidJson => write!(f, "status file is not valid JSON"),
            StatusError::NotAnObject => write!(f, "status file is not a JSON object"),
            StatusError::BadVersion => write!(f, "unsupported status version"),
            StatusError::BadField(name) => write!(f, "invalid or missing status field: {name}"),
            StatusError::BadRunId => write!(f, "status run id does not match the expected run"),
            StatusError::UnknownPhase => write!(f, "unknown status phase"),
            StatusError::PercentOutOfRange => write!(f, "status percent is out of range"),
            StatusError::PercentNotAllowed => {
                write!(f, "status percent is not allowed for its phase")
            }
            StatusError::BadTarget => write!(f, "invalid status target"),
            StatusError::BadTargetFull => write!(f, "invalid status target_full"),
            StatusError::MessageTooLong => write!(f, "status message is too long"),
            StatusError::MessageControlChar => {
                write!(f, "status message contains control characters")
            }
            StatusError::FutureTimestamp => write!(f, "status timestamp is in the future"),
            StatusError::Regress => write!(f, "status percent went backwards"),
            StatusError::PhaseRegress => write!(f, "status phase went backwards"),
        }
    }
}

impl std::error::Error for StatusError {}

const STATUS_KEYS: [&str; 9] = [
    "v",
    "run_id",
    "phase",
    "percent",
    "target",
    "target_full",
    "message",
    "method",
    "ts",
];

/// Parses and validates one status file. Fail closed: any violation is an
/// error and the caller keeps its previous status.
///
/// `expected_run_id` is the run the caller is watching; `prev` is the last
/// accepted status (only compared when it belongs to the same run);
/// `now_unix` is the current time in Unix seconds.
pub fn parse_status(
    bytes: &[u8],
    expected_run_id: &str,
    prev: Option<&RunStatus>,
    now_unix: u64,
) -> Result<RunStatus, StatusError> {
    if bytes.len() > MAX_STATUS_BYTES {
        return Err(StatusError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| StatusError::InvalidUtf8)?;
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| StatusError::InvalidJson)?;
    let object = value.as_object().ok_or(StatusError::NotAnObject)?;
    if object
        .keys()
        .any(|key| !STATUS_KEYS.contains(&key.as_str()))
    {
        return Err(StatusError::BadField("unexpected field"));
    }
    for key in STATUS_KEYS {
        if !object.contains_key(key) {
            return Err(StatusError::BadField(key));
        }
    }

    if uint_field(object, "v")? != 1 {
        return Err(StatusError::BadVersion);
    }

    let run_id = text_field(object, "run_id")?;
    if !is_valid_run_id(expected_run_id) || run_id != expected_run_id {
        return Err(StatusError::BadRunId);
    }

    let phase = parse_phase_name(text_field(object, "phase")?).ok_or(StatusError::UnknownPhase)?;

    let raw_percent = uint_field(object, "percent")?;
    if raw_percent > 100 {
        return Err(StatusError::PercentOutOfRange);
    }
    if phase == RunPhase::Done && raw_percent != 100 {
        return Err(StatusError::PercentNotAllowed);
    }
    if phase != RunPhase::Done && phase != RunPhase::Failed && raw_percent == 100 {
        return Err(StatusError::PercentNotAllowed);
    }
    if phase == RunPhase::Waiting
        && !(WAITING_PERCENT_MIN..=WAITING_PERCENT_MAX).contains(&raw_percent)
    {
        return Err(StatusError::PercentNotAllowed);
    }
    let percent = u8::try_from(raw_percent).map_err(|_| StatusError::PercentOutOfRange)?;

    let target = text_field(object, "target")?;
    if !is_lower_hex(target, 7) {
        return Err(StatusError::BadTarget);
    }
    let target_full = text_field(object, "target_full")?;
    if !target_full.is_empty() && !is_lower_hex(target_full, 40) {
        return Err(StatusError::BadTargetFull);
    }
    if !target_full.is_empty() && !target_full.starts_with(target) {
        return Err(StatusError::BadTarget);
    }

    let message = text_field(object, "message")?;
    if message.chars().count() > MAX_MESSAGE_CHARS {
        return Err(StatusError::MessageTooLong);
    }
    if message.chars().any(char::is_control) {
        return Err(StatusError::MessageControlChar);
    }

    let method = match text_field(object, "method")? {
        "exact" => StatusMethod::Exact,
        "estimated" => StatusMethod::Estimated,
        _ => return Err(StatusError::BadField("method")),
    };

    let ts = uint_field(object, "ts")?;
    if ts > now_unix.saturating_add(MAX_FUTURE_SKEW_SECS) {
        return Err(StatusError::FutureTimestamp);
    }

    if let Some(prev) = prev.filter(|p| p.run_id == run_id) {
        if prev.phase.is_terminal() {
            if phase != prev.phase {
                return Err(StatusError::PhaseRegress);
            }
        } else if phase != RunPhase::Failed && phase.rank() < prev.phase.rank() {
            return Err(StatusError::PhaseRegress);
        }
        if percent < prev.percent {
            return Err(StatusError::Regress);
        }
    }

    Ok(RunStatus {
        run_id: run_id.to_string(),
        phase,
        percent,
        target: target.to_string(),
        target_full: target_full.to_string(),
        message: message.to_string(),
        method,
        ts,
    })
}

fn field<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    name: &'static str,
) -> Result<&'a serde_json::Value, StatusError> {
    object.get(name).ok_or(StatusError::BadField(name))
}

fn text_field<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    name: &'static str,
) -> Result<&'a str, StatusError> {
    field(object, name)?
        .as_str()
        .ok_or(StatusError::BadField(name))
}

fn uint_field(
    object: &serde_json::Map<String, serde_json::Value>,
    name: &'static str,
) -> Result<u64, StatusError> {
    field(object, name)?
        .as_u64()
        .ok_or(StatusError::BadField(name))
}

fn parse_phase_name(name: &str) -> Option<RunPhase> {
    match name {
        "preparing" => Some(RunPhase::Preparing),
        "dependencies" => Some(RunPhase::Dependencies),
        "compiling" => Some(RunPhase::Compiling),
        "bundling" => Some(RunPhase::Bundling),
        "installing" => Some(RunPhase::Installing),
        "waiting" => Some(RunPhase::Waiting),
        "restarting" => Some(RunPhase::Restarting),
        "done" => Some(RunPhase::Done),
        "failed" => Some(RunPhase::Failed),
        _ => None,
    }
}

/// True when `text` is exactly `len` lowercase hex characters.
pub(crate) fn is_lower_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// True for a run id: exactly 16 lowercase hex characters.
pub fn is_valid_run_id(run_id: &str) -> bool {
    is_lower_hex(run_id, 16)
}

/// Creates a fresh run id from the operating system CSPRNG (`/dev/urandom`).
/// Returns an empty string when no randomness is available; callers must
/// check `is_valid_run_id` before use (empty fails closed everywhere).
pub fn new_run_id() -> String {
    let mut bytes = [0u8; 8];
    match fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut bytes)) {
        Ok(()) => bytes.iter().map(|b| format!("{b:02x}")).collect(),
        Err(_) => String::new(),
    }
}

/// The launchd label of the one-shot job for `run_id`.
pub fn job_label(run_id: &str) -> String {
    format!("{JOB_LABEL_PREFIX}{run_id}")
}

/// Refuses any label outside the update namespace (prefix + valid run id).
pub fn validate_job_label(label: &str) -> Result<(), RunnerError> {
    match label.strip_prefix(JOB_LABEL_PREFIX) {
        Some(run_id) if is_valid_run_id(run_id) => Ok(()),
        _ => Err(RunnerError::LabelRefused),
    }
}

/// The per-run status-file directory layout under the state directory.
pub fn update_state_dir(app_data: &Path) -> PathBuf {
    app_data.join("update")
}

/// `state_dir/status.json`.
pub fn status_path(state_dir: &Path) -> PathBuf {
    state_dir.join(STATUS_FILE_NAME)
}

/// `state_dir/run-<run_id>.log`.
pub fn log_path(state_dir: &Path, run_id: &str) -> PathBuf {
    state_dir.join(format!("run-{run_id}.log"))
}

/// `state_dir/run-<run_id>.plist`.
pub fn plist_path(state_dir: &Path, run_id: &str) -> PathBuf {
    state_dir.join(format!("run-{run_id}.plist"))
}

/// Creates the state directory with mode 0700, or validates an existing one.
/// Refuses symlinks, non-directories, directories owned by another user, and
/// group- or world-writable directories.
#[cfg(unix)]
pub fn ensure_state_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    match fs::symlink_metadata(dir) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new().mode(0o700).create(dir)?;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        Err(error) => return Err(error),
    }
    let meta = fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "state path is not a plain directory",
        ));
    }
    if std::os::unix::fs::MetadataExt::uid(&meta) != current_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "state directory is owned by another user",
        ));
    }
    if meta.permissions().mode() & 0o022 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "state directory is group or world writable",
        ));
    }
    Ok(())
}

/// Non-unix builds have no update runner: fail closed.
#[cfg(not(unix))]
pub fn ensure_state_dir(_dir: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "source update runner requires a unix host",
    ))
}

/// Keeps the newest `keep` runs (grouped by run id across `run-<id>.log` and
/// `run-<id>.plist`) and removes the rest. Other files are never touched and
/// only regular files directly inside `dir` are considered.
///
/// Callers must not pass a run whose job is still live: this function does
/// not know which run is active.
pub fn rotate_logs(dir: &Path, keep: usize) -> io::Result<()> {
    let mut runs: BTreeMap<String, (SystemTime, Vec<PathBuf>)> = BTreeMap::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(run_id) = run_id_of_artifact(name) else {
            continue;
        };
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        let modified = meta.modified().unwrap_or(UNIX_EPOCH);
        let slot = runs
            .entry(run_id.to_string())
            .or_insert((UNIX_EPOCH, Vec::new()));
        if modified > slot.0 {
            slot.0 = modified;
        }
        slot.1.push(path);
    }

    let mut ordered: Vec<(String, (SystemTime, Vec<PathBuf>))> = runs.into_iter().collect();
    ordered.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then_with(|| b.0.cmp(&a.0)));
    for (_, (_, paths)) in ordered.into_iter().skip(keep) {
        for path in paths {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

/// Extracts the run id from `run-<id>.log` or `run-<id>.plist`.
fn run_id_of_artifact(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("run-")?;
    let id = rest
        .strip_suffix(".log")
        .or_else(|| rest.strip_suffix(".plist"))?;
    is_valid_run_id(id).then_some(id)
}

/// Everything needed to describe the one-shot updater job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobParams {
    /// Run identifier (16 lowercase hex).
    pub run_id: String,
    /// Absolute checkout root containing `scripts/update-macos.sh`.
    pub source_dir: PathBuf,
    /// Absolute path of the status file.
    pub status_file: PathBuf,
    /// Absolute path of the state directory.
    pub state_dir: PathBuf,
    /// Absolute path of the job's log file.
    pub log_file: PathBuf,
    /// Full 40-hex target commit.
    pub target_full: String,
    /// Absolute home directory.
    pub home: PathBuf,
    /// Absolute temporary directory (also used for the lock).
    pub tmpdir: PathBuf,
    /// Absolute lock directory the script must use.
    pub lock_dir: PathBuf,
    /// Proxy variables to forward (allow-listed names only).
    pub proxy_env: Vec<(String, String)>,
}

const PROXY_NAMES: [&str; 6] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
];

const PLIST_HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n";

/// Builds the launchd plist for the one-shot update job. Every input is
/// validated first; all text is XML-escaped. The environment is an
/// allow-list: nothing from Vox's own environment is inherited except the
/// validated proxy variables and `USER`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn build_job_plist(params: &JobParams) -> Result<String, RunnerError> {
    build_job_plist_kind(params, RunKind::All)
}

/// Which phase of the update a job runs (DESIGN R7-1).
#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(test), allow(dead_code))]
pub enum RunKind<'a> {
    /// Manual single-phase run (build and install in one job). Bytes unchanged from the first runner.
    All,
    /// Build phase: stages the app and exits without installing.
    Build {
        /// True when an automatic policy started the run.
        origin_auto: bool,
    },
    /// Install phase for a record already moved to `committing`.
    Install {
        /// The committing record; its install run id must equal `JobParams::run_id`.
        pending: &'a PendingRun,
    },
}

/// Percent floor the install phase reports (the resting build percent).
pub const INSTALL_PERCENT_FLOOR: u8 = 91;

/// Builds the plist for one phase of the update. [`RunKind::All`] yields exactly
/// the bytes of [`build_job_plist`]. Each phase adds only its own keys.
pub fn build_job_plist_kind(params: &JobParams, kind: RunKind<'_>) -> Result<String, RunnerError> {
    if !is_valid_run_id(&params.run_id) {
        return Err(RunnerError::InvalidValue("run_id"));
    }
    if !is_lower_hex(&params.target_full, 40) {
        return Err(RunnerError::InvalidValue("target_full"));
    }
    let source = abs_text("source_dir", &params.source_dir)?;
    let status_file = abs_text("status_file", &params.status_file)?;
    let state_dir = abs_text("state_dir", &params.state_dir)?;
    let log_file = abs_text("log_file", &params.log_file)?;
    let home = abs_text("home", &params.home)?;
    let tmpdir = abs_text("tmpdir", &params.tmpdir)?;
    let lock_dir = abs_text("lock_dir", &params.lock_dir)?;
    let user = user_name(&params.home)?;
    let proxies = validated_proxies(&params.proxy_env)?;

    let script = format!("{source}/{UPDATE_SCRIPT_RELATIVE}");
    let path_env = format!(
        "{home}/.cargo/bin:{home}/.local/bin:{home}/.bun/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    );
    let mut env: Vec<(String, String)> = vec![
        ("HOME".into(), home.clone()),
        ("USER".into(), user.clone()),
        ("LOGNAME".into(), user),
        ("TMPDIR".into(), tmpdir),
        ("PATH".into(), path_env),
        ("LANG".into(), "en_US.UTF-8".into()),
        ("GIT_TERMINAL_PROMPT".into(), "0".into()),
        ("GIT_SSH_COMMAND".into(), "ssh -o BatchMode=yes".into()),
        ("VOX_SOURCE_UPDATE".into(), "1".into()),
        ("VOX_UPDATE_RUN_ID".into(), params.run_id.clone()),
        ("VOX_UPDATE_STATUS_FILE".into(), status_file.clone()),
        ("VOX_UPDATE_STATE_DIR".into(), state_dir),
    ];
    env.extend(kind_env(params, &kind)?);
    env.extend([
        ("VOX_UPDATE_NONINTERACTIVE".to_string(), "1".to_string()),
        ("VOX_UPDATE_LOCK_DIR".to_string(), lock_dir),
        ("CARGO_TERM_PROGRESS_WHEN".to_string(), "always".to_string()),
        ("CARGO_TERM_PROGRESS_WIDTH".to_string(), "100".to_string()),
    ]);
    env.extend(proxies);

    let mut xml = String::from(PLIST_HEADER);
    push_key_string(&mut xml, "Label", &job_label(&params.run_id));
    push_key_array(&mut xml, "ProgramArguments", &["/bin/bash", &script]);
    push_key_string(&mut xml, "WorkingDirectory", &source);
    push_key_bool(&mut xml, "RunAtLoad", true);
    push_key_string(&mut xml, "ProcessType", "Background");
    push_key_bool(&mut xml, "LowPriorityIO", true);
    push_key_integer(&mut xml, "Nice", 10);
    push_key_bool(&mut xml, "AbandonProcessGroup", true);
    push_key_string(&mut xml, "StandardOutPath", &log_file);
    push_key_string(&mut xml, "StandardErrorPath", &log_file);
    xml.push_str("\t<key>EnvironmentVariables</key>\n\t<dict>\n");
    for (name, value) in &env {
        xml.push_str(&format!(
            "\t\t<key>{}</key>\n\t\t<string>{}</string>\n",
            xml_escape(name),
            xml_escape(value)
        ));
    }
    xml.push_str("\t</dict>\n</dict>\n</plist>\n");
    Ok(xml)
}

/// The phase-specific environment keys, placed where the target sha sits for `All`.
fn kind_env(params: &JobParams, kind: &RunKind<'_>) -> Result<Vec<(String, String)>, RunnerError> {
    match kind {
        RunKind::All => Ok(vec![(
            "VOX_UPDATE_TARGET_SHA".to_string(),
            params.target_full.clone(),
        )]),
        RunKind::Build { origin_auto } => {
            let origin = if *origin_auto { "auto" } else { "manual" };
            Ok(vec![
                (
                    "VOX_UPDATE_TARGET_SHA".to_string(),
                    params.target_full.clone(),
                ),
                ("VOX_UPDATE_PHASE".to_string(), "build".to_string()),
                ("VOX_UPDATE_ORIGIN".to_string(), origin.to_string()),
            ])
        }
        RunKind::Install { pending } => {
            let PendingState::Committing(info) = &pending.state else {
                return Err(RunnerError::InvalidValue("install state"));
            };
            if info.install_run_id != params.run_id {
                return Err(RunnerError::InvalidValue("install run id"));
            }
            let pending_file = abs_text("pending_file", &params.state_dir.join(PENDING_FILE_NAME))?;
            Ok(vec![
                ("VOX_UPDATE_PHASE".to_string(), "install".to_string()),
                ("VOX_UPDATE_PENDING_FILE".to_string(), pending_file),
                (
                    "VOX_UPDATE_PERCENT_FLOOR".to_string(),
                    INSTALL_PERCENT_FLOOR.to_string(),
                ),
            ])
        }
    }
}

/// Checks that `source_dir` is absolute and contains a regular (non-symlink)
/// `scripts/update-macos.sh`.
pub fn validate_source_dir(source_dir: &Path) -> Result<(), RunnerError> {
    abs_text("source_dir", source_dir)?;
    let script = source_dir.join(UPDATE_SCRIPT_RELATIVE);
    let meta = fs::symlink_metadata(&script).map_err(|_| RunnerError::ScriptMissing)?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(RunnerError::ScriptMissing);
    }
    Ok(())
}

/// Writes a plist atomically with mode 0600 (temp file in the same directory,
/// then rename over `path`).
pub fn write_plist_file(path: &Path, xml: &str) -> io::Result<()> {
    write_private_atomic(path, xml.as_bytes())
}

/// Writes `bytes` to a sibling temp file created with mode 0600, then renames it
/// over `path`. A stale temp file from an earlier crash is replaced.
pub(crate) fn write_private_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "path has no parent directory")
    })?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let tmp = parent.join(format!(".{name}.tmp"));
    let _ = fs::remove_file(&tmp);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)
}

/// Writes `status` to `path` atomically (mode 0600) in the wire format the
/// script uses: the status fields plus `"v": 1`.
pub fn write_status_file(path: &Path, status: &RunStatus) -> io::Result<()> {
    let mut value = serde_json::to_value(status)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if let Some(object) = value.as_object_mut() {
        object.insert("v".to_string(), serde_json::Value::from(1));
    }
    write_private_atomic(path, value.to_string().as_bytes())
}

/// Reads a status file for at most `MAX_STATUS_BYTES + 1` bytes, so an oversized
/// file is seen as oversized by the parser. `Ok(None)` when it does not exist.
pub fn read_status_bounded(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take((MAX_STATUS_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

/// The run id recorded in status bytes, when that field is a valid run id.
/// Nothing else in the file is checked here.
pub fn run_id_in_status(bytes: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let run_id = value.get("run_id")?.as_str()?;
    is_valid_run_id(run_id).then(|| run_id.to_string())
}

/// Current Unix time in seconds (0 if the clock is before the epoch).
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whole seconds since `path` was last modified, or `None` when unknown.
pub fn file_age_secs(path: &Path) -> Option<u64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    SystemTime::now()
        .duration_since(modified)
        .ok()
        .map(|d| d.as_secs())
}

/// Proxy variables to forward to the job, read through `lookup` and filtered to
/// the allow-list. Empty when none is set.
pub fn proxy_env_from(lookup: &dyn Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    PROXY_NAMES
        .iter()
        .filter_map(|name| lookup(name).map(|value| ((*name).to_string(), value)))
        .filter(|(_, value)| !value.is_empty())
        .collect()
}

fn abs_text(field: &'static str, path: &Path) -> Result<String, RunnerError> {
    let text = path.to_str().ok_or(RunnerError::InvalidPath(field))?;
    if !path.is_absolute() || !is_printable(text) {
        return Err(RunnerError::InvalidPath(field));
    }
    Ok(text.to_string())
}

/// Non-empty and free of control characters (covers NUL and newlines).
fn is_printable(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(char::is_control)
}

fn user_name(home: &Path) -> Result<String, RunnerError> {
    if let Ok(user) = std::env::var("USER") {
        if is_printable(&user) {
            return Ok(user);
        }
    }
    home.file_name()
        .and_then(|n| n.to_str())
        .filter(|n| is_printable(n))
        .map(str::to_string)
        .ok_or(RunnerError::InvalidValue("user"))
}

fn validated_proxies(env: &[(String, String)]) -> Result<Vec<(String, String)>, RunnerError> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (name, value) in env {
        if !PROXY_NAMES.contains(&name.as_str()) {
            return Err(RunnerError::InvalidValue("proxy name"));
        }
        if out.iter().any(|(seen, _)| seen == name) {
            return Err(RunnerError::InvalidValue("duplicate proxy name"));
        }
        if value.chars().any(char::is_control) {
            return Err(RunnerError::InvalidValue("proxy value"));
        }
        out.push((name.clone(), value.clone()));
    }
    Ok(out)
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn push_key_string(xml: &mut String, key: &str, value: &str) {
    xml.push_str(&format!(
        "\t<key>{key}</key>\n\t<string>{}</string>\n",
        xml_escape(value)
    ));
}

fn push_key_array(xml: &mut String, key: &str, values: &[&str]) {
    xml.push_str(&format!("\t<key>{key}</key>\n\t<array>\n"));
    for value in values {
        xml.push_str(&format!("\t\t<string>{}</string>\n", xml_escape(value)));
    }
    xml.push_str("\t</array>\n");
}

fn push_key_bool(xml: &mut String, key: &str, value: bool) {
    let tag = if value { "true" } else { "false" };
    xml.push_str(&format!("\t<key>{key}</key>\n\t<{tag}/>\n"));
}

fn push_key_integer(xml: &mut String, key: &str, value: i64) {
    xml.push_str(&format!(
        "\t<key>{key}</key>\n\t<integer>{value}</integer>\n"
    ));
}

/// Errors from the runner. Display never includes secrets or raw input.
#[derive(Debug)]
pub enum RunnerError {
    /// A path is relative, empty, non-UTF-8, or contains control characters.
    InvalidPath(&'static str),
    /// A value fails validation.
    InvalidValue(&'static str),
    /// `scripts/update-macos.sh` is missing or not a regular file.
    ScriptMissing,
    /// A launchd label outside the update namespace was refused.
    LabelRefused,
    /// A launchctl subcommand failed.
    Launchctl {
        /// Subcommand name.
        operation: &'static str,
        /// Exit code, when there was one.
        code: Option<i32>,
    },
    /// An I/O failure.
    Io(io::Error),
    /// A run, lock, pending record or install plist already exists, so nothing new may start.
    Busy(&'static str),
}

impl fmt::Display for RunnerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunnerError::InvalidPath(field) => write!(f, "invalid absolute path for {field}"),
            RunnerError::InvalidValue(field) => write!(f, "invalid value for {field}"),
            RunnerError::ScriptMissing => {
                write!(
                    f,
                    "scripts/update-macos.sh is missing or not a regular file"
                )
            }
            RunnerError::LabelRefused => {
                write!(f, "refusing a job label outside the update namespace")
            }
            RunnerError::Launchctl { operation, code } => match code {
                Some(code) => write!(f, "launchctl {operation} failed (exit {code})"),
                None => write!(f, "launchctl {operation} failed"),
            },
            RunnerError::Io(error) => write!(f, "I/O error: {error}"),
            RunnerError::Busy(what) => write!(f, "an update is already in progress: {what}"),
        }
    }
}

impl std::error::Error for RunnerError {}

impl From<io::Error> for RunnerError {
    fn from(error: io::Error) -> Self {
        RunnerError::Io(error)
    }
}

/// What launchd reports about a job label (three-valued, R9-2).
///
/// `Unknown` means the state could not be determined (launchctl failed, timed
/// out or printed something unparseable). It is never treated as `Absent`:
/// callers treat it as maybe alive and never interrupt, clean up or release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobProbe {
    /// Confirmed not registered with launchd.
    Absent,
    /// Registered. `pid` is set only while a process is running.
    Registered {
        /// Running process id, if any.
        pid: Option<u32>,
        /// Last exit code, if the job has exited before.
        last_exit: Option<i32>,
    },
    /// The state could not be determined. Maybe alive.
    Unknown,
}

impl JobProbe {
    /// Live means registered AND a process is running.
    pub fn is_live(&self) -> bool {
        matches!(self, JobProbe::Registered { pid: Some(_), .. })
    }

    /// True when the state could not be determined.
    pub fn is_unknown(&self) -> bool {
        matches!(self, JobProbe::Unknown)
    }
}

/// Exit status `launchctl print` uses for "could not find service" (confirmed absence).
pub const LAUNCHCTL_NOT_FOUND_CODE: i32 = 113;

/// Parses the stdout of a successful `launchctl print gui/<uid>/<label>`.
/// Output with no recognised `state`, `pid` or `last exit code` line is
/// unparseable and therefore `Unknown`, never `Absent`.
pub fn parse_launchctl_print(text: &str) -> JobProbe {
    let mut state: Option<&str> = None;
    let mut pid: Option<u32> = None;
    let mut last_exit: Option<i32> = None;
    let mut seen = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "state" => {
                state = Some(value);
                seen = true;
            }
            "pid" => {
                pid = value.parse().ok();
                seen = true;
            }
            "last exit code" => {
                last_exit = value.parse().ok();
                seen = true;
            }
            _ => {}
        }
    }
    if !seen {
        return JobProbe::Unknown;
    }
    let pid = if state == Some("not running") {
        None
    } else {
        pid
    };
    JobProbe::Registered { pid, last_exit }
}

/// Launchd operations used by the runner. Implemented for real by
/// `LaunchctlJobControl` and for tests by `FakeJobControl`.
pub trait JobControl {
    /// Current state of `label`.
    fn probe(&self, label: &str) -> JobProbe;
    /// Registers the job described by `plist` (`launchctl bootstrap`).
    fn bootstrap(&self, plist: &Path) -> Result<(), RunnerError>;
    /// Removes the registration for `label` (`launchctl bootout`).
    fn bootout(&self, label: &str) -> Result<(), RunnerError>;
}

/// Real launchctl adapter for the current user's GUI domain.
#[derive(Debug, Clone, Copy)]
pub struct LaunchctlJobControl {
    uid: u32,
}

impl LaunchctlJobControl {
    /// Adapter bound to the effective uid of this process.
    pub fn current() -> Self {
        Self { uid: current_uid() }
    }
}

impl JobControl for LaunchctlJobControl {
    fn probe(&self, label: &str) -> JobProbe {
        if validate_job_label(label).is_err() {
            return JobProbe::Unknown;
        }
        match Command::new(LAUNCHCTL_PATH)
            .args(print_args(self.uid, label))
            .output()
        {
            Ok(output) if output.status.success() => {
                parse_launchctl_print(&String::from_utf8_lossy(&output.stdout))
            }
            Ok(output) if output.status.code() == Some(LAUNCHCTL_NOT_FOUND_CODE) => {
                JobProbe::Absent
            }
            _ => JobProbe::Unknown,
        }
    }

    fn bootstrap(&self, plist: &Path) -> Result<(), RunnerError> {
        let args = bootstrap_args(self.uid, plist)?;
        run_launchctl("bootstrap", &args)
    }

    fn bootout(&self, label: &str) -> Result<(), RunnerError> {
        validate_job_label(label)?;
        match run_launchctl("bootout", &bootout_args(self.uid, label)) {
            Ok(()) => Ok(()),
            // Already gone is success for an idempotent cleanup.
            Err(error) => {
                if self.probe(label) == JobProbe::Absent {
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
    }
}

fn run_launchctl(operation: &'static str, args: &[String]) -> Result<(), RunnerError> {
    let output = Command::new(LAUNCHCTL_PATH).args(args).output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(RunnerError::Launchctl {
            operation,
            code: output.status.code(),
        })
    }
}

/// argv for `launchctl print gui/<uid>/<label>`.
pub fn print_args(uid: u32, label: &str) -> Vec<String> {
    vec!["print".to_string(), format!("gui/{uid}/{label}")]
}

/// argv for `launchctl bootstrap gui/<uid> <plist>`.
pub fn bootstrap_args(uid: u32, plist: &Path) -> Result<Vec<String>, RunnerError> {
    let plist_text = abs_text("plist", plist)?;
    Ok(vec![
        "bootstrap".to_string(),
        format!("gui/{uid}"),
        plist_text,
    ])
}

/// argv for `launchctl bootout gui/<uid>/<label>`.
pub fn bootout_args(uid: u32, label: &str) -> Vec<String> {
    vec!["bootout".to_string(), format!("gui/{uid}/{label}")]
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

/// True when a process with `pid` exists (EPERM counts as existing).
#[cfg(unix)]
pub fn process_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only checks for existence; no signal is delivered.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Non-unix builds cannot prove a holder is dead, so they report alive.
#[cfg(not(unix))]
pub fn process_alive(_pid: u32) -> bool {
    true
}

/// State of the update lock directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    /// No lock directory.
    Free,
    /// Held by a process that is alive, or the holder cannot be determined.
    HeldAlive,
    /// Held by a process that no longer exists.
    HeldDead,
}

/// Reads the lock held by `scripts/lib/update-lock.sh`: a directory with an
/// `owner` file containing `<pid>-<epoch>-<random>`. Never removes or
/// modifies anything. Anything unreadable or unparseable counts as held and
/// alive, so cleanup never runs on uncertain evidence.
pub fn lock_state(lock_dir: &Path, pid_alive: &dyn Fn(u32) -> bool) -> LockState {
    match fs::symlink_metadata(lock_dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return LockState::Free,
        Err(_) => return LockState::HeldAlive,
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return LockState::HeldAlive;
        }
        Ok(_) => {}
    }
    let owner_path = lock_dir.join("owner");
    match fs::metadata(&owner_path) {
        Ok(meta) if meta.len() <= 256 => {}
        _ => return LockState::HeldAlive,
    }
    let Ok(owner) = fs::read_to_string(&owner_path) else {
        return LockState::HeldAlive;
    };
    match owner_pid(&owner) {
        Some(pid) if pid_alive(pid) => LockState::HeldAlive,
        Some(_) => LockState::HeldDead,
        None => LockState::HeldAlive,
    }
}

/// Extracts the holder pid from `<pid>-<epoch>-<random>` (all digits, pid > 0).
fn owner_pid(owner: &str) -> Option<u32> {
    let parts: Vec<&str> = owner.trim().split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let all_digits = parts
        .iter()
        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    if !all_digits {
        return None;
    }
    match parts[0].parse::<u32>() {
        Ok(pid) if pid > 0 => Some(pid),
        _ => None,
    }
}

/// Everything the reconciliation decision needs, gathered by the caller.
#[derive(Debug, Clone)]
pub struct ReconcileInput<'a> {
    /// Last valid status, if any.
    pub status: Option<RunStatus>,
    /// Age of the status (or of the run's plist when no status exists), seconds.
    pub status_age_secs: u64,
    /// Launchd state of the run's job.
    pub job: JobProbe,
    /// Lock directory state.
    pub lock: LockState,
    /// Full SHA this running build was compiled from, or empty when unknown.
    pub running_build_sha: &'a str,
    /// Whether the terminal outcome has already been published to the UI.
    pub published: bool,
    /// Non-terminal runs without a live job for this long are interrupted.
    pub interrupted_after_secs: u64,
    /// True when a valid `pending.json` in state `built` exists for this run (R7-4).
    pub pending_built: bool,
}

/// What the runner should do next. No variant removes a lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Nothing to do.
    None,
    /// Keep watching the live run.
    Adopt,
    /// Publish a successful outcome to the UI.
    PublishDone,
    /// Publish a failure with this message.
    PublishFailed(String),
    /// Mark the run interrupted with this message.
    MarkInterrupted(String),
    /// Boot out the job registration and remove its plist and old logs.
    CleanupFinished,
    /// Resting (R7-4): a `waiting` run with a valid built pending and no process.
    /// Nothing is interrupted, cleaned up or published; the watcher stops.
    Rest,
}

/// Pure decision table (R3-1). Order matters: a live job always wins.
pub fn reconcile(input: &ReconcileInput<'_>) -> Decision {
    if input.job.is_live() {
        return Decision::Adopt;
    }
    if input.job.is_unknown() {
        // Maybe alive: never interrupt, never clean up. A finished outcome may still be shown.
        return match &input.status {
            Some(status) if status.phase.is_terminal() && !input.published => {
                match terminal_result(status, input.running_build_sha) {
                    Ok(()) => Decision::PublishDone,
                    Err(message) => Decision::PublishFailed(message),
                }
            }
            _ => Decision::None,
        };
    }
    let lock_held = input.lock == LockState::HeldAlive;
    match &input.status {
        Some(status) if status.phase.is_terminal() => {
            if !input.published {
                return match terminal_result(status, input.running_build_sha) {
                    Ok(()) => Decision::PublishDone,
                    Err(message) => Decision::PublishFailed(message),
                };
            }
            if lock_held {
                Decision::None
            } else {
                Decision::CleanupFinished
            }
        }
        Some(status) if status.phase == RunPhase::Waiting && input.pending_built => Decision::Rest,
        Some(_) => {
            if input.status_age_secs < input.interrupted_after_secs {
                Decision::None
            } else {
                Decision::MarkInterrupted(interrupted_message(input.job))
            }
        }
        None => match input.job {
            // The job exited without writing a status for this run: say so once,
            // then clean up. A known exit code is final, so no waiting; otherwise
            // the job must have been silent for the whole threshold.
            JobProbe::Registered {
                pid: None,
                last_exit,
            } if !lock_held
                && (last_exit.is_some()
                    || input.status_age_secs >= input.interrupted_after_secs) =>
            {
                if input.published {
                    Decision::CleanupFinished
                } else {
                    Decision::PublishFailed(stopped_message(input.job))
                }
            }
            _ => Decision::None,
        },
    }
}

/// True when the running build is the status target, or when the running build
/// is unknown (empty). A `done` whose full target differs is a mismatch.
pub fn running_matches(status: &RunStatus, running_build_sha: &str) -> bool {
    running_build_sha.is_empty() || status.target_full == running_build_sha
}

/// The UI outcome of a terminal status: `Ok(())` for success, `Err(message)`
/// for a failure. `done` for a build other than the target is a failure.
pub fn terminal_result(status: &RunStatus, running_build_sha: &str) -> Result<(), String> {
    match status.phase {
        RunPhase::Done if running_matches(status, running_build_sha) => Ok(()),
        RunPhase::Done => Err("running version does not match the update target".to_string()),
        _ => Err(failure_message(&status.message)),
    }
}

fn failure_message(message: &str) -> String {
    if message.is_empty() {
        return "update failed".to_string();
    }
    message.chars().take(MAX_FAILURE_MESSAGE_CHARS).collect()
}

/// Failure text for a job that stopped without writing a status.
fn stopped_message(job: JobProbe) -> String {
    match job {
        JobProbe::Registered {
            last_exit: Some(code),
            ..
        } => format!("The update stopped unexpectedly (exit {code})."),
        _ => "The update stopped unexpectedly.".to_string(),
    }
}

fn interrupted_message(job: JobProbe) -> String {
    match job {
        JobProbe::Registered {
            last_exit: Some(code),
            ..
        } => format!("update interrupted (last exit code {code})"),
        _ => "update interrupted".to_string(),
    }
}

#[cfg(test)]
#[path = "source_update_waiting_tests.rs"]
mod waiting_tests;

#[cfg(test)]
#[path = "source_update_runner_tests.rs"]
mod tests;
