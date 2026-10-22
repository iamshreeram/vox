//! Spec for the macOS Phonon glue (DESIGN.md Rev 4, P3 + P9). Compiled on macOS tests only.
//! (a) runs without a model; (b) `natural_phonon_live_*` is ignored and needs VOX_PHONON_DIR
//! pointing at a folder holding the four pinned files (e.g. /tmp/vox-work/phonon31).
//! Authoritative: do not weaken an assertion; if a test is wrong, report it instead.

use crate::managers::natural_engine::SynthEngine;
use crate::managers::natural_model::{
    phonon_allowed_prefixes, phonon_assets, ModelCoordinator, PHONON_DIR_NAME, READY_MARKER,
};
use crate::managers::natural_phonon::phonon_loader;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tempfile::TempDir;

const SR: f64 = 24_000.0;
const PINNED_FILES: [&str; 4] = [
    "model.q8.gguf",
    "default-voice.safetensors",
    "tokenizer.json",
    "config.json",
];

fn load_err(r: Result<Box<dyn SynthEngine>, String>) -> String {
    match r {
        Ok(_) => panic!("expected the load to fail"),
        Err(e) => e,
    }
}

#[test]
fn phonon_loader_on_an_empty_root_asks_for_reinstall() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("phonon_models");
    let scratch = dir.path().join("scratch");
    fs::create_dir_all(&scratch).unwrap();
    let coord = Arc::new(ModelCoordinator::new_named(
        root,
        PHONON_DIR_NAME,
        phonon_assets(),
        phonon_allowed_prefixes(),
    ));
    let mut loader = phonon_loader(coord, scratch.clone());

    let err = load_err(loader.load());
    assert!(err.contains("reinstall"), "got {err:?}");
    assert_eq!(
        fs::read_dir(&scratch).unwrap().count(),
        0,
        "no leftover snapshot"
    );
}

/// Writes 16-bit PCM mono WAV. Hand-written header, no dependency.
fn write_wav_16(path: &Path, samples: &[f32], sample_rate: u32) -> std::io::Result<()> {
    let data_len = (samples.len() * 2) as u32;
    let mut out: Vec<u8> = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(path, out)
}

/// Copies ONLY the four pinned files and writes READY the way the installer does (an empty file).
fn install_from(source: &Path, root: &Path) -> std::path::PathBuf {
    let install = root.join(PHONON_DIR_NAME);
    fs::create_dir_all(&install).unwrap();
    for name in PINNED_FILES {
        fs::copy(source.join(name), install.join(name))
            .unwrap_or_else(|e| panic!("copy {name} from VOX_PHONON_DIR: {e}"));
    }
    fs::File::create(install.join(READY_MARKER)).unwrap();
    install
}

fn check_finite_and_bounded(pcm: &[f32], what: &str) {
    assert!(
        pcm.iter().all(|x| x.is_finite() && x.abs() <= 1.0),
        "{what}: every sample must be finite and within [-1, 1]"
    );
}

#[test]
#[ignore = "needs VOX_PHONON_DIR"]
fn natural_phonon_live_speaks_and_reverifies() {
    let Some(source) = std::env::var_os("VOX_PHONON_DIR") else {
        println!(
            "SKIPPED: VOX_PHONON_DIR is not set (point it at a folder with the four pinned files)"
        );
        return;
    };
    let source = std::path::PathBuf::from(source);

    let work = TempDir::new().unwrap();
    let root = work.path().join("phonon_models");
    let scratch = work.path().join("scratch");
    fs::create_dir_all(&scratch).unwrap();
    install_from(&source, &root);
    let coord = Arc::new(ModelCoordinator::new_named(
        root,
        PHONON_DIR_NAME,
        phonon_assets(),
        phonon_allowed_prefixes(),
    ));

    // First loader: load, synthesize, check the audio.
    let mut loader = phonon_loader(coord.clone(), scratch.clone());
    let load_start = Instant::now();
    let mut engine = match loader.load() {
        Ok(engine) => engine,
        Err(e) => panic!("live load failed: {e}"),
    };
    println!("load_ms={:.0}", load_start.elapsed().as_secs_f64() * 1000.0);

    let first_start = Instant::now();
    let pcm = engine
        .synth("Hello from Phonon.", "default", 1.0)
        .expect("first synth must succeed");
    println!(
        "first_synth_ms={:.0}",
        first_start.elapsed().as_secs_f64() * 1000.0
    );

    assert!(!pcm.is_empty(), "first utterance produced no audio");
    check_finite_and_bounded(&pcm, "first utterance");
    let duration = pcm.len() as f64 / SR;
    println!("duration_s={duration:.3} samples={}", pcm.len());
    assert!(
        (1.0..=6.0).contains(&duration),
        "duration {duration:.3} s outside 1.0..=6.0 s"
    );
    let rms = (pcm
        .iter()
        .map(|&x| f64::from(x) * f64::from(x))
        .sum::<f64>()
        / pcm.len() as f64)
        .sqrt();
    println!("rms={rms:.5}");
    assert!(
        rms > 0.005,
        "rms {rms:.5} is too quiet: the audio is not voiced"
    );
    let peak = pcm.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
    println!("peak={peak:.4}");
    let clipped = pcm.iter().filter(|x| x.abs() >= 0.999).count();
    assert!(
        (clipped as f64) < pcm.len() as f64 * 0.01,
        "{clipped} samples are at full scale: the peak is clipped flat"
    );

    let wav = std::env::temp_dir().join("vox-phonon-live.wav");
    write_wav_16(&wav, &pcm, SR as u32).expect("write the live wav");
    println!("wav={}", wav.display());

    // Second call on the same engine: timed.
    let second_start = Instant::now();
    let pcm2 = engine
        .synth("Hello from Phonon.", "default", 1.0)
        .expect("second synth must succeed");
    let second_s = second_start.elapsed().as_secs_f64();
    println!("second_synth_s={second_s:.3}");
    assert!(!pcm2.is_empty());
    check_finite_and_bounded(&pcm2, "second utterance");
    assert!(
        second_s < 3.0,
        "second synth took {second_s:.3} s (limit 3.0 s)"
    );

    // Edge inputs on the real engine.
    let empty = engine
        .synth("", "default", 1.0)
        .expect("empty text must not error");
    assert!(empty.is_empty(), "empty text gives no audio");
    let emoji = engine
        .synth("\u{1F600}", "default", 1.0)
        .expect("emoji-only text must not error");
    assert!(emoji.is_empty(), "emoji-only text gives no audio");

    let paragraph_unit = "The quick brown fox jumps over the lazy dog while the engineers review the release notes. ";
    let paragraph: String = paragraph_unit.repeat(8).chars().take(600).collect();
    assert_eq!(paragraph.chars().count(), 600);
    let long = engine
        .synth(&paragraph, "default", 1.0)
        .expect("600-character paragraph must synthesize");
    assert!(!long.is_empty(), "paragraph produced no audio");
    check_finite_and_bounded(&long, "paragraph");
    println!(
        "paragraph_samples={} paragraph_s={:.3}",
        long.len(),
        long.len() as f64 / SR
    );

    drop(engine);
    drop(loader);

    // A fresh loader re-verifies the install and leaves no snapshot behind.
    let mut fresh = phonon_loader(coord, scratch.clone());
    let mut engine2 = match fresh.load() {
        Ok(engine) => engine,
        Err(e) => panic!("second live load (fresh loader) failed: {e}"),
    };
    let again = engine2
        .synth("Hello from Phonon.", "default", 1.0)
        .expect("synth after a fresh load must succeed");
    assert!(!again.is_empty());
    drop(engine2);
    drop(fresh);
    assert_eq!(
        fs::read_dir(&scratch).unwrap().count(),
        0,
        "scratch dir must be empty after loading"
    );
}
