//! Live conversation transcript for the chat window (DESIGN-CHAT Rev 5: A4, A9, A12, A13).
//!
//! SKELETON: public contract only. Every method is `unimplemented!()` so the spec tests in
//! `chat_log_tests.rs` fail until the real implementation lands. Do not change signatures
//! without a design revision; do not weaken the tests.
//!
//! Contract summary (see the design file for the reasoning):
//! * Hooks (`say`, `reset`, `set_state`, `set_recording`) never wait on the worker, never emit
//!   and never do I/O. They clean + cap text, take a short inbox lock and wake the worker.
//! * One worker thread owns the log, assigns a global monotonic `seq` to every applied event
//!   (an entry's `id` is its `seq`) and calls the emitter AFTER releasing the log lock.
//! * Bounds: pending entries <= `PENDING_CAP` (extra entries are dropped and counted, then a
//!   `system:dropped:<n>` System entry is applied), log <= `LOG_CAP`, text <= `TEXT_CAP`
//!   chars, Clear waiters <= `CLEAR_WAITERS_CAP`. Reset/Clear are flags: a new wipe discards
//!   every still-pending entry (they would be wiped anyway) and coalesces with a pending wipe.
//! * Nothing is written to disk or logged.

use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

/// Maximum entries kept in the log (oldest dropped).
pub const LOG_CAP: usize = 200;
/// Maximum characters per entry (cut on a char boundary, ends with `…` when cut).
pub const TEXT_CAP: usize = 2000;
/// Maximum entries admitted but not yet applied.
pub const PENDING_CAP: usize = 256;
/// Maximum simultaneously registered Clear waiters.
pub const CLEAR_WAITERS_CAP: usize = 8;
/// Tauri event name carrying [`ChatEvent`].
pub const CHAT_EVENT: &str = "converse-chat";

/// Who said it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatRole {
    /// The person speaking to Vox.
    User,
    /// Vox (text actually handed to speech).
    Assistant,
    /// A fixed note such as `system:started` or `system:dropped:3`.
    System,
}

/// One transcript line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatEntry {
    /// Equals the `seq` of the event that carried the entry.
    pub id: u64,
    /// Milliseconds since the Unix epoch when applied.
    pub ts_ms: u64,
    /// Speaker.
    pub role: ChatRole,
    /// Cleaned, capped text.
    pub text: String,
}

/// What happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ChatEventKind {
    /// A line was appended.
    Entry {
        /// The new line.
        entry: ChatEntry,
    },
    /// A new conversation session started; the log was emptied.
    Reset,
    /// The user cleared the log.
    Clear,
    /// The conversation state changed.
    State {
        /// `listening`, `thinking`, `speaking`, `acting` or `idle`.
        state: String,
        /// End reason label when the session ended.
        end: Option<String>,
    },
    /// Recording was switched on or off.
    Config {
        /// Whether new lines are being recorded.
        recording: bool,
    },
}

/// One ordered event on the `converse-chat` stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatEvent {
    /// Global, strictly increasing sequence number (never reset during an app run).
    pub seq: u64,
    /// The payload.
    pub kind: ChatEventKind,
}

/// Authoritative state at one `seq`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSnapshot {
    /// Highest `seq` applied when the snapshot was taken.
    pub seq: u64,
    /// Current entries, oldest first (at most `LOG_CAP`).
    pub entries: Vec<ChatEntry>,
    /// Current conversation state label (`idle` initially).
    pub state: String,
    /// End reason of the last session, if any.
    pub end: Option<String>,
    /// Whether new lines are currently recorded.
    pub recording: bool,
}

/// Why a Clear request did not complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClearError {
    /// Too many Clear requests are already waiting.
    Busy,
    /// The worker did not apply the Clear in time.
    Timeout,
    /// The worker has stopped.
    Dead,
}

/// Receives events in `seq` order from the worker thread only.
pub trait ChatEmitter: Send + Sync + 'static {
    /// Deliver one event. May block (the hooks never wait for it).
    fn emit(&self, event: &ChatEvent);
}

/// Tunables (tests shorten the Clear wait).
#[derive(Debug, Clone, Copy)]
pub struct ChatLogConfig {
    /// How long `clear()` waits for the worker before returning `Timeout`.
    pub clear_timeout: Duration,
}

impl Default for ChatLogConfig {
    fn default() -> Self {
        Self {
            clear_timeout: Duration::from_secs(2),
        }
    }
}

/// The transcript log plus its worker. Share as `Arc<ChatLog>`.
pub struct ChatLog {
    _private: (),
}

impl ChatLog {
    /// Start a log with default tunables. `recording` is the initial recording flag.
    pub fn start(emitter: Arc<dyn ChatEmitter>, recording: bool) -> Arc<ChatLog> {
        Self::start_with(emitter, recording, ChatLogConfig::default())
    }

    /// Start a log with explicit tunables and spawn the worker thread.
    pub fn start_with(
        _emitter: Arc<dyn ChatEmitter>,
        _recording: bool,
        _config: ChatLogConfig,
    ) -> Arc<ChatLog> {
        unimplemented!("chat_log worker")
    }

    /// Record one line. Non-blocking. No-op when recording is off, after shutdown, or when the
    /// cleaned text is empty. Cleaning: strip control characters except `\n` (`\t` becomes a
    /// space), trim, cap to `TEXT_CAP` chars.
    pub fn say(&self, _role: ChatRole, _text: &str) {
        unimplemented!("chat_log say")
    }

    /// Session boundary: empty the log (pending entries are discarded, a `Reset` event follows).
    pub fn reset(&self) {
        unimplemented!("chat_log reset")
    }

    /// Latest conversation state (coalesced; the final value always lands).
    pub fn set_state(&self, _state: &str, _end: Option<&str>) {
        unimplemented!("chat_log set_state")
    }

    /// Switch recording on or off (takes effect for later `say` calls immediately; the final
    /// value always lands as a `Config` event; existing entries are kept).
    pub fn set_recording(&self, _on: bool) {
        unimplemented!("chat_log set_recording")
    }

    /// Current recording flag as seen by the hooks.
    pub fn recording(&self) -> bool {
        unimplemented!("chat_log recording")
    }

    /// User-initiated Clear. Waits up to `clear_timeout` for the worker (never call from a
    /// hook thread). `Ok` means the log was emptied before returning.
    pub fn clear(&self) -> Result<(), ClearError> {
        unimplemented!("chat_log clear")
    }

    /// Consistent view of the log (brief lock; never touches the emitter).
    pub fn snapshot(&self) -> ChatSnapshot {
        unimplemented!("chat_log snapshot")
    }

    /// Sticky shutdown: later hooks no-op, queued Clear waiters complete or fail explicitly
    /// and the worker exits.
    pub fn shutdown(&self) {
        unimplemented!("chat_log shutdown")
    }

    /// Wait until the worker thread has exited; false on timeout.
    pub fn wait_stopped(&self, _timeout: Duration) -> bool {
        unimplemented!("chat_log wait_stopped")
    }
}
