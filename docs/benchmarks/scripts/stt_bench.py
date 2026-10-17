"""Benchmark one STT engine on CPU. Prints one JSON line.

Usage: python -I stt_bench.py <engine> <models_dir> <audio_dir> <threads>
engines: fw:<size>  (faster-whisper int8, greedy) | parakeet:<v2|v3> | moonshine:<tiny|base>
"""
import json
import re
import statistics
import sys
import time
from pathlib import Path

import jiwer
import numpy as np
import soundfile as sf

engine, models, audio, threads = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3]), int(sys.argv[4])
refs = json.loads((audio / "refs.json").read_text())


def rss_mb():
    for line in open("/proc/self/status"):
        if line.startswith("VmRSS"):
            return int(line.split()[1]) / 1024


REF_FIX = {"one forty two": "142", "forty six": "46"}


def norm(s, ref=False):
    s = s.lower().replace("-", " ")
    if ref:
        for a, b in REF_FIX.items():
            s = s.replace(a, b)
    return re.sub(r"[^a-z0-9' ]", " ", s).split()


rss0 = rss_mb()
t0 = time.perf_counter()
kind, _, variant = engine.partition(":")
if kind == "fw":
    from faster_whisper import WhisperModel

    m = WhisperModel(variant, device="cpu", compute_type="int8", cpu_threads=threads, download_root=str(models / "fw"))

    def transcribe(x):
        segs, _ = m.transcribe(x, beam_size=1, language="en", vad_filter=False, condition_on_previous_text=False)
        return " ".join(s.text for s in segs)
elif kind == "parakeet":
    import onnx_asr
    import onnxruntime as ort

    so = ort.SessionOptions()
    so.intra_op_num_threads = threads
    m = onnx_asr.load_model(f"nemo-parakeet-tdt-0.6b-{variant}", quantization="int8", sess_options=so)

    def transcribe(x):
        return m.recognize(x, sample_rate=16000)
elif kind == "moonshine":
    import sherpa_onnx

    d = models / f"sherpa-onnx-moonshine-{variant}-en-int8"
    m = sherpa_onnx.OfflineRecognizer.from_moonshine(
        preprocessor=str(d / "preprocess.onnx"), encoder=str(d / "encode.int8.onnx"),
        uncached_decoder=str(d / "uncached_decode.int8.onnx"), cached_decoder=str(d / "cached_decode.int8.onnx"),
        tokens=str(d / "tokens.txt"), num_threads=threads)

    def transcribe(x):
        s = m.create_stream()
        s.accept_waveform(16000, x)
        m.decode_stream(s)
        return s.result.text
else:
    raise SystemExit(f"unknown engine {engine}")
load_s = time.perf_counter() - t0

clips = {}
for name in refs:
    x, sr = sf.read(audio / name, dtype="float32")
    assert sr == 16000, name
    clips[name] = x
transcribe(clips["cmd0.wav"])  # warm-up

rows = []
for name, x in clips.items():
    times = []
    for _ in range(3):
        t = time.perf_counter()
        hyp = transcribe(x)
        times.append(time.perf_counter() - t)
    dur = len(x) / 16000
    lat = statistics.median(times)
    rows.append({"clip": name, "dur": round(dur, 2), "lat": round(lat, 3), "rtf": round(lat / dur, 3), "hyp": hyp.strip(),
                 "wer": round(jiwer.wer(" ".join(norm(refs[name], ref=True)), " ".join(norm(hyp)) or "<empty>"), 3)})

cmd = [r for r in rows if r["clip"] != "jfk.wav"]
print(json.dumps({
    "engine": engine, "threads": threads, "load_s": round(load_s, 2), "rss_mb": round(rss_mb() - rss0),
    "cmd_lat_p50": round(statistics.median(r["lat"] for r in cmd), 3), "cmd_lat_max": max(r["lat"] for r in cmd),
    "cmd_wer_mean": round(statistics.mean(r["wer"] for r in cmd), 3),
    "jfk": next(r for r in rows if r["clip"] == "jfk.wav"), "rows": rows}))
