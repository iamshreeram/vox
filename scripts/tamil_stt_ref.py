#!/usr/bin/env python3
"""Offline reference for the Tamil speech pack, and graph inspection of the pack.

Two modes:

  --inspect PACK_DIR
      Prints the graph facts of encoder.onnx and ctc_decoder.onnx (inputs, outputs, dtypes,
      shapes, opset imports, external-data references). Loads with load_external_data=False,
      so no weights are read from the external file. Use it to fill the README of the fake
      pack ("Real pack graph facts").

  PACK_DIR [WAV ...] [--preprocessor PATH] [--out PATH]
      Runs the CTC path of the pack's model_onnx.py exactly: TorchScript preprocessor ->
      onnxruntime encoder -> onnxruntime CTC decoder -> language mask -> log_softmax ->
      argmax -> collapse repeats -> drop blank -> join -> '\u2581' to space -> strip.
      Writes a JSON file that src-tauri/src/managers/indic_stt_ort_tests.rs reads through
      VOX_INDIC_REF. The default WAVs are always included; extra WAVs are appended.

Run with the offline venv: /tmp/vox-work/venv3/bin/python scripts/tamil_stt_ref.py --help

The Rust side reads the WAV files itself, so this script accepts only 16 kHz mono WAVs and
refuses anything else instead of resampling (a resampled copy would not be the file Rust reads).
"""

import argparse
import json
import platform
import sys
from pathlib import Path

import numpy as np

LANG = "ta"
BLANK_ID = 256
SAMPLE_RATE = 16_000
CHECK_CHANNELS = 8
DEFAULT_PREPROCESSOR = Path("/tmp/vox-work/indic/assets/preprocessor.ts")
DEFAULT_OUT = Path("/tmp/vox-work/indic/ref.json")
DEFAULT_WAVS = [
    Path("/tmp/vox-work/indic/say_ta_1.wav"),
    Path("/tmp/vox-work/indic/say_ta_2.wav"),
]
GRAPH_FILES = ["encoder.onnx", "ctc_decoder.onnx"]


def dtype_name(elem_type: int) -> str:
    """ONNX element type number as its name (for example FLOAT or INT64)."""
    import onnx

    return onnx.TensorProto.DataType.Name(elem_type)


def shape_text(value_info) -> str:
    """Dimensions of a graph input or output: numbers, symbolic names, or '?' when unknown."""
    tensor = value_info.type.tensor_type
    if not tensor.HasField("shape"):
        return "?"
    dims = []
    for dim in tensor.shape.dim:
        if dim.HasField("dim_value"):
            dims.append(str(dim.dim_value))
        elif dim.dim_param:
            dims.append(dim.dim_param)
        else:
            dims.append("?")
    return "[" + ", ".join(dims) + "]"


def external_refs(tensor) -> str:
    """Location, offset and length of an externally stored tensor, as text."""
    refs = {entry.key: entry.value for entry in tensor.external_data}
    return f"location={refs.get('location')} offset={refs.get('offset')} length={refs.get('length')}"


def inspect_graph(path: Path) -> None:
    """Print the graph facts of one ONNX file without reading external weights."""
    import onnx

    model = onnx.load(str(path), load_external_data=False)
    print(f"== {path.name}: {path.stat().st_size} bytes")
    print(f"ir_version: {model.ir_version}; producer: {model.producer_name} {model.producer_version}")
    for opset in model.opset_import:
        print(f"opset: domain='{opset.domain}' version={opset.version}")
    for value in model.graph.input:
        print(f"input: {value.name} {dtype_name(value.type.tensor_type.elem_type)} {shape_text(value)}")
    for value in model.graph.output:
        print(f"output: {value.name} {dtype_name(value.type.tensor_type.elem_type)} {shape_text(value)}")
    external_initializers = 0
    for tensor in model.graph.initializer:
        if tensor.data_location == onnx.TensorProto.EXTERNAL:
            external_initializers += 1
            print(f"external initializer: {tensor.name} {external_refs(tensor)}")
    external_attributes = 0
    for node in model.graph.node:
        for attribute in node.attribute:
            if attribute.type == onnx.AttributeProto.TENSOR and attribute.t.data_location == onnx.TensorProto.EXTERNAL:
                external_attributes += 1
                print(f"external node tensor: {node.name or node.op_type} {external_refs(attribute.t)}")
    print(
        f"initializers: {len(model.graph.initializer)} total, {external_initializers} external; "
        f"node tensor attributes external: {external_attributes}"
    )


def run_inspect(pack_dir: Path) -> int:
    """Inspect both graphs of a pack directory. Returns the process exit code."""
    status = 0
    for name in GRAPH_FILES:
        path = pack_dir / name
        if not path.is_file():
            print(f"missing: {path}", file=sys.stderr)
            status = 1
            continue
        inspect_graph(path)
    return status


def read_wav_16k_mono(path: Path) -> np.ndarray:
    """Read a WAV as float32 mono. Only 16 kHz mono input is accepted (no resampling)."""
    import soundfile as sf

    data, rate = sf.read(str(path), dtype="float32", always_2d=True)
    if rate != SAMPLE_RATE or data.shape[1] != 1:
        raise SystemExit(
            f"{path}: need {SAMPLE_RATE} Hz mono, found {rate} Hz {data.shape[1]} channel(s); "
            "convert the file first (the Rust test reads the same file without resampling)"
        )
    return np.ascontiguousarray(data[:, 0])


def load_head(pack_dir: Path):
    """Tamil vocabulary and the boolean column mask (masked index space = vocab index)."""
    vocab = json.loads((pack_dir / "vocab.json").read_text(encoding="utf-8"))[LANG]
    mask = np.array(json.loads((pack_dir / "language_masks.json").read_text(encoding="utf-8"))[LANG], dtype=bool)
    return vocab, mask


def reference_one(path: Path, preprocessor, encoder, ctc, vocab, mask) -> dict:
    """The CTC path of model_onnx.py for one WAV, with the intermediate facts recorded."""
    import torch

    samples = read_wav_16k_mono(path)
    wav = torch.from_numpy(samples)[None]
    length = torch.tensor([wav.shape[-1]])
    audio_signal, feature_length = preprocessor(input_signal=wav, length=length)
    outputs, encoded_lengths = encoder.run(
        ["outputs", "encoded_lengths"],
        {"audio_signal": audio_signal.numpy(), "length": feature_length.numpy()},
    )
    logprobs = ctc.run(["logprobs"], {"encoder_output": outputs})[0]
    logprobs = torch.from_numpy(logprobs[:, :, mask]).log_softmax(dim=-1)
    indices = torch.argmax(logprobs[0], dim=-1)
    collapsed = torch.unique_consecutive(indices, dim=-1)
    text = "".join([vocab[x] for x in collapsed.tolist() if x != BLANK_ID]).replace("\u2581", " ").strip()

    channels = outputs.shape[1]
    return {
        "wav": str(path.resolve()),
        "samples": int(samples.shape[0]),
        "mel_frames": int(audio_signal.shape[-1]),
        "encoder_shape": [int(d) for d in outputs.shape],
        "encoder_head": [float(v) for v in outputs[0, 0, :CHECK_CHANNELS].tolist()],
        "encoder_tail": [float(v) for v in outputs[0, max(0, channels - CHECK_CHANNELS):, -1].tolist()],
        "encoded_lengths": [int(v) for v in encoded_lengths.tolist()],
        "ctc_argmax_path": [int(v) for v in indices.tolist()],
        "text": text,
    }


def versions() -> dict:
    """Runtime versions, recorded so the reference can be regenerated on the same stack."""
    import onnx
    import onnxruntime
    import soundfile
    import torch

    return {
        "python": platform.python_version(),
        "platform": platform.platform(),
        "numpy": np.__version__,
        "torch": torch.__version__,
        "onnxruntime": onnxruntime.__version__,
        "onnx": onnx.__version__,
        "soundfile": soundfile.__version__,
    }


def run_reference(pack_dir: Path, preprocessor_path: Path, out_path: Path, extra_wavs: list[Path]) -> int:
    """Write the reference JSON for the default WAVs plus any extra WAVs."""
    import onnxruntime as ort
    import torch

    torch.set_num_threads(1)
    preprocessor = torch.jit.load(str(preprocessor_path), map_location="cpu")
    encoder = ort.InferenceSession(str(pack_dir / "encoder.onnx"), providers=["CPUExecutionProvider"])
    ctc = ort.InferenceSession(str(pack_dir / "ctc_decoder.onnx"), providers=["CPUExecutionProvider"])
    vocab, mask = load_head(pack_dir)

    items = []
    for path in DEFAULT_WAVS + extra_wavs:
        item = reference_one(path, preprocessor, encoder, ctc, vocab, mask)
        print(f"{path.name}: {item['samples']} samples, {item['mel_frames']} frames, text: {item['text']}")
        items.append(item)

    document = {
        "schema": 1,
        "lang": LANG,
        "pack_dir": str(pack_dir.resolve()),
        "preprocessor": str(preprocessor_path.resolve()),
        "versions": versions(),
        "definitions": {
            "encoder_head": "outputs[0, 0, :8] (channel 0, first 8 encoder steps)",
            "encoder_tail": "outputs[0, -8:, -1] (last 8 channels at the final encoder step)",
            "ctc_argmax_path": "per step argmax over the masked columns (masked index space), no collapse",
        },
        "items": items,
    }
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(document, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {out_path}")
    return 0


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("pack_dir", nargs="?", type=Path, help="pack folder with the four files")
    parser.add_argument("wavs", nargs="*", type=Path, help="extra 16 kHz mono WAVs")
    parser.add_argument("--inspect", type=Path, metavar="PACK_DIR", help="print graph facts and exit")
    parser.add_argument("--preprocessor", type=Path, default=DEFAULT_PREPROCESSOR)
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    if args.inspect is not None:
        return run_inspect(args.inspect)
    if args.pack_dir is None:
        print("error: give PACK_DIR (or --inspect PACK_DIR)", file=sys.stderr)
        return 2
    return run_reference(args.pack_dir, args.preprocessor, args.out, args.wavs)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
