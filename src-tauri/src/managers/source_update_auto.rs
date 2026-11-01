//! Automatic-update policy (R6-5, R6-6, R7-6): the pure start decision, the
//! persisted attempt state with its retry rule, and the update coordinator.
//!
//! Nothing here starts a run. The decision is a pure function of facts the
//! caller gathers; the state is a small strict file; the coordinator is the one
//! lock the commit sequence and consent changes take.

#![cfg_attr(not(test), allow(dead_code))]

use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::{json, Map, Value};

use super::source_update_runner::{is_lower_hex, write_private_atomic};

/// Quiet time required before an automatic start (R6-5).
pub const AUTO_IDLE_SECS: u64 = 60;
/// A failed or cancelled target is not retried for this long (R6-6).
pub const AUTO_RETRY_COOLDOWN_SECS: u64 = 24 * 60 * 60;
/// File name of the persisted attempt state inside the state directory.
pub const AUTO_STATE_FILE_NAME: &str = "auto_update_state.json";
/// Largest attempt-state file accepted, in bytes.
pub const AUTO_STATE_MAX_BYTES: usize = 4096;
/// The only attempt-state schema version this build reads and writes.
pub const AUTO_STATE_VERSION: u64 = 1;

/// Idle requirement as a `Duration`.
pub const AUTO_IDLE: Duration = Duration::from_secs(AUTO_IDLE_SECS);

/// Result of the verified-commit gate for the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifiedState {
    /// Not checked yet.
    Unknown,
    /// GitHub reports the commit as verified.
    Verified,
    /// GitHub reports the commit as not verified.
    Unverified,
    /// The check could not be completed (fails closed).
    Failed,
}

/// Everything the start decision reads. Gathered by the caller, never by this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoFacts {
    /// The build is a source build.
    pub source_mode: bool,
    /// Update checks are effectively enabled (not forced off).
    pub checks_effective: bool,
    /// The owner switched automatic updates on.
    pub auto_enabled: bool,
    /// An update is available for the current build.
    pub state_available: bool,
    /// No install blocker is present.
    pub install_blocker_none: bool,
    /// An update run is already active.
    pub run_active: bool,
    /// The retry rule (see [`should_retry`]) allows this target.
    pub backoff_clear: bool,
    /// Verified-commit gate result for the target.
    pub verified: VerifiedState,
    /// The admission gate is free right now.
    pub gate_free: bool,
    /// No agent job runs and no permission prompt is pending.
    pub jobs_idle: bool,
    /// Time since the last activity of any kind.
    pub idle_for: Duration,
}

/// Why a start waits and may succeed later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitReason {
    /// An update run is already active.
    ActiveRun,
    /// The admission gate is busy with user work.
    Busy,
    /// Not quiet long enough, or agent jobs are running.
    NotIdle,
    /// The verified-commit check has not run yet.
    Verifying,
}

impl WaitReason {
    /// Stable code for i18n keys.
    pub fn code(self) -> &'static str {
        match self {
            WaitReason::ActiveRun => "active_run",
            WaitReason::Busy => "busy",
            WaitReason::NotIdle => "not_idle",
            WaitReason::Verifying => "verifying",
        }
    }
}

/// Why a start is refused for this target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// Checks are off or the owner switched automatic updates off.
    Disabled,
    /// The build is not a source build.
    NotSource,
    /// No update is available.
    NoUpdate,
    /// An install blocker is present.
    Blocked,
    /// The retry rule refuses this target for now.
    Backoff,
    /// GitHub does not show the commit as verified.
    Unverified,
    /// The verified-commit check could not be completed.
    VerifyFailed,
}

impl SkipReason {
    /// Stable code for i18n keys.
    pub fn code(self) -> &'static str {
        match self {
            SkipReason::Disabled => "disabled",
            SkipReason::NotSource => "not_source",
            SkipReason::NoUpdate => "no_update",
            SkipReason::Blocked => "blocked",
            SkipReason::Backoff => "backoff",
            SkipReason::Unverified => "unverified",
            SkipReason::VerifyFailed => "verify_failed",
        }
    }
}

/// The automatic policy's answer for one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoDecision {
    /// Start the automatic run now.
    Start,
    /// Not yet, try again later.
    Wait(WaitReason),
    /// Do not start for this target.
    Skip(SkipReason),
}

impl AutoDecision {
    /// Stable code for logs and the Settings status line.
    pub fn code(self) -> &'static str {
        match self {
            AutoDecision::Start => "start",
            AutoDecision::Wait(reason) => reason.code(),
            AutoDecision::Skip(reason) => reason.code(),
        }
    }
}

/// Pure start decision (R6-5). The first failing rule decides, in this order:
/// consent and source mode, availability, blockers, active run, retry rule,
/// verification, admission gate, idleness.
pub fn auto_update_decision(facts: &AutoFacts) -> AutoDecision {
    if !facts.checks_effective || !facts.auto_enabled {
        return AutoDecision::Skip(SkipReason::Disabled);
    }
    if !facts.source_mode {
        return AutoDecision::Skip(SkipReason::NotSource);
    }
    if !facts.state_available {
        return AutoDecision::Skip(SkipReason::NoUpdate);
    }
    if !facts.install_blocker_none {
        return AutoDecision::Skip(SkipReason::Blocked);
    }
    if facts.run_active {
        return AutoDecision::Wait(WaitReason::ActiveRun);
    }
    if !facts.backoff_clear {
        return AutoDecision::Skip(SkipReason::Backoff);
    }
    match facts.verified {
        VerifiedState::Unknown => return AutoDecision::Wait(WaitReason::Verifying),
        VerifiedState::Unverified => return AutoDecision::Skip(SkipReason::Unverified),
        VerifiedState::Failed => return AutoDecision::Skip(SkipReason::VerifyFailed),
        VerifiedState::Verified => {}
    }
    if !facts.gate_free {
        return AutoDecision::Wait(WaitReason::Busy);
    }
    if !facts.jobs_idle || facts.idle_for < AUTO_IDLE {
        return AutoDecision::Wait(WaitReason::NotIdle);
    }
    AutoDecision::Start
}

/// Outcome recorded for the last automatic attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoResult {
    /// The update installed.
    Updated,
    /// The attempt failed or was interrupted.
    Failed,
    /// The attempt was refused by the policy.
    Skipped,
}

impl AutoResult {
    /// Wire name used in the file.
    pub fn as_str(self) -> &'static str {
        match self {
            AutoResult::Updated => "updated",
            AutoResult::Failed => "failed",
            AutoResult::Skipped => "skipped",
        }
    }
}

/// Persisted memory of automatic attempts (R6-6). Every field is always present
/// on disk; absent values are `null`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AutoState {
    /// Full target of the last attempt.
    pub last_target: Option<String>,
    /// Outcome of the last attempt.
    pub last_result: Option<AutoResult>,
    /// Stable reason code of the last outcome (`[a-z_]{1,32}`).
    pub reason_code: Option<String>,
    /// Unix seconds of the last outcome.
    pub ts: u64,
    /// Full target of the last failed or cancelled attempt.
    pub failed_target: Option<String>,
    /// Unix seconds of that failure.
    pub failed_ts: Option<u64>,
}

impl AutoState {
    /// The state after a failed or cancelled attempt on `target`.
    pub fn with_failure(&self, target: &str, reason_code: &str, now: u64) -> AutoState {
        AutoState {
            last_target: Some(target.to_string()),
            last_result: Some(AutoResult::Failed),
            reason_code: Some(reason_code.to_string()),
            ts: now,
            failed_target: Some(target.to_string()),
            failed_ts: Some(now),
        }
    }

    /// The state after a successful update to `target`.
    pub fn with_updated(&self, target: &str, now: u64) -> AutoState {
        AutoState {
            last_target: Some(target.to_string()),
            last_result: Some(AutoResult::Updated),
            reason_code: None,
            ts: now,
            ..self.clone()
        }
    }

    /// The state after a refused attempt on `target`. Failure memory is kept.
    pub fn with_skipped(&self, target: &str, reason_code: &str, now: u64) -> AutoState {
        AutoState {
            last_target: Some(target.to_string()),
            last_result: Some(AutoResult::Skipped),
            reason_code: Some(reason_code.to_string()),
            ts: now,
            ..self.clone()
        }
    }
}

/// True when an automatic attempt on `target` may start now (R6-6).
///
/// A failed target is blocked for [`AUTO_RETRY_COOLDOWN_SECS`]. A different
/// target is always allowed. If the clock went backwards so that the failure
/// stamp lies in the future, the cooldown holds only for a small skew; a stamp
/// more than a cooldown ahead is treated as corrupt and does not block forever.
pub fn should_retry(state: &AutoState, target: &str, now: u64) -> bool {
    match (state.failed_target.as_deref(), state.failed_ts) {
        (Some(failed), Some(failed_ts)) if failed == target => cooldown_elapsed(failed_ts, now),
        _ => true,
    }
}

fn cooldown_elapsed(failed_ts: u64, now: u64) -> bool {
    if now >= failed_ts {
        now - failed_ts >= AUTO_RETRY_COOLDOWN_SECS
    } else {
        failed_ts - now > AUTO_RETRY_COOLDOWN_SECS
    }
}

/// Reasons an attempt-state file is rejected. Display never echoes the input.
#[derive(Debug)]
pub enum AutoStateError {
    /// Larger than `AUTO_STATE_MAX_BYTES`.
    TooLarge,
    /// Not valid UTF-8 or not parseable JSON.
    InvalidJson,
    /// Parseable JSON that is not an object.
    NotAnObject,
    /// `v` is not `AUTO_STATE_VERSION`.
    BadVersion,
    /// A key is missing, unexpected, or has the wrong value; the name is the key.
    BadField(&'static str),
    /// The path is a symlink or not a regular file.
    NotRegularFile,
    /// Filesystem failure.
    Io(io::Error),
}

impl fmt::Display for AutoStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AutoStateError::TooLarge => write!(f, "auto-update state is too large"),
            AutoStateError::InvalidJson => write!(f, "auto-update state is not valid JSON"),
            AutoStateError::NotAnObject => write!(f, "auto-update state is not an object"),
            AutoStateError::BadVersion => write!(f, "unsupported auto-update state version"),
            AutoStateError::BadField(name) => write!(f, "invalid auto-update field: {name}"),
            AutoStateError::NotRegularFile => write!(f, "auto-update state is not a regular file"),
            AutoStateError::Io(error) => write!(f, "auto-update state I/O error: {error}"),
        }
    }
}

impl std::error::Error for AutoStateError {}

const AUTO_STATE_KEYS: [&str; 7] = [
    "v",
    "last_target",
    "last_result",
    "reason_code",
    "ts",
    "failed_target",
    "failed_ts",
];

/// Strictly parses an attempt-state file: exact keys, typed values, size cap.
pub fn parse_auto_state(bytes: &[u8]) -> Result<AutoState, AutoStateError> {
    if bytes.len() > AUTO_STATE_MAX_BYTES {
        return Err(AutoStateError::TooLarge);
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| AutoStateError::InvalidJson)?;
    let object = value.as_object().ok_or(AutoStateError::NotAnObject)?;
    if object
        .keys()
        .any(|key| !AUTO_STATE_KEYS.contains(&key.as_str()))
    {
        return Err(AutoStateError::BadField("unexpected field"));
    }
    for key in AUTO_STATE_KEYS {
        if !object.contains_key(key) {
            return Err(AutoStateError::BadField(key));
        }
    }
    if uint(object, "v")? != AUTO_STATE_VERSION {
        return Err(AutoStateError::BadVersion);
    }
    let last_result = match optional_text(object, "last_result")? {
        None => None,
        Some("updated") => Some(AutoResult::Updated),
        Some("failed") => Some(AutoResult::Failed),
        Some("skipped") => Some(AutoResult::Skipped),
        Some(_) => return Err(AutoStateError::BadField("last_result")),
    };
    let failed_target = optional_sha(object, "failed_target")?;
    let failed_ts = optional_uint(object, "failed_ts")?;
    if failed_target.is_some() != failed_ts.is_some() {
        return Err(AutoStateError::BadField("failed_ts"));
    }
    Ok(AutoState {
        last_target: optional_sha(object, "last_target")?,
        last_result,
        reason_code: optional_code(object, "reason_code")?,
        ts: uint(object, "ts")?,
        failed_target,
        failed_ts,
    })
}

/// Serialises a state to the wire format and re-parses it, so an invalid
/// record can never reach disk.
pub fn encode_auto_state(state: &AutoState) -> Result<Vec<u8>, AutoStateError> {
    let mut map = Map::new();
    map.insert("v".into(), json!(AUTO_STATE_VERSION));
    map.insert("last_target".into(), json!(state.last_target));
    map.insert(
        "last_result".into(),
        json!(state.last_result.map(AutoResult::as_str)),
    );
    map.insert("reason_code".into(), json!(state.reason_code));
    map.insert("ts".into(), json!(state.ts));
    map.insert("failed_target".into(), json!(state.failed_target));
    map.insert("failed_ts".into(), json!(state.failed_ts));
    let bytes = serde_json::to_vec(&Value::Object(map)).map_err(|_| AutoStateError::InvalidJson)?;
    parse_auto_state(&bytes)?;
    Ok(bytes)
}

/// Reads `<state_dir>/auto_update_state.json`. `Ok(None)` when absent; symlinks
/// and non-regular files are refused.
pub fn load_auto_state(state_dir: &Path) -> Result<Option<AutoState>, AutoStateError> {
    let path = state_dir.join(AUTO_STATE_FILE_NAME);
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(AutoStateError::Io(error)),
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(AutoStateError::NotRegularFile);
    }
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .map_err(AutoStateError::Io)?
        .take(AUTO_STATE_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(AutoStateError::Io)?;
    parse_auto_state(&bytes).map(Some)
}

/// Writes the state atomically with mode 0600.
pub fn save_auto_state(state_dir: &Path, state: &AutoState) -> Result<(), AutoStateError> {
    let bytes = encode_auto_state(state)?;
    write_private_atomic(&state_dir.join(AUTO_STATE_FILE_NAME), &bytes).map_err(AutoStateError::Io)
}

fn uint(object: &Map<String, Value>, name: &'static str) -> Result<u64, AutoStateError> {
    object
        .get(name)
        .and_then(Value::as_u64)
        .ok_or(AutoStateError::BadField(name))
}

fn optional_uint(
    object: &Map<String, Value>,
    name: &'static str,
) -> Result<Option<u64>, AutoStateError> {
    match object.get(name) {
        Some(Value::Null) => Ok(None),
        _ => uint(object, name).map(Some),
    }
}

fn optional_text<'a>(
    object: &'a Map<String, Value>,
    name: &'static str,
) -> Result<Option<&'a str>, AutoStateError> {
    match object.get(name) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.as_str())),
        _ => Err(AutoStateError::BadField(name)),
    }
}

fn optional_sha(
    object: &Map<String, Value>,
    name: &'static str,
) -> Result<Option<String>, AutoStateError> {
    match optional_text(object, name)? {
        None => Ok(None),
        Some(text) if is_lower_hex(text, 40) => Ok(Some(text.to_string())),
        Some(_) => Err(AutoStateError::BadField(name)),
    }
}

fn optional_code(
    object: &Map<String, Value>,
    name: &'static str,
) -> Result<Option<String>, AutoStateError> {
    match optional_text(object, name)? {
        None => Ok(None),
        Some(text)
            if (1..=32).contains(&text.len())
                && text.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') =>
        {
            Ok(Some(text.to_string()))
        }
        Some(_) => Err(AutoStateError::BadField(name)),
    }
}

/// Serialises the consent settings and the commit sequence (R7-6).
///
/// The commit sequence and every change to `auto_update_enabled` or
/// `update_checks_enabled` take this guard. The guarded sections keep no state
/// of their own, so a poisoned lock is recovered and stays usable.
#[derive(Debug, Default)]
pub struct UpdateCoordinator {
    lock: Mutex<()>,
}

impl UpdateCoordinator {
    /// A coordinator with no holder.
    pub const fn new() -> Self {
        Self {
            lock: Mutex::new(()),
        }
    }

    /// Takes the coordinator, recovering from poisoning.
    pub fn guard(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// True while some caller holds the guard (test-only probe).
    #[cfg(test)]
    pub(super) fn is_held(&self) -> bool {
        matches!(
            self.lock.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        )
    }
}

#[cfg(test)]
#[path = "source_update_auto_tests.rs"]
mod tests;
