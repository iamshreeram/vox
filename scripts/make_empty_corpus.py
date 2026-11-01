#!/usr/bin/env python3
"""Generates the empty-path corpus gate clips (DESIGN Revision 6, R6-2).

Writes 16 kHz mono 16-bit PCM WAV files under <out>/neg and <out>/pos, plus
<out>/manifest.json. Used by the ignored Rust test `auto_lang_empty_corpus`.

Negatives are synthetic or macOS system sounds that must NOT be accepted.
Positives are Tamil (`say -v Vani`) and Hindi (`say -v Lekha`) sentences that
must be identified. Speech is not padded; negatives are 5 s long.

Usage: python3 scripts/make_empty_corpus.py [OUT_DIR]
Requires numpy, macOS `say`, and ffmpeg at /opt/homebrew/bin/ffmpeg.
"""

import json
import os
import subprocess
import sys
import tempfile
import wave

import numpy as np

SAMPLE_RATE = 16_000
NEG_SECONDS = 5.0
GAINS = [0.02, 0.1, 0.5]
SYS_GAINS = [0.1, 0.5]
SYSTEM_SOUNDS_DIR = "/System/Library/Sounds"
FFMPEG = "/opt/homebrew/bin/ffmpeg"
DEFAULT_OUT = "/tmp/vox-work/empty_corpus"
SEED = 20261101

TAMIL_SENTENCES = [
    "வணக்கம் இன்று எப்படி இருக்கிறீர்கள்",
    "எனக்கு ஒரு கப் தேநீர் வேண்டும்",
    "நாளை காலை பத்து மணிக்கு சந்திப்போம்",
    "இந்த பாடலை மீண்டும் ஒலிக்கச் செய்யுங்கள்",
]
HINDI_SENTENCES = [
    "नमस्ते आप कैसे हैं आज",
    "मुझे एक कप चाय चाहिए",
]


def peak_normalise(signal):
    """Scales so the largest absolute sample is exactly 1.0 (silence stays silence)."""
    peak = float(np.max(np.abs(signal))) if signal.size else 0.0
    if peak <= 0.0:
        return signal
    return signal / peak


def time_axis(seconds):
    return np.arange(int(round(seconds * SAMPLE_RATE))) / SAMPLE_RATE


def white_noise(rng, n):
    return rng.standard_normal(n)


def pink_noise(rng, n):
    spectrum = np.fft.rfft(rng.standard_normal(n))
    freqs = np.fft.rfftfreq(n, d=1.0 / SAMPLE_RATE)
    freqs[0] = freqs[1]
    spectrum = spectrum / np.sqrt(freqs)
    return np.fft.irfft(spectrum, n)


def brown_noise(rng, n):
    walk = np.cumsum(rng.standard_normal(n))
    return walk - np.mean(walk)


def hum(n):
    t = time_axis(n / SAMPLE_RATE)
    weights = {50: 1.0, 100: 0.5, 150: 0.25, 200: 0.125}
    return sum(w * np.sin(2 * np.pi * f * t) for f, w in weights.items())


def clicks(rng, n):
    signal = np.zeros(n)
    burst_len = int(0.005 * SAMPLE_RATE)
    envelope = np.exp(-np.arange(burst_len) / (0.001 * SAMPLE_RATE))
    count = int(round(8 * n / SAMPLE_RATE))
    starts = rng.integers(0, n - burst_len, size=count)
    for start in starts:
        signal[start:start + burst_len] += rng.standard_normal(burst_len) * envelope
    return signal


def band_limited_noise(rng, n, low_hz, high_hz):
    spectrum = np.fft.rfft(rng.standard_normal(n))
    freqs = np.fft.rfftfreq(n, d=1.0 / SAMPLE_RATE)
    spectrum[(freqs < low_hz) | (freqs > high_hz)] = 0.0
    return np.fft.irfft(spectrum, n)


def cough(rng, n):
    signal = np.zeros(n)
    burst_len = int(0.150 * SAMPLE_RATE)
    envelope = np.hanning(burst_len)
    for index in range(3):
        start = int((0.4 + index * 1.2) * SAMPLE_RATE)
        burst = band_limited_noise(rng, burst_len, 300.0, 2000.0)
        signal[start:start + burst_len] += burst * envelope
    return signal


def tone(n, freq_hz):
    return np.sin(2 * np.pi * freq_hz * time_axis(n / SAMPLE_RATE))


def chord(n):
    t = time_axis(n / SAMPLE_RATE)
    return sum(np.sin(2 * np.pi * f * t) for f in (261.6, 329.6, 392.0))


def fit_length(signal, n):
    """Loops a short signal or truncates a long one to exactly n samples."""
    if signal.size == 0:
        return np.zeros(n)
    repeats = int(np.ceil(n / signal.size))
    return np.tile(signal, repeats)[:n]


def decode_ffmpeg_mono_16k(path):
    """Decodes any audio file to float32 mono 16 kHz via ffmpeg."""
    result = subprocess.run(
        [
            FFMPEG, "-v", "error", "-i", path,
            "-ac", "1", "-ar", str(SAMPLE_RATE),
            "-f", "f32le", "-acodec", "pcm_f32le", "pipe:1",
        ],
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(f"ffmpeg failed on {path}: exit {result.returncode}")
    return np.frombuffer(result.stdout, dtype="<f4").astype(np.float64)


def write_wav(path, signal):
    """Writes float samples in [-1, 1] as 16 kHz mono 16-bit PCM."""
    clipped = np.clip(signal, -1.0, 1.0)
    pcm = np.round(clipped * 32767.0).astype("<i2")
    with wave.open(path, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(SAMPLE_RATE)
        wav.writeframes(pcm.tobytes())


def gain_name(gain):
    return f"{gain}"


def negative_signals(rng):
    """Yields (kind, base_signal) pairs for synthetic negatives. Base is peak 1.0 or DC."""
    n = int(NEG_SECONDS * SAMPLE_RATE)
    yield "white", peak_normalise(white_noise(rng, n))
    yield "pink", peak_normalise(pink_noise(rng, n))
    yield "brown", peak_normalise(brown_noise(rng, n))
    yield "hum", peak_normalise(hum(n))
    yield "clicks", peak_normalise(clicks(rng, n))
    yield "cough", peak_normalise(cough(rng, n))
    yield "tone440", peak_normalise(tone(n, 440.0))
    yield "chord", peak_normalise(chord(n))


def write_negatives(out_dir, manifest):
    rng = np.random.default_rng(SEED)
    n = int(NEG_SECONDS * SAMPLE_RATE)
    neg_dir = os.path.join(out_dir, "neg")
    count = 0
    for kind, base in negative_signals(rng):
        for gain in GAINS:
            name = f"{kind}_g{gain_name(gain)}.wav"
            write_wav(os.path.join(neg_dir, name), base * gain)
            manifest.append(entry(neg_dir, name, "neg", kind, gain, None))
            count += 1
    # DC offsets: constant value equal to the gain, and DC plus low noise.
    for gain in GAINS:
        name = f"dc_g{gain_name(gain)}.wav"
        write_wav(os.path.join(neg_dir, name), np.full(n, gain))
        manifest.append(entry(neg_dir, name, "neg", "dc", gain, None))
        count += 1
        noise = peak_normalise(white_noise(rng, n)) * (0.05 * gain)
        name = f"dc_plus_noise_g{gain_name(gain)}.wav"
        write_wav(os.path.join(neg_dir, name), gain + noise)
        manifest.append(entry(neg_dir, name, "neg", "dc_plus_noise", gain, None))
        count += 1
    count += write_system_sounds(neg_dir, manifest)
    return count


def write_system_sounds(neg_dir, manifest):
    if not os.path.isdir(SYSTEM_SOUNDS_DIR):
        raise RuntimeError(f"system sounds folder missing: {SYSTEM_SOUNDS_DIR}")
    n = int(NEG_SECONDS * SAMPLE_RATE)
    count = 0
    for file_name in sorted(os.listdir(SYSTEM_SOUNDS_DIR)):
        if not file_name.endswith(".aiff"):
            continue
        sound = os.path.splitext(file_name)[0].replace(" ", "_")
        decoded = decode_ffmpeg_mono_16k(os.path.join(SYSTEM_SOUNDS_DIR, file_name))
        base = peak_normalise(fit_length(decoded, n))
        for gain in SYS_GAINS:
            name = f"sys_{sound}_g{gain_name(gain)}.wav"
            write_wav(os.path.join(neg_dir, name), base * gain)
            manifest.append(entry(neg_dir, name, "neg", f"sys_{sound}", gain, None))
            count += 1
    return count


def say_to_signal(text, voice, tmp_dir, tag):
    aiff = os.path.join(tmp_dir, f"{tag}.aiff")
    subprocess.run(["say", "-v", voice, "-o", aiff, text], check=True)
    return decode_ffmpeg_mono_16k(aiff)


def write_positives(out_dir, manifest):
    pos_dir = os.path.join(out_dir, "pos")
    count = 0
    with tempfile.TemporaryDirectory() as tmp_dir:
        for groups, voice, lang, prefix in (
            (TAMIL_SENTENCES, "Vani", "ta", "tamil"),
            (HINDI_SENTENCES, "Lekha", "hi", "hindi"),
        ):
            for index, text in enumerate(groups, start=1):
                signal = say_to_signal(text, voice, tmp_dir, f"{prefix}{index}")
                base = peak_normalise(signal)
                for gain in GAINS:
                    kind = f"{prefix}{index}"
                    name = f"{kind}_g{gain_name(gain)}.wav"
                    write_wav(os.path.join(pos_dir, name), base * gain)
                    manifest.append(entry(pos_dir, name, "pos", kind, gain, lang))
                    count += 1
    return count


def entry(folder, name, group, kind, gain, expected_lang):
    return {
        "path": os.path.abspath(os.path.join(folder, name)),
        "group": group,
        "kind": kind,
        "gain": gain,
        "expected_lang": expected_lang,
    }


def main(argv):
    out_dir = os.path.abspath(argv[1] if len(argv) > 1 else DEFAULT_OUT)
    os.makedirs(os.path.join(out_dir, "neg"), exist_ok=True)
    os.makedirs(os.path.join(out_dir, "pos"), exist_ok=True)
    manifest = []
    neg_count = write_negatives(out_dir, manifest)
    pos_count = write_positives(out_dir, manifest)
    manifest_path = os.path.join(out_dir, "manifest.json")
    with open(manifest_path, "w", encoding="utf-8") as handle:
        json.dump(manifest, handle, ensure_ascii=False, indent=2)
    print(
        f"wrote {neg_count} negatives and {pos_count} positives to {out_dir} "
        f"(manifest {manifest_path}, total {len(manifest)})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
