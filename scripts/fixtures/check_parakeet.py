#!/usr/bin/env python
"""Where a Parakeet transcript differs between the Python daemon and the Rust port, say why.

For every clip, this compares the Rust port's encoder input (features the ignored test
``parakeet_dump_features`` writes under ``<fixtures>/rust-parakeet-features/``) with the
NumPy preprocessor's, and runs the Python model's encoder and decoder on the Rust features:
if that gives the Python transcript, the port's front end is not what differs, the
ONNX Runtime build is. Run through the oracle's environment (``tools/oracle``)::

    uv run --project tools/oracle python -I scripts/fixtures/check_parakeet.py
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np


def main() -> None:
    ghost = Path(__file__).resolve().parents[2]
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else ghost / ".scratch" / "spike-fixtures"
    import onnx_asr
    import onnxruntime as ort

    so = ort.SessionOptions()
    so.intra_op_num_threads = 4
    so.inter_op_num_threads = 1
    model = onnx_asr.load_model("nemo-parakeet-tdt-0.6b-v3", quantization="int8", sess_options=so)
    asr = model.asr
    clips = json.loads((out / "clips.json").read_text())
    worst = 0.0
    same_on_rust_features = 0
    checked = 0
    for clip in clips:
        rust_path = out / "rust-parakeet-features" / f"{clip['id']}.npy"
        if not rust_path.exists():
            continue
        checked += 1
        rust = np.load(rust_path)[None]
        rust_len = np.array(
            [int(np.load(out / "rust-parakeet-features" / f"{clip['id']}.len.npy")[0])]
        )
        import wave

        with wave.open(clip["wav"]) as w:
            pcm = (
                np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float32)
                / 32768.0
            )
        feats, lens = asr._preprocessor(pcm[None], np.array([len(pcm)]))
        diff = float(np.abs(feats - rust).max()) if feats.shape == rust.shape else float("inf")
        worst = max(worst, diff)
        want = json.loads((out / "parakeet" / f"{clip['id']}.json").read_text())["text"]
        enc, enc_lens = asr._encode(rust, rust_len)
        text = next(asr._decode_tokens(*t) for t in asr._decoding(enc, enc_lens)).text.strip()
        if text == want:
            same_on_rust_features += 1
        else:
            print(
                f"{clip['id']}: python on rust features {text!r} vs python {want!r} "
                f"(feature diff {diff:.2e}, lens {lens[0]} vs {rust_len[0]})"
            )
    print(
        f"{checked} clips: worst feature difference {worst:.2e}; "
        f"python decode of rust features matches python {same_on_rust_features}/{checked}"
    )


if __name__ == "__main__":
    main()
