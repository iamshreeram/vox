//! Tests for the NeMo-compatible log-mel front end (`indic_stt_mel.rs`).
//!
//! The golden is `tests/fixtures/tamil_stt/mel_golden.json`, produced by
//! `scripts/tamil_stt_golden.py` from the TorchScript preprocessor of the IndicConformer pack.
//! The test signals are regenerated here from the same 32-bit LCG, so no audio is stored.
//! Nothing in this file needs the model pack.

use crate::managers::indic_stt_mel::{
    hann_window, mel_filterbank, MelFeatures, MelFrontend, MelScale, PadMode, N_MELS,
};
use serde_json::Value;

const GOLDEN: &str = include_str!("../../tests/fixtures/tamil_stt/mel_golden.json");

/// Tolerance for every golden value except the silence case (see `SILENCE_TOL`).
const TOL: f32 = 1e-4;
/// Tolerance for the silence case. Torch's f32 mean of a constant row is one ulp off, which
/// gives -0.007505 where the exact answer is 0. A float64 implementation returns 0.
const SILENCE_TOL: f32 = 1e-2;
/// Mutants must move the golden comparison by more than this.
const MUTANT_MIN_ERR: f32 = 1e-3;
/// Filterbank checks (band sums and complete bands).
const FB_TOL: f32 = 1e-5;

fn golden() -> Value {
    serde_json::from_str(GOLDEN).expect("mel_golden.json parses")
}

fn case<'a>(g: &'a Value, name: &str) -> &'a Value {
    g["cases"]
        .get(name)
        .unwrap_or_else(|| panic!("golden case {name} missing"))
}

/// Regenerate the LCG noise used by the golden script (f32 arithmetic throughout).
fn lcg_signal(seed: u32, n: usize, sine: bool) -> Vec<f32> {
    let mut x = seed;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let mut s = ((x >> 8) as f32 / 16_777_216.0_f32 - 0.5_f32) * 0.2_f32;
        if sine {
            let t = i as f64 / 16000.0;
            let v = (0.5_f64 * (2.0 * std::f64::consts::PI * 220.0 * t).sin()) as f32;
            s += v;
        }
        out.push(s);
    }
    out
}

/// Rebuild the input signal described by a golden case's `spec`.
fn signal_for(c: &Value) -> Vec<f32> {
    let spec = &c["spec"];
    let n = spec["n"].as_u64().expect("n") as usize;
    match spec["kind"].as_str().expect("kind") {
        "silence" => vec![0.0_f32; n],
        "impulse" => {
            let mut x = vec![0.0_f32; n];
            let idx = spec["index"].as_u64().expect("index") as usize;
            x[idx] = 0.5_f32;
            x
        }
        _ => {
            let seed = spec["seed"].as_u64().expect("seed") as u32;
            let sine = spec.get("sine").and_then(Value::as_bool).unwrap_or(false);
            lcg_signal(seed, n, sine)
        }
    }
}

/// Value of band `b` at frame `t` in band-major features.
fn at(f: &MelFeatures, b: usize, t: usize) -> f32 {
    f.data[b * f.frames + t]
}

/// Max abs error between the stored golden frames and the features, over all stored bands.
/// Returns an error string if the frame count or shape does not match.
fn max_err_vs_golden(f: &MelFeatures, c: &Value) -> Result<f32, String> {
    let frames = c["frames"].as_u64().expect("frames") as usize;
    if f.frames != frames {
        return Err(format!("frames {} != golden {}", f.frames, frames));
    }
    if f.data.len() != N_MELS * frames {
        return Err(format!("data len {} != {}", f.data.len(), N_MELS * frames));
    }
    let indices = c["frame_indices"].as_array().expect("frame_indices");
    let values = c["values"].as_array().expect("values");
    let mut worst = 0.0_f32;
    for (k, idx) in indices.iter().enumerate() {
        let t = idx.as_u64().expect("index") as usize;
        let row = values[k].as_array().expect("row");
        for (b, v) in row.iter().enumerate() {
            let g = v.as_f64().expect("value") as f32;
            worst = worst.max((at(f, b, t) - g).abs());
        }
    }
    Ok(worst)
}

/// Max abs error of the whole-tensor statistics (mean, population std, min, max).
fn stats_err(f: &MelFeatures, c: &Value) -> f32 {
    let n = f.data.len() as f64;
    let mean = f.data.iter().map(|&v| v as f64).sum::<f64>() / n;
    let var = f
        .data
        .iter()
        .map(|&v| (v as f64 - mean).powi(2))
        .sum::<f64>()
        / n;
    let min = f.data.iter().fold(f32::INFINITY, |a, &v| a.min(v)) as f64;
    let max = f.data.iter().fold(f32::NEG_INFINITY, |a, &v| a.max(v)) as f64;
    let s = &c["stats"];
    let errs = [
        (mean - s["mean"].as_f64().expect("mean")).abs(),
        (var.sqrt() - s["std_population"].as_f64().expect("std")).abs(),
        (min - s["min"].as_f64().expect("min")).abs(),
        (max - s["max"].as_f64().expect("max")).abs(),
    ];
    errs.iter().fold(0.0_f64, |a, &e| a.max(e)) as f32
}

fn assert_golden(name: &str, tol: f32, f: &MelFeatures) {
    let g = golden();
    let c = case(&g, name);
    let err = max_err_vs_golden(f, c).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert!(err <= tol, "{name}: max abs err {err} > {tol}");
    let serr = stats_err(f, c);
    assert!(serr <= tol, "{name}: stats err {serr} > {tol}");
}

fn features_of(name: &str) -> MelFeatures {
    let g = golden();
    let c = case(&g, name);
    MelFrontend::new()
        .features(&signal_for(c))
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

// ---- golden comparisons -----------------------------------------------------------------

#[test]
fn golden_silence_matches_torch_within_documented_tolerance() {
    let f = features_of("silence");
    assert_golden("silence", SILENCE_TOL, &f);
    assert!(
        f.data.iter().all(|v| v.is_finite()),
        "silence must be finite"
    );
    assert!(
        f.data.iter().all(|v| v.abs() <= SILENCE_TOL),
        "silence rows must be zero-ish"
    );
}

#[test]
fn golden_impulse_start_matches_torch() {
    assert_golden("impulse_start", TOL, &features_of("impulse_start"));
}

#[test]
fn golden_impulse_mid_matches_torch() {
    assert_golden("impulse_mid", TOL, &features_of("impulse_mid"));
}

#[test]
fn golden_impulse_end_matches_torch() {
    assert_golden("impulse_end", TOL, &features_of("impulse_end"));
}

#[test]
fn golden_noise_05s_matches_torch_all_bands() {
    assert_golden("noise_05s", TOL, &features_of("noise_05s"));
}

#[test]
fn golden_hop_aligned_and_edge_lengths_match_torch() {
    for name in ["len_400", "len_401", "len_479", "len_480"] {
        assert_golden(name, TOL, &features_of(name));
    }
}

#[test]
fn golden_noise_sine_3s_matches_torch() {
    assert_golden("noise_sine_3s", TOL, &features_of("noise_sine_3s"));
}

/// N = 399 is below the public minimum (N < 400 is an Err), so the golden is checked through
/// the internal path that has the same maths. The public call must reject it.
#[test]
fn len_399_maths_matches_torch_through_internal_path() {
    let g = golden();
    let c = case(&g, "len_399");
    let fe = MelFrontend::new()
        .compute(&signal_for(c), PadMode::Reflect)
        .expect("internal path accepts 399");
    assert_golden("len_399", TOL, &fe);
}

#[test]
fn public_api_rejects_len_399_as_too_short() {
    let g = golden();
    let c = case(&g, "len_399");
    let err = MelFrontend::new()
        .features(&signal_for(c))
        .expect_err("N=399 must be Err");
    assert!(err.contains("too short"), "unexpected error: {err}");
}

// ---- filterbank -------------------------------------------------------------------------

#[test]
fn filterbank_band_sums_match_torch() {
    let g = golden();
    let sums = g["filterbank"]["band_sums"].as_array().expect("band_sums");
    assert_eq!(sums.len(), N_MELS);
    let fe = MelFrontend::new();
    let fb = fe.filterbank();
    assert_eq!(fb.len(), 257 * N_MELS);
    for (b, s) in sums.iter().enumerate() {
        let expected = s.as_f64().expect("sum") as f32;
        let got: f32 = (0..257).map(|k| fb[k * N_MELS + b]).sum();
        assert!(
            (got - expected).abs() <= FB_TOL,
            "band {b} sum {got} != {expected}"
        );
    }
}

#[test]
fn formula_filterbank_band_sums_match_torch() {
    let g = golden();
    let sums = g["filterbank"]["band_sums"].as_array().expect("band_sums");
    let fb = mel_filterbank(MelScale::Slaney);
    for (b, s) in sums.iter().enumerate() {
        let expected = s.as_f64().expect("sum") as f32;
        let got: f32 = (0..257).map(|k| fb[k * N_MELS + b]).sum();
        assert!(
            (got - expected).abs() <= FB_TOL,
            "formula band {b} sum {got} != {expected}"
        );
    }
}

/// The slaney formula reproduces the shipped filterbank to 1e-6 (the spec gate). It is not
/// bit-exact, and the 6.9e-8 gap alone moves short-clip features by about 1e-4, so the
/// front end ships the pack's weights.
#[test]
fn formula_filterbank_matches_shipped_within_1e6() {
    let shipped = MelFrontend::new();
    let formula = mel_filterbank(MelScale::Slaney);
    let worst = shipped
        .filterbank()
        .iter()
        .zip(&formula)
        .fold(0.0_f32, |a, (&x, &y)| a.max((x - y).abs()));
    assert!(worst <= 1e-6, "formula vs shipped max diff {worst}");
}

#[test]
fn filterbank_complete_bands_match_torch() {
    let g = golden();
    let fe = MelFrontend::new();
    let fb = fe.filterbank();
    let bands = g["filterbank"]["bands"].as_object().expect("bands");
    assert_eq!(bands.len(), 4);
    for (key, col) in bands {
        let b: usize = key.parse().expect("band index");
        let col = col.as_array().expect("column");
        assert_eq!(col.len(), 257);
        for (k, v) in col.iter().enumerate() {
            let expected = v.as_f64().expect("value") as f32;
            let got = fb[k * N_MELS + b];
            assert!(
                (got - expected).abs() <= FB_TOL,
                "band {b} bin {k}: {got} != {expected}"
            );
        }
    }
}

#[test]
fn default_frontend_uses_the_slaney_filterbank_and_symmetric_window() {
    let fe = MelFrontend::new();
    assert_eq!(fe.filterbank().len(), 257 * N_MELS);
    assert_eq!(fe.window(), hann_window(false).as_slice());
    assert_eq!(fe.window().len(), 400);
}

// ---- shape, contract and robustness -----------------------------------------------------

#[test]
fn frames_equal_floor_n_over_160_plus_one() {
    let fe = MelFrontend::new();
    for n in [400_usize, 401, 479, 480, 559, 560, 1000, 16000, 48000] {
        let f = fe.features(&lcg_signal(9, n, false)).expect("ok");
        assert_eq!(f.frames, n / 160 + 1, "n={n}");
        assert_eq!(f.data.len(), N_MELS * f.frames, "n={n}");
    }
}

#[test]
fn every_length_from_400_to_1200_is_finite_and_never_panics() {
    let fe = MelFrontend::new();
    let base = lcg_signal(11, 1200, true);
    for n in 400..=1200 {
        let f = fe.features(&base[..n]).expect("ok");
        assert_eq!(f.frames, n / 160 + 1, "n={n}");
        assert!(f.data.iter().all(|v| v.is_finite()), "n={n} not finite");
    }
}

#[test]
fn non_finite_input_is_rejected() {
    let fe = MelFrontend::new();
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut x = lcg_signal(3, 8000, false);
        x[4000] = bad;
        assert!(fe.features(&x).is_err(), "{bad} must be rejected");
    }
}

#[test]
fn inputs_shorter_than_400_samples_are_rejected() {
    let fe = MelFrontend::new();
    for n in [0_usize, 1, 160, 256, 257, 399] {
        let err = fe
            .features(&lcg_signal(5, n, false))
            .expect_err("short input must be Err");
        assert!(err.contains("too short"), "n={n}: {err}");
    }
}

#[test]
fn silence_is_finite_and_constant_per_band() {
    let fe = MelFrontend::new();
    let f = fe.features(&vec![0.0_f32; 8000]).expect("ok");
    assert!(f.data.iter().all(|v| v.is_finite()));
    for b in 0..N_MELS {
        let first = at(&f, b, 0);
        for t in 0..f.frames {
            assert!((at(&f, b, t) - first).abs() <= 1e-6, "band {b} frame {t}");
        }
    }
}

#[test]
fn from_parts_validates_lengths() {
    assert!(MelFrontend::from_parts(vec![0.0; 399], vec![0.0; 257 * N_MELS]).is_err());
    assert!(MelFrontend::from_parts(vec![0.0; 400], vec![0.0; 10]).is_err());
    assert!(MelFrontend::from_parts(hann_window(false), mel_filterbank(MelScale::Slaney)).is_ok());
}

// ---- mutation checks: each deliberate change must FAIL the golden comparison ------------

fn mutant_err(fe: &MelFrontend, pad: PadMode) -> f32 {
    let g = golden();
    let c = case(&g, "noise_05s");
    let f = fe
        .compute(&signal_for(c), pad)
        .expect("mutant front end runs");
    max_err_vs_golden(&f, c).expect("shape matches")
}

#[test]
fn control_unmodified_front_end_passes_the_mutant_comparison() {
    let err = mutant_err(&MelFrontend::new(), PadMode::Reflect);
    assert!(err <= TOL, "control err {err} > {TOL}");
}

#[test]
fn mutant_htk_mel_scale_fails_golden() {
    let fe =
        MelFrontend::from_parts(hann_window(false), mel_filterbank(MelScale::Htk)).expect("parts");
    let err = mutant_err(&fe, PadMode::Reflect);
    assert!(err > MUTANT_MIN_ERR, "htk mutant only moved by {err}");
}

#[test]
fn mutant_periodic_hann_fails_golden() {
    let fe = MelFrontend::from_parts(hann_window(true), mel_filterbank(MelScale::Slaney))
        .expect("parts");
    let err = mutant_err(&fe, PadMode::Reflect);
    assert!(
        err > MUTANT_MIN_ERR,
        "periodic hann mutant only moved by {err}"
    );
}

#[test]
fn mutant_zero_padding_instead_of_reflect_fails_golden() {
    let err = mutant_err(&MelFrontend::new(), PadMode::Zero);
    assert!(err > MUTANT_MIN_ERR, "zero-pad mutant only moved by {err}");
}
