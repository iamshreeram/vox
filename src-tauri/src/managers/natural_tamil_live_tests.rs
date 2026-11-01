//! Live end-to-end checks for the Tamil voice (DESIGN.md S8). Both tests are `#[ignore]`d: they
//! need the Tamil pack (`VOX_TAMIL_DIR`) and, for the round trip, the installed Whisper
//! large-v3-turbo GGUF. They never run in `cargo test --lib`.
//!
//! Run: `VOX_TAMIL_DIR=<pack> cargo test --lib natural_tamil_live -- --ignored --test-threads=1 --nocapture`

use crate::managers::emotion::Emotion;
use crate::managers::natural_audio::resample_mono;
use crate::managers::natural_engine::{NaturalHealth, SynthEngine, DEFAULT_AUTO_RETRY_COOLDOWN};
use crate::managers::natural_router::{EngineBackend, RouterChoice, RouterConfig};
use crate::managers::natural_sink::{PlaybackCtl, StreamFactory};
use crate::managers::natural_tamil::{
    build_tamil_route, TamilEngine, TAMIL_MODEL_DIRNAME, TAMIL_SAMPLE_RATE,
};
use crate::managers::tamil_pocket_core::GenParams;
use crate::managers::tamil_pocket_ort::PocketModel;
use crate::managers::tts::{SpeechBackend, SpeechHandle};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use transcribe_cpp::{Model, ModelOptions, RunOptions};

/// The six sentences of the quality check, in order (`tamil_01.wav` .. `tamil_06.wav`).
const TAMIL_SENTENCES: [&str; 6] = [
    "வணக்கம், நீங்கள் எப்படி இருக்கிறீர்கள்?",
    "இன்று வானிலை மிகவும் அழகாக இருக்கிறது.",
    "எனக்கு ஒரு கதை சொல்லுங்கள்.",
    "தமிழ் ஒரு பழமையான மொழி.",
    "நாளை காலை ஏழு மணிக்கு சந்திப்போம்.",
    "ஒரு காலத்தில் ஒரு சிறிய கிராமத்தில் ஒரு அன்பான முயல் வாழ்ந்தது.",
];

/// Fixed seed so repeated runs draw the same noise (production uses the clock).
const SYNTH_SEED: u64 = 20_261_031;
/// Whisper expects 16 kHz mono.
const ASR_SAMPLE_RATE: u32 = 16_000;
/// The installed Whisper large-v3-turbo file (catalog entry `handy-computer/whisper-large-v3-turbo-gguf`).
const ASR_MODEL_FILE: &str = "whisper-large-v3-turbo-Q5_K_M.gguf";
/// Overall character error rate must stay below this (catches garbage and silence only).
const CER_LIMIT: f64 = 0.5;
const MIN_AUDIO_SECS: f64 = 0.8;
const MAX_AUDIO_SECS: f64 = 20.0;
/// Every wait in the router test is bounded by this, so the test can never hang.
const ROUTER_DEADLINE: Duration = Duration::from_secs(30);
/// Sentence for the router test (a Tamil burst) and the English follow-up.
const ROUTER_TAMIL: &str = "இன்று வானிலை மிகவும் அழகாக இருக்கிறது.";
const ROUTER_ENGLISH: &str = "The weather is lovely today.";

fn pack_dir() -> Result<PathBuf, String> {
    let dir = std::env::var_os("VOX_TAMIL_DIR")
        .ok_or("VOX_TAMIL_DIR is not set (point it at the pocket-tts-tamil-int4 folder)")?;
    Ok(PathBuf::from(dir))
}

fn output_dir() -> PathBuf {
    std::env::var_os("VOX_TAMIL_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/vox-work/tamil_live"))
}

fn asr_model_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    let path = PathBuf::from(home)
        .join("Library/Application Support/com.iamshreeram.vox/models")
        .join(ASR_MODEL_FILE);
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!("Whisper model not found: {}", path.display()))
    }
}

/// Writes mono 16-bit PCM at the Tamil engine rate (samples are clamped to [-1, 1]).
fn write_wav(path: &Path, samples: &[f32]) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TAMIL_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    for &sample in samples {
        let pcm = (sample.clamp(-1.0, 1.0) * 32_767.0).round() as i16;
        writer
            .write_sample(pcm)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    writer
        .finalize()
        .map_err(|e| format!("cannot finish {}: {e}", path.display()))
}

/// Reads a 16-bit WAV back as f32 in [-1, 1]; returns the samples and the file's rate.
fn read_wav_f32(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let reader =
        hound::WavReader::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let rate = reader.spec().sample_rate;
    let pcm = reader
        .into_samples::<i16>()
        .collect::<Result<Vec<i16>, _>>()
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let samples = pcm.iter().map(|&s| f32::from(s) / 32_768.0).collect();
    Ok((samples, rate))
}

/// Comparison form: lowercase, no whitespace, no `. , ? ! ; :` or ellipsis. NFC is not applied
/// (no normalisation crate is a direct dependency), so both sides are compared as typed.
fn normalise(text: &str) -> Vec<char> {
    text.chars()
        .filter(|c| {
            !c.is_whitespace() && !matches!(c, '.' | ',' | '?' | '!' | ';' | ':' | '\u{2026}')
        })
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Levenshtein distance over Unicode scalar values (two-row dynamic programme).
fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0_usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Edit distance and reference length (in normalised characters) for one sentence pair.
fn edit_counts(reference: &str, hypothesis: &str) -> (usize, usize) {
    let r = normalise(reference);
    let h = normalise(hypothesis);
    (levenshtein(&r, &h), r.len())
}

/// Collapses whitespace so a transcript fits on one table row.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
#[ignore = "needs the Tamil pack and a Whisper model; set VOX_TAMIL_DIR"]
fn tamil_live_wav_and_asr_round_trip() -> Result<(), String> {
    let dir = pack_dir()?;
    let out = output_dir();
    fs::create_dir_all(&out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    println!("output_dir={}", out.display());

    // Sanity: the normaliser drops the listed punctuation and the CER of identical text is 0.
    let (same_edits, same_len) = edit_counts("தமிழ், ஒரு!", "தமிழ் ஒரு");
    assert_eq!(same_edits, 0, "punctuation and spaces are ignored");
    assert!(same_len > 0);

    let load_started = Instant::now();
    let model = PocketModel::load(&dir)?;
    println!("load_s={:.2}", load_started.elapsed().as_secs_f64());
    let PocketModel {
        bundle,
        tokenizer,
        backend,
        voice,
    } = model;
    let mut engine = TamilEngine::from_parts(
        Box::new(backend),
        bundle,
        tokenizer,
        voice,
        GenParams::default(),
        SYNTH_SEED,
    );

    let mut wavs: Vec<PathBuf> = Vec::new();
    println!("tamil_NN | time_to_audio_s | audio_s | rtf | peak");
    for (index, sentence) in TAMIL_SENTENCES.iter().copied().enumerate() {
        let number = index + 1;
        let started = Instant::now();
        let samples = engine.synth(sentence, "", 1.0)?;
        let synth_s = started.elapsed().as_secs_f64();
        let audio_s = samples.len() as f64 / f64::from(TAMIL_SAMPLE_RATE);
        let peak = samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(
            samples.iter().all(|s| s.is_finite()),
            "sentence {number}: non-finite sample"
        );
        assert!(
            (0.05..=1.0).contains(&peak),
            "sentence {number}: peak {peak:.3} outside 0.05..=1.0"
        );
        assert!(
            (MIN_AUDIO_SECS..=MAX_AUDIO_SECS).contains(&audio_s),
            "sentence {number}: duration {audio_s:.2} s outside {MIN_AUDIO_SECS}..={MAX_AUDIO_SECS} s"
        );
        let rtf = synth_s / audio_s;
        println!("tamil_{number:02} | {synth_s:.2} | {audio_s:.2} | {rtf:.3} | {peak:.3}");
        let path = out.join(format!("tamil_{number:02}.wav"));
        write_wav(&path, &samples)?;
        wavs.push(path);
    }

    // Round trip: the same Whisper model, the same transcribe-cpp path the app uses, language "ta".
    let asr_path = asr_model_path()?;
    let asr = Model::load_with(&asr_path, &ModelOptions::default())
        .map_err(|e| format!("cannot load the Whisper model: {e}"))?;
    let mut session = asr
        .session()
        .map_err(|e| format!("cannot open a Whisper session: {e}"))?;
    let options = RunOptions {
        language: Some("ta".to_string()),
        ..RunOptions::default()
    };

    println!("sentence | reference | Whisper text | CER");
    let (mut total_edits, mut total_ref) = (0_usize, 0_usize);
    for (index, (sentence, wav)) in TAMIL_SENTENCES.iter().copied().zip(wavs.iter()).enumerate() {
        let number = index + 1;
        let (pcm, rate) = read_wav_f32(wav)?;
        if rate != TAMIL_SAMPLE_RATE {
            return Err(format!(
                "tamil_{number:02}.wav has rate {rate}, expected {TAMIL_SAMPLE_RATE}"
            ));
        }
        let pcm16 = resample_mono(&pcm, rate, ASR_SAMPLE_RATE)?;
        let transcript = session
            .run(&pcm16, &options)
            .map_err(|e| format!("Whisper failed on tamil_{number:02}.wav: {e}"))?;
        let (edits, ref_len) = edit_counts(sentence, &transcript.text);
        total_edits += edits;
        total_ref += ref_len;
        let cer = edits as f64 / ref_len.max(1) as f64;
        println!(
            "{number} | {} | {} | {cer:.3}",
            one_line(sentence),
            one_line(&transcript.text)
        );
    }
    let overall = total_edits as f64 / total_ref.max(1) as f64;
    println!("overall_cer={overall:.3} edits={total_edits} ref_chars={total_ref}");
    println!("output_dir={}", out.display());
    assert!(
        overall < CER_LIMIT,
        "overall CER {overall:.3} is not below {CER_LIMIT}"
    );
    Ok(())
}

/// Sink that stands in for the audio device. Each open starts a thread that pulls
/// `PlaybackCtl::render` in real time and keeps everything it rendered.
struct CaptureFactory {
    rate: u32,
    captured: Arc<Mutex<Vec<f32>>>,
}

/// Closes the capture thread when the stream is dropped.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl StreamFactory for CaptureFactory {
    fn device_rate(&self) -> Result<u32, String> {
        Ok(self.rate)
    }

    fn open(&self, ctl: Arc<PlaybackCtl>) -> Result<Box<dyn Send>, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (flag, sink) = (stop.clone(), self.captured.clone());
        std::thread::spawn(move || {
            let mut frame = vec![0.0_f32; 240];
            while !flag.load(Ordering::SeqCst) {
                ctl.render(&mut frame, Instant::now());
                sink.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend_from_slice(&frame);
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Ok(Box::new(StopOnDrop(stop)))
    }
}

/// Counts non-zero samples: silence rendered while no stream carries audio does not count.
fn nonzero_count(captured: &Mutex<Vec<f32>>) -> usize {
    captured
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|s| **s != 0.0)
        .count()
}

/// Polls `done` until it returns true or `limit` passes. Returns the final answer.
fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

/// Fake system voice: counts every spawn and never makes sound.
struct FakeSystem {
    calls: Arc<AtomicUsize>,
}

struct FakeHandle;

impl SpeechHandle for FakeHandle {
    fn kill(&mut self) -> bool {
        true
    }
    fn is_running(&mut self) -> bool {
        false
    }
}

impl SpeechBackend for FakeSystem {
    fn spawn(
        &self,
        _text: &str,
        _voice: Option<&str>,
        _rate_wpm: Option<u32>,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeHandle))
    }
}

/// No Natural or Phonon peer is selected: the Tamil route is the only peer in play.
fn no_peer_config() -> RouterConfig {
    RouterConfig {
        choice: Arc::new(|| RouterChoice {
            natural: false,
            voice: String::new(),
            phonon: false,
        }),
        model_ready: Arc::new(|| false),
        phonon_ready: Arc::new(|| false),
    }
}

#[test]
#[ignore = "needs the Tamil pack; set VOX_TAMIL_DIR"]
fn tamil_live_router_end_to_end() -> Result<(), String> {
    let dir = pack_dir()?;
    if dir.file_name().and_then(|name| name.to_str()) != Some(TAMIL_MODEL_DIRNAME) {
        return Err(format!(
            "VOX_TAMIL_DIR must be the {TAMIL_MODEL_DIRNAME} folder itself, got {}",
            dir.display()
        ));
    }
    let root = dir
        .parent()
        .ok_or("VOX_TAMIL_DIR has no parent folder")?
        .to_path_buf();
    let audio_needed = (MIN_AUDIO_SECS * f64::from(TAMIL_SAMPLE_RATE)) as usize;

    let captured = Arc::new(Mutex::new(Vec::new()));
    let factory = Arc::new(CaptureFactory {
        rate: TAMIL_SAMPLE_RATE,
        captured: captured.clone(),
    });
    let route = build_tamil_route(&root, factory, Arc::new(|| true));

    let system_calls = Arc::new(AtomicUsize::new(0));
    let router = EngineBackend::new(
        Box::new(FakeSystem {
            calls: system_calls.clone(),
        }),
        Box::new(FakeSystem {
            calls: system_calls.clone(),
        }),
        None,
        NaturalHealth::new(DEFAULT_AUTO_RETRY_COOLDOWN),
        no_peer_config(),
    )
    .with_tamil_route(route);

    // Tamil burst: must go to the Tamil route, never to the fake system.
    let mut tamil = router
        .spawn_styled(ROUTER_TAMIL, None, None, Emotion::Neutral)
        .map_err(|e| format!("the Tamil spawn was refused: {e}"))?;
    let spoken = wait_until(ROUTER_DEADLINE, || {
        nonzero_count(&captured) >= audio_needed && !tamil.is_running()
    });
    let tamil_audio = nonzero_count(&captured);
    if !spoken {
        return Err(format!(
            "no complete Tamil utterance within 30 s (non-zero samples: {tamil_audio})"
        ));
    }
    let system_after_tamil = system_calls.load(Ordering::SeqCst);
    println!(
        "router tamil_nonzero_samples={tamil_audio} tamil_audio_s={:.2} system_calls={system_after_tamil}",
        tamil_audio as f64 / f64::from(TAMIL_SAMPLE_RATE)
    );
    if system_after_tamil != 0 {
        return Err(format!(
            "the Tamil sentence reached the system voice ({system_after_tamil} calls)"
        ));
    }
    let voiced: Vec<f32> = captured
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .copied()
        .filter(|s| *s != 0.0)
        .collect();
    if !voiced.iter().all(|s| s.is_finite()) {
        return Err("the captured Tamil audio has non-finite samples".to_string());
    }

    // English burst: must reach the fake system and add no Tamil audio.
    let before = nonzero_count(&captured);
    let mut english = router
        .spawn(ROUTER_ENGLISH, None, None)
        .map_err(|e| format!("the English spawn was refused: {e}"))?;
    std::thread::sleep(Duration::from_millis(500));
    let after = nonzero_count(&captured);
    let system_after_english = system_calls.load(Ordering::SeqCst);
    println!(
        "router english_system_calls={system_after_english} english_new_nonzero_samples={}",
        after.saturating_sub(before)
    );
    let _ = english.kill();
    let _ = tamil.kill();
    drop(router);
    if system_after_english != 1 {
        return Err(format!(
            "the English sentence should reach the system voice once, got {system_after_english}"
        ));
    }
    if after != before {
        return Err(format!(
            "the English sentence produced Tamil audio ({} new non-zero samples)",
            after - before
        ));
    }
    Ok(())
}
