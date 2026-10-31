#!/usr/bin/env python3
"""Generate the offline NeMo log-mel golden for the Tamil STT front end.

Runs the TorchScript preprocessor shipped with the IndicConformer pack
(assets/preprocessor.ts) on deterministic synthetic signals and writes:

  src-tauri/tests/fixtures/tamil_stt/mel_golden.json
  src-tauri/tests/fixtures/tamil_stt/README.md

Run with the offline venv:  /tmp/vox-work/venv3/bin/python scripts/tamil_stt_golden.py [path/to/preprocessor.ts]

The signals are generated without any RNG library so the Rust tests can
regenerate them exactly:
  x_next = (1664525 * x + 1013904223) mod 2^32   (32-bit LCG, seed per case)
  noise  = ((x_next >> 8) as f32 / 16777216.0 - 0.5) * 0.2      (f32 arithmetic)
  sine   = 0.5 * sin(2 * pi * 220 * (i / 16000))                (f64, cast to f32, added in f32)
"""

import hashlib
import json
import math
import platform
import sys
from pathlib import Path

import numpy as np
import torch

REPO_ROOT = Path(__file__).resolve().parents[1]
FIXTURE_DIR = REPO_ROOT / "src-tauri" / "tests" / "fixtures" / "tamil_stt"
DEFAULT_PREPROCESSOR = Path("/tmp/vox-work/indic/assets/preprocessor.ts")
MAX_JSON_BYTES = 150_000
FULL_DUMP_LIMIT = 4100  # store every band of every frame when frames * 80 <= this
SUBSAMPLE_STEP = 20

LCG_A = 1664525
LCG_C = 1013904223
MASK32 = 0xFFFFFFFF
BANDS_COMPLETE = [0, 20, 40, 79]


def lcg_noise(seed: int, n: int) -> np.ndarray:
    """Deterministic f32 noise in [-0.1, 0.1) from the 32-bit LCG described above."""
    out = np.empty(n, dtype=np.float32)
    x = seed & MASK32
    scale = np.float32(16777216.0)
    half = np.float32(0.5)
    amp = np.float32(0.2)
    for i in range(n):
        x = (LCG_A * x + LCG_C) & MASK32
        out[i] = (np.float32(x >> 8) / scale - half) * amp
    return out


def sine_220(n: int) -> np.ndarray:
    """Return 0.5 * sin(2*pi*220*t) at 16 kHz as f32, computed in f64 first."""
    out = np.empty(n, dtype=np.float32)
    for i in range(n):
        t = i / 16000.0
        out[i] = np.float32(0.5 * math.sin(2.0 * math.pi * 220.0 * t))
    return out


def make_signal(spec: dict) -> np.ndarray:
    """Build the f32 input signal described by a case spec."""
    n = spec["n"]
    kind = spec["kind"]
    if kind == "silence":
        return np.zeros(n, dtype=np.float32)
    if kind == "impulse":
        x = np.zeros(n, dtype=np.float32)
        x[spec["index"]] = np.float32(0.5)
        return x
    x = lcg_noise(spec["seed"], n)
    if spec.get("sine"):
        x = (x + sine_220(n)).astype(np.float32)
    return x


CASES = [
    # Silence rows are constant by construction; stored subsampled to stay under the JSON budget.
    {"name": "silence", "n": 8000, "kind": "silence", "full": False},
    {"name": "impulse_start", "n": 4000, "kind": "impulse", "index": 0},
    {"name": "impulse_mid", "n": 4000, "kind": "impulse", "index": 2000},
    {"name": "impulse_end", "n": 4000, "kind": "impulse", "index": 3999},
    {"name": "noise_05s", "n": 8000, "kind": "noise", "seed": 1},
    {"name": "len_399", "n": 399, "kind": "noise", "seed": 2},
    {"name": "len_400", "n": 400, "kind": "noise", "seed": 3},
    {"name": "len_401", "n": 401, "kind": "noise", "seed": 4},
    {"name": "len_479", "n": 479, "kind": "noise", "seed": 5},
    {"name": "len_480", "n": 480, "kind": "noise", "seed": 6},
    {"name": "noise_sine_3s", "n": 48000, "kind": "noise", "seed": 7, "sine": True},
]


def r6(value: float) -> float:
    """Round to 6 decimals for the JSON."""
    return round(float(value), 6)


def load_filterbank(module: torch.jit.ScriptModule) -> np.ndarray:
    """Pull the 257x80 mel filterbank constant out of the TorchScript module."""
    _, constants = module.code_with_constants
    mapping = constants.const_mapping
    fb = mapping["c4"].detach().cpu().numpy().astype(np.float64)
    if fb.shape != (257, 80):
        raise SystemExit(f"unexpected filterbank shape {fb.shape}")
    return fb


def run_case(module: torch.jit.ScriptModule, case: dict) -> dict:
    """Run the preprocessor on one case and build its JSON record."""
    signal = make_signal(case)
    x = torch.from_numpy(np.ascontiguousarray(signal)).unsqueeze(0)
    length = torch.tensor([case["n"]], dtype=torch.int64)
    with torch.no_grad():
        mel, out_len = module(x, length)
    mel_np = mel.numpy()[0]
    frames = int(mel_np.shape[1])
    expected_frames = case["n"] // 160 + 1
    if frames != expected_frames or int(out_len[0]) != expected_frames:
        raise SystemExit(f"{case['name']}: frames {frames} != {expected_frames}")

    flat = mel_np.astype(np.float64).ravel()
    record = {
        "n": case["n"],
        "frames": frames,
        "torch_length": int(out_len[0]),
        "stats": {
            "mean": r6(flat.mean()),
            "std_population": r6(flat.std()),
            "min": r6(flat.min()),
            "max": r6(flat.max()),
        },
    }
    if case.get("full", True) and frames * 80 <= FULL_DUMP_LIMIT:
        indices = list(range(frames))
    else:
        indices = list(range(0, frames, SUBSAMPLE_STEP))
        if indices[-1] != frames - 1:
            indices.append(frames - 1)
    record["frame_indices"] = indices
    # values[k] is the 80-band vector of frame frame_indices[k]
    record["values"] = [[r6(v) for v in mel_np[:, t]] for t in indices]
    return record


def rust_filterbank_table(fb: np.ndarray) -> str:
    """Render the non-zero spans of the pack filterbank as a Rust const table.

    Each band is (first_bin, weights). Weights are printed as the shortest decimal that round-trips to the same f32.
    """
    lines = ["const SLANEY_FILTERBANK: [(usize, &[f32]); N_MELS] = ["]
    for band in range(fb.shape[1]):
        rows = np.nonzero(fb[:, band])[0]
        first, last = int(rows[0]), int(rows[-1])
        weights = ", ".join(np.format_float_positional(np.float32(v), unique=True, trim="-") for v in fb[first:last + 1, band])
        lines.append(f"    ({first}, &[{weights}]),")
    lines.append("];")
    return "\n".join(lines)


def main() -> None:
    """Write mel_golden.json and the fixture README (or print the Rust table with --emit-rust-fb)."""
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    emit_table = "--emit-rust-fb" in sys.argv[1:]
    pre_path = Path(args[0]) if args else DEFAULT_PREPROCESSOR
    torch.set_num_threads(1)
    module = torch.jit.load(str(pre_path), map_location="cpu")
    module.eval()
    sha = hashlib.sha256(pre_path.read_bytes()).hexdigest()

    fb = load_filterbank(module)
    if emit_table:
        print(rust_filterbank_table(fb))
        return
    golden = {
        "source": "TorchScript NeMo AudioToMelSpectrogramPreprocessor (assets/preprocessor.ts)",
        "preprocessor_sha256": sha,
        "sample_rate": 16000,
        "signal_generator": {
            "lcg": "x = (1664525*x + 1013904223) mod 2^32; sample = ((x>>8) as f32 / 16777216.0 - 0.5) * 0.2",
            "sine": "0.5*sin(2*pi*220*i/16000) added in f32 when sine=true",
        },
        "filterbank": {
            "shape": [257, 80],
            "band_sums": [r6(v) for v in fb.sum(axis=0)],
            "bands": {str(b): [r6(v) for v in fb[:, b]] for b in BANDS_COMPLETE},
        },
        "cases": {},
    }
    for case in CASES:
        record = run_case(module, case)
        record["spec"] = {k: v for k, v in case.items() if k not in ("name", "full")}
        golden["cases"][case["name"]] = record
        print(f"{case['name']}: n={record['n']} frames={record['frames']} stored={len(record['frame_indices'])}")

    payload = json.dumps(golden, separators=(",", ":"))
    size = len(payload.encode("utf-8"))
    if size > MAX_JSON_BYTES:
        raise SystemExit(f"mel_golden.json is {size} bytes, over {MAX_JSON_BYTES}")
    FIXTURE_DIR.mkdir(parents=True, exist_ok=True)
    (FIXTURE_DIR / "mel_golden.json").write_text(payload + "\n", encoding="utf-8")
    write_readme(sha, size)
    print(f"wrote {FIXTURE_DIR / 'mel_golden.json'} ({size} bytes)")


def write_readme(sha: str, size: int) -> None:
    """Record the versions and provenance of the golden."""
    text = f"""# Tamil STT mel golden

Generated by `scripts/tamil_stt_golden.py` (run with `/tmp/vox-work/venv3/bin/python`).

- mel_golden.json: {size} bytes
- Python: {platform.python_version()} ({platform.platform()})
- torch: {torch.__version__} (plain PyPI CPU build, 1 thread)
- numpy: {np.__version__}
- preprocessor: assets/preprocessor.ts from the IndicConformer int8 pack (revision pinned in
  scripts/download_indic_stt.sh), sha256 {sha}

Contract notes:
- The public Rust front end rejects N < 400 samples. The `len_399` case is still recorded
  from the preprocessor (torch accepts N > 256) and is checked through the internal path
  so the maths is covered at the boundary.
- Values are rounded to 6 decimals. Statistics use population std.
- Size budget: silence is stored subsampled (every 20th frame + last), not every band of every
  frame. Its rows are identical by construction, and the full dump pushed the JSON over 150 KB.
- Silence artefact: torch's f32 mean of a constant row is one ulp off the row value, so the
  normalised silence rows come out as -0.007505 instead of 0. A float64 implementation gives 0.
  The Rust test uses a 1e-2 tolerance for the silence case only (documented there).
- Signals are regenerated in Rust from the LCG described in mel_golden.json.
- The Rust front end ships the pack filterbank weights (`SLANEY_FILTERBANK` in
  `indic_stt_mel.rs`), printed by `scripts/tamil_stt_golden.py --emit-rust-fb`. The slaney
  formula agrees to 6.9e-8, which is inside the 1e-6 gate but moves 3-frame clips by about
  1e-4 after normalisation, so the pack weights are used.
- The Rust FFT runs in f64 and log values stay f64 until the output is written (an f32 FFT
  gave 1.0e-4 on the 3-frame clip). Torch itself is within about 3e-5 of an exact f64 run.
"""
    (FIXTURE_DIR / "README.md").write_text(text, encoding="utf-8")


if __name__ == "__main__":
    main()
