//! Opacity of the see-through conversation window (Track T, DESIGN Revision 3).
//!
//! Pure rules plus the process-wide snapshot holder and the replay worker lifecycle. The worker
//! takes its clock, liveness check and emitter as closures, so tests drive a fake clock. Nothing
//! here touches the file system or writes to a log.

use crate::chat_presentation::{LinuxBackend, Os, PlatformEnv};
use serde::{Deserialize, Deserializer, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// Lowest opacity of the conversation window, in percent.
pub const CHAT_OPACITY_MIN: u8 = 20;
/// Highest opacity (fully opaque), in percent.
pub const CHAT_OPACITY_MAX: u8 = 100;
/// Opacity used when nothing is stored or the stored value is not a number.
pub const CHAT_OPACITY_DEFAULT: u8 = 70;
/// Tauri event that carries `{ rev, pct }` to the conversation page.
pub const CHAT_OPACITY_EVENT: &str = "chat-opacity";
/// The only webview label that receives the opacity event.
pub const CHAT_OPACITY_TARGET: &str = crate::chat_window::CHAT_WINDOW_LABEL;

/// Clamps any integer into the opacity range (20..=100).
pub fn clamp_chat_opacity_pct(value: i64) -> u8 {
    let clamped = value.clamp(i64::from(CHAT_OPACITY_MIN), i64::from(CHAT_OPACITY_MAX));
    u8::try_from(clamped).unwrap_or(CHAT_OPACITY_DEFAULT)
}

/// The serde default of the setting.
pub fn default_chat_opacity_pct() -> u8 {
    CHAT_OPACITY_DEFAULT
}

/// Field-local deserializer for `chat_opacity_pct`. Any integral JSON number is clamped through
/// a wide intermediate (i64) to 20..=100. Null, strings, fractional numbers and anything else
/// read as the default. Other fields of the settings object are not touched.
pub fn deserialize_chat_opacity<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = serde_json::Value::deserialize(deserializer)?;
    Ok(match raw {
        serde_json::Value::Number(number) => match number.as_i64() {
            Some(value) => clamp_chat_opacity_pct(value),
            // An integer above i64::MAX is still an integer: it is above the range.
            None if number.is_u64() => CHAT_OPACITY_MAX,
            None => CHAT_OPACITY_DEFAULT,
        },
        _ => CHAT_OPACITY_DEFAULT,
    })
}

/// Whether the platform cannot composite the window's transparency: Linux without a compositing
/// window manager, or Wayland (transparent windows are blocked there).
fn compositing_unavailable(env: &PlatformEnv) -> bool {
    env.os == Os::Linux && (env.linux_backend == LinuxBackend::Wayland || !env.composited)
}

/// The opacity the window is really drawn with. Without compositing it is forced to 100 (opaque),
/// whatever the slider says. Otherwise it is the clamped setting.
pub fn effective_chat_opacity_pct(pct: u8, env: &PlatformEnv) -> u8 {
    if compositing_unavailable(env) {
        return CHAT_OPACITY_MAX;
    }
    clamp_chat_opacity_pct(i64::from(pct))
}

/// How often the replay worker re-emits the current snapshot to the loaded page.
pub const fn replay_interval() -> Duration {
    Duration::from_secs(2)
}

/// A committed opacity: `rev` grows on every commit, `pct` is the clamped setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OpacitySnapshot {
    /// Monotonic revision. The page applies only strictly higher revisions.
    pub rev: u64,
    /// The opacity in percent (20..=100).
    pub pct: u8,
}

/// Process-wide holder of the latest committed snapshot.
pub struct OpacityHolder {
    inner: Mutex<OpacitySnapshot>,
}

impl OpacityHolder {
    /// An empty holder: revision 0 until it is seeded or committed.
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(OpacitySnapshot {
                rev: 0,
                pct: CHAT_OPACITY_DEFAULT,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, OpacitySnapshot> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sets the first value (revision 1) when nothing has been committed yet. Returns the current
    /// snapshot either way.
    pub fn seed(&self, pct: i64) -> OpacitySnapshot {
        let mut current = self.lock();
        if current.rev == 0 {
            current.rev = 1;
            current.pct = clamp_chat_opacity_pct(pct);
        }
        *current
    }

    /// Records a new value with the next revision and returns it.
    pub fn commit(&self, pct: i64) -> OpacitySnapshot {
        let mut current = self.lock();
        current.rev = current.rev.saturating_add(1);
        current.pct = clamp_chat_opacity_pct(pct);
        *current
    }

    /// The latest committed snapshot.
    pub fn snapshot(&self) -> OpacitySnapshot {
        *self.lock()
    }
}

impl Default for OpacityHolder {
    fn default() -> Self {
        Self::new()
    }
}

/// The one opacity holder of the app.
pub static CHAT_OPACITY: OpacityHolder = OpacityHolder::new();

/// Single-instance guard of the replay worker: at most one worker runs at a time.
pub struct ReplayGate {
    running: AtomicBool,
}

impl ReplayGate {
    /// A gate with no worker running.
    pub const fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
        }
    }

    /// Claims the gate. Returns false when a worker already runs.
    pub fn try_start(&self) -> bool {
        self.running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Releases the gate when the worker has ended (its window was destroyed).
    pub fn finish(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

impl Default for ReplayGate {
    fn default() -> Self {
        Self::new()
    }
}

/// The worker gate of the conversation window.
pub static CHAT_OPACITY_REPLAY: ReplayGate = ReplayGate::new();

/// The replay loop of the opacity alone. The production worker is `chat_window::start_replay_worker`,
/// which runs `appearance::run_consumer_replay`; this stays as the tested opacity-only form.
#[cfg_attr(not(test), allow(dead_code))]
pub fn run_replay(
    mut snapshot: impl FnMut() -> OpacitySnapshot,
    mut alive: impl FnMut() -> bool,
    mut sleep: impl FnMut(Duration),
    mut emit: impl FnMut(OpacitySnapshot),
) -> usize {
    let mut emits = 0;
    while alive() {
        emit(snapshot());
        emits += 1;
        sleep(replay_interval());
    }
    emits
}

/// What to emit for one snapshot: `None` when the window is absent (not an error), otherwise the
/// payload with the effective opacity. The target is always the chat label.
pub fn plan_emit(
    window_exists: bool,
    snapshot: OpacitySnapshot,
    env: &PlatformEnv,
) -> Option<(&'static str, OpacitySnapshot)> {
    if !window_exists {
        return None;
    }
    Some((
        CHAT_OPACITY_TARGET,
        OpacitySnapshot {
            rev: snapshot.rev,
            pct: effective_chat_opacity_pct(snapshot.pct, env),
        },
    ))
}

/// The snapshot the conversation page is built with: the revision, and the opacity the window is
/// really drawn with (forced to 100 where compositing is unavailable). The page applies only
/// strictly higher revisions and never corrects itself from a replay of the same revision, so the
/// URL value must already be the effective one.
pub fn page_snapshot(snapshot: OpacitySnapshot, env: &PlatformEnv) -> OpacitySnapshot {
    OpacitySnapshot {
        rev: snapshot.rev,
        pct: effective_chat_opacity_pct(snapshot.pct, env),
    }
}

/// Runs one replay worker and decides whether it goes on. `run_until_gone` replays until the window
/// is destroyed. The gate is then released, and it is taken again only if a window exists, so a
/// window rebuilt in the gap gets exactly one worker (the rebuild's claim or this loop's, never
/// both). Returns the number of replay runs.
pub fn replay_lifecycle(
    gate: &ReplayGate,
    mut window_alive: impl FnMut() -> bool,
    mut run_until_gone: impl FnMut(),
) -> usize {
    let mut runs = 0;
    loop {
        run_until_gone();
        runs += 1;
        gate.finish();
        if !window_alive() || !gate.try_start() {
            return runs;
        }
    }
}

#[cfg(test)]
#[path = "chat_opacity_tests.rs"]
mod chat_opacity_tests;
