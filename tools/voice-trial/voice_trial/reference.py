"""A yardstick for the windowed scheme's spectral distance: how far the whole render moves
when only the style row changes, as it does whenever Ghost cuts a reply into pieces (Kokoro
picks the voice's row by the piece's token count). Same durations and pitch are forced by
reusing the encoder's output, so only the decoder's conditioning differs.

uv run --project tools/voice-trial python -I -m voice_trial.reference SPLIT_DIR VOICES_JSON
"""

import json
import pathlib
import sys

import numpy as np

from voice_trial.scheme import mel_distance, rms_db
from voice_trial.windowed import HARMONICS, session


def main(split: str, styles: str) -> None:
    split = pathlib.Path(split)
    lines = json.loads((split / "lines.json").read_text())
    rows = json.loads(pathlib.Path(styles).read_text())
    enc = session(str(split / "kokoro-encoder.onnx"))
    dec = session(str(split / "kokoro-decoder.onnx"))
    zero = np.zeros((1, 1, HARMONICS), dtype=np.float32)
    for words, row in lines.items():
        tokens = np.array([[0, *row["tokens"], 0]], dtype=np.int64)
        style = np.array([row["style"]], dtype=np.float32)
        asr, f0, n, _ = enc.run(None, {"tokens": tokens, "style": style, "speed": np.array([1.0], np.float32)})
        (whole,) = dec.run(None, {"asr": asr, "f0": f0, "n": n, "style": style, "phase": zero})
        k = len(row["tokens"])
        out = []
        for delta in (-10, -5, 5, 10):
            idx = min(max(k + delta, 1), len(rows) - 1)
            other = np.array([rows[idx]], dtype=np.float32)
            (moved,) = dec.run(None, {"asr": asr, "f0": f0, "n": n, "style": other, "phase": zero})
            out.append(f"row {delta:+d}: {mel_distance(moved, whole):.2f} dB, level {rms_db(moved) - rms_db(whole):+.1f}")
        print(f"  {words:>2}w ({k} tokens): " + "; ".join(out))


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
