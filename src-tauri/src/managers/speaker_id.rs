//! Speaker embedding math, voiceprint store and verification policy (design D6, A8, A8', A9).
//!
//! Pinned by `speaker_id_tests.rs`. Embeddings are never logged or formatted.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::settings::SpeakerSensitivity;

/// Constants of the speaker model (WeSpeaker ResNet34, design A10) and of the
/// capture-shaped evaluation (design SD3 / A8).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeakerModelSpec {
    pub file: &'static str,
    pub sha256: &'static str,
    pub dim: usize,
    pub min_verify_buffer_ms: u32,
    pub min_embed_samples: usize,
    pub lenient: f32,
    pub balanced: f32,
    pub strict: f32,
}

pub const SPEAKER_SPEC: SpeakerModelSpec = SpeakerModelSpec {
    file: "wespeaker_en_voxceleb_resnet34.onnx",
    sha256: "5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94",
    dim: 256,
    min_verify_buffer_ms: 2000,
    min_embed_samples: 16000,
    lenient: 0.59,
    balanced: 0.69,
    strict: 0.74,
};

/// Cosine threshold for a sensitivity level. Accept when cosine >= threshold.
pub fn threshold(s: SpeakerSensitivity) -> f32 {
    match s {
        SpeakerSensitivity::Lenient => SPEAKER_SPEC.lenient,
        SpeakerSensitivity::Balanced => SPEAKER_SPEC.balanced,
        SpeakerSensitivity::Strict => SPEAKER_SPEC.strict,
    }
}

/// Cosine similarity. `None` for empty input, length mismatch, any non-finite value,
/// or a zero-norm vector.
pub fn cosine(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.is_empty() || a.len() != b.len() {
        return None;
    }
    let mut dot = 0.0_f64;
    let mut norm_a = 0.0_f64;
    let mut norm_b = 0.0_f64;
    for (x, y) in a.iter().zip(b) {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let (x, y) = (f64::from(*x), f64::from(*y));
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return None;
    }
    let cos = dot / (norm_a.sqrt() * norm_b.sqrt());
    cos.is_finite().then(|| cos.clamp(-1.0, 1.0) as f32)
}

/// Scales `v` to unit L2 norm. Same `None` rules as [`cosine`].
pub fn l2_normalize(v: &[f32]) -> Option<Vec<f32>> {
    if v.is_empty() || v.iter().any(|x| !x.is_finite()) {
        return None;
    }
    let norm_sq: f64 = v.iter().map(|x| f64::from(*x) * f64::from(*x)).sum();
    if norm_sq == 0.0 || !norm_sq.is_finite() {
        return None;
    }
    let inv = 1.0 / norm_sq.sqrt();
    Some(v.iter().map(|x| (f64::from(*x) * inv) as f32).collect())
}

/// Persisted voiceprint (design A8'/A9). `version` must be 1.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct Voiceprint {
    pub version: u8,
    pub model_sha256: String,
    pub dim: usize,
    pub embedding: Vec<f32>,
    pub clips: u8,
}

/// Manual Debug: never prints the embedding or the model hash.
impl std::fmt::Debug for Voiceprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Voiceprint")
            .field("version", &self.version)
            .field("dim", &self.dim)
            .field("clips", &self.clips)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrollError {
    TooFewClips,
    DimMismatch,
    NonFinite,
    ZeroVector,
    Inconsistent,
}

/// Minimum number of enrollment clips.
pub const MIN_ENROLL_CLIPS: usize = 3;
/// Minimum pairwise cosine between enrollment clips (design A8').
pub const ENROLL_MIN_PAIR_COSINE: f32 = 0.30;

/// Validates and unit-normalises one enrollment clip.
fn normalise_clip(e: &[f32]) -> Result<Vec<f32>, EnrollError> {
    if e.iter().any(|x| !x.is_finite()) {
        return Err(EnrollError::NonFinite);
    }
    l2_normalize(e).ok_or(EnrollError::ZeroVector)
}

/// Enrollment: `l2_normalize(mean_i(l2_normalize(e_i)))` (design A8').
///
/// The mean is accumulated in f64 and then normalised. Dividing by the clip count before the
/// final normalisation does not change the direction, so the sum is normalised directly.
pub fn enroll(embeddings: &[Vec<f32>], model_sha256: &str) -> Result<Voiceprint, EnrollError> {
    if embeddings.len() < MIN_ENROLL_CLIPS {
        return Err(EnrollError::TooFewClips);
    }
    let dim = SPEAKER_SPEC.dim;
    if embeddings.iter().any(|e| e.len() != dim) {
        return Err(EnrollError::DimMismatch);
    }
    let mut clips = Vec::with_capacity(embeddings.len());
    for e in embeddings {
        clips.push(normalise_clip(e)?);
    }
    for (i, a) in clips.iter().enumerate() {
        for b in &clips[i + 1..] {
            let c = cosine(a, b).ok_or(EnrollError::ZeroVector)?;
            if c < ENROLL_MIN_PAIR_COSINE {
                return Err(EnrollError::Inconsistent);
            }
        }
    }
    let mut sum = vec![0.0_f64; dim];
    for clip in &clips {
        for (s, x) in sum.iter_mut().zip(clip) {
            *s += f64::from(*x);
        }
    }
    let norm = sum.iter().map(|s| s * s).sum::<f64>().sqrt();
    if norm == 0.0 || !norm.is_finite() {
        return Err(EnrollError::ZeroVector);
    }
    let embedding = sum.iter().map(|s| (s / norm) as f32).collect();
    Ok(Voiceprint {
        version: 1,
        model_sha256: model_sha256.to_string(),
        dim,
        embedding,
        clips: u8::try_from(embeddings.len()).unwrap_or(u8::MAX),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Accept,
    Reject,
    SkippedTooShort,
}

/// Verification policy. Order: buffer too short (SkippedTooShort) first; then the
/// not-enrolled-equivalent checks (sha mismatch, dim mismatch, version != 1,
/// non-finite or zero embedding) which Accept; then cosine against the threshold.
pub fn verify(
    vp: &Voiceprint,
    embedding: &[f32],
    sens: SpeakerSensitivity,
    buffer_ms: u32,
    current_model_sha256: &str,
) -> Verdict {
    if buffer_ms < SPEAKER_SPEC.min_verify_buffer_ms {
        return Verdict::SkippedTooShort;
    }
    let shape_ok = vp.version == 1
        && vp.model_sha256 == current_model_sha256
        && vp.dim == SPEAKER_SPEC.dim
        && vp.dim == embedding.len()
        && vp.embedding.len() == vp.dim;
    if !shape_ok {
        return Verdict::Accept;
    }
    match cosine(&vp.embedding, embedding) {
        None => Verdict::Accept,
        Some(cos) if cos >= threshold(sens) => Verdict::Accept,
        Some(_) => Verdict::Reject,
    }
}

/// Duration of a buffer in milliseconds at 16 kHz, saturating (len / 16).
pub fn buffer_ms(samples_len: usize) -> u32 {
    u32::try_from(samples_len / 16).unwrap_or(u32::MAX)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    UnsupportedPlatform,
    SymlinkRefused,
    Io,
    Corrupt,
}

const VOICEPRINT_FILE: &str = "voiceprint.json";

/// Unique suffix for temp file names within this process.
#[cfg(unix)]
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// True when the voiceprint has the shape the store accepts: version 1, spec dim, matching
/// embedding length, enough clips, and a finite non-zero embedding.
fn is_well_formed(vp: &Voiceprint) -> bool {
    vp.version == 1
        && vp.dim == SPEAKER_SPEC.dim
        && vp.embedding.len() == vp.dim
        && usize::from(vp.clips) >= MIN_ENROLL_CLIPS
        && l2_normalize(&vp.embedding).is_some()
}

/// Owner-only voiceprint store (design A9). Unix only; elsewhere `save` fails closed.
pub struct VoiceprintStore {
    dir: PathBuf,
    cache: Mutex<Option<Arc<Voiceprint>>>,
    generation: AtomicU64,
}

impl VoiceprintStore {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            cache: Mutex::new(None),
            generation: AtomicU64::new(0),
        }
    }

    /// Monotonic counter bumped by every save and every delete.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Cached load. Missing, garbage, truncated, version != 1, dim != spec dim,
    /// non-finite floats or wrong clip count => None (never a panic).
    ///
    /// The cache is cleared under the same lock as every generation bump, so a cached value
    /// is always the file as of the current generation.
    pub fn load(&self) -> Option<Arc<Voiceprint>> {
        let mut cache = self.lock_cache();
        if let Some(vp) = cache.as_ref() {
            return Some(Arc::clone(vp));
        }
        let vp = Arc::new(self.read_valid()?);
        *cache = Some(Arc::clone(&vp));
        Some(vp)
    }

    /// Atomic owner-only write. Refuses symlinked dir or final path (SymlinkRefused).
    #[cfg(unix)]
    pub fn save(&self, vp: &Voiceprint) -> Result<(), StoreError> {
        if !is_well_formed(vp) {
            return Err(StoreError::Corrupt);
        }
        let bytes = serde_json::to_vec(vp).map_err(|_| StoreError::Corrupt)?;
        let mut cache = self.lock_cache();

        ensure_private_dir(&self.dir)?;
        let final_path = self.final_path();
        refuse_symlink(&final_path)?;

        let tmp = write_temp_file(&self.dir, &bytes)?;
        if fs::rename(&tmp, &final_path).is_err() {
            let _ = fs::remove_file(&tmp);
            return Err(StoreError::Io);
        }
        self.bump_generation(&mut cache);

        sync_dir(&self.dir)?;
        if verify_modes(&self.dir, &final_path).is_err() {
            let _ = fs::remove_file(&final_path);
            return Err(StoreError::Io);
        }
        Ok(())
    }

    /// Non-Unix hosts cannot enforce owner-only storage, so enrollment fails closed.
    #[cfg(not(unix))]
    pub fn save(&self, _vp: &Voiceprint) -> Result<(), StoreError> {
        Err(StoreError::UnsupportedPlatform)
    }

    /// Removes the voiceprint if present (Ok when absent); always bumps the generation.
    pub fn delete(&self) -> Result<(), StoreError> {
        let mut cache = self.lock_cache();
        let result =
            refuse_symlink(&self.dir).and_then(|()| match fs::remove_file(self.final_path()) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(_) => Err(StoreError::Io),
            });
        self.bump_generation(&mut cache);
        result
    }

    pub fn is_enrolled(&self) -> bool {
        self.load().is_some()
    }

    fn final_path(&self) -> PathBuf {
        self.dir.join(VOICEPRINT_FILE)
    }

    fn lock_cache(&self) -> MutexGuard<'_, Option<Arc<Voiceprint>>> {
        match self.cache.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Bumps the generation and drops the cached voiceprint. Callers hold the cache lock.
    fn bump_generation(&self, cache: &mut Option<Arc<Voiceprint>>) {
        *cache = None;
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Reads and validates the final file. Symlinks and non-regular files are never read.
    fn read_valid(&self) -> Option<Voiceprint> {
        let path = self.final_path();
        let meta = fs::symlink_metadata(&path).ok()?;
        if !meta.is_file() {
            return None;
        }
        let bytes = fs::read(&path).ok()?;
        let vp: Voiceprint = serde_json::from_slice(&bytes).ok()?;
        is_well_formed(&vp).then_some(vp)
    }
}

/// Fails with SymlinkRefused when `path` is a symlink. Absent paths are fine.
fn refuse_symlink(path: &Path) -> Result<(), StoreError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(StoreError::SymlinkRefused),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(StoreError::Io),
    }
}

/// Makes sure `dir` is a real directory with mode 0700. Creates it when missing and repairs an
/// existing 0755 directory to 0700. The chmod only succeeds for the owner (or root), so a
/// directory owned by someone else fails with Io.
#[cfg(unix)]
fn ensure_private_dir(dir: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    match fs::symlink_metadata(dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(StoreError::SymlinkRefused);
            }
            if !meta.is_dir() {
                return Err(StoreError::Io);
            }
            match meta.permissions().mode() & 0o777 {
                0o700 => Ok(()),
                0o755 => fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
                    .map_err(|_| StoreError::Io),
                _ => Err(StoreError::Io),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|_| StoreError::Io),
        Err(_) => Err(StoreError::Io),
    }
}

/// Writes `bytes` to a new 0600 temp file in `dir` and fsyncs it. Returns the temp path.
/// On any failure after the file was created, the temp file is removed.
#[cfg(unix)]
fn write_temp_file(dir: &Path, bytes: &[u8]) -> Result<PathBuf, StoreError> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let id = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
    let tmp = dir.join(format!(
        ".{VOICEPRINT_FILE}.tmp-{}-{id}",
        std::process::id()
    ));
    // create_new never follows or reuses an existing path, so a pre-planted name fails here
    // and is not removed by the cleanup below (we did not create it).
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|_| StoreError::Io)?;

    let written = (|| -> std::io::Result<bool> {
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(file.metadata()?.permissions().mode() & 0o777 == 0o600)
    })();
    drop(file);
    match written {
        Ok(true) => Ok(tmp),
        _ => {
            let _ = fs::remove_file(&tmp);
            Err(StoreError::Io)
        }
    }
}

/// fsyncs the directory so the rename is durable.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> Result<(), StoreError> {
    fs::File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|_| StoreError::Io)
}

/// Re-checks that the directory is 0700 and the file is 0600.
#[cfg(unix)]
fn verify_modes(dir: &Path, file: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt;

    let mode_of = |p: &Path| -> Result<u32, StoreError> {
        fs::symlink_metadata(p)
            .map(|m| m.permissions().mode() & 0o777)
            .map_err(|_| StoreError::Io)
    };
    if mode_of(dir)? == 0o700 && mode_of(file)? == 0o600 {
        Ok(())
    } else {
        Err(StoreError::Io)
    }
}

/// False ONLY when the verdict is Reject, the generation is unchanged since
/// `started_generation`, and a voiceprint is still enrolled. Otherwise true.
pub fn accept_after(started_generation: u64, store: &VoiceprintStore, verdict: Verdict) -> bool {
    !(verdict == Verdict::Reject && store.generation() == started_generation && store.is_enrolled())
}

/// How long the capture worker waits for one speaker check before it accepts the utterance.
pub const VERIFY_DEADLINE: std::time::Duration = std::time::Duration::from_millis(500);

/// Outcome of one speaker check: the policy verdict and how long the check took (numbers only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateResult {
    pub verdict: Verdict,
    pub verify_ms: Option<u32>,
}

/// The seam the capture worker uses for the optional speaker filter. Implemented over the
/// inference service by step S8b; faked in tests.
pub trait SpeakerGate: Send + Sync {
    /// `buffer16k` is the VAD-kept 16 kHz mono buffer of ONE utterance (preroll + speech +
    /// hangover). Blocks at most `VERIFY_DEADLINE`. Any failure, timeout, cancellation, missing
    /// model or missing voiceprint must return `Verdict::Accept`. Must never panic and never
    /// keep or log the audio or the embedding.
    fn check(&self, buffer16k: &[f32], cancel: &std::sync::atomic::AtomicBool) -> GateResult;
}

#[cfg(test)]
#[path = "speaker_id_tests.rs"]
mod tests;
