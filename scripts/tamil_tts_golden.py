#!/usr/bin/env python3
"""Regenerate the small JSON fixtures used by the Tamil TTS parity tests.

Run this only when the Tamil engine's expected behaviour is meant to change. It is NOT run in CI.

It drives the published reference runtime (Apache-2.0, by KevinAHM, https://huggingface.co/KevinAHM/pocket-tts-onnx,
file pocket_tts_onnx.py) against the Tamil pack and records numbers only: token ids, chunk texts, frame
counts, a few latent values and an audio RMS. No audio and no model weights are written.

IMPORTANT: use onnxruntime==1.24.2, the version bundled by the Rust `ort` crate (rc.12). Measured: the int8 voice encoder and
language model give slightly different numbers on other ONNX Runtime versions (voice embedding sum 30.358 on 1.31.1 vs 30.399
on 1.24.2), which the autoregressive loop amplifies. With matching versions Rust and Python agree to about 1e-5.

Requirements: python3 with onnxruntime==1.24.2, numpy, sentencepiece, scipy, soundfile, safetensors, huggingface_hub.

Usage:
  python3 scripts/tamil_tts_golden.py --ref /path/to/pocket_tts_onnx.py --pack /path/to/pocket-tts-tamil-int4

The pack folder is the one the download script fills (see scripts/download_tamil_tts.sh). The reference
runtime expects the file names mimi_encoder.onnx and flow_lm_main.onnx, so a scratch folder of symlinks is
built automatically.
"""
import argparse
import importlib.util
import json
import os
import sys
import tempfile
from pathlib import Path

import numpy as np

OUT = Path(__file__).resolve().parent.parent / "src-tauri" / "tests" / "fixtures" / "tamil"

TAMIL_SENTENCES = [
    "வணக்கம்",
    "நன்றி.",
    "இன்று வானிலை எப்படி இருக்கிறது?",
    "நான் உங்களுக்கு எப்படி உதவ முடியும்!",
    "தமிழ் ஒரு பழமையான மொழி.",
    "எனக்கு ஒரு கதை சொல்லுங்கள்",
    "நாளை காலை ஏழு மணிக்கு சந்திப்போம்.",
    "அவன் வீட்டுக்குச் சென்றான்; அவள் பள்ளிக்குச் சென்றாள்.",
    "ஒரு காலத்தில் ஒரு சிறிய கிராமத்தில் ஒரு அன்பான முயல் வாழ்ந்தது.",
    "அது தினமும் காட்டுக்குச் சென்று கேரட் சேகரித்தது.",
    "ஒரு நாள் அது ஒரு புதிய நண்பனைச் சந்தித்தது.",
    "மழை பெய்தால் பயிர்கள் நன்றாக வளரும், விவசாயிகள் மகிழ்ச்சியாக இருப்பார்கள்.",
    "கடல் அலைகள் கரையை மெதுவாகத் தொட்டுச் சென்றன…",
    "நீங்கள் எங்கே போகிறீர்கள்: சந்தைக்கா அல்லது பள்ளிக்கா?",
]

TOKENIZER_EXTRAS = [
    "",
    " ",
    "   வணக்கம்   உலகம்   ",
    "வணக்கம்\tஉலகம்\nநண்பா",
    "இன்று 2024 ஆம் ஆண்டு",
    "௧௨௩ ரூபாய்",
    "Hello வணக்கம்",
    "hello world",
    "வணக்கம்  நண்பா",
    "ﬁne ａｂｃ ＡＢＣ １２３",
    "வணக்கம்\u00a0உலகம்",
    "க\u200dஷ்",
    "க்\u200cஷ",
    "மூ?",
    "!!!",
    "...",
    "…",
    "வணக்கம்.",
    "வணக்கம்,நண்பா",
    "ஆ" * 40,
    "hi \U0001F600 வணக்கம்",
]

# Texts for the prepare/chunk parity corpus (None is replaced by a generated long text).
TEXT_CASES = TAMIL_SENTENCES + [
    "வணக்கம்   உலகம்",
    "வணக்கம்\nஉலகம்\r\nநண்பா",
    "  வணக்கம்  ",
    "இன்று 2024",
    "இது ஒரு test",
    "hello",
    "Hello world.",
    "",
    "   ",
    "க்",
    "ம்.",
    "ஐந்து சொற்கள் உள்ள ஒரு வாக்கியம்",
    "ஒன்று இரண்டு மூன்று நான்கு",
    None,  # many sentences (> 50 tokens)
    "NO_PUNCT_LONG",  # one long run-on without sentence end (> 50 tokens, only commas)
    "UNSPLITTABLE_LONG",  # one very long sentence with no separators at all
    "HUGE",  # > 200 tokens, many sentences
]


def load_reference(ref_path: str):
    spec = importlib.util.spec_from_file_location("pocket_tts_onnx", ref_path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules["pocket_tts_onnx"] = mod
    spec.loader.exec_module(mod)
    return mod.PocketTTSOnnx


def build_scratch(pack: Path) -> Path:
    scratch = Path(tempfile.mkdtemp(prefix="tamil-ref-"))
    for name in ("bundle.json", "bos_before_voice.npy", "tokenizer.model", "sample_geetha.wav",
                 "text_conditioner.onnx", "flow_lm_flow_int8.onnx", "mimi_decoder_int8.onnx"):
        os.symlink(pack / name, scratch / name)
    os.symlink(pack / "flow_lm_main_int4.onnx", scratch / "flow_lm_main.onnx")
    os.symlink(pack / "mimi_encoder_int8.onnx", scratch / "mimi_encoder.onnx")
    return scratch


def materialise(case):
    if case is None:
        return " ".join(TAMIL_SENTENCES[:9])
    if case == "NO_PUNCT_LONG":
        return ", ".join(s.rstrip(".?!…") for s in TAMIL_SENTENCES[:8])
    if case == "UNSPLITTABLE_LONG":
        return " ".join(s.rstrip(".?!…;:,").replace(",", "").replace(";", "").replace(":", "") for s in TAMIL_SENTENCES[:10])
    if case == "HUGE":
        return " ".join(TAMIL_SENTENCES * 2)
    return case


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ref", required=True)
    ap.add_argument("--pack", required=True)
    ap.add_argument("--out", help="output folder for the fixtures (default: src-tauri/tests/fixtures/tamil)")
    ap.add_argument("--dump-voice", help="optional: also write the reference voice embedding (.npy) here, for the strict Rust parity test (set VOX_TAMIL_REF_VOICE to it). Never commit this file.")
    args = ap.parse_args()
    global OUT
    if args.out:
        OUT = Path(args.out)
    pack = Path(args.pack).expanduser()
    Tts = load_reference(args.ref)
    scratch = build_scratch(pack)
    tts = Tts(models_dir=str(scratch), language="tamil", precision="int8", device="cpu", temperature=0.0, lsd_steps=1)
    sp = tts.tokenizer
    OUT.mkdir(parents=True, exist_ok=True)

    # 1. tokenizer parity
    tok = []
    for s in TAMIL_SENTENCES + TOKENIZER_EXTRAS:
        ids = sp.Encode(s)
        tok.append({"text": s, "ids": ids, "decoded": sp.Decode(ids)})
    # decode of arbitrary id lists (includes unk id 0 and the sentence-end ids)
    decodes = []
    for ids in ([0], [5, 0, 7], [3, 4, 5], sp.Encode("வணக்கம் உலகம்"), sp.Encode(".!...?"), sp.Encode(",;:")):
        decodes.append({"ids": ids, "text": sp.Decode(ids)})
    json.dump({"encode": tok, "decode": decodes,
               "eos_ids": sp.Encode(".!...?")[1:], "fallback_ids": sp.Encode(",;:")[1:]},
              open(OUT / "tamil_tokenizer_parity.json", "w", encoding="utf-8"), ensure_ascii=False, indent=1)

    # 2. text prep + chunk parity
    cases = []
    for case in TEXT_CASES:
        text = materialise(case)
        entry = {"text": text}
        try:
            prepared, guess = tts._prepare_text_prompt(text)
            entry.update({"error": False, "prepared": prepared, "frames_after_eos_guess": guess})
            chunks = tts._split_into_best_sentences(text)
            entry["chunks"] = chunks
            entry["chunk_token_counts"] = [len(sp.Encode(tts._prepare_text_prompt(c)[0])) for c in chunks]
            entry["chunk_ids"] = [sp.Encode(tts._prepare_text_prompt(c)[0]) for c in chunks]
            entry["max_frames"] = [tts._estimate_max_gen_len(n) for n in entry["chunk_token_counts"]]
            entry["reference_overflow"] = any(n > tts.max_token_per_chunk for n in entry["chunk_token_counts"])
        except ValueError:
            entry.update({"error": True})
        cases.append(entry)
    json.dump({"max_token_per_chunk": tts.max_token_per_chunk, "cases": cases},
              open(OUT / "tamil_text_parity.json", "w", encoding="utf-8"), ensure_ascii=False, indent=1)

    # 3. engine golden (temperature 0 = deterministic)
    emb = tts.encode_voice(str(scratch / "sample_geetha.wav"))
    if args.dump_voice:
        np.save(args.dump_voice, emb.astype(np.float32))
    import onnxruntime as _ort
    out = {"onnxruntime_version": _ort.__version__, "voice_emb_shape": list(emb.shape), "voice_emb_sum": float(emb.sum()), "texts": {}}
    for key, text in (("t1", "வணக்கம், நீங்கள் எப்படி இருக்கிறீர்கள்"), ("t2", "இன்று வானிலை மிகவும் அழகாக இருக்கிறது."),
                      ("t3", " ".join(TAMIL_SENTENCES[8:11]))):
        lat = tts.generate_latents(text, emb)
        audio = tts.decode_latents(lat)
        out["texts"][key] = {
            "text": text, "frames": int(lat.shape[1]), "audio_len": int(len(audio)),
            "rms": float(np.sqrt((audio ** 2).mean())),
            "first_latents": lat[0, :3, :].round(5).tolist(),
        }
    json.dump(out, open(OUT / "tamil_engine_golden.json", "w", encoding="utf-8"), ensure_ascii=False, indent=1)
    print("wrote fixtures to", OUT)


if __name__ == "__main__":
    main()
