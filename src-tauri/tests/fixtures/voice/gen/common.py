"""Shared helpers for the vox-voice fixture generators.

Everything here is deterministic: no RNG is used anywhere (signals are analytic),
torch/onnxruntime are pinned to a single thread.
"""
import hashlib
import json
import os
import sys

import numpy as np
import soundfile as sf
import torch

torch.set_num_threads(1)

ROOT = "/tmp/vox-voice-assets"
FIX = f"{ROOT}/fixtures"
GEN = f"{FIX}/gen"
AUDIO = f"{ROOT}/audio"      # existing clips (fixtures use these)
AUDIO2 = f"{ROOT}/audio2"    # new 4-voice set (speaker matrix uses these)
CAMPPLUS = f"{ROOT}/campplus.onnx"
RESNET34 = f"{ROOT}/resnet34.onnx"
ST_MODEL = f"{ROOT}/wheel/pipecat/audio/turn/smart_turn/data/smart-turn-v3.2-cpu.onnx"
WHEEL = f"{ROOT}/wheel"

SR = 16000


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def load_mono(path):
    x, sr = sf.read(path, dtype="float32")
    assert sr == SR and x.ndim == 1, (path, sr, x.shape)
    return x


def tone(n):
    """x[n] = 0.3 sin(2pi 220 n/16000) + 0.1 sin(2pi 1330 n/16000), float64 -> float32."""
    idx = np.arange(n, dtype=np.float64)
    x = 0.3 * np.sin(2 * np.pi * 220 * idx / 16000) + 0.1 * np.sin(2 * np.pi * 1330 * idx / 16000)
    return x.astype(np.float32)


# ---------------------------------------------------------------- speaker frontends
# sherpa-onnx style (golden_speaker.py): snip_edges=False, high_freq=-400, povey window.
# wespeaker_ref: wespeaker/bin/infer_onnx.py compute_fbank (lines 31-51):
#   waveform*(1<<15), kaldi.fbank(num_mel_bins=80, frame_length=25, frame_shift=10,
#   dither=0.0, window_type='hamming', use_energy=False) [torchaudio defaults:
#   snip_edges=True, low_freq=20, high_freq=0 (Nyquist), preemph 0.97, remove_dc_offset=True,
#   round_to_power_of_two=True, use_power=True, use_log_fbank=True, htk_compat=False],
#   then CMN: mat - torch.mean(mat, dim=0)  (line 50, no CVN).
FRONTENDS = {
    "sherpa_campplus": dict(scale=1.0, window="povey", snip_edges=False, high_freq=-400.0, cmn=True),
    "sherpa_nocmn": dict(scale=32768.0, window="povey", snip_edges=False, high_freq=-400.0, cmn=False),
    "wespeaker_ref": dict(scale=32768.0, window="hamming", snip_edges=True, high_freq=0.0, cmn=True),
}
MODEL_OF = {"sherpa_campplus": "campplus", "sherpa_nocmn": "resnet34", "wespeaker_ref": "resnet34"}


def fbank(samples, scale=1.0, window="povey", snip_edges=False, high_freq=-400.0, cmn=True):
    import torchaudio.compliance.kaldi as kaldi

    x = torch.from_numpy(np.ascontiguousarray(samples, dtype=np.float32) * np.float32(scale)).unsqueeze(0)
    feats = kaldi.fbank(
        x,
        sample_frequency=16000.0,
        num_mel_bins=80,
        frame_length=25.0,
        frame_shift=10.0,
        low_freq=20.0,
        high_freq=high_freq,
        dither=0.0,
        snip_edges=snip_edges,
        remove_dc_offset=True,
        preemphasis_coefficient=0.97,
        window_type=window,
        round_to_power_of_two=True,
        use_energy=False,
        use_log_fbank=True,
        use_power=True,
        energy_floor=0.0,
        raw_energy=True,
        htk_compat=False,
    )
    if cmn and feats.shape[0] > 0:
        feats = feats - torch.mean(feats, dim=0)
    return feats.numpy().astype(np.float32)


def frontend(name, samples, **override):
    kw = dict(FRONTENDS[name])
    kw.update(override)
    return fbank(samples, **kw)


_SESSIONS = {}


def session(model_path):
    import onnxruntime as ort

    if model_path not in _SESSIONS:
        so = ort.SessionOptions()
        so.intra_op_num_threads = 1
        so.inter_op_num_threads = 1
        _SESSIONS[model_path] = ort.InferenceSession(model_path, sess_options=so, providers=["CPUExecutionProvider"])
    return _SESSIONS[model_path]


def embed(model_key, feats):
    """feats [T,80] float32 -> raw embedding float32 [D] (not L2-normalised)."""
    path = CAMPPLUS if model_key == "campplus" else RESNET34
    sess = session(path)
    in_name = sess.get_inputs()[0].name
    out = sess.run(None, {in_name: np.ascontiguousarray(feats[None, :, :], dtype=np.float32)})[0][0]
    return out.astype(np.float32)


def cosine(a, b):
    a = np.asarray(a, dtype=np.float64)
    b = np.asarray(b, dtype=np.float64)
    return float(np.dot(a, b) / (np.linalg.norm(a) * np.linalg.norm(b)))


def l2n(v):
    v = np.asarray(v, dtype=np.float64)
    return v / np.linalg.norm(v)


# ---------------------------------------------------------------- Smart Turn reference
sys.path.insert(0, WHEEL)
sys.path.insert(0, ROOT)
import pipecat.audio.turn.smart_turn._whisper_features as WF  # noqa: E402
from audio_utils import truncate_audio_to_last_n_seconds  # noqa: E402

ST_N = 8 * SR  # 128000


def st_official(audio):
    """Official path exactly as local_smart_turn_v3._predict_endpoint (mel only)."""
    padded = truncate_audio_to_last_n_seconds(audio, n_seconds=8)
    assert padded.shape == (ST_N,)
    return WF.compute_whisper_log_mel_features(padded, do_normalize=True)


def st_variant(audio, pad="left", normalise=True, window="periodic", float64=False):
    """Re-implementation of the same algorithm with one switchable deviation.

    defaults (pad=left, normalise=True, window=periodic, float64=False) are verified to be
    bit-identical to st_official(); float64=True runs the whole chain in float64.
    """
    x = np.asarray(audio, dtype=np.float32)
    if x.size > ST_N:
        x = x[-ST_N:]
    elif x.size < ST_N:
        p = ST_N - x.size
        x = np.pad(x, (p, 0)) if pad == "left" else np.pad(x, (0, p))
    x = x.astype(np.float64) if float64 else x.astype(np.float32)
    if normalise:
        x = (x - x.mean()) / np.sqrt(x.var() + WF._NORM_VARIANCE_EPS)
    win = WF._HANN_WINDOW if window == "periodic" else np.hanning(WF._N_FFT)
    pad_n = WF._N_FFT // 2
    padded = np.pad(x.astype(np.float64), (pad_n, pad_n), mode="reflect")
    from numpy.lib.stride_tricks import sliding_window_view

    windows = sliding_window_view(padded, WF._N_FFT)[:: WF._HOP_LENGTH]
    spec = np.fft.rfft(windows * win.astype(np.float64), axis=-1)
    mags = (np.abs(spec) ** 2).T
    mel = np.maximum(WF._MEL_FLOOR, WF._MEL_FILTERS.T @ mags)
    log_spec = np.log10(mel)[:, :-1]
    log_spec = np.maximum(log_spec, log_spec.max() - 8.0)
    log_spec = (log_spec + 4.0) / 4.0
    return log_spec.astype(np.float32) if not float64 else log_spec


# ---------------------------------------------------------------- manifest helpers
def entry(rel_path, arr_or_none, dtype, shape, description, base=FIX):
    path = os.path.join(base, rel_path)
    return {
        "name": rel_path,
        "bytes": os.path.getsize(path),
        "sha256": sha256_file(path),
        "dtype": dtype,
        "shape": list(shape) if shape is not None else None,
        "description": description,
    }


def write_f32(path, arr):
    arr = np.ascontiguousarray(arr, dtype="<f4")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    arr.tofile(path)
    return arr.shape


def dump_json(path, obj):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as fh:
        json.dump(obj, fh, indent=2, sort_keys=True)
        fh.write("\n")
