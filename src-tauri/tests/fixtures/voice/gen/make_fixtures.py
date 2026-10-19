"""Generate all deterministic vox-voice fixtures + speaker decision matrix + manifest.

Usage: python3 make_fixtures.py [OUTDIR]     (default /tmp/vox-voice-assets/fixtures)
No RNG is used anywhere. All float artifacts are little-endian float32 (.f32).
"""
import json
import os
import platform
import sys

import numpy as np
import soundfile as sf

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common as C  # noqa: E402

# ----------------------------------------------------------------------------- helpers
DESC = {}  # rel path -> description (for manifest)


def out_f32(outdir, rel, arr, desc):
    path = os.path.join(outdir, rel)
    C.write_f32(path, arr)
    DESC[rel] = desc


def out_json(outdir, rel, obj, desc):
    C.dump_json(os.path.join(outdir, rel), obj)
    DESC[rel] = desc


def out_text(outdir, rel, text, desc):
    path = os.path.join(outdir, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as fh:
        fh.write(text)
    DESC[rel] = desc


def write_pcm16(outdir, rel, x, desc):
    """Exact PCM16 (value = int/32768 round-trips bit-exactly)."""
    k = np.round(x.astype(np.float64) * 32768.0)
    assert np.array_equal(k / 32768.0, x.astype(np.float64)), rel
    path = os.path.join(outdir, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    sf.write(path, k.astype(np.int16), C.SR, subtype="PCM_16")
    DESC[rel] = desc


# ----------------------------------------------------------------------------- speaker
SPK_INPUTS = [
    # name, samples, description
    ("s1a_2.0s", lambda: C.load_mono(f"{C.AUDIO}/s1a.wav")[:32000],
     "audio/s1a.wav first 2.0 s (32000 samples, Samantha)"),
    ("s2a_1.5s", lambda: C.load_mono(f"{C.AUDIO}/s2a.wav")[:24000],
     "audio/s2a.wav first 1.5 s (24000 samples, Daniel)"),
    ("s3a_3.0s", lambda: C.load_mono(f"{C.AUDIO}/s3a.wav")[:48000],
     "audio/s3a.wav first 3.0 s (48000 samples, Karen)"),
    ("silence_0.5s", lambda: np.zeros(8000, dtype=np.float32),
     "8000 samples of digital silence (0.5 s)"),
    ("tone_399", lambda: C.tone(399), "first 399 samples of deterministic tone x[n]"),
    ("tone_400", lambda: C.tone(400), "first 400 samples of deterministic tone x[n]"),
    ("tone_401", lambda: C.tone(401), "first 401 samples of deterministic tone x[n]"),
    ("tone_16000", lambda: C.tone(16000), "16000 samples of deterministic tone x[n]=0.3sin(2pi220n/16k)+0.1sin(2pi1330n/16k)"),
]
SPK_COMBOS = [("campplus", "sherpa_campplus"), ("resnet34", "wespeaker_ref"), ("resnet34", "sherpa_nocmn")]
SPK_EXPECTED_ERR_MSG = "torchaudio.compliance.kaldi.fbank asserts window_size(400) <= len(waveform); no fbank/embedding produced"


def build_speaker(outdir, results):
    frames = {}
    nonfinite = []
    speaker_inputs = {}
    for case, make, desc in SPK_INPUTS:
        x = np.ascontiguousarray(make(), dtype=np.float32)
        speaker_inputs[case] = x
        out_f32(outdir, f"speaker/inputs/{case}.input.f32", x, f"input samples float32 [{x.size}] in [-1,1]: {desc}")
    for model, fe in SPK_COMBOS:
        tag = f"{model}__{fe}"
        for case, _, desc in SPK_INPUTS:
            x = speaker_inputs[case]
            base = f"speaker/{tag}"
            try:
                fb = C.frontend(fe, x)
            except AssertionError as exc:
                out_json(outdir, f"{base}/{case}.expected_error.json",
                         {"error_type": "AssertionError", "message": str(exc)[:200],
                          "reason": SPK_EXPECTED_ERR_MSG, "input_samples": int(x.size)},
                         f"expected failure of the torchaudio reference frontend ({tag}) for {case}")
                frames[f"{tag}/{case}"] = "ERROR"
                print(f"{tag:28s} {case:14s} samples={x.size:6d} -> EXPECTED ERROR (torchaudio window > len)")
                continue
            emb = C.embed(model, fb)
            if not np.isfinite(emb).all():
                nonfinite.append(f"{tag}/{case}")
                print(f"  WARNING non-finite reference embedding for {tag}/{case} (frames={fb.shape[0]})")
            out_f32(outdir, f"{base}/{case}.fbank.f32", fb,
                    f"{fe} Kaldi fbank [frames,80] computed on the cropped input {case} itself (CMN={C.FRONTENDS[fe]['cmn']} applied)")
            out_f32(outdir, f"{base}/{case}.embedding.f32", emb,
                    f"{model} raw embedding (not L2-normalised) for {case} via frontend {fe}")
            frames[f"{tag}/{case}"] = int(fb.shape[0])
            print(f"{tag:28s} {case:14s} samples={x.size:6d} frames={fb.shape[0]:4d} emb_dim={emb.shape[0]} norm={np.linalg.norm(emb):.4f}")
    results["speaker_nonfinite_reference_embeddings"] = nonfinite
    results["speaker_frame_counts"] = frames

    # Negative controls for s1a_2.0s
    x = speaker_inputs["s1a_2.0s"]
    correct_cam = C.embed("campplus", C.frontend("sherpa_campplus", x))
    correct_res = C.embed("resnet34", C.frontend("wespeaker_ref", x))
    neg_specs = {
        "campplus__no_globalmean": ("campplus", C.frontend("sherpa_campplus", x, cmn=False), correct_cam),
        "campplus__int16_scale": ("campplus", C.frontend("sherpa_campplus", x, scale=32768.0), correct_cam),
        "campplus__hamming_window": ("campplus", C.frontend("sherpa_campplus", x, window="hamming"), correct_cam),
        "resnet34__scale_1": ("resnet34", C.frontend("wespeaker_ref", x, scale=1.0), correct_res),
        "resnet34__no_cmn": ("resnet34", C.frontend("wespeaker_ref", x, cmn=False), correct_res),
        "resnet34__scale_1_no_cmn": ("resnet34", C.frontend("wespeaker_ref", x, scale=1.0, cmn=False), correct_res),
    }
    neg_res = {}
    for name, (model, fb, correct) in neg_specs.items():
        emb = C.embed(model, fb)
        cs = C.cosine(emb, correct)
        neg_res[name] = {"cos_vs_correct": round(cs, 6), "frames": int(fb.shape[0])}
        out_f32(outdir, f"speaker/negative_controls/s1a_2.0s__{name}.embedding.f32", emb,
                f"NEGATIVE CONTROL {name} embedding of s1a_2.0s (must NOT match the correct embedding)")
        print(f"negative {name:26s} cos_vs_correct={cs:.6f}")
    # sherpa-nocmn comparison for resnet34 on same input
    nocmn = C.embed("resnet34", C.frontend("sherpa_nocmn", x))
    neg_res["resnet34__sherpa_nocmn_frontend_reference"] = {
        "cos_vs_correct": round(C.cosine(nocmn, correct_res), 6), "note": "sherpa frontend (no CMN, snip_edges=False, high_freq=-400) vs wespeaker_ref"}
    print(f"comparison resnet34 sherpa_nocmn vs wespeaker_ref cos={C.cosine(nocmn, correct_res):.6f}")
    results["speaker_negative_controls"] = {
        "reference_clip": "audio/s1a.wav[:2.0 s]",
        "correct": {"campplus": "sherpa_campplus", "resnet34": "wespeaker_ref"},
        "controls": neg_res,
    }
    results["speaker_model_sanity_vs_golden"] = {
        "campplus_full_clip_emb_cos_vs_golden": round(C.cosine(
            C.embed("campplus", C.frontend("sherpa_campplus", C.load_mono(f"{C.AUDIO}/s1a.wav"))),
            np.fromfile(f"{C.ROOT}/golden/s1a.campplus.emb.f32", dtype="<f4")), 12),
        "resnet34_sherpa_nocmn_full_clip_emb_maxabs_vs_golden": float(np.abs(
            C.embed("resnet34", C.frontend("sherpa_nocmn", C.load_mono(f"{C.AUDIO}/s1a.wav")))
            - np.fromfile(f"{C.ROOT}/golden/s1a.resnet34.emb.f32", dtype="<f4")).max()),
    }


# ----------------------------------------------------------------------------- smart turn
def build_smartturn(outdir, results):
    c = {n: C.load_mono(f"{C.AUDIO}/{n}.wav") for n in ["c1", "c2", "i1", "i2", "i3"]}
    cat = np.concatenate([c["c1"], c["c2"], c["i1"], c["i2"], c["i3"]]).astype(np.float32)
    # 155802 samples (< 160000): long_10s needs the concatenation to be TILED (repeated) to reach 10 s
    tiled = np.tile(cat, 2)
    assert tiled.size >= 160000
    nan_in = c["c1"].copy()
    nan_in[100] = np.nan
    nan_in[2000] = np.inf
    nan_in[5000] = -np.inf
    cases = {
        "silence_1s": (np.zeros(16000, dtype=np.float32), "16000 zeros (1 s); left-padded by official path to 8 s", True),
        "short_0.5s": (c["c2"][:8000].copy(), "audio/c2.wav first 8000 samples (0.5 s), left-padded", True),
        "exact_8s": (cat[:128000].copy(), "cat(c1,c2,i1,i2,i3)[:128000] (exactly 8 s, no padding/truncation)", True),
        "long_10s": (tiled[:160000].copy(), "tile(cat(c1,c2,i1,i2,i3),2)[:160000] (10 s; cat is 155802 samples so it is looped once); official path keeps the LAST 8 s", True),
        "nan_input": (nan_in, "c1 with samples[100]=NaN, [2000]=+inf, [5000]=-inf (RAW, not sanitised; sanitised to 0.0 before official path)", False),
        "reversed_c1": (c["c1"][::-1].copy(), "time-reversed audio/c1.wav (negative control)", True),
    }
    st_res = {"tolerance_stats": {}}
    for name, (x, desc, wav_ok) in cases.items():
        raw = np.ascontiguousarray(x, dtype=np.float32)
        out_f32(outdir, f"smartturn/{name}.input.f32", raw, f"input float32 [{raw.size}]: {desc}")
        if wav_ok:
            write_pcm16(outdir, f"smartturn/{name}.wav", raw, f"PCM_16 16 kHz mono wav of {name} (exact int16 values)")
        clean = np.where(np.isfinite(raw), raw, np.float32(0.0)).astype(np.float32)
        mel = C.st_official(clean)
        out_f32(outdir, f"smartturn/{name}.mel.f32", mel,
                f"official Smart Turn log-mel [80,800] of {name} (official numpy path; sanitised input)")
        ref64 = C.st_variant(clean, float64=True)
        d = np.abs(mel.astype(np.float64) - ref64)
        st_res["tolerance_stats"][name] = {"max_abs": float(d.max()), "mean_abs": float(d.mean())}
        print(f"smartturn {name:12s} mel {mel.shape} min={mel.min():.3f} max={mel.max():.3f} "
              f"f32-vs-f64 max={d.max():.3e} mean={d.mean():.3e}")

    # the official path on c1 must equal the algorithm variant with defaults (sanity)
    c1 = c["c1"]
    mel_c1 = C.st_official(c1)
    assert np.array_equal(mel_c1, C.st_variant(c1)), "variant defaults differ from official"
    out_f32(outdir, "smartturn/c1.mel.f32", mel_c1, "official Smart Turn log-mel [80,800] of audio/c1.wav (correct reference)")
    d_c1 = np.abs(mel_c1.astype(np.float64) - C.st_variant(c1, float64=True))
    st_res["tolerance_stats"]["c1"] = {"max_abs": float(d_c1.max()), "mean_abs": float(d_c1.mean())}
    print(f"smartturn c1 (official vs float64) max={d_c1.max():.3e} mean={d_c1.mean():.3e}")
    st_res["c1_official_vs_golden_maxabs"] = float(np.abs(
        mel_c1 - np.fromfile(f"{C.ROOT}/golden/c1.mel.f32", dtype="<f4").reshape(80, 800)).max())

    negs = {
        "no_waveform_norm": C.st_variant(c1, normalise=False),
        "right_pad": C.st_variant(c1, pad="right"),
        "symmetric_hann": C.st_variant(c1, window="symmetric"),
    }
    st_res["c1_negative_controls"] = {}
    for name, m in negs.items():
        dd = np.abs(m - mel_c1)
        st_res["c1_negative_controls"][name] = {"mean_abs_vs_correct": float(dd.mean()), "max_abs_vs_correct": float(dd.max())}
        out_f32(outdir, f"smartturn/negative/c1__{name}.mel.f32", m,
                f"NEGATIVE CONTROL c1 mel with {name} (must differ from smartturn/c1.mel.f32)")
        print(f"negative c1 {name:18s} mean|diff|={dd.mean():.6f} max|diff|={dd.max():.4f}")
    results["smartturn"] = st_res


# ----------------------------------------------------------------------------- matrix
MAT_DURS = [1.0, 1.5, 2.0, 3.0, None]  # None = full clip
MAT_CONFIGS = [
    ("campplus_sherpa_campplus", "campplus", "sherpa_campplus", "PRIMARY CAM++"),
    ("resnet34_wespeaker_ref", "resnet34", "wespeaker_ref", "PRIMARY ResNet34"),
    ("resnet34_sherpa_nocmn", "resnet34", "sherpa_nocmn", "comparison: sherpa ResNet34 (no CMN)"),
]
SPKS = ["s1", "s2", "s3", "s4"]
VOICE = {"s1": "Samantha", "s2": "Daniel", "s3": "Karen", "s4": "Fred"}


def eer_threshold(tgt, imp):
    grid = np.unique(np.concatenate([tgt, imp]))
    best = None
    for t in grid:
        far = float(np.mean(imp >= t))
        frr = float(np.mean(tgt < t))
        gap = abs(far - frr)
        if best is None or gap < best[0]:
            best = (gap, float(t), (far + frr) / 2)
    return best[1], best[2]


def run_matrix(model, fe):
    clips = {}
    for s in SPKS:
        for L in "abcde":
            clips[f"{s}{L}"] = C.load_mono(f"{C.AUDIO2}/{s}{L}.wav")
    emb = {}  # (clip, dur) -> embedding
    for name, w in clips.items():
        for d in MAT_DURS:
            seg = w if d is None else w[: int(round(d * C.SR))]
            emb[(name, d)] = C.embed(model, C.frontend(fe, seg))
    enroll = {}
    for s in SPKS:
        vecs = [C.l2n(emb[(f"{s}{L}", None)]) for L in "abc"]
        enroll[s] = C.l2n(np.mean(vecs, axis=0))
    rows = []
    for k in SPKS:
        for j in SPKS:
            for L in "de":
                for d in MAT_DURS:
                    cs = C.cosine(enroll[k], emb[(f"{j}{L}", d)])
                    rows.append({"enrolled": k, "test_clip": f"{j}{L}", "test_speaker": j,
                                 "duration": "full" if d is None else d,
                                 "dur_s": round(d, 3) if d is not None else round(clips[f"{j}{L}"].size / C.SR, 3),
                                 "cosine": round(cs, 6), "target": k == j})
    return rows, {s: float(np.linalg.norm(clips[f"{s}a"])) for s in SPKS}


def metrics_at(rows, thr):
    def dur_eq(r, d):
        return r["duration"] == d

    tgt15 = [r for r in rows if r["target"] and (r["duration"] == "full" or r["duration"] >= 1.5)]
    imp15 = [r for r in rows if not r["target"] and (r["duration"] == "full" or r["duration"] >= 1.5)]
    acc = lambda rs: float(np.mean([r["cosine"] >= thr for r in rs])) if rs else float("nan")  # noqa: E731
    out = {
        "threshold": round(thr, 6),
        "owner_accept_dur_ge_1.5s": acc(tgt15),
        "owner_accept_2.0s": acc([r for r in rows if r["target"] and dur_eq(r, 2.0)]),
        "impostor_FA_dur_ge_1.5s_overall": acc(imp15),
        "impostor_FA_by_impostor_voice": {},
        "impostor_FA_by_enrolled_to_impostor": {},
        "per_duration": {},
    }
    for j in SPKS:
        out["impostor_FA_by_impostor_voice"][f"{j}({VOICE[j]})"] = acc([r for r in imp15 if r["test_speaker"] == j])
    for k in SPKS:
        for j in SPKS:
            if k == j:
                continue
            out["impostor_FA_by_enrolled_to_impostor"][f"{k}->{j}"] = acc(
                [r for r in imp15 if r["enrolled"] == k and r["test_speaker"] == j])
    for d in MAT_DURS:
        key = "full" if d is None else d
        out["per_duration"][str(key)] = {
            "owner_accept": acc([r for r in rows if r["target"] and r["duration"] == key]),
            "impostor_FA": acc([r for r in rows if not r["target"] and r["duration"] == key]),
        }
    return out


def build_matrix(outdir, results):
    matrix = {"description": "speaker decision matrix; enrollment = L2norm(mean(L2norm(emb of a,b,c at full length))); "
                             "tests = d,e of every speaker; crop = first N s; target = same speaker; impostors = other 3 voices",
              "clips_source": "audio2/ (s1a..s4e, 16 kHz mono PCM16)",
              "durations": ["1.0", "1.5", "2.0", "3.0", "full"],
              "metric_scope": "owner accept and impostor FA are computed over trials with duration >= 1.5 s unless labelled per_duration",
              "models": {}}
    md = ["# Speaker decision matrix (generated by gen/make_fixtures.py)", "",
          "Enrollment: L2norm(mean of L2norm embeddings of clips a,b,c at full length).",
          "Trials: test clips d,e of every speaker x durations {1.0,1.5,2.0,3.0,full}; target = same speaker.",
          "Impostor FA = fraction of impostor trials with cosine >= threshold.", ""]
    for label, model, fe, role in MAT_CONFIGS:
        rows, _ = run_matrix(model, fe)
        tgt = np.array([r["cosine"] for r in rows if r["target"] and (r["duration"] == "full" or r["duration"] >= 1.5)])
        imp = np.array([r["cosine"] for r in rows if (not r["target"]) and (r["duration"] == "full" or r["duration"] >= 1.5)])
        thr_eer, eer = eer_threshold(tgt, imp)
        thr_list = [("0.45", 0.45), ("0.55", 0.55), ("0.65", 0.65), (f"EER({thr_eer:.4f})", thr_eer)]
        mets = {lab: metrics_at(rows, t) for lab, t in thr_list}
        matrix["models"][label] = {"model": model, "frontend": fe, "role": role, "EER": round(eer, 6),
                                   "EER_threshold": round(thr_eer, 6), "trials": rows, "thresholds": mets}
        md += [f"## {label}  ({role}; frontend={fe})", "", f"EER = {eer:.4f} at threshold {thr_eer:.4f}", "",
               "| threshold | owner accept (>=1.5s) | owner accept (2.0s) | impostor FA (>=1.5s) | FA s1 Samantha | FA s2 Daniel | FA s3 Karen | FA s4 Fred |",
               "|---|---|---|---|---|---|---|---|"]
        for lab, _ in thr_list:
            m = mets[lab]
            fa = m["impostor_FA_by_impostor_voice"]
            md.append(f"| {lab} | {m['owner_accept_dur_ge_1.5s']:.3f} | {m['owner_accept_2.0s']:.3f} | "
                      f"{m['impostor_FA_dur_ge_1.5s_overall']:.3f} | " + " | ".join(f"{fa[f'{j}({VOICE[j]})']:.3f}" for j in SPKS) + " |")
        md += ["", "Per duration (owner accept / impostor FA):", "",
               "| threshold | 1.0s | 1.5s | 2.0s | 3.0s | full |", "|---|---|---|---|---|---|"]
        for lab, _ in thr_list:
            pd = mets[lab]["per_duration"]
            md.append(f"| {lab} | " + " | ".join(f"{pd[d]['owner_accept']:.2f} / {pd[d]['impostor_FA']:.2f}"
                                                for d in ["1.0", "1.5", "2.0", "3.0", "full"]) + " |")
        md += ["", "Enrolled -> impostor FA at EER threshold (dur >= 1.5 s):", ""]
        eer_key = f"EER({thr_eer:.4f})"
        pairs = mets[eer_key]["impostor_FA_by_enrolled_to_impostor"]
        md.append("| enrolled \\ impostor | " + " | ".join(f"{j}" for j in SPKS) + " |")
        md.append("|---|---|---|---|---|")
        for k in SPKS:
            md.append(f"| {k} | " + " | ".join("-" if k == j else f"{pairs[f'{k}->{j}']:.3f}" for j in SPKS) + " |")
        md.append("")
    results["speaker_matrix_summary"] = {
        label: {"EER": v["EER"], "EER_threshold": v["EER_threshold"],
                "owner_accept_2.0s_at_EER": v["thresholds"][f"EER({v['EER_threshold']:.4f})"]["owner_accept_2.0s"]}
        for label, v in matrix["models"].items()}
    C.dump_json(os.path.join(outdir, "speaker_matrix.json"), matrix)
    DESC["speaker_matrix.json"] = "decision matrix: all cosine trials (enrolled x test clip x duration) + thresholds 0.45/0.55/0.65/EER per model"
    with open(os.path.join(outdir, "speaker_matrix.md"), "w") as fh:
        fh.write("\n".join(md) + "\n")
    DESC["speaker_matrix.md"] = "human-readable decision matrix tables"


# ----------------------------------------------------------------------------- manifest
def describe_shape(rel, size):
    if rel.endswith(".mel.f32"):
        return "float32", [80, 800]
    if rel.endswith(".fbank.f32"):
        return "float32", [size // (80 * 4), 80]
    if rel.endswith(".f32"):
        return "float32", [size // 4]
    if rel.endswith(".wav"):
        return "int16", None
    if rel.endswith(".json") or rel.endswith(".md") or rel.endswith(".txt"):
        return "text", None
    return "bytes", None


def library_versions():
    import onnx
    import onnxruntime as ort
    import torch
    import torchaudio
    return {"python": platform.python_version(), "platform": platform.platform(), "numpy": np.__version__,
            "soundfile": sf.__version__, "torch": torch.__version__, "torchaudio": torchaudio.__version__,
            "onnxruntime": ort.__version__, "onnx": onnx.__version__, "torch_threads": 1, "ort_intra_op_threads": 1}


def build_manifest(outdir, results):
    files = []
    for root, _, names in os.walk(outdir):
        for n in sorted(names):
            rel = os.path.relpath(os.path.join(root, n), outdir)
            top = rel.split(os.sep)[0]
            if top in ("gen", "src") or "__pycache__" in rel or rel in ("manifest.json", "timing_this_mac.json"):
                continue
            files.append(rel)
    entries = []
    for rel in sorted(files):
        size = os.path.getsize(os.path.join(outdir, rel))
        dtype, shape = describe_shape(rel, size)
        entries.append({"name": rel, "bytes": size, "sha256": C.sha256_file(os.path.join(outdir, rel)),
                        "dtype": dtype, "shape": shape, "reproducible": True,
                        "description": DESC.get(rel, "see name")})
    timing_path = os.path.join(outdir, "timing_this_mac.json")
    if os.path.exists(timing_path):
        entries.append({"name": "timing_this_mac.json", "bytes": os.path.getsize(timing_path),
                        "sha256": C.sha256_file(timing_path), "dtype": "text", "shape": None,
                        "reproducible": False, "description": "machine-dependent timing (NOT reproducible; excluded from regenerate_all)"})
    scripts = {}
    for n in sorted(os.listdir(C.GEN)):
        p = os.path.join(C.GEN, n)
        if os.path.isfile(p) and not n.endswith(".pyc"):
            scripts[f"fixtures/gen/{n}"] = C.sha256_file(p)
    ext = {}
    for n in sorted(os.listdir(C.AUDIO)):
        if n.endswith(".wav"):
            ext[f"audio/{n}"] = {"bytes": os.path.getsize(f"{C.AUDIO}/{n}"), "sha256": C.sha256_file(f"{C.AUDIO}/{n}")}
    for n in sorted(os.listdir(C.AUDIO2)):
        if n.endswith(".wav"):
            ext[f"audio2/{n}"] = {"bytes": os.path.getsize(f"{C.AUDIO2}/{n}"), "sha256": C.sha256_file(f"{C.AUDIO2}/{n}")}
    for n in ["golden/c1.mel.f32", "golden/s1a.campplus.emb.f32", "golden/s1a.resnet34.emb.f32"]:
        ext[n] = {"bytes": os.path.getsize(f"{C.ROOT}/{n}"), "sha256": C.sha256_file(f"{C.ROOT}/{n}")}
    models = {}
    for label, p in [("campplus.onnx", C.CAMPPLUS), ("resnet34.onnx", C.RESNET34), ("smart-turn-v3.2-cpu.onnx", C.ST_MODEL)]:
        models[label] = {"path": p, "bytes": os.path.getsize(p), "sha256": C.sha256_file(p)}
    refs = {}
    srcdir = f"{C.FIX}/src"  # fetched once from GitHub raw; not regenerated
    if os.path.isdir(srcdir):
        for root, _, names in os.walk(srcdir):
            for n in names:
                p = os.path.join(root, n)
                rel = os.path.relpath(p, srcdir)
                refs[rel] = {"url": f"https://raw.githubusercontent.com/wenet-e2e/wespeaker/master/{rel}",
                             "bytes": os.path.getsize(p), "sha256": C.sha256_file(p)}
    return {
        "schema": "vox-voice-fixtures/1",
        "generated_by": "fixtures/gen/make_fixtures.py",
        "determinism": "no RNG used; signals analytic; torch/ORT single thread; float32 little-endian (.f32)",
        "provenance": {"scripts_sha256": scripts, "libraries": library_versions(), "models": models,
                       "say_voices": "Samantha(en_US) Daniel(en_GB) Karen(en_AU) Fred(en_US); see gen/make_audio2.sh; audio2 wav sha in external_inputs (say output not verified bit-reproducible)"},
        "external_inputs": ext,
        "reference_sources": refs,
        "results": results,
        "files": entries,
    }


def main(outdir=C.FIX):
    os.makedirs(outdir, exist_ok=True)
    results = {}
    print("== speaker fixtures")
    build_speaker(outdir, results)
    print("== smart turn fixtures")
    build_smartturn(outdir, results)
    print("== speaker matrix")
    build_matrix(outdir, results)
    # results.json is deterministic and also a file in the manifest
    out_json(outdir, "results.json", results, "all numeric results: negative-control cosines, smart turn tolerance stats, matrix summary, frame counts")
    manifest = build_manifest(outdir, results)
    C.dump_json(os.path.join(outdir, "manifest.json"), manifest)
    print(f"manifest: {len(manifest['files'])} files -> {os.path.join(outdir, 'manifest.json')}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else C.FIX))
