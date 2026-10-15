//! Pure output pipeline for background job output (M3 S2a): line framing,
//! per-line sanitising and redaction, a bounded tail ring, and private-key
//! block suppression. No process, tokio or Tauri use.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

/// Reader chunk size used by the job runner (documented here; the reader lives in agent_jobs.rs).
pub const CHUNK_BYTES: usize = 8 * 1024;
/// Framing cap in bytes: a longer line is discarded entirely, never truncated.
pub const LINE_CAP: usize = 2000;
/// Display truncation in bytes, applied after redaction.
pub const DISPLAY_CAP: usize = 200;
/// Maximum number of lines kept in the tail ring.
pub const RING_LINES: usize = 50;
/// Maximum total bytes kept in the tail ring.
pub const RING_BYTES: usize = 32 * 1024;
/// After this many bytes fed to one stream, further bytes are read and dropped.
pub const TOTAL_CAP: u64 = 64 * 1024 * 1024;

/// Marker pushed in place of an overlong line.
const LONG_LINE_MARKER: &str = "[long line omitted]";
/// Marker pushed once when a private-key block starts being suppressed.
const BLOCK_MARKER: &str = "[private key block omitted]";
/// Marker pushed by [`OutputPipeline::read_error`].
const READ_ERROR_MARKER: &str = "[output read error]";
/// Start marker of a private-key block.
const BEGIN_MARK: &[u8] = b"-----BEGIN";
/// End marker of a private-key block.
const END_MARK: &[u8] = b"-----END";
/// Key marker that must follow BEGIN/END closely.
const KEY_MARK: &[u8] = b"PRIVATE KEY-----";
/// Maximum distance in bytes between a BEGIN/END marker and the key marker.
const MARK_GAP: usize = 40;
/// Rolling byte window used by the private-key marker scan.
const SCAN_WINDOW: usize = 96;

/// Bounded ring of the newest output lines; the oldest lines are dropped first.
#[derive(Debug, Default)]
pub struct TailRing {
    lines: VecDeque<String>,
    bytes: usize,
}

impl TailRing {
    /// Creates an empty ring.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a line, evicting the oldest lines until both caps hold.
    pub fn push(&mut self, line: String) {
        self.bytes += line.len();
        self.lines.push_back(line);
        while self.lines.len() > RING_LINES || self.bytes > RING_BYTES {
            match self.lines.pop_front() {
                Some(old) => self.bytes = self.bytes.saturating_sub(old.len()),
                None => break,
            }
        }
    }

    /// Returns the newest `n` lines, oldest first.
    pub fn last(&self, n: usize) -> Vec<String> {
        let skip = self.lines.len().saturating_sub(n);
        self.lines.iter().skip(skip).cloned().collect()
    }

    /// Total bytes currently held by the ring.
    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Number of lines currently held by the ring.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.lines.len()
    }
}

/// Bytes kept by a [`ResultCapture`].
const RESULT_CAP: usize = 8192;

/// M4: bounded capture of the sanitized, redacted lines of an ask job. Keeps the
/// last RESULT_CAP bytes of the lines joined with a newline between each pair.
#[derive(Debug, Default)]
pub(super) struct ResultCapture {
    lines: VecDeque<String>,
    /// Sum of the byte lengths of `lines`, not counting separators.
    bytes: usize,
}

impl ResultCapture {
    /// Creates an empty capture.
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Adds one already sanitized and redacted line (no newline).
    pub(super) fn push_line(&mut self, line: &str) {
        self.bytes += line.len();
        self.lines.push_back(line.to_string());
        while self.joined_len() > RESULT_CAP && self.lines.len() > 1 {
            if let Some(old) = self.lines.pop_front() {
                self.bytes = self.bytes.saturating_sub(old.len());
            }
        }
        if self.joined_len() > RESULT_CAP {
            self.keep_tail_of_single_line();
        }
    }

    /// Returns the captured text (last RESULT_CAP bytes, char-boundary safe).
    pub(super) fn text(&self) -> String {
        let mut out = String::with_capacity(self.joined_len());
        for (i, line) in self.lines.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(line);
        }
        out
    }

    /// Length of the lines joined with one newline between each pair.
    fn joined_len(&self) -> usize {
        self.bytes + self.lines.len().saturating_sub(1)
    }

    /// Keeps only the last RESULT_CAP bytes of the single remaining line, cut
    /// forward to a char boundary so the result stays valid UTF-8 and within the cap.
    fn keep_tail_of_single_line(&mut self) {
        if let Some(line) = self.lines.front_mut() {
            let mut cut = line.len().saturating_sub(RESULT_CAP);
            while !line.is_char_boundary(cut) {
                cut += 1;
            }
            line.drain(..cut);
            self.bytes = line.len();
        }
    }
}

/// Frames one output stream into sanitised, redacted, display-truncated lines.
pub struct OutputPipeline {
    ring: Arc<Mutex<TailRing>>,
    total_cap: u64,
    /// Bytes offered so far (saturating); bytes past `total_cap` are ignored.
    fed: u64,
    /// Framing buffer for the current line; never longer than LINE_CAP.
    line: Vec<u8>,
    /// The current line overflowed LINE_CAP; its bytes are dropped until newline.
    discarding: bool,
    /// A CR was seen; it is a terminator only if LF follows.
    pending_cr: bool,
    /// Private-key END line: drop the rest of the line up to newline.
    skip_rest: bool,
    /// Inside a private-key block.
    suppressing: bool,
    /// Last SCAN_WINDOW bytes, for the private-key marker scan.
    window: VecDeque<u8>,
    /// Optional capture of the sanitized, redacted lines (ask jobs only).
    capture: Option<Arc<Mutex<ResultCapture>>>,
}

impl OutputPipeline {
    /// Creates a pipeline that pushes finished lines into `ring`, using [`TOTAL_CAP`].
    pub fn new(ring: Arc<Mutex<TailRing>>) -> Self {
        Self::with_total_cap(ring, TOTAL_CAP)
    }

    /// Like [`OutputPipeline::new`] but with an injectable total byte cap (tests).
    pub fn with_total_cap(ring: Arc<Mutex<TailRing>>, total_cap: u64) -> Self {
        Self {
            ring,
            total_cap,
            fed: 0,
            line: Vec::with_capacity(LINE_CAP),
            discarding: false,
            pending_cr: false,
            skip_rest: false,
            suppressing: false,
            window: VecDeque::with_capacity(SCAN_WINDOW),
            capture: None,
        }
    }

    /// Also feeds every finished line (after redaction, before display truncation)
    /// into `capture`.
    pub(super) fn with_capture(mut self, capture: Arc<Mutex<ResultCapture>>) -> Self {
        self.capture = Some(capture);
        self
    }

    /// Feeds one chunk of raw bytes (any size, any boundaries).
    pub fn feed(&mut self, chunk: &[u8]) {
        let remaining = self.total_cap.saturating_sub(self.fed);
        let take = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(chunk.len());
        for &byte in &chunk[..take] {
            self.process_byte(byte);
        }
        self.fed = self
            .fed
            .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
    }

    /// EOF: flushes a partial last line under the same rules, ends suppression.
    pub fn finish(&mut self) {
        if !self.suppressing {
            if self.pending_cr {
                self.pending_cr = false;
                self.append(b'\r');
            }
            if self.discarding || !self.line.is_empty() {
                self.end_line();
            }
        }
        self.suppressing = false;
        self.skip_rest = false;
        self.pending_cr = false;
        self.discarding = false;
        self.line.clear();
    }

    /// Pushes the marker line "[output read error]".
    pub fn read_error(&mut self) {
        self.push_line(READ_ERROR_MARKER.to_string());
    }

    /// Current framing buffer size in bytes (must never exceed LINE_CAP).
    #[cfg(test)]
    pub fn buffered_bytes(&self) -> usize {
        self.line.len()
    }

    /// True while a private-key block is being suppressed.
    #[cfg(test)]
    pub fn suppressing(&self) -> bool {
        self.suppressing
    }

    /// Handles one byte: marker scan first, then framing.
    fn process_byte(&mut self, byte: u8) {
        self.scan_markers(byte);
        if self.suppressing {
            return;
        }
        if self.skip_rest {
            if byte == b'\n' {
                self.skip_rest = false;
            }
            return;
        }
        if byte == b'\n' {
            self.pending_cr = false;
            self.end_line();
            return;
        }
        if self.pending_cr {
            self.pending_cr = false;
            self.append(b'\r');
        }
        if byte == b'\r' {
            self.pending_cr = true;
            return;
        }
        self.append(byte);
    }

    /// Appends one data byte to the framing buffer, overflowing to discard mode.
    fn append(&mut self, byte: u8) {
        if self.discarding {
            return;
        }
        if self.line.len() >= LINE_CAP {
            self.discarding = true;
            self.line.clear();
            return;
        }
        self.line.push(byte);
    }

    /// Completes the current line and pushes its display form.
    fn end_line(&mut self) {
        let mut text = if self.discarding {
            LONG_LINE_MARKER.to_string()
        } else {
            clean_line(&self.line)
        };
        self.discarding = false;
        self.line.clear();
        if let Some(capture) = self.capture.as_ref() {
            lock_capture(capture).push_line(&text);
        }
        truncate_to_char_boundary(&mut text, DISPLAY_CAP);
        self.push_line(text);
    }

    /// Updates the rolling window and enters or leaves suppression on a match.
    fn scan_markers(&mut self, byte: u8) {
        if self.window.len() == SCAN_WINDOW {
            self.window.pop_front();
        }
        self.window.push_back(byte);
        if byte != b'-' || !self.ends_with_key_mark() {
            return;
        }
        let bytes: Vec<u8> = self.window.iter().copied().collect();
        let key_start = bytes.len() - KEY_MARK.len();
        if !self.suppressing && has_mark_before(&bytes, key_start, BEGIN_MARK) {
            self.enter_block();
        } else if self.suppressing && has_mark_before(&bytes, key_start, END_MARK) {
            self.exit_block();
        }
    }

    /// True when the rolling window ends with the key marker.
    fn ends_with_key_mark(&self) -> bool {
        self.window.len() >= KEY_MARK.len()
            && self
                .window
                .iter()
                .rev()
                .zip(KEY_MARK.iter().rev())
                .all(|(a, b)| a == b)
    }

    /// Starts suppression: the BEGIN line is dropped and one marker is pushed.
    fn enter_block(&mut self) {
        self.suppressing = true;
        self.discarding = false;
        self.pending_cr = false;
        self.line.clear();
        self.push_line(BLOCK_MARKER.to_string());
    }

    /// Ends suppression; the rest of the END line is dropped.
    fn exit_block(&mut self) {
        self.suppressing = false;
        self.skip_rest = true;
        self.discarding = false;
        self.pending_cr = false;
        self.line.clear();
    }

    /// Pushes one finished display line into the shared ring.
    fn push_line(&mut self, text: String) {
        lock_ring(&self.ring).push(text);
    }
}

/// Locks the ring, recovering the data if a previous holder panicked.
fn lock_ring(ring: &Mutex<TailRing>) -> MutexGuard<'_, TailRing> {
    ring.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Locks the result capture, recovering the data if a previous holder panicked.
fn lock_capture(capture: &Mutex<ResultCapture>) -> MutexGuard<'_, ResultCapture> {
    capture
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// True if `mark` occurs in `bytes` ending at or before `key_start`, at most MARK_GAP bytes before it.
fn has_mark_before(bytes: &[u8], key_start: usize, mark: &[u8]) -> bool {
    bytes
        .windows(mark.len())
        .enumerate()
        .any(|(start, window)| {
            let end = start + mark.len();
            window == mark && end <= key_start && key_start - end <= MARK_GAP
        })
}

/// Sanitises and redacts one framed line. Display truncation is applied later.
fn clean_line(bytes: &[u8]) -> String {
    let lossy = String::from_utf8_lossy(bytes);
    let sanitised: String = lossy
        .chars()
        .map(|c| if c == '\t' || !c.is_control() { c } else { '?' })
        .collect();
    crate::managers::redact::redact(&sanitised)
}

/// Truncates `text` to at most `max` bytes without splitting a character.
fn truncate_to_char_boundary(text: &mut String, max: usize) {
    if text.len() <= max {
        return;
    }
    let mut cut = max;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.truncate(cut);
}

#[cfg(test)]
#[path = "agent_jobs_output_tests.rs"]
mod agent_jobs_output_tests;
