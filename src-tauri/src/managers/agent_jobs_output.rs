#![allow(dead_code)]

//! Pure output pipeline for background job output (M3 S2a): line framing,
//! per-line sanitising and redaction, a bounded tail ring, and private-key
//! block suppression. No process, tokio or Tauri use.
//!
//! Scaffolding only: bodies are filled in by ram-implementer (Phase B).

use std::sync::{Arc, Mutex};

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

/// Bounded ring of the newest output lines; the oldest lines are dropped first.
#[derive(Debug, Default)]
pub struct TailRing {
    lines: std::collections::VecDeque<String>,
    bytes: usize,
}

#[allow(clippy::len_without_is_empty)]
impl TailRing {
    /// Creates an empty ring.
    pub fn new() -> Self {
        todo!("S2a Phase B: TailRing::new")
    }

    /// Appends a line, evicting the oldest lines until both caps hold.
    pub fn push(&mut self, _line: String) {
        todo!("S2a Phase B: TailRing::push")
    }

    /// Returns the newest `n` lines, oldest first.
    pub fn last(&self, _n: usize) -> Vec<String> {
        todo!("S2a Phase B: TailRing::last")
    }

    /// Total bytes currently held by the ring.
    pub fn bytes(&self) -> usize {
        todo!("S2a Phase B: TailRing::bytes")
    }

    /// Number of lines currently held by the ring.
    pub fn len(&self) -> usize {
        todo!("S2a Phase B: TailRing::len")
    }
}

/// Frames one output stream into sanitised, redacted, display-truncated lines.
pub struct OutputPipeline {
    ring: Arc<Mutex<TailRing>>,
    total_cap: u64,
}

impl OutputPipeline {
    /// Creates a pipeline that pushes finished lines into `ring`, using [`TOTAL_CAP`].
    pub fn new(_ring: Arc<Mutex<TailRing>>) -> Self {
        todo!("S2a Phase B: OutputPipeline::new")
    }

    /// Like [`OutputPipeline::new`] but with an injectable total byte cap (tests).
    pub fn with_total_cap(_ring: Arc<Mutex<TailRing>>, _total_cap: u64) -> Self {
        todo!("S2a Phase B: OutputPipeline::with_total_cap")
    }

    /// Feeds one chunk of raw bytes (any size, any boundaries).
    pub fn feed(&mut self, _chunk: &[u8]) {
        todo!("S2a Phase B: OutputPipeline::feed")
    }

    /// EOF: flushes a partial last line under the same rules, ends suppression.
    pub fn finish(&mut self) {
        todo!("S2a Phase B: OutputPipeline::finish")
    }

    /// Pushes the marker line "[output read error]".
    pub fn read_error(&mut self) {
        todo!("S2a Phase B: OutputPipeline::read_error")
    }

    /// Current framing buffer size in bytes (must never exceed LINE_CAP).
    pub fn buffered_bytes(&self) -> usize {
        todo!("S2a Phase B: OutputPipeline::buffered_bytes")
    }

    /// True while a private-key block is being suppressed.
    pub fn suppressing(&self) -> bool {
        todo!("S2a Phase B: OutputPipeline::suppressing")
    }
}

#[cfg(test)]
#[path = "agent_jobs_output_tests.rs"]
mod agent_jobs_output_tests;
