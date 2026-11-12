"""Kokoro's vocoder a window of frames at a time, against the whole render.

For each line: the encoder once, then the decoder over windows of frames with context on
each side and the harmonic source's phase carried in, stitched at the frame edges. Prints
how far the stitched audio is from the whole render (SNR, the largest step at a join,
loudness per window) and what each window costs.

uv run --project tools/voice-trial python -I tools/voice-trial/voice_trial/windowed.py \
    SPLIT_DIR [--first 10] [--grow 2] [--left 8] [--right 8] [--save DIR]
"""

import argparse
import json
import os
import pathlib
import time
import wave

import numpy as np
import onnxruntime as ort

HOP = 600  # samples per encoder frame at 24 kHz
F0_HOP = 300  # samples per F0 frame
RATE = 24000
HARMONICS = 9


def session(path: str) -> ort.InferenceSession:
    o = ort.SessionOptions()
    o.inter_op_num_threads = 1
    o.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
    o.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    return ort.InferenceSession(path, o, providers=["CPUExecutionProvider"])


def phase_before(f0: np.ndarray, start: int) -> np.ndarray:
    """The sine generator's phase, in cycles, at F0 frame `start`: the sum over the frames
    before it of each harmonic's cycles per sample, reduced mod 1 as the model does."""
    h = np.arange(1, HARMONICS + 1, dtype=np.float32)
    rad = np.mod(f0[0, :start, None].astype(np.float32) * h[None, :] / RATE, 1.0)
    total = np.mod(rad.sum(axis=0, dtype=np.float64), 1.0).astype(np.float32)
    return total.reshape(1, 1, HARMONICS)


def windows(frames: int, first: int, grow: float) -> list[tuple[int, int]]:
    out, s, size = [], 0, first
    while s < frames:
        e = min(frames, s + size)
        if frames - e < size // 2:
            e = frames
        out.append((s, e))
        s = e
        size = int(size * grow)
    return out


def rms_db(x: np.ndarray) -> float:
    return 20 * np.log10(np.sqrt(np.mean(x.astype(np.float64) ** 2)) + 1e-12)


def save(path: pathlib.Path, audio: np.ndarray) -> None:
    pcm = (np.clip(audio, -1, 1) * 32767).astype(np.int16)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(pcm.tobytes())


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("split")
    p.add_argument("--first", type=int, default=10)
    p.add_argument("--grow", type=float, default=2.0)
    p.add_argument("--left", type=int, default=8)
    p.add_argument("--right", type=int, default=8)
    p.add_argument("--no-phase", action="store_true")
    p.add_argument("--save")
    a = p.parse_args()
    split = pathlib.Path(a.split)
    lines = json.loads((split / "lines.json").read_text())
    enc = session(str(split / "kokoro-encoder.onnx"))
    dec = session(str(split / "kokoro-decoder.onnx"))
    print(
        f"load {os.getloadavg()[0]:.1f}  first {a.first} frames, grow x{a.grow}, context {a.left}/{a.right}"
    )
    for words, row in lines.items():
        tokens = np.array([[0, *row["tokens"], 0]], dtype=np.int64)
        style = np.array([row["style"]], dtype=np.float32)
        speed = np.array([1.0], dtype=np.float32)
        t0 = time.perf_counter()
        asr, f0, n = enc.run(None, {"tokens": tokens, "style": style, "speed": speed})
        t_enc = time.perf_counter() - t0
        frames = asr.shape[-1]
        zero = np.zeros((1, 1, HARMONICS), dtype=np.float32)
        (whole,) = dec.run(None, {"asr": asr, "f0": f0, "n": n, "style": style, "phase": zero})
        pieces, costs = [], []
        first_audio = None
        for s, e in windows(frames, a.first, a.grow):
            ws, we = max(0, s - a.left), min(frames, e + a.right)
            ph = zero if a.no_phase else phase_before(f0, 2 * ws)
            t = time.perf_counter()
            (audio,) = dec.run(
                None,
                {
                    "asr": asr[:, :, ws:we],
                    "f0": f0[:, 2 * ws : 2 * we],
                    "n": n[:, 2 * ws : 2 * we],
                    "style": style,
                    "phase": ph,
                },
            )
            costs.append(time.perf_counter() - t)
            if first_audio is None:
                first_audio = t_enc + costs[0]
            pieces.append(audio[(s - ws) * HOP : (e - ws) * HOP])
        stitched = np.concatenate(pieces)
        whole = whole[: len(stitched)]
        err = stitched - whole
        snr = 10 * np.log10(
            np.sum(whole.astype(np.float64) ** 2) / (np.sum(err.astype(np.float64) ** 2) + 1e-20)
        )
        joins, loud = [], []
        pos = 0
        for piece in pieces:
            loud.append(rms_db(piece) - rms_db(whole[pos : pos + len(piece)]))
            pos += len(piece)
            if pos < len(stitched):
                joins.append(abs(float(stitched[pos] - stitched[pos - 1])))
        typical = float(np.median(np.abs(np.diff(whole))))
        print(
            f"  {words:>2} words {frames:4d} frames, {len(pieces)} windows: first audio {first_audio * 1000:5.0f} ms"
            f" (encoder {t_enc * 1000:3.0f}, window {costs[0] * 1000:4.0f}), all windows {sum(costs) * 1000:5.0f} ms;"
            f" SNR {snr:5.1f} dB; loudness by window {', '.join(f'{x:+.2f}' for x in loud)} dB;"
            f" largest step at a join {max(joins, default=0):.4f} (median step {typical:.4f})"
        )
        if a.save:
            out = pathlib.Path(a.save)
            out.mkdir(parents=True, exist_ok=True)
            save(out / f"{words}-whole.wav", whole)
            save(out / f"{words}-windowed.wav", stitched)


if __name__ == "__main__":
    main()
