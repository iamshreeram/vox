//! Spoken replies via the macOS `say` command run as a child process.
//!
//! Why a child process: an `NSSpeechSynthesizer` design was reviewed and
//! rejected (cross-thread Cocoa calls, unknown run-loop requirement). A `say`
//! child needs no Objective-C at all and cancelling is just killing a process.
//!
//! Contracts (from the design review):
//! - Speech text goes to the child over STDIN (`say -f -`), never argv, so it is
//!   not visible in process listings. The only argv values are a validated
//!   voice name and a clamped rate.
//! - Embedded speech-markup (`[[ ... ]]`) is neutralized by turning `[`/`]`
//!   into spaces; control characters never reach the child.
//! - ONE manager mutex serializes speak/stop/recording transitions, so the
//!   "is a recording active?" check and the spawn can never be separated by a
//!   recording start. The mutex is held only across a bounded kill (<= 200 ms)
//!   plus spawn, never an unbounded wait.
//! - `note_recording_started()` stops speech SYNCHRONOUSLY (bounded) before the
//!   mic is considered live; there is no delayed/async stop that could later
//!   kill unrelated speech. If the kill cannot be confirmed within the bound,
//!   recording proceeds anyway and TTS bleed into the mic is possible -- this is
//!   an accepted, documented limitation, and `speak` keeps refusing while the
//!   recording flag is set.
//! - If cancelling the previous speech fails, `speak` REFUSES to start new
//!   speech (`CancelFailed`) rather than overlap voices.
//! - Every spawned child has a detached reaper thread, so a naturally finished
//!   child is reaped even if nobody polls it.
//! - Speech content is never logged.

#![allow(dead_code)]

use crate::managers::emotion::{effective_rate, pitch_prefix, Emotion};
use std::io::{self, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::watch;

pub const MAX_SPEECH_CHARS: usize = 2000;
pub const MIN_RATE_WPM: u32 = 80;
pub const MAX_RATE_WPM: u32 = 400;
pub const DEFAULT_RATE_WPM: u32 = 180;
/// Upper bound on how long a kill is awaited.
pub const KILL_CONFIRM_BOUND: Duration = Duration::from_millis(200);

/// Fixed phrases (event payloads of command outcomes are never spoken).
pub const DONE_PHRASE: &str = "Done.";
pub const ERROR_PHRASE: &str = "Sorry, that didn't work.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TtsError {
    /// Nothing speakable after sanitizing.
    Empty,
    /// A recording is active; automatic speech is suppressed.
    Recording,
    /// The previous speech could not be confirmed stopped; nothing was spawned.
    CancelFailed,
    Backend(String),
    Unsupported,
    /// A newer speech intent (or a stop/recording/config change) replaced this one.
    Superseded,
}

pub trait SpeechHandle: Send {
    /// Kill the speech process and confirm it stopped within
    /// [`KILL_CONFIRM_BOUND`]. `true` only if termination is confirmed (or the
    /// process had already exited).
    fn kill(&mut self) -> bool;
    fn is_running(&mut self) -> bool;
    /// True for a handle onto a resident, queue-capable speaker (see `tts_queue`): further
    /// sentences can be appended without replacing it. Default: false.
    fn queue_capable(&self) -> bool {
        false
    }
}

pub trait SpeechBackend: Send + Sync {
    fn spawn(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
    ) -> io::Result<Box<dyn SpeechHandle>>;

    /// Like [`SpeechBackend::spawn`] with an emotion hint. The default ignores
    /// the emotion so existing backends keep working unchanged.
    fn spawn_styled(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        _emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.spawn(text, voice, rate_wpm)
    }

    /// True when `enqueue_styled` appends to a resident queue instead of replacing speech.
    fn supports_queue(&self) -> bool {
        false
    }

    /// Appends one utterance to the backend's resident queue (no replacement). The returned
    /// handle refers to the shared queue. Default: unsupported.
    fn enqueue_styled(
        &self,
        _text: &str,
        _voice: Option<&str>,
        _rate_wpm: Option<u32>,
        _emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

/// Removes control characters (newlines/tabs become a single space so word
/// boundaries survive), replaces `[` and `]` with spaces, collapses
/// whitespace, trims, and truncates to [`MAX_SPEECH_CHARS`] characters on a
/// char boundary. `None` if nothing speakable remains.
pub fn sanitize_speech_text(text: &str) -> Option<String> {
    let mapped: String = text
        .chars()
        .map(|c| match c {
            '[' | ']' => ' ',
            '\n' | '\r' | '\t' => ' ',
            c if c.is_control() => '\0',
            c => c,
        })
        .filter(|c| *c != '\0')
        .collect();
    let collapsed = mapped.split_whitespace().collect::<Vec<_>>().join(" ");
    let truncated: String = collapsed.chars().take(MAX_SPEECH_CHARS).collect();
    let trimmed = truncated.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Accepts only `[A-Za-z0-9 ._()-]{1,64}` (after trimming); anything else is
/// dropped (`None`) so it can never reach argv.
pub fn sanitize_voice(voice: &str) -> Option<String> {
    let trimmed = voice.trim();
    let len = trimmed.chars().count();
    if !(1..=64).contains(&len) {
        return None;
    }
    let allowed = trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '(' | ')' | '-'));
    // A leading '-' would be parsed by `say` as an option, so it is refused.
    if allowed && !trimmed.starts_with('-') {
        Some(trimmed.to_string())
    } else {
        None
    }
}

pub fn clamp_rate(rate: u32) -> u32 {
    rate.clamp(MIN_RATE_WPM, MAX_RATE_WPM)
}

/// argv for `/usr/bin/say` (WITHOUT the program name and WITHOUT the text):
/// `[-v <voice>] [-r <rate>] -f -`. Voice is sanitized and rate clamped here.
pub fn say_args(voice: Option<&str>, rate_wpm: Option<u32>) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if let Some(v) = voice.and_then(sanitize_voice) {
        args.push("-v".to_string());
        args.push(v);
    }
    if let Some(r) = rate_wpm {
        args.push("-r".to_string());
        args.push(clamp_rate(r).to_string());
    }
    args.push("-f".to_string());
    args.push("-".to_string());
    args
}

/// Which fixed phrase (if any) an app event should speak. Command-outcome
/// payloads are never spoken; only the agent reply text is.
pub fn phrase_for_event(event_name: &str, payload: &str) -> Option<String> {
    match event_name {
        "agent-bridge-reply" => {
            if payload.trim().is_empty() {
                None
            } else {
                Some(payload.to_string())
            }
        }
        "agent-bridge-error" | "voice-command-error" => Some(ERROR_PHRASE.to_string()),
        "voice-command-executed" => Some(DONE_PHRASE.to_string()),
        _ => None,
    }
}

pub struct TtsManager {
    inner: Arc<Mutex<TtsState>>,
    backend: Box<dyn SpeechBackend>,
    epoch_tx: watch::Sender<u64>,
}

struct TtsState {
    current: Option<Box<dyn SpeechHandle>>,
    recording_active: bool,
    /// Bumped (under the state mutex) by every event that supersedes older
    /// pending speech intents. See `begin_utterance`.
    epoch: u64,
}

impl TtsManager {
    pub fn new(backend: Box<dyn SpeechBackend>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(TtsState {
                current: None,
                recording_active: false,
                epoch: 0,
            })),
            backend,
            epoch_tx: watch::channel(0u64).0,
        }
    }

    /// Bump the epoch while the state lock is held and publish it to watchers.
    /// Every epoch mutation goes through here. Returns the new epoch.
    fn bump_locked(&self, st: &mut TtsState) -> u64 {
        st.epoch = st.epoch.wrapping_add(1);
        self.epoch_tx.send_replace(st.epoch);
        st.epoch
    }

    /// Reserve a new epoch for a speech intent. `Err(Recording)` while a
    /// recording is active (nothing is consumed). Bumps under the state lock.
    pub fn begin_utterance(&self) -> Result<u64, TtsError> {
        let mut state = lock_state(&self.inner);
        if state.recording_active {
            return Err(TtsError::Recording);
        }
        Ok(self.bump_locked(&mut state))
    }

    /// Bump the epoch (settings change / disable) WITHOUT killing speech.
    pub fn invalidate(&self) {
        let mut state = lock_state(&self.inner);
        self.bump_locked(&mut state);
    }

    pub fn is_current(&self, epoch: u64) -> bool {
        let state = lock_state(&self.inner);
        state.epoch == epoch
    }

    pub fn epoch_receiver(&self) -> watch::Receiver<u64> {
        self.epoch_tx.subscribe()
    }

    /// Cancel the previous speech (refuse with `CancelFailed` if unconfirmed),
    /// then spawn with the emotion hint and store the handle. Caller holds the
    /// state lock, so this is one critical section with the caller's checks.
    fn replace_locked(
        &self,
        state: &mut TtsState,
        text: &str,
        voice: Option<&str>,
        rate: Option<u32>,
        emotion: Emotion,
    ) -> Result<(), TtsError> {
        let cancelled = match state.current.as_mut() {
            Some(handle) => handle.kill(),
            None => true,
        };
        if !cancelled {
            return Err(TtsError::CancelFailed);
        }
        state.current = None;
        match self.backend.spawn_styled(text, voice, rate, emotion) {
            Ok(handle) => {
                state.current = Some(handle);
                Ok(())
            }
            Err(err) => Err(TtsError::Backend(err.to_string())),
        }
    }

    /// Stops speech ONLY if `epoch` is still current: same effect and return value as
    /// [`TtsManager::stop`] (bumps the epoch, kills, `false` only when the kill could not be
    /// confirmed). A stale epoch changes nothing and returns `true` (someone else owns the
    /// speaker now).
    pub fn stop_if_current(&self, epoch: u64) -> bool {
        let mut state = lock_state(&self.inner);
        if state.epoch != epoch {
            return true;
        }
        self.bump_locked(&mut state);
        let ok = match state.current.as_mut() {
            None => return true,
            Some(handle) => handle.kill(),
        };
        if ok {
            state.current = None;
        }
        ok
    }

    /// True when the backend can queue sentences (resident speaker).
    pub fn queue_capable(&self) -> bool {
        self.backend.supports_queue()
    }

    /// Appends `text` to the resident speaker's queue ONLY if `epoch` is still current,
    /// atomically (same critical section). Order of checks: sanitize (`Empty`), epoch
    /// (`Superseded`), recording (`Recording`), backend support (`Unsupported`). Never kills
    /// earlier queued speech; keeps the existing live queue handle as `current` (a new
    /// handle is stored only when `current` is absent or no longer running; a non-queue
    /// `current` is replaced exactly like `speak_styled_if_current`). A backend refusal
    /// because a previous stop failed maps to `CancelFailed`; other backend errors to
    /// `Backend`. Consumes no epoch.
    pub fn speak_queued_if_current(
        &self,
        epoch: u64,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
    ) -> Result<(), TtsError> {
        let text = sanitize_speech_text(text).ok_or(TtsError::Empty)?;
        let voice = voice.and_then(sanitize_voice);
        let rate = rate_wpm.map(clamp_rate);
        let mut state = lock_state(&self.inner);
        if state.epoch != epoch {
            return Err(TtsError::Superseded);
        }
        if state.recording_active {
            return Err(TtsError::Recording);
        }
        if !self.backend.supports_queue() {
            return Err(TtsError::Unsupported);
        }
        let live_queue = match state.current.as_mut() {
            Some(handle) => handle.queue_capable() && handle.is_running(),
            None => false,
        };
        if live_queue {
            // Append to the live queue; the existing handle stays `current`.
            return match self
                .backend
                .enqueue_styled(&text, voice.as_deref(), rate, emotion)
            {
                Ok(_handle) => Ok(()),
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => Err(TtsError::CancelFailed),
                Err(err) => Err(TtsError::Backend(err.to_string())),
            };
        }
        // No live queue: replace any non-queue or finished handle exactly like a normal speak.
        let cancelled = match state.current.as_mut() {
            Some(handle) => handle.kill(),
            None => true,
        };
        if !cancelled {
            return Err(TtsError::CancelFailed);
        }
        state.current = None;
        match self
            .backend
            .enqueue_styled(&text, voice.as_deref(), rate, emotion)
        {
            Ok(handle) => {
                state.current = Some(handle);
                Ok(())
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => Err(TtsError::CancelFailed),
            Err(err) => Err(TtsError::Backend(err.to_string())),
        }
    }

    /// Legacy path: `speak` is `speak_styled(.., Emotion::Neutral)`. Bumps the
    /// epoch ONLY when it will attempt a spawn (after the Empty and Recording
    /// checks pass), so the latest speech intent wins.
    pub fn speak_styled(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
    ) -> Result<(), TtsError> {
        let text = sanitize_speech_text(text).ok_or(TtsError::Empty)?;
        let voice = voice.and_then(sanitize_voice);
        let rate = rate_wpm.map(clamp_rate);
        let mut state = lock_state(&self.inner);
        if state.recording_active {
            return Err(TtsError::Recording);
        }
        self.bump_locked(&mut state);
        self.replace_locked(&mut state, &text, voice.as_deref(), rate, emotion)
    }

    /// Speak only if `epoch` is still current, atomically with the spawn
    /// (same critical section). Refused with `Superseded` WITHOUT touching the
    /// backend otherwise. Consumes no epoch.
    pub fn speak_styled_if_current(
        &self,
        epoch: u64,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
    ) -> Result<(), TtsError> {
        let text = sanitize_speech_text(text).ok_or(TtsError::Empty)?;
        let voice = voice.and_then(sanitize_voice);
        let rate = rate_wpm.map(clamp_rate);
        let mut state = lock_state(&self.inner);
        if state.epoch != epoch {
            return Err(TtsError::Superseded);
        }
        if state.recording_active {
            return Err(TtsError::Recording);
        }
        self.replace_locked(&mut state, &text, voice.as_deref(), rate, emotion)
    }

    /// Legacy path: `speak` is `speak_styled(.., Emotion::Neutral)`.
    pub fn speak(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
    ) -> Result<(), TtsError> {
        self.speak_styled(text, voice, rate_wpm, Emotion::Neutral)
    }

    /// Kill current speech. `true` if nothing was speaking or the kill was
    /// confirmed; `false` if it could not be confirmed (handle is retained).
    /// Always bumps the epoch first, even when nothing is playing.
    pub fn stop(&self) -> bool {
        let mut state = lock_state(&self.inner);
        self.bump_locked(&mut state);
        let ok = match state.current.as_mut() {
            None => return true,
            Some(handle) => handle.kill(),
        };
        if ok {
            state.current = None;
        }
        ok
    }

    pub fn is_speaking(&self) -> bool {
        let mut state = lock_state(&self.inner);
        let running = match state.current.as_mut() {
            Some(handle) => handle.is_running(),
            None => false,
        };
        if !running {
            state.current = None;
        }
        running
    }

    /// Recording is starting: under the lock, set the recording flag and
    /// synchronously stop current speech (bounded). No background work.
    pub fn note_recording_started(&self) {
        let mut state = lock_state(&self.inner);
        state.recording_active = true;
        self.bump_locked(&mut state);
        let ok = match state.current.as_mut() {
            None => true,
            Some(handle) => handle.kill(),
        };
        if ok {
            state.current = None;
        }
    }

    /// The mic stopped: clear the recording flag so speech may resume.
    pub fn note_recording_stopped(&self) {
        let mut state = lock_state(&self.inner);
        state.recording_active = false;
    }
}

/// Completes when the epoch is no longer `epoch` (or the sender is gone).
pub async fn wait_superseded(mut rx: watch::Receiver<u64>, epoch: u64) {
    loop {
        // Copy the value out first so the borrow is dropped before awaiting.
        let current = *rx.borrow();
        if current != epoch {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

/// stdin text for `say`: the pitch prefix (if any) followed by the ALREADY
/// SANITIZED text. The prefix is added last so text can never inject markup.
pub fn say_stdin_text(sanitized_text: &str, emotion: Emotion) -> String {
    match pitch_prefix(emotion) {
        Some(prefix) => format!("{prefix}{sanitized_text}"),
        None => sanitized_text.to_string(),
    }
}

/// argv for `say` with the emotion's rate applied via `effective_rate`.
pub fn say_args_styled(
    voice: Option<&str>,
    rate_wpm: Option<u32>,
    emotion: Emotion,
) -> Vec<String> {
    say_args(voice, effective_rate(rate_wpm, emotion))
}

/// A real child process wrapped as a [`SpeechHandle`], with a detached reaper
/// thread so natural completion is reaped without anyone polling.
pub struct ProcessHandle {
    child: Arc<Mutex<Option<Child>>>,
    pid: u32,
}

impl ProcessHandle {
    pub fn pid(&self) -> u32 {
        self.pid
    }
}

impl SpeechHandle for ProcessHandle {
    fn kill(&mut self) -> bool {
        let mut slot = lock_slot(&self.child);
        if let Some(child) = slot.as_mut() {
            // Ignore the error: the child may already have exited.
            let _ = child.kill();
        }
        let deadline = Instant::now() + KILL_CONFIRM_BOUND;
        loop {
            match try_reap(&mut *slot) {
                None | Some(false) => return true,
                Some(true) => {}
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn is_running(&mut self) -> bool {
        let mut slot = lock_slot(&self.child);
        matches!(try_reap(&mut *slot), Some(true))
    }
}

/// Spawns `command` with piped stdin, writes `stdin_text` then closes stdin
/// (write failures are tolerated: the child may already be gone), and starts
/// the reaper.
pub fn spawn_process_handle(command: &mut Command, stdin_text: &str) -> io::Result<ProcessHandle> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn()?;
    let pid = child.id();
    if let Some(mut stdin) = child.stdin.take() {
        let text = stdin_text.to_owned();
        thread::spawn(move || {
            // Errors (e.g. broken pipe) are expected if the child exited early.
            let _ = stdin.write_all(text.as_bytes());
            // `stdin` is dropped here, closing the pipe.
        });
    }
    let child = Arc::new(Mutex::new(Some(child)));
    spawn_reaper(child.clone());
    Ok(ProcessHandle { child, pid })
}

/// macOS: spawns `/usr/bin/say` via [`spawn_process_handle`].
pub struct SayBackend;

impl SpeechBackend for SayBackend {
    fn spawn(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        spawn_say(text, voice, rate_wpm, Emotion::Neutral)
    }

    fn spawn_styled(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        spawn_say(text, voice, rate_wpm, emotion)
    }
}

/// Non-macOS: speaking is unsupported.
pub struct NullBackend;

impl SpeechBackend for NullBackend {
    fn spawn(
        &self,
        _text: &str,
        _voice: Option<&str>,
        _rate_wpm: Option<u32>,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "speech is not supported on this platform",
        ))
    }
}

/// The speech backend appropriate for the current platform.
pub fn default_backend() -> Box<dyn SpeechBackend> {
    if cfg!(target_os = "macos") {
        Box::new(SayBackend)
    } else {
        Box::new(NullBackend)
    }
}

fn lock_state(inner: &Mutex<TtsState>) -> MutexGuard<'_, TtsState> {
    inner.lock().unwrap_or_else(|e| e.into_inner())
}

fn lock_slot(slot: &Mutex<Option<Child>>) -> MutexGuard<'_, Option<Child>> {
    slot.lock().unwrap_or_else(|e| e.into_inner())
}

/// Polls the child in `slot`. Returns `None` if there is no child, `Some(true)`
/// if it is still running, and `Some(false)` if it exited (or cannot be polled),
/// in which case the child is taken out of the slot (and thereby reaped/dropped).
fn try_reap(slot: &mut Option<Child>) -> Option<bool> {
    let status = match slot.as_mut() {
        None => return None,
        Some(child) => child.try_wait(),
    };
    match status {
        Ok(None) => Some(true),
        _ => {
            slot.take();
            Some(false)
        }
    }
}

/// Detached reaper: polls the child without holding the lock while sleeping,
/// so a naturally finished child is reaped without any external polling.
fn spawn_reaper(child: Arc<Mutex<Option<Child>>>) {
    thread::spawn(move || loop {
        let still_running = {
            let mut slot = lock_slot(&child);
            try_reap(&mut *slot)
        };
        if !matches!(still_running, Some(true)) {
            return;
        }
        thread::sleep(Duration::from_millis(75));
    });
}

#[cfg(target_os = "macos")]
fn spawn_say(
    text: &str,
    voice: Option<&str>,
    rate_wpm: Option<u32>,
    emotion: Emotion,
) -> io::Result<Box<dyn SpeechHandle>> {
    let mut cmd = Command::new("/usr/bin/say");
    cmd.args(say_args_styled(voice, rate_wpm, emotion));
    let handle = spawn_process_handle(&mut cmd, &say_stdin_text(text, emotion))?;
    Ok(Box::new(handle))
}

#[cfg(not(target_os = "macos"))]
fn spawn_say(
    _text: &str,
    _voice: Option<&str>,
    _rate_wpm: Option<u32>,
    _emotion: Emotion,
) -> io::Result<Box<dyn SpeechHandle>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "speech is not supported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::thread;
    use std::time::Instant;

    // ---------- pure helpers ----------

    #[test]
    fn sanitize_speech_text_basics() {
        assert_eq!(
            sanitize_speech_text("  hello   world  ").as_deref(),
            Some("hello world")
        );
        assert_eq!(
            sanitize_speech_text("line1\nline2\tx").as_deref(),
            Some("line1 line2 x")
        );
        assert_eq!(
            sanitize_speech_text("a\u{0}b\u{7}c").as_deref(),
            Some("abc")
        );
        assert_eq!(sanitize_speech_text(""), None);
        assert_eq!(sanitize_speech_text("   \n\t "), None);
        assert_eq!(sanitize_speech_text("\u{0}\u{1}\u{2}"), None);
    }

    #[test]
    fn sanitize_speech_text_neutralizes_embedded_speech_markup() {
        let out = sanitize_speech_text("[[volm 0.0]]hello [[rate 500]]").unwrap();
        assert!(!out.contains('['), "{out}");
        assert!(!out.contains(']'), "{out}");
        assert!(out.contains("hello"));
        assert_eq!(sanitize_speech_text("[[]]"), None);
    }

    #[test]
    fn sanitize_speech_text_truncates_on_a_char_boundary() {
        let long = "e\u{301}".repeat(3000); // multi-codepoint graphemes, 6000 chars
        let out = sanitize_speech_text(&long).unwrap();
        assert_eq!(out.chars().count(), MAX_SPEECH_CHARS);
        let multibyte = "\u{e9}".repeat(3000);
        assert_eq!(
            sanitize_speech_text(&multibyte).unwrap().chars().count(),
            2000
        );
        let exact = "a".repeat(2000);
        assert_eq!(sanitize_speech_text(&exact).unwrap(), exact);
    }

    #[test]
    fn sanitize_voice_is_a_strict_allowlist() {
        assert_eq!(sanitize_voice("Samantha").as_deref(), Some("Samantha"));
        assert_eq!(sanitize_voice("  Daniel  ").as_deref(), Some("Daniel"));
        assert_eq!(
            sanitize_voice("Eddy (English (US))").as_deref(),
            Some("Eddy (English (US))")
        );
        assert_eq!(
            sanitize_voice("en_US.voice-1").as_deref(),
            Some("en_US.voice-1")
        );
        for bad in [
            "",
            "   ",
            "-v evil",
            "a;b",
            "a|b",
            "a`b",
            "a$b",
            "../x",
            "a\nb",
            "a\u{0}b",
            "voz\u{e9}",
        ] {
            assert_eq!(sanitize_voice(bad), None, "{bad:?}");
        }
        assert!(sanitize_voice(&"a".repeat(64)).is_some());
        assert_eq!(sanitize_voice(&"a".repeat(65)), None);
    }

    #[test]
    fn rate_is_clamped() {
        assert_eq!(clamp_rate(0), 80);
        assert_eq!(clamp_rate(79), 80);
        assert_eq!(clamp_rate(80), 80);
        assert_eq!(clamp_rate(180), 180);
        assert_eq!(clamp_rate(400), 400);
        assert_eq!(clamp_rate(401), 400);
        assert_eq!(clamp_rate(u32::MAX), 400);
    }

    #[test]
    fn say_args_are_exact_and_never_contain_text() {
        assert_eq!(say_args(None, None), vec!["-f", "-"]);
        assert_eq!(
            say_args(Some("Samantha"), Some(200)),
            vec!["-v", "Samantha", "-r", "200", "-f", "-"]
        );
        assert_eq!(say_args(None, Some(10)), vec!["-r", "80", "-f", "-"]);
        assert_eq!(say_args(None, Some(99_999)), vec!["-r", "400", "-f", "-"]);
        assert_eq!(say_args(Some("bad;voice"), None), vec!["-f", "-"]);
        assert_eq!(
            say_args(Some("-v evil"), Some(150)),
            vec!["-r", "150", "-f", "-"]
        );
    }

    #[test]
    fn events_map_to_fixed_phrases_and_agent_reply_text() {
        assert_eq!(
            phrase_for_event("agent-bridge-reply", "It is sunny."),
            Some("It is sunny.".into())
        );
        assert_eq!(phrase_for_event("agent-bridge-reply", "   "), None);
        assert_eq!(
            phrase_for_event("agent-bridge-error", "stack trace with /secret/path"),
            Some(ERROR_PHRASE.into())
        );
        assert_eq!(
            phrase_for_event("voice-command-executed", "Opened https://secret.example/x"),
            Some(DONE_PHRASE.into())
        );
        assert_eq!(
            phrase_for_event("voice-command-error", "token abc failed"),
            Some(ERROR_PHRASE.into())
        );
        assert_eq!(
            phrase_for_event("memory-remembered", "my password is hunter2"),
            None
        );
        assert_eq!(phrase_for_event("theme-changed", ""), None);
        assert!(!DONE_PHRASE.contains('[') && !ERROR_PHRASE.contains('['));
    }

    // ---------- manager with a fake backend ----------

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Ev {
        Spawn {
            id: usize,
            text: String,
            voice: Option<String>,
            rate: Option<u32>,
        },
        Kill {
            id: usize,
            ok: bool,
        },
    }

    struct Shared {
        events: Mutex<Vec<Ev>>,
        next_id: AtomicUsize,
        alive: AtomicUsize,
        alive_at_spawn_max: AtomicUsize,
        kill_succeeds: AtomicBool,
        spawn_fails: AtomicBool,
        finished: Mutex<Vec<usize>>,
    }
    impl Shared {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                events: Mutex::new(vec![]),
                next_id: AtomicUsize::new(1),
                alive: AtomicUsize::new(0),
                alive_at_spawn_max: AtomicUsize::new(0),
                kill_succeeds: AtomicBool::new(true),
                spawn_fails: AtomicBool::new(false),
                finished: Mutex::new(vec![]),
            })
        }
        fn events(&self) -> Vec<Ev> {
            self.events.lock().unwrap().clone()
        }
        fn spawn_count(&self) -> usize {
            self.events()
                .iter()
                .filter(|e| matches!(e, Ev::Spawn { .. }))
                .count()
        }
    }

    struct FakeBackend(Arc<Shared>);
    struct FakeHandle {
        id: usize,
        shared: Arc<Shared>,
        alive: bool,
    }
    impl SpeechBackend for FakeBackend {
        fn spawn(
            &self,
            text: &str,
            voice: Option<&str>,
            rate: Option<u32>,
        ) -> io::Result<Box<dyn SpeechHandle>> {
            if self.0.spawn_fails.load(Ordering::SeqCst) {
                return Err(io::Error::new(io::ErrorKind::Other, "spawn failed"));
            }
            let alive_now = self.0.alive.load(Ordering::SeqCst);
            self.0
                .alive_at_spawn_max
                .fetch_max(alive_now, Ordering::SeqCst);
            let id = self.0.next_id.fetch_add(1, Ordering::SeqCst);
            self.0.alive.fetch_add(1, Ordering::SeqCst);
            self.0.events.lock().unwrap().push(Ev::Spawn {
                id,
                text: text.into(),
                voice: voice.map(str::to_string),
                rate,
            });
            Ok(Box::new(FakeHandle {
                id,
                shared: self.0.clone(),
                alive: true,
            }))
        }
    }
    impl SpeechHandle for FakeHandle {
        fn kill(&mut self) -> bool {
            if !self.alive {
                return true;
            }
            let ok = self.shared.kill_succeeds.load(Ordering::SeqCst);
            self.shared
                .events
                .lock()
                .unwrap()
                .push(Ev::Kill { id: self.id, ok });
            if ok {
                self.alive = false;
                self.shared.alive.fetch_sub(1, Ordering::SeqCst);
            }
            ok
        }
        fn is_running(&mut self) -> bool {
            if self.alive && self.shared.finished.lock().unwrap().contains(&self.id) {
                self.alive = false;
                self.shared.alive.fetch_sub(1, Ordering::SeqCst);
            }
            self.alive
        }
    }

    fn manager() -> (TtsManager, Arc<Shared>) {
        let shared = Shared::new();
        (
            TtsManager::new(Box::new(FakeBackend(shared.clone()))),
            shared,
        )
    }

    #[test]
    fn speak_spawns_once_with_sanitized_arguments() {
        let (tts, shared) = manager();
        tts.speak("  hello\nthere ", Some("Samantha"), Some(9999))
            .unwrap();
        assert_eq!(
            shared.events(),
            vec![Ev::Spawn {
                id: 1,
                text: "hello there".into(),
                voice: Some("Samantha".into()),
                rate: Some(400)
            }]
        );
        assert!(tts.is_speaking());
    }

    #[test]
    fn invalid_voice_is_dropped_before_the_backend() {
        let (tts, shared) = manager();
        tts.speak("hi", Some("bad;voice"), None).unwrap();
        match &shared.events()[0] {
            Ev::Spawn { voice, rate, .. } => {
                assert_eq!(*voice, None);
                assert_eq!(*rate, None);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn second_speak_kills_the_first_before_spawning() {
        let (tts, shared) = manager();
        tts.speak("one", None, None).unwrap();
        tts.speak("two", None, None).unwrap();
        let kinds: Vec<_> = shared
            .events()
            .into_iter()
            .map(|e| match e {
                Ev::Spawn { id, .. } => format!("spawn{id}"),
                Ev::Kill { id, ok } => format!("kill{id}:{ok}"),
            })
            .collect();
        assert_eq!(kinds, vec!["spawn1", "kill1:true", "spawn2"]);
        assert_eq!(
            shared.alive_at_spawn_max.load(Ordering::SeqCst),
            0,
            "never two voices at once"
        );
    }

    #[test]
    fn empty_text_is_rejected_without_touching_current_speech_or_backend() {
        let (tts, shared) = manager();
        tts.speak("first", None, None).unwrap();
        for empty in ["", "   ", "\u{0}\n", "[[ ]]"] {
            assert_eq!(
                tts.speak(empty, None, None),
                Err(TtsError::Empty),
                "{empty:?}"
            );
        }
        assert_eq!(shared.spawn_count(), 1);
        assert!(tts.is_speaking(), "current speech untouched");
    }

    #[test]
    fn stop_kills_and_is_idempotent() {
        let (tts, shared) = manager();
        assert!(tts.stop(), "nothing speaking is a successful stop");
        tts.speak("hi", None, None).unwrap();
        assert!(tts.stop());
        assert!(!tts.is_speaking());
        assert!(tts.stop());
        assert_eq!(
            shared
                .events()
                .iter()
                .filter(|e| matches!(e, Ev::Kill { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn is_speaking_turns_false_when_the_process_finishes_naturally() {
        let (tts, shared) = manager();
        tts.speak("hi", None, None).unwrap();
        assert!(tts.is_speaking());
        shared.finished.lock().unwrap().push(1);
        assert!(!tts.is_speaking());
        // A later speak does not need to (and does not) kill the finished one.
        tts.speak("again", None, None).unwrap();
        assert_eq!(
            shared
                .events()
                .iter()
                .filter(|e| matches!(e, Ev::Kill { .. }))
                .count(),
            0
        );
    }

    #[test]
    fn backend_spawn_failure_is_an_error_and_leaves_nothing_speaking() {
        let (tts, shared) = manager();
        tts.speak("first", None, None).unwrap();
        shared.spawn_fails.store(true, Ordering::SeqCst);
        assert!(matches!(
            tts.speak("second", None, None),
            Err(TtsError::Backend(_))
        ));
        assert!(
            !tts.is_speaking(),
            "previous speech was cancelled and nothing replaced it"
        );
    }

    #[test]
    fn failed_cancellation_refuses_replacement_speech() {
        let (tts, shared) = manager();
        tts.speak("first", None, None).unwrap();
        shared.kill_succeeds.store(false, Ordering::SeqCst);
        assert_eq!(tts.speak("second", None, None), Err(TtsError::CancelFailed));
        assert_eq!(shared.spawn_count(), 1, "nothing new spawned");
        assert!(
            tts.is_speaking(),
            "the unkillable handle is retained, not forgotten"
        );
        assert!(!tts.stop(), "stop reports it could not confirm");
        shared.kill_succeeds.store(true, Ordering::SeqCst);
        tts.speak("third", None, None)
            .expect("works once kills succeed again");
        assert_eq!(shared.spawn_count(), 2);
    }

    #[test]
    fn recording_start_stops_speech_synchronously_and_blocks_new_speech() {
        let (tts, shared) = manager();
        tts.speak("long reply", None, None).unwrap();
        tts.note_recording_started();
        assert!(
            !tts.is_speaking(),
            "speech is already stopped when note_recording_started returns"
        );
        assert_eq!(
            tts.speak("late reply", None, None),
            Err(TtsError::Recording)
        );
        assert_eq!(shared.spawn_count(), 1);
        tts.note_recording_stopped();
        tts.speak("after", None, None)
            .expect("speech resumes after recording");
        assert_eq!(shared.spawn_count(), 2);
    }

    #[test]
    fn recording_start_with_unkillable_speech_still_returns_and_keeps_blocking() {
        let (tts, shared) = manager();
        tts.speak("stuck", None, None).unwrap();
        shared.kill_succeeds.store(false, Ordering::SeqCst);
        tts.note_recording_started(); // must return (bounded), not hang or panic
        assert_eq!(tts.speak("x", None, None), Err(TtsError::Recording));
        assert_eq!(shared.spawn_count(), 1);
    }

    #[test]
    fn no_delayed_stop_can_kill_speech_started_after_a_recording_ends() {
        let (tts, shared) = manager();
        tts.speak("a", None, None).unwrap();
        tts.note_recording_started();
        tts.note_recording_stopped();
        tts.speak("b", None, None).unwrap();
        thread::sleep(Duration::from_millis(300)); // any stray async stop would fire here
        assert!(tts.is_speaking(), "B must still be speaking");
        let kills_of_b = shared
            .events()
            .iter()
            .filter(|e| matches!(e, Ev::Kill { id: 2, .. }))
            .count();
        assert_eq!(kills_of_b, 0);
    }

    #[test]
    fn recording_flag_and_spawn_cannot_interleave() {
        // Hammer speak() from several threads while another toggles recording.
        // Invariant: whenever recording is active no new child is spawned, so at
        // every recording start the stop leaves zero live children.
        let (tts, shared) = manager();
        let tts = Arc::new(tts);
        let stop = Arc::new(AtomicBool::new(false));
        let mut speakers = vec![];
        for _ in 0..4 {
            let tts = tts.clone();
            let stop = stop.clone();
            speakers.push(thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let _ = tts.speak("chatter", None, None);
                }
            }));
        }
        for _ in 0..50 {
            tts.note_recording_started();
            assert_eq!(
                shared.alive.load(Ordering::SeqCst),
                0,
                "a child was alive right after recording start"
            );
            thread::sleep(Duration::from_millis(2));
            assert_eq!(
                shared.alive.load(Ordering::SeqCst),
                0,
                "a child spawned during recording"
            );
            tts.note_recording_stopped();
            thread::sleep(Duration::from_millis(2));
        }
        stop.store(true, Ordering::SeqCst);
        for speaker in speakers {
            speaker.join().unwrap();
        }
    }

    #[test]
    fn concurrent_speak_and_stop_never_leave_two_voices() {
        let (tts, shared) = manager();
        let tts = Arc::new(tts);
        let mut workers = vec![];
        for _ in 0..8 {
            let tts = tts.clone();
            workers.push(thread::spawn(move || {
                for i in 0..25 {
                    if i % 5 == 0 {
                        tts.stop();
                    } else {
                        let _ = tts.speak("hello", None, None);
                    }
                }
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(shared.alive_at_spawn_max.load(Ordering::SeqCst), 0);
        assert!(shared.alive.load(Ordering::SeqCst) <= 1);
    }

    #[test]
    fn null_backend_reports_a_backend_error() {
        let tts = TtsManager::new(Box::new(NullBackend));
        assert!(matches!(
            tts.speak("hi", None, None),
            Err(TtsError::Backend(_)) | Err(TtsError::Unsupported)
        ));
        assert!(!tts.is_speaking());
    }

    // ---------- real child processes (unix) ----------

    #[cfg(unix)]
    mod process {
        use super::*;
        use std::process::Command;

        fn process_exists(pid: u32) -> bool {
            // `ps -p` succeeds for live AND zombie processes.
            Command::new("ps")
                .args(["-p", &pid.to_string()])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }

        fn wait_until(deadline: Duration, mut predicate: impl FnMut() -> bool) -> bool {
            let end = Instant::now() + deadline;
            while Instant::now() < end {
                if predicate() {
                    return true;
                }
                thread::sleep(Duration::from_millis(25));
            }
            predicate()
        }

        #[test]
        fn stdin_text_reaches_the_child_and_is_not_in_argv() {
            let tmp = tempfile::TempDir::new().unwrap();
            let out = tmp.path().join("captured.txt");
            let mut cmd = Command::new("/bin/sh");
            cmd.arg("-c").arg(format!("cat > '{}'", out.display()));
            let mut handle = spawn_process_handle(&mut cmd, "secret words").expect("spawn");
            assert!(wait_until(Duration::from_secs(3), || !handle.is_running()));
            assert_eq!(std::fs::read_to_string(&out).unwrap(), "secret words");
        }

        #[test]
        fn a_naturally_finished_child_is_reaped_without_anyone_polling() {
            let mut cmd = Command::new("/bin/sh");
            cmd.arg("-c").arg("exit 0");
            let handle = spawn_process_handle(&mut cmd, "").expect("spawn");
            let pid = handle.pid();
            // Never call is_running()/kill(): the reaper thread alone must reap it.
            assert!(
                wait_until(Duration::from_secs(3), || !process_exists(pid)),
                "child {pid} was left as a zombie"
            );
        }

        #[test]
        fn kill_stops_a_long_running_child_and_confirms_it() {
            let mut cmd = Command::new("/bin/sh");
            cmd.arg("-c").arg("sleep 30");
            let mut handle = spawn_process_handle(&mut cmd, "").expect("spawn");
            let pid = handle.pid();
            assert!(handle.is_running());
            let started = Instant::now();
            assert!(handle.kill(), "kill must be confirmed");
            assert!(
                started.elapsed() < Duration::from_millis(600),
                "bounded: {:?}",
                started.elapsed()
            );
            assert!(!handle.is_running());
            assert!(wait_until(Duration::from_secs(2), || !process_exists(pid)));
        }

        #[test]
        fn kill_of_an_already_exited_child_is_true() {
            let mut cmd = Command::new("/bin/sh");
            cmd.arg("-c").arg("exit 0");
            let mut handle = spawn_process_handle(&mut cmd, "").expect("spawn");
            assert!(wait_until(Duration::from_secs(3), || !handle.is_running()));
            assert!(handle.kill());
        }

        #[test]
        fn writing_to_a_child_that_ignores_stdin_does_not_block_or_fail() {
            let mut cmd = Command::new("/bin/sh");
            cmd.arg("-c").arg("exit 0");
            let big = "x".repeat(200_000);
            let started = Instant::now();
            let result = spawn_process_handle(&mut cmd, &big);
            assert!(result.is_ok());
            assert!(started.elapsed() < Duration::from_secs(2));
        }

        #[test]
        fn missing_program_is_an_io_error() {
            let mut cmd = Command::new("/definitely/not/a/program");
            assert!(spawn_process_handle(&mut cmd, "hi").is_err());
        }
    }
}

#[cfg(test)]
mod epoch_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    #[derive(Debug, Clone, PartialEq)]
    struct Spawned {
        text: String,
        voice: Option<String>,
        rate: Option<u32>,
        emotion: Emotion,
    }

    struct Log {
        spawns: Mutex<Vec<Spawned>>,
        kills: AtomicUsize,
        kill_ok: AtomicBool,
    }
    impl Log {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                spawns: Mutex::new(vec![]),
                kills: AtomicUsize::new(0),
                kill_ok: AtomicBool::new(true),
            })
        }
        fn spawned(&self) -> Vec<Spawned> {
            self.spawns.lock().unwrap().clone()
        }
        fn count(&self) -> usize {
            self.spawned().len()
        }
    }

    /// Overrides spawn_styled (records the emotion); plain spawn = Neutral.
    struct RecBackend(Arc<Log>);
    struct RecHandle {
        log: Arc<Log>,
        alive: bool,
    }
    impl SpeechBackend for RecBackend {
        fn spawn(
            &self,
            text: &str,
            voice: Option<&str>,
            rate: Option<u32>,
        ) -> io::Result<Box<dyn SpeechHandle>> {
            self.spawn_styled(text, voice, rate, Emotion::Neutral)
        }
        fn spawn_styled(
            &self,
            text: &str,
            voice: Option<&str>,
            rate: Option<u32>,
            emotion: Emotion,
        ) -> io::Result<Box<dyn SpeechHandle>> {
            self.0.spawns.lock().unwrap().push(Spawned {
                text: text.into(),
                voice: voice.map(str::to_string),
                rate,
                emotion,
            });
            Ok(Box::new(RecHandle {
                log: self.0.clone(),
                alive: true,
            }))
        }
    }
    impl SpeechHandle for RecHandle {
        fn kill(&mut self) -> bool {
            if !self.alive {
                return true;
            }
            if self.log.kill_ok.load(Ordering::SeqCst) {
                self.alive = false;
                self.log.kills.fetch_add(1, Ordering::SeqCst);
                true
            } else {
                false
            }
        }
        fn is_running(&mut self) -> bool {
            self.alive
        }
    }

    /// Implements ONLY `spawn`: spawn_styled must fall back to it.
    struct PlainBackend(Arc<Log>);
    impl SpeechBackend for PlainBackend {
        fn spawn(
            &self,
            text: &str,
            voice: Option<&str>,
            rate: Option<u32>,
        ) -> io::Result<Box<dyn SpeechHandle>> {
            self.0.spawns.lock().unwrap().push(Spawned {
                text: text.into(),
                voice: voice.map(str::to_string),
                rate,
                emotion: Emotion::Neutral,
            });
            Ok(Box::new(RecHandle {
                log: self.0.clone(),
                alive: true,
            }))
        }
    }

    fn mgr() -> (TtsManager, Arc<Log>) {
        let log = Log::new();
        (TtsManager::new(Box::new(RecBackend(log.clone()))), log)
    }

    fn latest(rx: &tokio::sync::watch::Receiver<u64>) -> u64 {
        *rx.borrow()
    }

    // ---- begin / is_current / speak_styled_if_current ----

    #[test]
    fn begin_then_speak_if_current_spawns_exactly_once() {
        let (m, log) = mgr();
        let e = m.begin_utterance().unwrap();
        assert!(m.is_current(e));
        m.speak_styled_if_current(e, "hello", None, None, Emotion::Neutral)
            .unwrap();
        assert_eq!(log.count(), 1);
        assert_eq!(log.spawned()[0].text, "hello");
    }

    #[test]
    fn epochs_increase_and_a_newer_begin_supersedes_the_older() {
        let (m, log) = mgr();
        let a = m.begin_utterance().unwrap();
        let b = m.begin_utterance().unwrap();
        assert!(b > a);
        assert!(!m.is_current(a));
        assert!(m.is_current(b));
        assert_eq!(
            m.speak_styled_if_current(a, "old", None, None, Emotion::Neutral),
            Err(TtsError::Superseded)
        );
        assert_eq!(
            log.count(),
            0,
            "a superseded call never reaches the backend"
        );
        m.speak_styled_if_current(b, "new", None, None, Emotion::Neutral)
            .unwrap();
        assert_eq!(log.count(), 1);
        assert_eq!(log.spawned()[0].text, "new");
    }

    #[test]
    fn recording_started_after_begin_refuses_the_pending_speech() {
        let (m, log) = mgr();
        let e = m.begin_utterance().unwrap();
        m.note_recording_started();
        assert!(!m.is_current(e));
        assert_eq!(
            m.speak_styled_if_current(e, "late", None, None, Emotion::Neutral),
            Err(TtsError::Superseded)
        );
        assert_eq!(log.count(), 0);
    }

    #[test]
    fn a_fixed_phrase_spoken_after_begin_supersedes_the_pending_composition() {
        let (m, log) = mgr();
        let e = m.begin_utterance().unwrap();
        m.speak("Done.", None, None).unwrap();
        assert_eq!(log.count(), 1);
        assert_eq!(
            m.speak_styled_if_current(e, "composed", None, None, Emotion::Neutral),
            Err(TtsError::Superseded)
        );
        assert_eq!(log.count(), 1, "the composed text never spoke");
    }

    #[test]
    fn begin_during_recording_is_refused_and_consumes_nothing() {
        let (m, _log) = mgr();
        m.note_recording_started();
        let rx = m.epoch_receiver();
        let before = latest(&rx);
        assert_eq!(m.begin_utterance(), Err(TtsError::Recording));
        assert_eq!(latest(&rx), before);
        m.note_recording_stopped();
        assert!(m.begin_utterance().is_ok());
    }

    #[test]
    fn speak_if_current_during_recording_after_a_valid_begin_is_refused() {
        // begin() succeeded, then recording started and stopped: the epoch was
        // bumped by the recording start, so the pending speech is refused.
        let (m, log) = mgr();
        let e = m.begin_utterance().unwrap();
        m.note_recording_started();
        m.note_recording_stopped();
        assert_eq!(
            m.speak_styled_if_current(e, "x", None, None, Emotion::Neutral),
            Err(TtsError::Superseded)
        );
        assert_eq!(log.count(), 0);
    }

    #[test]
    fn stop_and_invalidate_and_recording_start_bump_but_recording_stop_does_not() {
        let (m, _log) = mgr();
        let e = m.begin_utterance().unwrap();
        m.invalidate();
        assert!(!m.is_current(e));
        let e = m.begin_utterance().unwrap();
        assert!(m.stop());
        assert!(!m.is_current(e));
        let e = m.begin_utterance().unwrap();
        m.note_recording_started();
        assert!(!m.is_current(e));
        let e = m.epoch_receiver();
        let cur = latest(&e);
        m.note_recording_stopped();
        assert_eq!(latest(&e), cur, "note_recording_stopped must not bump");
        assert!(m.is_current(cur));
    }

    #[test]
    fn invalidate_does_not_kill_current_speech() {
        let (m, log) = mgr();
        let e = m.begin_utterance().unwrap();
        m.speak_styled_if_current(e, "talking", None, None, Emotion::Neutral)
            .unwrap();
        m.invalidate();
        assert!(m.is_speaking());
        assert_eq!(log.kills.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn stop_kills_current_speech_as_before() {
        let (m, log) = mgr();
        let e = m.begin_utterance().unwrap();
        m.speak_styled_if_current(e, "talking", None, None, Emotion::Neutral)
            .unwrap();
        assert!(m.stop());
        assert!(!m.is_speaking());
        assert_eq!(log.kills.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn speak_if_current_consumes_no_epoch_and_replaces_prior_speech() {
        let (m, log) = mgr();
        let e = m.begin_utterance().unwrap();
        m.speak_styled_if_current(e, "one", None, None, Emotion::Neutral)
            .unwrap();
        assert!(m.is_current(e), "speaking must not consume the epoch");
        m.speak_styled_if_current(e, "two", None, None, Emotion::Calm)
            .unwrap();
        assert_eq!(log.count(), 2);
        assert_eq!(log.kills.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn speak_if_current_empty_text_is_empty_and_keeps_the_epoch() {
        let (m, log) = mgr();
        let e = m.begin_utterance().unwrap();
        assert_eq!(
            m.speak_styled_if_current(e, "  \n ", None, None, Emotion::Neutral),
            Err(TtsError::Empty)
        );
        assert!(m.is_current(e));
        assert_eq!(log.count(), 0);
    }

    #[test]
    fn speak_if_current_refuses_when_the_previous_speech_cannot_be_stopped() {
        let (m, log) = mgr();
        let e = m.begin_utterance().unwrap();
        m.speak_styled_if_current(e, "one", None, None, Emotion::Neutral)
            .unwrap();
        log.kill_ok.store(false, Ordering::SeqCst);
        assert_eq!(
            m.speak_styled_if_current(e, "two", None, None, Emotion::Neutral),
            Err(TtsError::CancelFailed)
        );
        assert_eq!(log.count(), 1);
    }

    // ---- legacy speak: epoch rules ----

    #[test]
    fn legacy_speak_bumps_only_when_it_will_attempt_a_spawn() {
        let (m, log) = mgr();
        let e = m.begin_utterance().unwrap();
        assert_eq!(m.speak("   ", None, None), Err(TtsError::Empty));
        assert!(m.is_current(e), "empty text consumes no epoch");
        m.speak("hi", None, None).unwrap();
        assert!(!m.is_current(e), "a real speak supersedes older intents");
        assert_eq!(log.count(), 1);
    }

    #[test]
    fn legacy_speak_during_recording_consumes_no_epoch() {
        let (m, log) = mgr();
        m.note_recording_started();
        let rx = m.epoch_receiver();
        let before = latest(&rx);
        assert_eq!(m.speak("hi", None, None), Err(TtsError::Recording));
        assert_eq!(latest(&rx), before);
        assert_eq!(log.count(), 0);
    }

    #[test]
    fn speak_styled_passes_emotion_and_sanitized_text_to_the_backend() {
        let (m, log) = mgr();
        m.speak_styled(
            "[[rate 999]] hello",
            Some("Samantha"),
            Some(200),
            Emotion::Cheerful,
        )
        .unwrap();
        let s = log.spawned();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].emotion, Emotion::Cheerful);
        assert_eq!(s[0].text, "rate 999 hello");
        assert_eq!(s[0].voice.as_deref(), Some("Samantha"));
        assert_eq!(s[0].rate, Some(200));
        m.speak("again", None, None).unwrap();
        assert_eq!(log.spawned()[1].emotion, Emotion::Neutral);
    }

    #[test]
    fn a_backend_that_only_implements_spawn_still_works_with_emotions() {
        let log = Log::new();
        let m = TtsManager::new(Box::new(PlainBackend(log.clone())));
        m.speak_styled("hello", None, None, Emotion::Excited)
            .unwrap();
        assert_eq!(log.count(), 1);
        assert_eq!(log.spawned()[0].text, "hello");
    }

    // ---- watch receiver / wait_superseded ----

    #[test]
    fn receiver_observes_the_latest_epoch_value() {
        let (m, _log) = mgr();
        let rx = m.epoch_receiver();
        let a = m.begin_utterance().unwrap();
        m.invalidate();
        m.invalidate();
        let latest_value = latest(&rx);
        assert!(latest_value > a);
        assert!(m.is_current(latest_value));
    }

    #[test]
    fn wait_superseded_completes_immediately_when_already_superseded() {
        let (m, _log) = mgr();
        let e = m.begin_utterance().unwrap();
        m.invalidate();
        let rx = m.epoch_receiver();
        tauri::async_runtime::block_on(async {
            tokio::time::timeout(Duration::from_secs(2), wait_superseded(rx, e))
                .await
                .expect("must complete");
        });
    }

    #[test]
    fn wait_superseded_completes_when_a_bump_arrives_later_and_not_before() {
        let (m, _log) = mgr();
        let m = Arc::new(m);
        let e = m.begin_utterance().unwrap();
        let rx = m.epoch_receiver();
        let m2 = m.clone();
        let bumper = thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            m2.invalidate();
        });
        let started = Instant::now();
        tauri::async_runtime::block_on(async {
            tokio::time::timeout(Duration::from_secs(5), wait_superseded(rx, e))
                .await
                .expect("must complete");
        });
        assert!(started.elapsed() >= Duration::from_millis(100));
        bumper.join().unwrap();
    }

    #[test]
    fn wait_superseded_stays_pending_while_the_epoch_is_current() {
        let (m, _log) = mgr();
        let e = m.begin_utterance().unwrap();
        let rx = m.epoch_receiver();
        let r = tauri::async_runtime::block_on(async {
            tokio::time::timeout(Duration::from_millis(200), wait_superseded(rx, e)).await
        });
        assert!(r.is_err(), "must still be pending");
    }

    // ---- concurrency ----

    #[test]
    fn concurrent_begin_and_speak_never_double_spawns_for_one_epoch() {
        let (m, log) = mgr();
        let m = Arc::new(m);
        let oks = Arc::new(AtomicUsize::new(0));
        let mut handles = vec![];
        for i in 0..8 {
            let m = m.clone();
            let oks = oks.clone();
            handles.push(thread::spawn(move || {
                if let Ok(e) = m.begin_utterance() {
                    thread::sleep(Duration::from_millis(5));
                    if m.speak_styled_if_current(e, &format!("t{i}"), None, None, Emotion::Neutral)
                        .is_ok()
                    {
                        oks.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(log.count(), oks.load(Ordering::SeqCst));
        assert!(log.count() >= 1, "the globally last begin always wins");
        assert!(log.count() <= 8);
    }

    // ---- SayBackend input (pure) ----

    #[test]
    fn say_stdin_text_adds_the_pitch_prefix_after_the_text_is_final() {
        assert_eq!(say_stdin_text("hello", Emotion::Neutral), "hello");
        assert_eq!(
            say_stdin_text("hello", Emotion::Cheerful),
            "[[pbas +6]] hello"
        );
        assert_eq!(
            say_stdin_text("hello", Emotion::Serious),
            "[[pbas -6]] hello"
        );
    }

    #[test]
    fn say_args_styled_applies_the_emotion_rate_and_keeps_neutral_legacy() {
        assert_eq!(
            say_args_styled(None, None, Emotion::Neutral),
            say_args(None, None)
        );
        assert_eq!(
            say_args_styled(Some("Samantha"), Some(200), Emotion::Neutral),
            say_args(Some("Samantha"), Some(200))
        );
        assert_eq!(
            say_args_styled(None, Some(200), Emotion::Cheerful),
            say_args(None, Some(216))
        );
        assert_eq!(
            say_args_styled(None, None, Emotion::Calm),
            say_args(None, Some(166))
        );
        assert_eq!(
            say_args_styled(Some("bad;voice"), Some(400), Emotion::Excited),
            say_args(None, Some(400))
        );
    }

    #[test]
    fn only_our_own_prefix_can_contain_double_brackets_on_stdin() {
        let sanitized = sanitize_speech_text("[[rate 999]] [[pbas +99]] hi").unwrap();
        let stdin = say_stdin_text(&sanitized, Emotion::Excited);
        assert_eq!(stdin.matches("[[").count(), 1);
        assert!(stdin.starts_with("[[pbas +8]] "));
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "macOS smoke: runs the real `say` writing to a temp file"]
    fn real_say_accepts_the_pitch_prefix() {
        use std::process::{Command, Stdio};
        let out = std::env::temp_dir().join("vox-say-smoke.aiff");
        let mut child = Command::new("/usr/bin/say")
            .args(["-o", out.to_str().unwrap(), "-f", "-"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(say_stdin_text("Smoke test.", Emotion::Cheerful).as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success());
        assert!(std::fs::metadata(&out).unwrap().len() > 0);
        let _ = std::fs::remove_file(&out);
    }
}

#[cfg(test)]
#[path = "tts_queue_manager_tests.rs"]
mod queue_manager_tests;
