#!/usr/bin/env python3
"""SD3: capture-shaped speaker evaluation (deterministic).

Each buffer mirrors the production VAD-kept buffer:
    buf = noise(450 ms) + clip_crop + noise(450 ms)        (float32, 16 kHz)
Frontend = wespeaker_ref (kaldi fbank, CMN) -> ResNet34 ORT session (gen/common.py).

Noise (documented integer seed, one numpy default_rng stream per buffer):
    seed = 10000*spk_idx + 100*letter_idx + crop_idx
      spk_idx    = N for speakerN (s1->1 ... s4->4)
      letter_idx = a..e -> 0..4
      crop_idx   = 0..6 for S in [0.4, 0.6, 0.8, 1.0, 1.5, 2.0, 3.0] s, 7 = full clip
    rng = numpy.random.default_rng(seed)
    pre  = (rng.standard_normal(7200) * 1e-4).astype(float32)   # 450 ms, sigma 1e-4
    post = (rng.standard_normal(7200) * 1e-4).astype(float32)   # next 7200 draws, same rng

Crop buffers: buffer_ms = S*1000 + 900 ; full clip: buffer_ms = len/16 + 900.
Enrollment (per speaker): full-clip capture buffers a,b,c ->
    l2n(mean_i(l2n(e_i)))   (raw-clip enrollment is also reported for comparison)
Trials: clips d,e of every speaker at every crop; target = same speaker; impostors = other three.
Cosines are rounded to 6 dp (same convention as make_fixtures.py); analysis uses the rounded values.

Usage: python3 make_capture_matrix.py [OUTDIR]      (default /tmp/vox-voice-assets/fixtures)
Writes OUTDIR/capture/*.f32, OUTDIR/speaker_capture_matrix.{json,md}, OUTDIR/capture_manifest.json
"""
import json
import os
import platform
import sys
from collections import defaultdict

import numpy as np
import soundfile as sf

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common as C  # noqa: E402

SPKS = ["s1", "s2", "s3", "s4"]
VOICE = {"s1": "Samantha", "s2": "Daniel", "s3": "Karen", "s4": "Fred"}
LETTERS = "abcde"
ENROLL_LETTERS = "abc"
TEST_LETTERS = "de"
CROPS_S = [0.4, 0.6, 0.8, 1.0, 1.5, 2.0, 3.0]
FULL_IDX = len(CROPS_S)  # 7
CROP_ORDER = [f"{s:.1f} s" for s in CROPS_S] + ["full"]
PAD_SAMPLES = 7200  # 450 ms @ 16 kHz
SIGMA = 1e-4
FRONTEND = "wespeaker_ref"
MODEL = "resnet34"
RAW_MODEL_KEY = "resnet34_wespeaker_ref"
MIN_CANDIDATES = [1500, 2000, 2500, 3000]
MARGIN_MIN = 0.05


def crop_tag(crop_idx):
    return "full" if crop_idx == FULL_IDX else f"crop{CROPS_S[crop_idx]:.1f}s"


def crop_label(crop_idx):
    return "full" if crop_idx == FULL_IDX else f"{CROPS_S[crop_idx]:.1f} s"


def crop_s_value(crop_idx):
    return None if crop_idx == FULL_IDX else CROPS_S[crop_idx]


def noise_pair(spk, letter, crop_idx):
    seed = 10000 * int(spk[1:]) + 100 * LETTERS.index(letter) + crop_idx
    rng = np.random.default_rng(seed)
    pre = (rng.standard_normal(PAD_SAMPLES) * SIGMA).astype(np.float32)
    post = (rng.standard_normal(PAD_SAMPLES) * SIGMA).astype(np.float32)
    return seed, pre, post


def clip_crop(wave, crop_idx):
    if crop_idx == FULL_IDX:
        return wave
    return wave[: int(round(CROPS_S[crop_idx] * C.SR))]


def build_buffer(spk, letter, crop_idx, wave):
    seed, pre, post = noise_pair(spk, letter, crop_idx)
    seg = clip_crop(wave, crop_idx).astype(np.float32)
    buf = np.concatenate([pre, seg, post]).astype(np.float32)
    return buf, seed


# ----------------------------------------------------------------------------- analysis
def fmt(x, nd=3):
    return "n/a" if x is None else f"{x:.{nd}f}"


def group_rows(rows, keyfn):
    groups = defaultdict(list)
    for r in rows:
        groups[keyfn(r)].append(r)
    out = []
    for key in sorted(groups):
        g = groups[key]
        own = [r["cosine"] for r in g if r["target"]]
        imp = [r["cosine"] for r in g if not r["target"]]
        margin = round(min(own) - max(imp), 6)
        out.append({
            "key": key,
            "crops": "/".join(sorted({r["crop"] for r in g})),
            "buffer_ms": round(float(np.mean([r["buffer_ms"] for r in g])), 4),
            "n_owner": len(own),
            "owner_min": round(min(own), 6),
            "owner_mean": round(float(np.mean(own)), 6),
            "n_impostor": len(imp),
            "impostor_max": round(max(imp), 6),
            "impostor_mean": round(float(np.mean(imp)), 6),
            "margin": margin,
            "margin_ok": bool(margin >= MARGIN_MIN),
        })
    return out


def derive_min_verify(by_ms):
    checks = {}
    chosen = None
    for cand in MIN_CANDIDATES:
        grp = [g for g in by_ms if g["buffer_ms"] >= cand]
        worst = min(g["margin"] for g in grp) if grp else None
        passes = bool(grp) and worst >= MARGIN_MIN
        checks[str(cand)] = {
            "n_buffer_ms_groups": len(grp),
            "worst_margin": worst,
            "passes": passes,
            "failing_buffer_ms": [g["buffer_ms"] for g in grp if not g["margin_ok"]],
        }
        if passes and chosen is None:
            chosen = cand
    best = max(MIN_CANDIDATES, key=lambda c: checks[str(c)]["worst_margin"]
               if checks[str(c)]["worst_margin"] is not None else -1e9)
    return {
        "rule": "smallest candidate in {1500,2000,2500,3000} such that every buffer_ms >= candidate has "
                "min owner cos - max impostor cos >= 0.05",
        "chosen": chosen,
        "best_candidate_if_none": None if chosen is not None else best,
        "best_candidate_worst_margin": None if chosen is not None else checks[str(best)]["worst_margin"],
        "used_for_thresholds": chosen if chosen is not None else best,
        "candidates": checks,
    }


def derive_thresholds(rows, min_ms):
    sel = [r for r in rows if r["buffer_ms"] >= min_ms]
    own = [r["cosine"] for r in sel if r["target"]]
    imp = [r["cosine"] for r in sel if not r["target"]]
    mid = (max(imp) + min(own)) / 2.0
    bal = round(mid, 2)
    return {
        "min_verify_buffer_ms_used": min_ms,
        "n_owner": len(own),
        "n_impostor": len(imp),
        "min_owner_cos": round(min(own), 6),
        "max_impostor_cos": round(max(imp), 6),
        "midpoint_raw": round(mid, 6),
        "Balanced": round(bal, 2),
        "Lenient": round(bal - 0.10, 2),
        "Strict": round(bal + 0.05, 2),
    }


def rate(rs, thr):
    return float(np.mean([r["cosine"] >= thr for r in rs])) if rs else None


def accept_report(rows, thr):
    own = [r for r in rows if r["target"]]
    imp = [r for r in rows if not r["target"]]
    return {
        "threshold": round(thr, 6),
        "owner_accept": rate(own, thr),
        "n_owner": len(own),
        "impostor_FA_overall": rate(imp, thr),
        "n_impostor": len(imp),
        "impostor_FA_by_voice": {f"{j}({VOICE[j]})": rate([r for r in imp if r["test_speaker"] == j], thr)
                                 for j in SPKS},
        "owner_accept_by_speaker": {f"{k}({VOICE[k]})": rate([r for r in own if r["enrolled"] == k], thr)
                                    for k in SPKS},
    }


def per_voice_max(rows):
    # keyed by crop level: every crop level contains all four voices (full-clip lengths differ per clip)
    out = []
    for crop in CROP_ORDER:
        sel = [r for r in rows if r["crop"] == crop]
        row = {"crop": crop, "buffer_ms": round(float(np.mean([r["buffer_ms"] for r in sel])), 4)}
        for j in SPKS:
            vals = [r["cosine"] for r in sel if not r["target"] and r["test_speaker"] == j]
            row[f"impostor_{j}_{VOICE[j]}_max"] = round(max(vals), 6)
        out.append(row)
    return out


def pair_report(rows):
    out = []
    for crop in CROP_ORDER:
        def sel(k, j):
            return [r["cosine"] for r in rows if r["crop"] == crop and r["enrolled"] == k and r["test_speaker"] == j]
        s1s3, s3s1 = sel("s1", "s3"), sel("s3", "s1")
        own1, own3 = sel("s1", "s1"), sel("s3", "s3")
        out.append({
            "crop": crop,
            "buffer_ms": round(float(np.mean([r["buffer_ms"] for r in rows if r["crop"] == crop])), 4),
            "owner_s1_min": round(min(own1), 6), "owner_s3_min": round(min(own3), 6),
            "s1_enrolled_vs_karen_max": round(max(s1s3), 6), "s1_enrolled_vs_karen_mean": round(float(np.mean(s1s3)), 6),
            "s3_enrolled_vs_samantha_max": round(max(s3s1), 6), "s3_enrolled_vs_samantha_mean": round(float(np.mean(s3s1)), 6),
        })
    return out


def raw_matrix_block(raw_model):
    rows = []
    for t in raw_model["trials"]:
        if t["duration"] == "full" or float(t["duration"]) >= 1.5:
            rows.append({"enrolled": t["enrolled"], "test_speaker": t["test_speaker"], "cosine": t["cosine"],
                         "target": t["target"], "buffer_ms": None})
    block = {"model": RAW_MODEL_KEY, "scope": "raw clips, duration>=1.5 s or full (speaker_matrix.json convention)",
             "thresholds": {}}
    for key, rec in raw_model["thresholds"].items():
        thr = rec["threshold"]
        rep = accept_report(rows, thr)
        block["thresholds"][key] = rep
        # cross-check against the numbers stored in speaker_matrix.json
        stored_own = rec["owner_accept_dur_ge_1.5s"]
        stored_fa = rec["impostor_FA_dur_ge_1.5s_overall"]
        assert abs(stored_own - rep["owner_accept"]) < 1e-9, (key, stored_own, rep["owner_accept"])
        assert abs(stored_fa - rep["impostor_FA_overall"]) < 1e-9, (key, stored_fa, rep["impostor_FA_overall"])
        for j in SPKS:
            sv = rec["impostor_FA_by_impostor_voice"][f"{j}({VOICE[j]})"]
            assert abs(sv - rep["impostor_FA_by_voice"][f"{j}({VOICE[j]})"]) < 1e-9, (key, j)
    block["cross_check"] = "recomputed owner accept / impostor FA equal speaker_matrix.json stored values (asserted < 1e-9)"
    return block


def library_versions():
    import onnxruntime as ort
    import torch
    import torchaudio
    return {"python": platform.python_version(), "platform": platform.platform(), "numpy": np.__version__,
            "soundfile": sf.__version__, "torch": torch.__version__, "torchaudio": torchaudio.__version__,
            "onnxruntime": ort.__version__, "torch_threads": 1, "ort_intra_op_threads": 1}


# ----------------------------------------------------------------------------- main
def main(outdir):
    cap_dir = os.path.join(outdir, "capture")
    os.makedirs(cap_dir, exist_ok=True)
    clips = {f"{s}{L}": C.load_mono(f"{C.AUDIO2}/{s}{L}.wav") for s in SPKS for L in LETTERS}
    emb = {}
    info = {}
    entries = []

    def embed_buffer(spk, letter, crop_idx):
        key = (spk, letter, crop_idx)
        if key in emb:
            return
        buf, seed = build_buffer(spk, letter, crop_idx, clips[f"{spk}{letter}"])
        rel = f"capture/{spk}{letter}_{crop_tag(crop_idx)}.f32"
        path = os.path.join(outdir, rel)
        C.write_f32(path, buf)
        buffer_ms = round(buf.size * 1000.0 / C.SR, 4)
        info[key] = {"file": rel, "seed": seed, "buffer_ms": buffer_ms, "samples": int(buf.size)}
        entries.append({
            "name": rel, "bytes": os.path.getsize(path), "sha256": C.sha256_file(path),
            "dtype": "float32", "shape": [int(buf.size)], "reproducible": True,
            "speaker": spk, "clip": f"{spk}{letter}", "crop": crop_label(crop_idx),
            "crop_s": crop_s_value(crop_idx), "buffer_ms": buffer_ms, "seed": seed,
            "description": (f"capture buffer {spk}{letter} crop={crop_label(crop_idx)}: noise450ms(seed={seed}) + clip + "
                            f"noise450ms(same rng); buffer_ms={buffer_ms}; float32 LE"),
        })
        emb[key] = C.embed(MODEL, C.frontend(FRONTEND, buf))

    # enrollment (capture-shaped FULL clips a,b,c) and raw-clip enrollment
    enroll, enroll_raw, enroll_info = {}, {}, {}
    for s in SPKS:
        for L in ENROLL_LETTERS:
            embed_buffer(s, L, FULL_IDX)
        enroll[s] = C.l2n(np.mean([C.l2n(emb[(s, L, FULL_IDX)]) for L in ENROLL_LETTERS], axis=0))
        raw_vecs = [C.l2n(C.embed(MODEL, C.frontend(FRONTEND, clips[f"{s}{L}"]))) for L in ENROLL_LETTERS]
        enroll_raw[s] = C.l2n(np.mean(raw_vecs, axis=0))
        enroll_info[s] = {
            "enrollment_buffers": [info[(s, L, FULL_IDX)]["file"] for L in ENROLL_LETTERS],
            "enrollment_buffer_ms": [info[(s, L, FULL_IDX)]["buffer_ms"] for L in ENROLL_LETTERS],
            "capture_vs_raw_enrollment_cosine": round(C.cosine(enroll[s], enroll_raw[s]), 6),
        }

    # trials
    for s in SPKS:
        for L in TEST_LETTERS:
            for ci in range(FULL_IDX + 1):
                embed_buffer(s, L, ci)
    rows = []
    for k in SPKS:
        for j in SPKS:
            for L in TEST_LETTERS:
                for ci in range(FULL_IDX + 1):
                    cs = round(C.cosine(enroll[k], emb[(j, L, ci)]), 6)
                    rec = info[(j, L, ci)]
                    rows.append({"enrolled": k, "test_speaker": j, "test_clip": f"{j}{L}",
                                 "crop": crop_label(ci), "S_s": crop_s_value(ci),
                                 "buffer_ms": rec["buffer_ms"], "cosine": cs, "target": k == j,
                                 "buffer_file": rec["file"]})

    by_ms = group_rows(rows, lambda r: r["buffer_ms"])
    by_crop = group_rows(rows, lambda r: r["crop"])
    min_v = derive_min_verify(by_ms)
    used = min_v["used_for_thresholds"]
    rows_ge = [r for r in rows if r["buffer_ms"] >= used]
    thr_cap = derive_thresholds(rows, used)
    cap_acc = {name: accept_report(rows_ge, thr_cap[name]) for name in ["Balanced", "Lenient", "Strict"]}
    with open(os.path.join(C.FIX, "speaker_matrix.json")) as fh:
        raw_model = json.load(fh)["models"][RAW_MODEL_KEY]
    raw_block = raw_matrix_block(raw_model)
    short_rows = [g for g in by_ms if g["buffer_ms"] < used]
    voice_max = per_voice_max(rows)
    pairs = pair_report(rows)

    # ------------------------------------------------------------ JSON
    summary = {
        "MIN_VERIFY_BUFFER_MS": min_v["chosen"],
        "MIN_VERIFY_note": ("none qualifies" if min_v["chosen"] is None else "smallest passing candidate"),
        "thresholds_capture": thr_cap,
        "raw_matrix_thresholds": {k: v["threshold"] for k, v in raw_block["thresholds"].items()},
    }
    out = {
        "description": "SD3 capture-shaped speaker evaluation: buffers = noise450ms + clip crop + noise450ms; "
                       "wespeaker_ref CMN fbank -> ResNet34; enrollment = l2n(mean(l2n(e_a,e_b,e_c))) of full-clip buffers",
        "seed_formula": "seed = 10000*spk_idx + 100*letter_idx + crop_idx; spk_idx=N(sN), letter_idx=a..e->0..4, "
                        "crop_idx=0..6 for S in [0.4,0.6,0.8,1.0,1.5,2.0,3.0], 7=full; rng=default_rng(seed); "
                        "pre=standard_normal(7200)*1e-4, post=next standard_normal(7200)*1e-4, cast float32",
        "buffer_format": {"pad_ms": 450, "pad_samples": PAD_SAMPLES, "sigma": SIGMA, "sr": C.SR,
                          "buffer_ms": "S*1000+900 for crops; len/16+900 for full clip"},
        "crops_s": CROPS_S + ["full"],
        "enrollment": {"per_speaker": enroll_info},
        "constants": summary,
        "min_verify_derivation": min_v,
        "capture_accept_at_buffer_ms_ge_used": cap_acc,
        "raw_matrix_accept": raw_block,
        "by_buffer_ms": by_ms,
        "by_crop_level": by_crop,
        "short_buffers_below_used": short_rows,
        "per_impostor_voice_max": voice_max,
        "pair_samantha_karen": pairs,
        "trials": rows,
    }
    C.dump_json(os.path.join(outdir, "speaker_capture_matrix.json"), out)

    # ------------------------------------------------------------ Markdown
    md = ["# SD3 capture-shaped speaker evaluation", "",
          "Generated by gen/make_capture_matrix.py. Buffer = noise(450 ms) + clip crop + noise(450 ms) "
          "(float32, sigma 1e-4, seeded). Frontend wespeaker_ref (CMN) -> ResNet34.",
          "Enrollment = L2norm(mean(L2norm(e_a,e_b,e_c))) of FULL-clip capture buffers. "
          "Trials: clips d,e of all 4 voices at every crop; target = same voice; impostors = other 3.", "",
          "## Constants", "",
          f"- MIN_VERIFY_BUFFER_MS: **{min_v['chosen'] if min_v['chosen'] is not None else 'NONE QUALIFIES'}**",
          f"- thresholds computed on buffer_ms >= {used}"
          + ("" if min_v["chosen"] is not None else f" (provisional: best candidate {min_v['best_candidate_if_none']}, "
                                                   f"worst margin {fmt(min_v['best_candidate_worst_margin'], 4)})"),
          f"- Balanced = {thr_cap['Balanced']:.2f} (midpoint raw {thr_cap['midpoint_raw']:.4f} = "
          f"(max impostor {thr_cap['max_impostor_cos']:.4f} + min owner {thr_cap['min_owner_cos']:.4f})/2)",
          f"- Lenient = {thr_cap['Lenient']:.2f}   Strict = {thr_cap['Strict']:.2f}", "",
          "### MIN_VERIFY_BUFFER_MS candidates", "",
          "| candidate ms | groups >= cand | worst margin | passes | failing buffer_ms |", "|---|---|---|---|---|"]
    for cand in MIN_CANDIDATES:
        c = min_v["candidates"][str(cand)]
        md.append(f"| {cand} | {c['n_buffer_ms_groups']} | {fmt(c['worst_margin'], 4)} | {c['passes']} | "
                  f"{', '.join(str(x) for x in c['failing_buffer_ms']) or '-'} |")
    md += ["", f"## Accept rates, capture buffers with buffer_ms >= {used}", "",
           "| threshold | owner accept | impostor FA overall | FA s1 Samantha | FA s2 Daniel | FA s3 Karen | FA s4 Fred |",
           "|---|---|---|---|---|---|---|"]
    for name in ["Balanced", "Lenient", "Strict"]:
        a = cap_acc[name]
        md.append(f"| {name} {a['threshold']:.2f} | {fmt(a['owner_accept'])} | {fmt(a['impostor_FA_overall'])} | "
                  + " | ".join(fmt(a["impostor_FA_by_voice"][f'{j}({VOICE[j]})']) for j in SPKS) + " |")
    md += ["", f"n_owner={cap_acc['Balanced']['n_owner']}, n_impostor={cap_acc['Balanced']['n_impostor']}", "",
           "## Raw-clip matrix thresholds (resnet34_wespeaker_ref; raw trials dur>=1.5 s or full)", "",
           "| threshold | owner accept | impostor FA overall | FA s1 Samantha | FA s2 Daniel | FA s3 Karen | FA s4 Fred |",
           "|---|---|---|---|---|---|---|"]
    for key, a in raw_block["thresholds"].items():
        md.append(f"| {key} ({a['threshold']:.4f}) | {fmt(a['owner_accept'])} | {fmt(a['impostor_FA_overall'])} | "
                  + " | ".join(fmt(a["impostor_FA_by_voice"][f'{j}({VOICE[j]})']) for j in SPKS) + " |")
    md += ["", "## Per buffer_ms: owner vs impostor cosines", "",
           "| buffer_ms | crop | n_own | owner min | owner mean | n_imp | impostor max | impostor mean | margin | margin>=0.05 |",
           "|---|---|---|---|---|---|---|---|---|---|"]
    for g in by_ms:
        md.append(f"| {g['buffer_ms']:.3f} | {g['crops']} | {g['n_owner']} | {g['owner_min']:.4f} | {g['owner_mean']:.4f} | "
                  f"{g['n_impostor']} | {g['impostor_max']:.4f} | {g['impostor_mean']:.4f} | {g['margin']:+.4f} | "
                  f"{'yes' if g['margin_ok'] else 'NO'} |")
    md += ["", "Note: full-clip buffer_ms values are per clip, so those groups have n_owner=1 (one clip per value).", "",
           "## Pooled by crop level", "",
           "| crop | buffer_ms | n_own | owner min | owner mean | n_imp | impostor max | impostor mean | margin |",
           "|---|---|---|---|---|---|---|---|---|"]
    for g in by_crop:
        md.append(f"| {g['key']} | {g['buffer_ms']:.1f} | {g['n_owner']} | {g['owner_min']:.4f} | {g['owner_mean']:.4f} | "
                  f"{g['n_impostor']} | {g['impostor_max']:.4f} | {g['impostor_mean']:.4f} | {g['margin']:+.4f} |")
    md += ["", "## Per impostor voice: max cosine (any enrolled speaker, both clips)", "",
           "| crop | buffer_ms | " + " | ".join(f"{j} {VOICE[j]}" for j in SPKS) + " |", "|---|---|---|---|---|---|"]
    for v in voice_max:
        md.append(f"| {v['crop']} | {v['buffer_ms']:.1f} | " + " | ".join(
            f"{v[f'impostor_{j}_{VOICE[j]}_max']:.4f}" for j in SPKS) + " |")
    md += ["", "## Samantha (s1) vs Karen (s3) pair, by crop level", "",
           "| crop | buffer_ms | owner s1 min | owner s3 min | s1 enrolled vs Karen max | mean | s3 enrolled vs Samantha max | mean |",
           "|---|---|---|---|---|---|---|---|"]
    for p in pairs:
        md.append(f"| {p['crop']} | {p['buffer_ms']:.1f} | {p['owner_s1_min']:.4f} | {p['owner_s3_min']:.4f} | "
                  f"{p['s1_enrolled_vs_karen_max']:.4f} | {p['s1_enrolled_vs_karen_mean']:.4f} | "
                  f"{p['s3_enrolled_vs_samantha_max']:.4f} | {p['s3_enrolled_vs_samantha_mean']:.4f} |")
    md += ["", "## Enrollment: capture-shaped vs raw-clip enrollment vector", "",
           "| speaker | cosine(capture enrollment, raw enrollment) |", "|---|---|"]
    for s in SPKS:
        md.append(f"| {s} {VOICE[s]} | {enroll_info[s]['capture_vs_raw_enrollment_cosine']:.6f} |")
    md += ["", f"## Short buffers (buffer_ms < {used}): skipped by a MIN_VERIFY_BUFFER_MS gate", "",
           "| buffer_ms | crop | n_own | owner min | owner mean | n_imp | impostor max | impostor mean | margin |",
           "|---|---|---|---|---|---|---|---|---|"]
    for g in short_rows:
        md.append(f"| {g['buffer_ms']:.3f} | {g['crops']} | {g['n_owner']} | {g['owner_min']:.4f} | {g['owner_mean']:.4f} | "
                  f"{g['n_impostor']} | {g['impostor_max']:.4f} | {g['impostor_mean']:.4f} | {g['margin']:+.4f} |")
    with open(os.path.join(outdir, "speaker_capture_matrix.md"), "w") as fh:
        fh.write("\n".join(md) + "\n")

    # ------------------------------------------------------------ manifest
    for name, desc in [("speaker_capture_matrix.json", "SD3 capture matrix: every cosine (enrolled x test voice x clip x crop) + derived constants"),
                       ("speaker_capture_matrix.md", "SD3 capture matrix tables (margins, thresholds, accept rates, pair, short buffers)")]:
        path = os.path.join(outdir, name)
        entries.append({"name": name, "bytes": os.path.getsize(path), "sha256": C.sha256_file(path),
                        "dtype": "text", "shape": None, "reproducible": True, "description": desc})
    entries.sort(key=lambda e: e["name"])
    scripts = {}
    for n in ["common.py", "make_capture_matrix.py", "verify_capture_manifest.py"]:
        p = os.path.join(C.GEN, n)
        if os.path.isfile(p):
            scripts[f"fixtures/gen/{n}"] = {"path": p, "sha256": C.sha256_file(p)}
    ext = {}
    for s in SPKS:
        for L in LETTERS:
            p = f"{C.AUDIO2}/{s}{L}.wav"
            ext[f"audio2/{s}{L}.wav"] = {"path": p, "bytes": os.path.getsize(p), "sha256": C.sha256_file(p)}
    manifest = {
        "schema": "vox-voice-capture/1",
        "generated_by": "fixtures/gen/make_capture_matrix.py",
        "determinism": "numpy default_rng per buffer (seed formula in speaker_capture_matrix.json); float32 LE .f32; "
                       "torch/ORT single thread; cosines rounded to 6 dp",
        "provenance": {
            "scripts_sha256": scripts,
            "libraries": library_versions(),
            "models": {"resnet34.onnx": {"path": C.RESNET34, "bytes": os.path.getsize(C.RESNET34),
                                         "sha256": C.sha256_file(C.RESNET34)}},
            "external_inputs": ext,
        },
        "results": summary,
        "files": entries,
    }
    C.dump_json(os.path.join(outdir, "capture_manifest.json"), manifest)

    print(f"buffers: {sum(1 for e in entries if e['name'].startswith('capture/'))} .f32 files in {cap_dir}")
    print(f"trials: {len(rows)} cosines; MIN_VERIFY_BUFFER_MS={min_v['chosen']}; used={used}")
    print(f"thresholds: Balanced={thr_cap['Balanced']:.2f} Lenient={thr_cap['Lenient']:.2f} Strict={thr_cap['Strict']:.2f}")
    print(f"manifest: {len(entries)} entries -> {os.path.join(outdir, 'capture_manifest.json')}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else C.FIX))
