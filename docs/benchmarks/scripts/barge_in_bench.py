"""Barge-in: how long Ghost keeps talking once the operator talks over it, interims off vs on.

Headless, no person and no devices: a real ``build_session`` (Silero VAD, Parakeet, Smart
Turn, Kokoro) over the perf harness's scripted transport (``ghostd.perf.audio``). Ghost
says a long greeting; 1.5 s after its voice starts the "operator" (Kokoro's am_michael,
16 kHz, cached under .scratch) talks over it. Per run:

  kept    the operator's first frame -> the end of Ghost's last loud audio at the speaker
  record  the latency record's ``barge_in`` (VAD start minus start_secs -> interruption)
  cpu     process CPU seconds per wall second while both talk (the clip's length), with
          barge-in held off (min words 99) so Ghost keeps talking: what interims cost

No model is called: the LLM endpoint is a closed local port and each run ends once the
interruption is measured. Configs are interleaved run by run.

    uv run python docs/benchmarks/scripts/barge_in_bench.py --runs 5 [--json out.json]
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import resource
import statistics
import tempfile
import time
from pathlib import Path

os.environ["ANTHROPIC_BASE_URL"] = "http://127.0.0.1:9"  # nothing leaves the machine
os.environ["ANTHROPIC_AUTH_TOKEN"] = "unused"
os.environ["GHOST_LLM_ENDPOINT"] = "proxy"

import numpy as np  # noqa: E402
from ghost_core import Store, WriteGate  # noqa: E402
from ghost_voice import VoiceSettings, build_session  # noqa: E402
from ghost_voice.parakeet import ParakeetSTTService  # noqa: E402
from ghost_voice.pipeline import TTS_SAMPLE_RATE, Models  # noqa: E402
from ghostd.perf.audio import LOUD, Mouth, NullSpeaker, ScriptedTransport, Voice  # noqa: E402
from loguru import logger  # noqa: E402
from pipecat.transports.base_transport import TransportParams  # noqa: E402

GREETING = (
    "Here is where things stand this morning. There are four issues waiting for triage in "
    "the app repository, two pull requests that need your sign-off, and one security "
    "advisory about the webhook secret, which I would look at first. The session cache "
    "question is still open, and it blocks two other pieces of work, so it is worth a "
    "decision today. After that, the dark mode request has six new comments, most of "
    "them asking for a date, and the release checklist is nearly done."
)
LINES = (
    "Wait, stop, that's the wrong issue.",
    "Hold on, I meant the other one.",
    "No no, go back to the cache question please.",
)
CPU_LINE = LINES[2]
START_AFTER = 1.5  # seconds of Ghost's voice before the operator cuts in


def cpu() -> float:
    r = resource.getrusage(resource.RUSAGE_SELF)
    return r.ru_utime + r.ru_stime


class TimedMouth(Mouth):
    """Notes when the operator's first frame goes out, and the process CPU then."""

    onset: float | None = None
    onset_cpu = 0.0

    def next_frame(self, n: int) -> bytes:
        was = self.playing
        out = super().next_frame(n)
        if self.playing and not was and self.onset is None:
            self.onset = time.monotonic()
            self.onset_cpu = cpu()
        return out


class TimedSpeaker(NullSpeaker):
    """Also notes when Ghost's last loud frame finishes playing."""

    last_loud_end = 0.0

    async def write_audio_frame(self, frame) -> bool:
        if frame.audio:
            pcm = np.frombuffer(frame.audio, dtype=np.int16).astype(np.float32) / 32768.0
            if float(np.sqrt(np.mean(pcm * pcm))) >= LOUD:
                secs = len(frame.audio) / 2 / frame.sample_rate / max(1, frame.num_channels)
                self.last_loud_end = max(self._until, time.monotonic()) + secs
        return await super().write_audio_frame(frame)


class TimedTransport(ScriptedTransport):
    def output(self):
        if self._output is None:
            self._output = TimedSpeaker(self._params)
        return self._output


RECOGNITIONS: list[dict] = []
_recognize = ParakeetSTTService._recognize


def _counted(self, pcm, *, wait=True):
    t = time.perf_counter()
    out = _recognize(self, pcm, wait=wait)
    RECOGNITIONS.append(
        {
            "kind": "final" if wait else "interim",
            "ran": out is not None,
            "secs": time.perf_counter() - t,
            "audio": len(pcm) / 16000,
            "at": time.monotonic(),
        }
    )
    return out


ParakeetSTTService._recognize = _counted  # type: ignore[method-assign]


async def one(models: Models, voice: Voice, line: str, *, interim: float, min_words: int) -> dict:
    base = VoiceSettings()  # type: ignore[call-arg]
    settings = base.model_copy(
        update={
            "input_device": None,
            "output_device": None,
            "thinking_sound": False,
            "barge_in_interim_secs": interim,
            "barge_in_min_words": min_words,
        }
    )
    clip = voice.render(line)
    tmp = Path(tempfile.mkdtemp(prefix="ghost-barge-"))
    store = Store(tmp / "ghost.db")
    records: list[dict] = []
    mouth = TimedMouth()
    built: list[TimedTransport] = []

    def transport_factory(mixer):
        t = TimedTransport(
            TransportParams(
                audio_in_enabled=True,
                audio_out_enabled=True,
                audio_out_sample_rate=TTS_SAMPLE_RATE,
                audio_out_mixer=mixer,
            ),
            mouth,
        )
        built.append(t)
        return t

    session = build_session(
        settings,
        models=models,
        sources=[],
        gate=WriteGate(store),
        transport_factory=transport_factory,
        greeting=GREETING,
        on_latency=records.append,
    )
    await session.start()  # type: ignore[attr-defined]
    run_task = asyncio.ensure_future(session.run())
    speaker = built[0].output()
    try:
        deadline = time.monotonic() + 20
        while not speaker.first_loud and time.monotonic() < deadline:
            await asyncio.sleep(0.02)
        if not speaker.first_loud:
            raise RuntimeError("Ghost never spoke")
        await asyncio.sleep(max(0.0, speaker.first_loud[0] + START_AFTER - time.monotonic()))
        n0 = len(RECOGNITIONS)
        await mouth.say(clip).wait()
        cpu_end, wall_end = cpu(), time.monotonic()  # the clip's last frame went out
        assert mouth.onset is not None
        await asyncio.sleep(3.0)  # a final after the pause, STT, the interruption
        recs = [r for r in RECOGNITIONS[n0:] if r["kind"] == "interim"]
        cut = next((r for r in records if "barge_in" in r), None)
        still_talking = speaker.last_loud_end > wall_end + 2.5
        return {
            "line": line,
            "interim": interim,
            "min_words": min_words,
            "clip": round(clip.seconds, 2),
            "kept": None if still_talking else round(speaker.last_loud_end - mouth.onset, 3),
            "barge_in": cut["barge_in"] if cut else None,
            "interims_run": sum(1 for r in recs if r["ran"]),
            "interims_skipped": sum(1 for r in recs if not r["ran"]),
            "interim_secs": [round(r["secs"], 3) for r in recs if r["ran"]],
            "cpu_cores": round((cpu_end - mouth.onset_cpu) / (wall_end - mouth.onset), 3),
        }
    finally:
        await session.cancel()
        await asyncio.gather(run_task, return_exceptions=True)
        store.close()


def p(values, q):
    xs = sorted(v for v in values if v is not None)
    if not xs:
        return None
    k = (len(xs) - 1) * q / 100
    lo = int(k)
    hi = min(lo + 1, len(xs) - 1)
    return xs[lo] + (xs[hi] - xs[lo]) * (k - lo)


def fmt(x, nd=2):
    return "-" if x is None else f"{x:.{nd}f}"


async def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--interim", type=float, default=0.3)
    ap.add_argument("--json", type=Path)
    ap.add_argument("--cache", type=Path, default=Path(".scratch/perf/utterances"))
    args = ap.parse_args()
    logger.remove()
    # Pipecat reports the closed LLM port and the cut-off TTS context; neither matters here.
    logger.add(lambda m: print(m, end=""), level="CRITICAL", diagnose=False)

    models = Models.load(VoiceSettings())  # type: ignore[call-arg]
    voice = Voice(models.tts.kokoro, args.cache)
    configs = {
        "off": {"interim": 0.0, "min_words": 2},
        "on": {"interim": args.interim, "min_words": 2},
    }
    cpu_configs = {
        "cpu-off": {"interim": 0.0, "min_words": 99},
        "cpu-on": {"interim": args.interim, "min_words": 99},
    }
    results: dict[str, list[dict]] = {k: [] for k in (*configs, *cpu_configs)}
    for run in range(args.runs):
        for line in LINES:
            for name, kw in configs.items():
                r = await one(models, voice, line, **kw)
                results[name].append(r)
                print(
                    f"run {run + 1} {name:<7} kept {fmt(r['kept'])} record {fmt(r['barge_in'])} "
                    f"interims {r['interims_run']}+{r['interims_skipped']} skipped "
                    f"cpu {fmt(r['cpu_cores'])}  [{r['line'][:24]}…]",
                    flush=True,
                )
        for name, kw in cpu_configs.items():
            r = await one(models, voice, CPU_LINE, **kw)
            results[name].append(r)
            print(
                f"run {run + 1} {name:<7} cpu {fmt(r['cpu_cores'])} cores over "
                f"{r['clip']:.2f}s, interims {r['interims_run']}+{r['interims_skipped']} "
                f"skipped, each {fmt(p(r['interim_secs'], 50), 3)}s",
                flush=True,
            )

    print()
    print(
        "| config | runs | Ghost kept talking p50 / p95 (s) | record barge_in p50 / p95 (s) "
        "| CPU cores while both talk p50 | interims per run | interim pass p50 (s) |"
    )
    print("|---|---|---|---|---|---|---|")
    for name, rs in results.items():
        kept = [r["kept"] for r in rs]
        rec = [r["barge_in"] for r in rs]
        passes = [s for r in rs for s in r["interim_secs"]]
        print(
            f"| {name} | {len(rs)} | {fmt(p(kept, 50))} / {fmt(p(kept, 95))} "
            f"| {fmt(p(rec, 50))} / {fmt(p(rec, 95))} | {fmt(p([r['cpu_cores'] for r in rs], 50))} "
            f"| {statistics.mean(r['interims_run'] for r in rs):.1f} "
            f"| {fmt(p(passes, 50), 3)} |"
        )
    if args.json:
        args.json.write_text(json.dumps(results, indent=1))


if __name__ == "__main__":
    asyncio.run(main())
