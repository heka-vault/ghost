"""Kokoro's token ids and style rows for the measured lines, from kokoro-onnx (the oracle).

Run with the oracle's environment, which has kokoro-onnx 0.6.1:
tools/oracle/.venv/bin/python -I tools/voice-trial/voice_trial/tokens.py KOKORO_DIR OUT.json
"""

import json
import sys

import numpy as np
from kokoro_onnx import Kokoro

LINES = {
    "2": "Labeled it.",
    "4": "Three issues are waiting.",
    "8": "Three issues are waiting for your sign-off today.",
    "16": "Issue one forty two asks whether to use Redis or Postgres for the session cache today.",
    "32": (
        "Issue one forty two asks whether to use Redis or Postgres for the session cache, and it "
        "blocks two others, so it is probably the one to decide first before the launch next week starts."
    ),
}


def main(kdir: str, out: str) -> None:
    k = Kokoro(f"{kdir}/kokoro-v1.0.onnx", f"{kdir}/voices-v1.0.bin")
    voice = k.get_voice_style("af_heart")
    rows = {}
    for words, text in LINES.items():
        phonemes = k.tokenizer.phonemize(text, "en-us")
        tokens = k.tokenizer.tokenize(phonemes)
        style = np.asarray(voice[len(tokens)]).reshape(256).tolist()
        rows[words] = {"text": text, "tokens": [int(t) for t in tokens], "style": style}
    with open(out, "w") as f:
        json.dump(rows, f)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
