//! Platform-neutral helpers for the Phonon TTS engine (DESIGN.md Rev 4, P3).
//!
//! Pure functions only: no I/O, no threads, no ptts types. The macOS-only glue that calls
//! the `ptts` crate lives in `natural_phonon.rs`. Spec = `natural_phonon_core_tests.rs`.
#![allow(dead_code)] // Consumers (natural_phonon.rs, router) land in later P3/P4 steps.

use crate::managers::natural_engine::{EngineLoader, SynthEngine};
use crate::managers::natural_model::ModelCoordinator;
use crate::managers::natural_snapshot::Snapshot;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Native sample rate of the Phonon model (mono, float32 PCM).
pub const PHONON_SAMPLE_RATE: u32 = 24_000;

/// True when `text` contains at least one alphanumeric char. Text without one (empty,
/// whitespace, punctuation, emoji, symbols) is not worth an engine call.
pub fn has_speakable_chars(text: &str) -> bool {
    text.chars().any(char::is_alphanumeric)
}

/// Makes engine output safe to play: NaN and +/-inf become 0.0, finite samples are
/// clamped to [-1.0, 1.0].
// The `&mut Vec<f32>` signature is the frozen contract (natural_phonon_core_tests.rs).
#[allow(clippy::ptr_arg)]
pub fn sanitize_pcm(pcm: &mut Vec<f32>) {
    for sample in pcm.iter_mut() {
        *sample = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
    }
}

/// Removes the quiet tail after the last sample whose magnitude reaches
/// `threshold_dbfs` (linear threshold `10^(dbfs/20)`), keeping up to `keep_ms` of audio
/// after it. Interior samples are never touched.
///
/// If no sample reaches the threshold (all quiet or empty input) the buffer is cleared.
pub fn trim_trailing_silence(
    pcm: &mut Vec<f32>,
    sample_rate: u32,
    threshold_dbfs: f32,
    keep_ms: u32,
) {
    let threshold_lin = 10f32.powf(threshold_dbfs / 20.0);
    let Some(last_audible) = pcm.iter().rposition(|sample| sample.abs() >= threshold_lin) else {
        pcm.clear();
        return;
    };
    let keep_samples = u64::from(sample_rate) * u64::from(keep_ms) / 1000;
    let end = (last_audible as u64 + 1)
        .saturating_add(keep_samples)
        .min(pcm.len() as u64);
    pcm.truncate(end as usize);
}

/// Turns one piece of text into playable audio via `say` (the engine call).
///
/// * Text with no speakable character returns `Ok(vec![])` WITHOUT calling `say`.
/// * Otherwise `say(text)` runs; its error passes through unchanged.
/// * The output is sanitised and its quiet tail trimmed (see the helpers above).
/// * Speakable text that still ends up with no audio is an error, never silence.
pub fn synthesize_with(
    text: &str,
    say: &mut dyn FnMut(&str) -> Result<Vec<f32>, String>,
) -> Result<Vec<f32>, String> {
    if !has_speakable_chars(text) {
        return Ok(Vec::new());
    }
    let mut pcm = say(text)?;
    sanitize_pcm(&mut pcm);
    trim_trailing_silence(&mut pcm, PHONON_SAMPLE_RATE, -50.0, 150);
    if pcm.is_empty() {
        return Err("the voice produced no audio".to_string());
    }
    Ok(pcm)
}

/// Prefix of the private snapshot directories created under the scratch root.
const SNAPSHOT_DIR_PREFIX: &str = "snapshot-";

/// Builds the engine from a verified snapshot. Called while the snapshot exists.
pub type SnapshotBuilder =
    Box<dyn Fn(&Snapshot) -> Result<Box<dyn SynthEngine>, String> + Send + Sync>;

/// Loads an engine from a verified private snapshot of the installed model.
///
/// Each `load()` re-verifies the install (no cached verdict), hands the snapshot to `builder`
/// while it exists, and then drops it, so its directory is gone when `load` returns.
pub struct SnapshotLoader {
    coordinator: Arc<ModelCoordinator>,
    scratch_root: PathBuf,
    builder: SnapshotBuilder,
}

impl SnapshotLoader {
    /// Removes stale `snapshot-*` directories left directly under `scratch_root` by an earlier
    /// crash. Nothing else under `scratch_root` is touched.
    pub fn new(
        coordinator: Arc<ModelCoordinator>,
        scratch_root: PathBuf,
        builder: SnapshotBuilder,
    ) -> Self {
        remove_stale_snapshots(&scratch_root);
        Self {
            coordinator,
            scratch_root,
            builder,
        }
    }
}

impl EngineLoader for SnapshotLoader {
    fn load(&mut self) -> Result<Box<dyn SynthEngine>, String> {
        let snapshot = self.coordinator.snapshot(&self.scratch_root).map_err(|e| {
            format!("the Phonon voice files are missing or damaged and must be reinstalled ({e})")
        })?;
        // `snapshot` is dropped when this function returns, on every path.
        (self.builder)(&snapshot)
    }
}

/// Deletes directories named `snapshot-*` directly under `scratch_root`. Symlinks and other
/// entries are left alone. A missing or unreadable `scratch_root` is not an error here.
fn remove_stale_snapshots(scratch_root: &Path) {
    let Ok(entries) = std::fs::read_dir(scratch_root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(SNAPSHOT_DIR_PREFIX) {
            continue;
        }
        let path = entry.path();
        let is_real_dir = std::fs::symlink_metadata(&path)
            .map(|meta| meta.file_type().is_dir())
            .unwrap_or(false);
        if is_real_dir {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}
