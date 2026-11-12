"""Where Kokoro's time goes: the whole model against its encoder and decoder, by length and
by ONNX Runtime's intra-op threads.

uv run --project tools/voice-trial python -I tools/voice-trial/voice_trial/time_halves.py \
    KOKORO_ONNX SPLIT_DIR [--threads 0,1,2,4,6] [--n 7]
"""

import argparse
import json
import os
import pathlib
import statistics
import time

import numpy as np
import onnxruntime as ort


def session(path: str, threads: int, level: str = "all") -> ort.InferenceSession:
    o = ort.SessionOptions()
    o.inter_op_num_threads = 1
    o.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
    o.graph_optimization_level = {
        "all": ort.GraphOptimizationLevel.ORT_ENABLE_ALL,
        "extended": ort.GraphOptimizationLevel.ORT_ENABLE_EXTENDED,
        "basic": ort.GraphOptimizationLevel.ORT_ENABLE_BASIC,
    }[level]
    if threads:
        o.intra_op_num_threads = threads
    return ort.InferenceSession(path, o, providers=["CPUExecutionProvider"])


def p50(xs):
    return statistics.median(xs)


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("model")
    p.add_argument("split")
    p.add_argument("--threads", default="0,1,2,4,6")
    p.add_argument("--n", type=int, default=7)
    p.add_argument("--level", default="all")
    a = p.parse_args()
    split = pathlib.Path(a.split)
    lines = json.loads((split / "lines.json").read_text())
    print(f"load {os.getloadavg()}")
    for threads in [int(t) for t in a.threads.split(",")]:
        whole = session(a.model, threads, a.level)
        enc = session(str(split / "kokoro-encoder.onnx"), threads, a.level)
        dec = session(str(split / "kokoro-decoder.onnx"), threads, a.level)
        dec_inputs = {i.name for i in dec.get_inputs()}
        print(f"threads {threads or 'default'}  ({a.level})")
        for words, row in lines.items():
            tokens = np.array([[0, *row["tokens"], 0]], dtype=np.int64)
            style = np.array([row["style"]], dtype=np.float32)
            speed = np.array([1.0], dtype=np.float32)
            feed = {"tokens": tokens, "style": style, "speed": speed}
            whole.run(None, feed)
            tw, te, td = [], [], []
            for _ in range(a.n):
                t = time.perf_counter()
                (audio,) = whole.run(None, feed)
                tw.append(time.perf_counter() - t)
                t = time.perf_counter()
                asr, f0, n = enc.run(None, feed)
                te.append(time.perf_counter() - t)
                dfeed = {"asr": asr, "f0": f0, "n": n, "style": style}
                if "phase" in dec_inputs:
                    dfeed["phase"] = np.zeros((1, 1, 9), dtype=np.float32)
                t = time.perf_counter()
                (audio2,) = dec.run(None, dfeed)
                td.append(time.perf_counter() - t)
            diff = float(np.abs(audio - audio2).max())
            secs = audio.shape[-1] / 24000
            print(
                f"  {words:>2} words audio {secs:5.2f}s frames {asr.shape[-1]:4d}  whole {p50(tw)*1000:6.0f} ms"
                f"  encoder {p50(te)*1000:5.0f} ms  decoder {p50(td)*1000:6.0f} ms"
                f"  ({p50(td)/secs:.3f} s/s)  max diff {diff:.2e}"
            )
    print(f"load {os.getloadavg()}")


if __name__ == "__main__":
    main()
