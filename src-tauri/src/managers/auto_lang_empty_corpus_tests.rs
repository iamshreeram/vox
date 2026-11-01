//! Corpus gate for the empty-text path (DESIGN Revision 6, R6-2).
//!
//! Ignored live test. It reads the clips written by `scripts/make_empty_corpus.py`, runs
//! each one through the REAL speech gate (`EarshotMeter`) and, when admitted, through the
//! REAL installed Whisper identifier (`WhisperLid`) and `accept_empty_lid`. It prints a table
//! and asserts the ship gate. No transcript text is ever printed: only letter and word counts.
//!
//! Run after generating the corpus:
//!   python3 scripts/make_empty_corpus.py /tmp/vox-work/empty_corpus
//!   cargo test --lib auto_lang_empty_corpus -- --ignored --nocapture
//! Set `VOX_CORPUS_REPORT=<path>` to also write the table as markdown.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::auto_lang::{letter_count, LID_MAX_SECS, SAMPLE_RATE_HZ};
use super::auto_lang_empty::{
    accept_empty_lid, EarshotMeter, SpeechMeter, EMPTY_MIN_AUDIO_SECS, EMPTY_MIN_SPEECH_SECS,
};
use super::lid_slot::LidSession;
use super::lid_whisper::WhisperLid;

/// Positives at or above this gain are gated; lower gains are only reported.
const GATED_MIN_GAIN: f64 = 0.1;
const CORPUS_DIR: &str = "/tmp/vox-work/empty_corpus";
const WHISPER_MODEL_FILE: &str = "whisper-large-v3-turbo-Q5_K_M.gguf";

/// One manifest entry.
struct Clip {
    path: PathBuf,
    group: String,
    kind: String,
    gain: f64,
    expected: Option<String>,
}

/// One printed row: the outcome of a single clip.
struct Row {
    group: String,
    kind: String,
    gain: f64,
    speech_s: f32,
    admitted: bool,
    lid_lang: String,
    accepted: bool,
    letters: usize,
    words: usize,
    expected: String,
    matched: Option<bool>,
}

fn text_field<'a>(item: &'a Value, key: &str) -> &'a str {
    item.get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("manifest entry lacks string field `{key}`"))
}

fn read_manifest(path: &Path) -> Vec<Clip> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|error| {
        panic!(
            "cannot read {}: {error} (run scripts/make_empty_corpus.py first)",
            path.display()
        )
    });
    let value: Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("manifest is not valid JSON: {error}"));
    let items = value
        .as_array()
        .unwrap_or_else(|| panic!("manifest must be a JSON array"));
    items
        .iter()
        .map(|item| Clip {
            path: PathBuf::from(text_field(item, "path")),
            group: text_field(item, "group").to_string(),
            kind: text_field(item, "kind").to_string(),
            gain: item
                .get("gain")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("manifest entry lacks numeric `gain`")),
            expected: item
                .get("expected_lang")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
        .collect()
}

/// Parses a 16 kHz mono 16-bit PCM WAV into f32 samples in [-1, 1].
fn read_wav_mono_16k(path: &Path) -> Result<Vec<f32>, String> {
    let file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut wav = hound::WavReader::new(BufReader::new(file))
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let spec = wav.spec();
    let is_expected_format = spec.channels == 1
        && spec.sample_rate == 16_000
        && spec.bits_per_sample == 16
        && spec.sample_format == hound::SampleFormat::Int;
    if !is_expected_format {
        return Err(format!(
            "{}: expected 16 kHz mono 16-bit PCM, got {spec:?}",
            path.display()
        ));
    }
    wav.samples::<i16>()
        .map(|sample| sample.map(|value| f32::from(value) / 32_768.0))
        .collect::<Result<Vec<f32>, _>>()
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn whisper_model_path() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME is not set");
    PathBuf::from(home)
        .join("Library/Application Support/com.iamshreeram.vox/models")
        .join(WHISPER_MODEL_FILE)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Runs one clip through the real gate and, when admitted, the real identifier.
fn run_clip(clip: &Clip, lid: &mut WhisperLid) -> Row {
    let samples = read_wav_mono_16k(&clip.path).unwrap_or_else(|error| panic!("{error}"));
    let clip_secs = samples.len() as f32 / SAMPLE_RATE_HZ;
    let speech_s = EarshotMeter.speech_seconds(&samples);
    let admitted = speech_s >= EMPTY_MIN_SPEECH_SECS && clip_secs >= EMPTY_MIN_AUDIO_SECS;

    let mut row = Row {
        group: clip.group.clone(),
        kind: clip.kind.clone(),
        gain: clip.gain,
        speech_s,
        admitted,
        lid_lang: "-".to_string(),
        accepted: false,
        letters: 0,
        words: 0,
        expected: clip.expected.clone().unwrap_or_else(|| "-".to_string()),
        matched: None,
    };
    if !admitted {
        return row;
    }

    // The production path passes at most LID_MAX_SECS of audio (see `SlotLid` in transcription.rs).
    let cap = (LID_MAX_SECS * SAMPLE_RATE_HZ) as usize;
    let capped = &samples[..samples.len().min(cap)];
    match lid.identify(capped) {
        Ok(result) => {
            row.accepted = accept_empty_lid(&result);
            row.lid_lang = result.lang.clone().unwrap_or_else(|| "none".to_string());
            row.letters = letter_count(&result.text);
            row.words = result.text.split_whitespace().count();
        }
        Err(error) => {
            row.lid_lang = "error".to_string();
            println!("identify failed for {}: {error}", file_name(&clip.path));
        }
    }
    if let Some(expected) = &clip.expected {
        row.matched = Some(row.lid_lang == *expected);
    }
    row
}

fn print_table(rows: &[Row]) {
    println!(
        "{:<5} {:<18} {:>5} {:>8} {:<8} {:<7} {:<8} {:>7} {:>5} {:<8} {:<7}",
        "group",
        "kind",
        "gain",
        "speech_s",
        "admitted",
        "lid",
        "accepted",
        "letters",
        "words",
        "expected",
        "match"
    );
    for row in rows {
        let matched = match row.matched {
            Some(true) => "yes",
            Some(false) => "NO",
            None => "-",
        };
        println!(
            "{:<5} {:<18} {:>5} {:>8.1} {:<8} {:<7} {:<8} {:>7} {:>5} {:<8} {:<7}",
            row.group,
            row.kind,
            row.gain,
            row.speech_s,
            row.admitted,
            row.lid_lang,
            row.accepted,
            row.letters,
            row.words,
            row.expected,
            matched
        );
    }
}

fn markdown_table(rows: &[Row]) -> String {
    let mut text = String::from(
        "| group | kind | gain | speech_s | admitted | lid_lang | accepted | letters | words | expected | match |\n\
         |---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for row in rows {
        let matched = match row.matched {
            Some(true) => "yes",
            Some(false) => "NO",
            None => "-",
        };
        text.push_str(&format!(
            "| {} | {} | {} | {:.1} | {} | {} | {} | {} | {} | {} | {} |\n",
            row.group,
            row.kind,
            row.gain,
            row.speech_s,
            row.admitted,
            row.lid_lang,
            row.accepted,
            row.letters,
            row.words,
            row.expected,
            matched
        ));
    }
    text
}

#[test]
#[ignore = "live: needs /tmp/vox-work/empty_corpus (run scripts/make_empty_corpus.py) and the installed Whisper Turbo model"]
fn auto_lang_empty_corpus() {
    let corpus_dir =
        PathBuf::from(std::env::var("VOX_CORPUS_DIR").unwrap_or_else(|_| CORPUS_DIR.to_string()));
    let clips = read_manifest(&corpus_dir.join("manifest.json"));
    assert!(!clips.is_empty(), "manifest lists no clips");

    crate::managers::transcription::init_transcribe_backend();
    let model_path = whisper_model_path();
    let mut lid = WhisperLid::load(&model_path).unwrap_or_else(|error| {
        panic!(
            "cannot load the installed Whisper model {}: {error}",
            model_path.display()
        )
    });

    let rows: Vec<Row> = clips.iter().map(|clip| run_clip(clip, &mut lid)).collect();
    print_table(&rows);

    if let Ok(report_path) = std::env::var("VOX_CORPUS_REPORT") {
        std::fs::write(&report_path, markdown_table(&rows))
            .unwrap_or_else(|error| panic!("cannot write report {report_path}: {error}"));
    }

    let negatives: Vec<&Row> = rows.iter().filter(|row| row.group == "neg").collect();
    let positives: Vec<&Row> = rows.iter().filter(|row| row.group == "pos").collect();
    let neg_admitted = negatives.iter().filter(|row| row.admitted).count();
    let neg_accepted: Vec<String> = negatives
        .iter()
        .filter(|row| row.accepted)
        .map(|row| format!("{} gain {}", row.kind, row.gain))
        .collect();
    let pos_admitted = positives.iter().filter(|row| row.admitted).count();
    let pos_accepted = positives.iter().filter(|row| row.accepted).count();
    let pos_correct = positives
        .iter()
        .filter(|row| row.matched == Some(true))
        .count();
    println!(
        "SUMMARY negatives={} admitted={} accepted={}",
        negatives.len(),
        neg_admitted,
        neg_accepted.len()
    );
    println!(
        "SUMMARY positives={} admitted={} accepted={} language_correct={}",
        positives.len(),
        pos_admitted,
        pos_accepted,
        pos_correct
    );

    // SHIP GATE (a): no negative may be accepted.
    assert!(
        neg_accepted.is_empty(),
        "SHIP GATE (a) failed: negatives accepted as speech: {neg_accepted:?}"
    );

    // SHIP GATE (b): gated positives must be admitted, accepted, and in the right language.
    let mut failures: Vec<String> = Vec::new();
    for row in &positives {
        if row.gain + 1e-9 < GATED_MIN_GAIN {
            continue;
        }
        if !row.admitted {
            failures.push(format!("{} gain {}: not admitted", row.kind, row.gain));
        } else if !row.accepted {
            failures.push(format!("{} gain {}: not accepted", row.kind, row.gain));
        } else if row.matched != Some(true) {
            failures.push(format!(
                "{} gain {}: language {} expected {}",
                row.kind, row.gain, row.lid_lang, row.expected
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "SHIP GATE (b) failed for positives at gain >= {GATED_MIN_GAIN}: {failures:?}"
    );
}
