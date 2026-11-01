//! Real language identifier: an installed multilingual Whisper model run through transcribe-cpp.
//!
//! Loads the same way production loads a Whisper GGUF (`transcription.rs`,
//! `load_model_with_device`): `Model::load_with` with `ModelOptions`, then `model.session()`.
//! The backend is `Backend::Auto` with no exact device, which is what production selects under
//! the default accelerator setting. Backends are initialised once at app start-up
//! (`init_transcribe_backend` in `lib.rs`), so the loader does not repeat that.
//!
//! The session is run with no language hint, so the model reports the language it hears.
//! Used by `LidSlot` through [`whisper_lid_loader`].

use std::path::{Path, PathBuf};

use transcribe_cpp::{Model, ModelOptions, RunOptions, Session, Task};

use super::auto_lang::LidResult;
use super::lid_slot::{LidLoader, LidSession};

/// A loaded multilingual Whisper session used for language identification.
///
/// The `Session` keeps its `Model` alive, so only the session is stored.
pub struct WhisperLid {
    session: Session,
}

impl WhisperLid {
    /// Loads the model at `model_path` and opens a session on it. A missing file or a load
    /// failure is returned as text, never a panic.
    pub fn load(model_path: &Path) -> Result<WhisperLid, String> {
        if !model_path.is_file() {
            return Err(format!(
                "language model not found: {}",
                model_path.display()
            ));
        }
        let options = ModelOptions {
            backend: transcribe_cpp::Backend::Auto,
            device: None,
        };
        let model = Model::load_with(model_path, &options).map_err(|error| {
            format!(
                "failed to load language model {}: {error}",
                model_path.display()
            )
        })?;
        let session = model
            .session()
            .map_err(|error| format!("failed to open language session: {error}"))?;
        Ok(WhisperLid { session })
    }
}

impl LidSession for WhisperLid {
    fn identify(&mut self, samples: &[f32]) -> Result<LidResult, String> {
        if samples.is_empty() {
            return Err("empty audio".to_string());
        }
        let options = RunOptions {
            task: Task::Transcribe,
            language: None,
            ..Default::default()
        };
        let transcript = self
            .session
            .run(samples, &options)
            .map_err(|error| format!("language identification failed: {error}"))?;
        Ok(LidResult {
            lang: normalise_lang(transcript.language.as_deref()),
            text: transcript.text.trim().to_string(),
        })
    }
}

/// Lower-cases and trims the detected code. An empty code becomes `None`.
fn normalise_lang(raw: Option<&str>) -> Option<String> {
    raw.map(|code| code.trim().to_lowercase())
        .filter(|code| !code.is_empty())
}

/// A loader for [`LidSlot`] that opens the Whisper model at `model_path` on first use.
pub fn whisper_lid_loader(model_path: PathBuf) -> LidLoader {
    Box::new(move || {
        let session = WhisperLid::load(&model_path)?;
        Ok(Box::new(session) as Box<dyn LidSession>)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_model_file_is_an_error_not_a_panic() {
        let loader = whisper_lid_loader(PathBuf::from("/nonexistent/vox-lid-model.gguf"));
        let result = loader();
        let message = match result {
            Ok(_) => panic!("a missing model must not load"),
            Err(message) => message,
        };
        assert!(message.contains("not found"), "{message}");
    }

    #[test]
    fn normalise_lang_lowercases_trims_and_drops_empty() {
        assert_eq!(normalise_lang(Some(" TA ")), Some("ta".to_string()));
        assert_eq!(normalise_lang(Some("   ")), None);
        assert_eq!(normalise_lang(None), None);
    }
}

#[cfg(test)]
mod live_tests {
    //! Measurement only (no accuracy assertions). Ignored by default; needs:
    //!   VOX_LID_MODEL     path to the whisper GGUF
    //!   VOX_LID_MANIFEST  TSV: wav_path<TAB>expected_lang (16 kHz mono 16-bit PCM)

    use super::super::auto_lang::LidOutcome;
    use super::super::indic_stt_slot::{SystemClock, UnloadPolicy};
    use super::super::lid_slot::LidSlot;
    use super::*;
    use std::collections::BTreeMap;
    use std::fs::File;
    use std::io::BufReader;
    use std::sync::Arc;
    use std::time::Instant;

    /// Parses a 16 kHz mono 16-bit PCM WAV into f32 samples in [-1, 1].
    fn read_wav_mono_16k(path: &Path) -> Result<Vec<f32>, String> {
        let file = File::open(path).map_err(|error| format!("{path:?}: {error}"))?;
        let mut wav = hound::WavReader::new(BufReader::new(file))
            .map_err(|error| format!("{path:?}: {error}"))?;
        let spec = wav.spec();
        let is_expected_format = spec.channels == 1
            && spec.sample_rate == 16_000
            && spec.bits_per_sample == 16
            && spec.sample_format == hound::SampleFormat::Int;
        if !is_expected_format {
            return Err(format!(
                "{path:?}: expected 16 kHz mono 16-bit PCM, got {spec:?}"
            ));
        }
        wav.samples::<i16>()
            .map(|sample| sample.map(|value| f32::from(value) / 32_768.0))
            .collect::<Result<Vec<f32>, _>>()
            .map_err(|error| format!("{path:?}: {error}"))
    }

    fn read_manifest(path: &Path) -> Vec<(PathBuf, String)> {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("cannot read manifest {path:?}: {error}"));
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let (wav, expected) = line
                    .split_once('\t')
                    .unwrap_or_else(|| panic!("manifest line lacks a tab: {line:?}"));
                (PathBuf::from(wav.trim()), expected.trim().to_string())
            })
            .collect()
    }

    #[test]
    #[ignore]
    fn whisper_lid_live_identifies_the_spike_clips() {
        let model_path = PathBuf::from(
            std::env::var("VOX_LID_MODEL").expect("set VOX_LID_MODEL to the whisper GGUF path"),
        );
        let manifest_path = PathBuf::from(
            std::env::var("VOX_LID_MANIFEST").expect("set VOX_LID_MANIFEST to the TSV manifest"),
        );
        crate::managers::transcription::init_transcribe_backend();

        let slot = LidSlot::new(whisper_lid_loader(model_path), Arc::new(SystemClock::new()));
        // (correct, total) per expected language.
        let mut tally: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        let mut warm_ms: Vec<f64> = Vec::new();

        for (index, (wav, expected)) in read_manifest(&manifest_path).into_iter().enumerate() {
            let pcm = read_wav_mono_16k(&wav).unwrap_or_else(|error| panic!("{error}"));
            let name = wav
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let started = Instant::now();
            let outcome = slot.identify(&pcm, UnloadPolicy::Never);
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            let (detected, text) = match outcome {
                LidOutcome::Done(result) => (result.lang, result.text),
                other => (None, format!("{other:?}")),
            };
            let correct = detected.as_deref() == Some(expected.as_str());
            println!(
                "CLIP {name} expected={expected} detected={} correct={correct} ms={ms:.0} text={:?}",
                detected.as_deref().unwrap_or("none"),
                text.chars().take(60).collect::<String>()
            );
            let entry = tally.entry(expected.clone()).or_insert((0, 0));
            entry.1 += 1;
            if correct {
                entry.0 += 1;
            }
            // The first clip includes the model load, so it is left out of the warm median.
            if index > 0 {
                warm_ms.push(ms);
            }
        }

        for (language, (correct, total)) in &tally {
            println!("SUMMARY {language} correct={correct}/{total}");
        }
        warm_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        if let Some(median) = warm_ms.get(warm_ms.len() / 2) {
            println!(
                "SUMMARY warm_median_ms={median:.0} warm_clips={}",
                warm_ms.len()
            );
        }
    }
}
