"""Cut Kokoro v1.0's ONNX graph in two at the line between its encoder and its decoder.

The encoder (`tokens`, `style`, `speed` -> `asr`, `f0`, `n`, `durations`, the frames
each token lasts) is the text side: BERT, the
duration and prosody predictors, the alignment. The decoder (`asr`, `f0`, `n`, `style` ->
`audio`) is the vocoder. Nothing is changed inside either half: run one after the other
they give the whole model's output. A third file, the decoder with a `phase` input, is
written when `--phase` is given: the harmonic source's running phase added after its
cumulative sum, so a window of frames can be decoded with the phase the whole render would
have had at its start (ADR 0053).

uv run --project tools/voice-trial python -I tools/voice-trial/voice_trial/split_kokoro.py \
    MODEL OUT_DIR [--phase]
"""

import argparse
import hashlib
import pathlib

import onnx
from onnx import TensorProto, helper

ASR = "/encoder/MatMul_1_output_0"
F0 = "/encoder/Reshape_output_0"
N = "/encoder/Reshape_1_output_0"
DURATIONS = "/encoder/Cast_output_0"
CUMSUM = "/decoder/decoder/generator/m_source/l_sin_gen/CumSum"


def rename(model: onnx.ModelProto, old: str, new: str) -> None:
    g = model.graph
    for n in g.node:
        n.input[:] = [new if i == old else i for i in n.input]
        n.output[:] = [new if o == old else o for o in n.output]
    for v in list(g.input) + list(g.output):
        if v.name == old:
            v.name = new


def sha256(path: pathlib.Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def add_phase(model: onnx.ModelProto) -> None:
    """A `phase` input [1, 1, harmonics] added to the sine generator's cumulative sum."""
    g = model.graph
    node = next(n for n in g.node if n.name == CUMSUM)
    out = node.output[0]
    added = out + "_phased"
    for n in g.node:
        if n is node:
            continue
        n.input[:] = [added if i == out else i for i in n.input]
    idx = list(g.node).index(node)
    g.node.insert(idx + 1, helper.make_node("Add", [out, "phase"], [added], name=CUMSUM + "_phase"))
    g.input.append(helper.make_tensor_value_info("phase", TensorProto.FLOAT, [1, 1, "harmonics"]))


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("model")
    p.add_argument("out")
    p.add_argument("--phase", action="store_true")
    a = p.parse_args()
    out = pathlib.Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    enc = out / "kokoro-encoder.onnx"
    dec = out / "kokoro-decoder.onnx"
    onnx.utils.extract_model(a.model, str(enc), ["tokens", "style", "speed"], [ASR, F0, N, DURATIONS])
    onnx.utils.extract_model(a.model, str(dec), [ASR, F0, N, "style"], ["audio"])
    for path, names in (
        (enc, [(ASR, "asr"), (F0, "f0"), (N, "n"), (DURATIONS, "durations")]),
        (dec, [(ASR, "asr"), (F0, "f0"), (N, "n")]),
    ):
        m = onnx.load(str(path))
        for old, new in names:
            rename(m, old, new)
        if path == dec and a.phase:
            add_phase(m)
        onnx.checker.check_model(m)
        onnx.save(m, str(path))
    for path in (enc, dec):
        print(f"{sha256(path)}  {path.name}  {path.stat().st_size} bytes")


if __name__ == "__main__":
    main()
