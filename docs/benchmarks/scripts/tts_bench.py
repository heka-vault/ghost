"""Benchmark one TTS engine on CPU and save samples. Prints one JSON line.

Usage: python -I tts_bench.py <engine> <models_dir> <out_dir> <threads>
engines: kokoro:<int8|fp32> | piper:<lessac-medium|ryan-high>
"""
import json
import os
import statistics
import sys
import time
from pathlib import Path

import numpy as np
import soundfile as sf

engine, models, out, threads = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3]), int(sys.argv[4])
os.environ["OMP_NUM_THREADS"] = str(threads)

REPLIES = {
    "short": "Okay. I'll mark issue one forty two ready for development. Confirm?",
    "first_sentence": "Three issues need your sign-off.",
    "long": ("Three issues need your sign-off. The top one is heka vault issue one forty two, which is waiting on a choice "
             "between Redis and Postgres for the session cache. It blocks four other issues, and the acceptance criteria "
             "are already written. I'd recommend Postgres, since you already run it."),
}

rss0 = None


def rss_mb():
    for line in open("/proc/self/status"):
        if line.startswith("VmRSS"):
            return int(line.split()[1]) / 1024


rss0 = rss_mb()
t0 = time.perf_counter()
kind, _, variant = engine.partition(":")
if kind == "kokoro":
    import onnxruntime as ort
    from kokoro_onnx import Kokoro

    name = "kokoro-v1.0.int8.onnx" if variant == "int8" else "kokoro-v1.0.onnx"
    so = ort.SessionOptions()
    so.intra_op_num_threads = threads
    sess = ort.InferenceSession(str(models / name), so, providers=["CPUExecutionProvider"])
    k = Kokoro.from_session(sess, str(models / "voices-v1.0.bin"))

    def synth(text):
        return k.create(text, voice="af_heart", speed=1.0, lang="en-us")
elif kind == "piper":
    from piper import PiperVoice

    v = PiperVoice.load(str(models / f"en_US-{variant}.onnx"))

    def synth(text):
        chunks = list(v.synthesize(text))
        return np.concatenate([c.audio_float_array for c in chunks]), chunks[0].sample_rate
else:
    raise SystemExit(f"unknown engine {engine}")
load_s = time.perf_counter() - t0
synth("Warm up.")

res = {}
for key, text in REPLIES.items():
    times = []
    for _ in range(3):
        t = time.perf_counter()
        samples, sr = synth(text)
        times.append(time.perf_counter() - t)
    dur = len(samples) / sr
    lat = statistics.median(times)
    res[key] = {"lat": round(lat, 3), "audio_s": round(dur, 2), "rtf": round(lat / dur, 3)}
    sf.write(out / f"{engine.replace(':', '_')}_{key}.wav", samples, sr)

print(json.dumps({"engine": engine, "threads": threads, "load_s": round(load_s, 2), "rss_mb": round(rss_mb() - rss0), **res}))
