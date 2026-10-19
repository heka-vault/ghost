"""Component micro-benchmarks: STT by utterance length, Kokoro time to first audio by
sentence length, the Jev confirmation classifier, and Smart Turn inference.

Runs on the same CPU as everything else (and as a running ghostd, if there is one);
the numbers are what a session sees under that load, not a clean-room figure.
"""

from __future__ import annotations

import asyncio
import json
import os
import statistics
import time
from pathlib import Path
from typing import Any

import numpy as np
from ghost_voice import VoiceSettings
from ghost_voice.pipeline import Models

from ghostd.perf.audio import IN_RATE, Voice

STT_LINES = {
    "yes (1 word)": "Yes.",
    "short (4 words)": "Close infra seventeen.",
    "medium (9 words)": "What's going on with app one forty two today?",
    "long (22 words)": "What's going on with app one forty two, and what have people said "
    "about it in the comments since last week, roughly?",
}
TTS_LINES = {
    "3 words": "Labeled it bug.",
    "8 words": "Three issues are waiting for your sign-off today.",
    "15 words": "Issue one forty two asks whether to use Redis or Postgres for the session cache.",
    "30 words": "Issue one forty two asks whether to use Redis or Postgres for the session "
    "cache, and it blocks two others, so it is probably the one to decide first before "
    "the launch next week.",
}
JEV_LINES = ("Yes, go ahead.", "Sure.", "No, wait.", "Okay, do it.")


def _stats(xs: list[float]) -> dict[str, float]:
    xs = sorted(xs)
    return {
        "p50": statistics.median(xs),
        "p95": xs[min(len(xs) - 1, round(0.95 * (len(xs) - 1)))],
        "min": xs[0],
    }


async def run_micro(only: list[str], n: int, out: Path) -> int:
    settings = VoiceSettings()  # type: ignore[call-arg]
    results: dict[str, Any] = {"n": n, "cpus": os.cpu_count()}
    lines: list[str] = []
    models = Models.load(settings) if {"stt", "tts", "turn"} & set(only) else None
    voice = Voice(models.tts.kokoro, out.parent / "utterances") if models else None

    if "stt" in only and models and voice:
        stt = models.stt._model  # noqa: SLF001 - the loaded onnx-asr model
        lines.append(
            f"STT (Parakeet v3 int8, {settings.stt_threads} threads): seconds per utterance"
        )
        results["stt"] = {}
        for label, text in STT_LINES.items():
            u = voice.render(text)
            pcm = np.frombuffer(u.pcm, dtype=np.int16).astype(np.float32) / 32768.0
            pcm = np.concatenate([pcm, np.zeros(int(0.2 * IN_RATE), dtype=np.float32)])
            times = []
            for _ in range(n):
                t = time.perf_counter()
                await asyncio.to_thread(stt.recognize, pcm, sample_rate=IN_RATE)
                times.append(time.perf_counter() - t)
            s = _stats(times) | {"audio": u.seconds}
            results["stt"][label] = s
            lines.append(
                f"  {label:<18} audio {u.seconds:4.1f}s  p50 {s['p50']:.3f}  p95 {s['p95']:.3f}"
            )

    if "tts" in only and models:
        kokoro = models.tts.kokoro
        lines.append(f"TTS (Kokoro, voice {settings.tts_voice}): time to first audio / whole")
        results["tts"] = {}
        for label, text in TTS_LINES.items():
            first, whole, audio = [], [], 0.0
            for _ in range(n):
                t = time.perf_counter()
                got_first = None
                total = 0
                async for samples, rate in kokoro.create_stream(
                    text, voice=settings.tts_voice, speed=settings.tts_speed, lang="en-us"
                ):
                    if got_first is None:
                        got_first = time.perf_counter() - t
                    total += len(samples)
                    audio = total / rate
                first.append(got_first or 0.0)
                whole.append(time.perf_counter() - t)
            s = {"first": _stats(first), "whole": _stats(whole), "audio": audio}
            results["tts"][label] = s
            lines.append(
                f"  {label:<10} audio {audio:4.1f}s  first p50 {s['first']['p50']:.3f} "
                f"p95 {s['first']['p95']:.3f}  whole p50 {s['whole']['p50']:.3f}"
            )

    if "turn" in only and models and voice:
        turn = models.turn
        lines.append("Smart Turn v3: inference on the last 8 s of an utterance")
        u = voice.render(STT_LINES["medium (9 words)"])
        pcm = np.frombuffer(u.pcm, dtype=np.int16).astype(np.float32) / 32768.0
        times = []
        for _ in range(n):
            t = time.perf_counter()
            await asyncio.to_thread(turn._predict_endpoint, pcm)  # noqa: SLF001
            times.append(time.perf_counter() - t)
        results["turn"] = _stats(times)
        lines.append(f"  p50 {results['turn']['p50']:.3f}  p95 {results['turn']['p95']:.3f}")

    if "jev" in only:
        from ghost_core import ConfirmationContext
        from ghost_core.jev import JevClient

        jev = JevClient.from_env(
            "auto",
            {
                "TYPESAFE_API_KEY": os.environ.get("TYPESAFE_API_KEY", ""),
                "OPENROUTER_API_KEY": os.environ.get("OPENROUTER_API_KEY", ""),
            },
            timeout=2.0,
        )
        if jev is None:
            lines.append("Jev: no key, skipped")
        else:
            ctx = ConfirmationContext(
                pending_action=(
                    'On ghost-mock/infra#17 "Rotate the Discord webhook": close the issue'
                ),
                question="Okay to close infra 17?",
            )
            times, errors, server = [], 0, []
            for i in range(n * len(JEV_LINES)):
                t = time.perf_counter()
                try:
                    a = await jev.classify_utterance(JEV_LINES[i % len(JEV_LINES)], ctx)
                    server.append(a.seconds)
                except Exception:  # noqa: BLE001
                    errors += 1
                times.append(time.perf_counter() - t)
            await jev.aclose()
            results["jev"] = _stats(times) | {"errors": errors, "via": jev.provider}
            over = sum(1 for x in times if x > 0.5)
            lines.append(
                f"Jev via {jev.provider}: p50 {results['jev']['p50']:.3f}  p95 "
                f"{results['jev']['p95']:.3f}  over the 0.5 s deadline {over}/{len(times)}  "
                f"errors {errors}"
            )

    (out / "micro.json").write_text(json.dumps(results, indent=1))
    (out / "micro.txt").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    print(f"\nmicro: {out / 'micro.txt'}")
    return 0
