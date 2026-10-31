#!/usr/bin/env python3
"""Build the tiny offline fake pack used by the Rust tests of the Indic STT backend.

Writes two ONNX graphs with the REAL input/output names, dtypes and ranks of the
IndicConformer pack, but with trivial maths, into:

  src-tauri/tests/fixtures/tamil_stt/fake_pack/encoder.onnx
  src-tauri/tests/fixtures/tamil_stt/fake_pack/ctc_decoder.onnx

Run with the offline venv (needs the `onnx` package, public PyPI):
  /tmp/vox-work/venv3/bin/python scripts/tamil_stt_fake_pack.py

Encoder (audio_signal [1,80,T] f32, length [1] i64):
  band 0 is averaged over blocks of 8 frames (AveragePool, ceil_mode), expanded to 16
  channels -> outputs [1,16,T'] with T' = ceil(T/8); encoded_lengths = ceil(length/8).

CTC decoder (encoder_output [1,16,T'] f32 -> logprobs [1,T',5633] f32):
  step value = channel 0 of the encoder output.
  target column = 4865 when the value is > 0, otherwise 4869 (both inside the Tamil block).
  logprobs[t, v] = -(v - target[t])^2, so the argmax over the Tamil columns is the target.

Nothing here is random; re-running gives byte-identical files for the same onnx version.
The pack is NOT the real model: its sizes are tiny on purpose, so the pack-status gate
rejects it, and tests call the session loader directly.
"""

from pathlib import Path

import numpy as np
import onnx
from onnx import TensorProto, helper, numpy_helper

REPO_ROOT = Path(__file__).resolve().parents[1]
FAKE_DIR = REPO_ROOT / "src-tauri" / "tests" / "fixtures" / "tamil_stt" / "fake_pack"
OPSET = 13
IR_VERSION = 9
LOGIT_WIDTH = 5633
POSITIVE_COLUMN = 4865.0
NON_POSITIVE_COLUMN = 4869.0
POOL = 8
CHANNELS = 16


def const_i64(name: str, values: list[int]) -> onnx.TensorProto:
    return numpy_helper.from_array(np.array(values, dtype=np.int64), name)


def const_f32(name: str, values: list[float]) -> onnx.TensorProto:
    return numpy_helper.from_array(np.array(values, dtype=np.float32), name)


def build_encoder() -> onnx.ModelProto:
    audio = helper.make_tensor_value_info("audio_signal", TensorProto.FLOAT, [1, 80, "T"])
    length = helper.make_tensor_value_info("length", TensorProto.INT64, [1])
    outputs = helper.make_tensor_value_info("outputs", TensorProto.FLOAT, [1, CHANNELS, "T_enc"])
    enc_len = helper.make_tensor_value_info("encoded_lengths", TensorProto.INT64, [1])
    initializers = [
        const_i64("band0_starts", [0]),
        const_i64("band0_ends", [1]),
        const_i64("band0_axes", [1]),
        const_i64("expand_shape", [1, CHANNELS, 1]),
        const_i64("seven", [7]),
        const_i64("eight", [8]),
    ]
    nodes = [
        helper.make_node(
            "Slice",
            ["audio_signal", "band0_starts", "band0_ends", "band0_axes"],
            ["band0"],
            name="take_band0",
        ),
        helper.make_node(
            "AveragePool",
            ["band0"],
            ["pooled"],
            name="pool_frames",
            kernel_shape=[POOL],
            strides=[POOL],
            ceil_mode=1,
        ),
        helper.make_node("Expand", ["pooled", "expand_shape"], ["outputs"], name="to_channels"),
        helper.make_node("Add", ["length", "seven"], ["length_plus_seven"], name="ceil_add"),
        helper.make_node("Div", ["length_plus_seven", "eight"], ["encoded_lengths"], name="ceil_div"),
    ]
    graph = helper.make_graph(
        nodes,
        "fake_indic_encoder",
        [audio, length],
        [outputs, enc_len],
        initializer=initializers,
    )
    return helper.make_model(
        graph,
        producer_name="vox-fake-pack",
        opset_imports=[helper.make_opsetid("", OPSET)],
        ir_version=IR_VERSION,
    )


def build_ctc_decoder() -> onnx.ModelProto:
    encoder_output = helper.make_tensor_value_info(
        "encoder_output", TensorProto.FLOAT, [1, CHANNELS, "T_enc"]
    )
    logprobs = helper.make_tensor_value_info("logprobs", TensorProto.FLOAT, [1, "T_enc", LOGIT_WIDTH])
    initializers = [
        const_i64("channel0_starts", [0]),
        const_i64("channel0_ends", [1]),
        const_i64("channel0_axes", [1]),
        const_f32("zero", [0.0]),
        const_f32("positive_column", [POSITIVE_COLUMN]),
        const_f32("non_positive_column", [NON_POSITIVE_COLUMN]),
        numpy_helper.from_array(np.float32(0.0), "ramp_start"),
        numpy_helper.from_array(np.float32(LOGIT_WIDTH), "ramp_limit"),
        numpy_helper.from_array(np.float32(1.0), "ramp_delta"),
    ]
    nodes = [
        helper.make_node(
            "Slice",
            ["encoder_output", "channel0_starts", "channel0_ends", "channel0_axes"],
            ["channel0"],
            name="take_channel0",
        ),
        helper.make_node("Transpose", ["channel0"], ["step_values"], name="to_steps", perm=[0, 2, 1]),
        helper.make_node("Greater", ["step_values", "zero"], ["is_positive"], name="sign"),
        helper.make_node(
            "Where",
            ["is_positive", "positive_column", "non_positive_column"],
            ["target"],
            name="pick_column",
        ),
        helper.make_node("Range", ["ramp_start", "ramp_limit", "ramp_delta"], ["ramp"], name="columns"),
        helper.make_node("Sub", ["ramp", "target"], ["offset"], name="distance"),
        helper.make_node("Mul", ["offset", "offset"], ["squared"], name="square"),
        helper.make_node("Neg", ["squared"], ["logprobs"], name="negate"),
    ]
    graph = helper.make_graph(
        nodes,
        "fake_indic_ctc_decoder",
        [encoder_output],
        [logprobs],
        initializer=initializers,
    )
    return helper.make_model(
        graph,
        producer_name="vox-fake-pack",
        opset_imports=[helper.make_opsetid("", OPSET)],
        ir_version=IR_VERSION,
    )


def write(model: onnx.ModelProto, name: str) -> Path:
    onnx.checker.check_model(model)
    path = FAKE_DIR / name
    onnx.save(model, str(path))
    return path


def main() -> None:
    FAKE_DIR.mkdir(parents=True, exist_ok=True)
    for model, name in ((build_encoder(), "encoder.onnx"), (build_ctc_decoder(), "ctc_decoder.onnx")):
        path = write(model, name)
        print(f"wrote {path.relative_to(REPO_ROOT)}: {path.stat().st_size} bytes")


if __name__ == "__main__":
    main()
