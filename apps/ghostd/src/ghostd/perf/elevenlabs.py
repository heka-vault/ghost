"""ElevenLabs time to first audio for ``ghostd perf micro``, next to Kokoro's (ADR 0026).

Drives Ghost's own ``ElevenLabsTTS``, the processor a session would use (spoken-text
filter, clause aggregation, Flash v2.5 over the multi-stream websocket, 24 kHz PCM), in a
pipeline of its own: one connection, timed, then every line on that warm socket, as in a
session. Only the benchmark's fixed lines are sent. Without ELEVENLABS_API_KEY and
GHOST_ELEVENLABS_VOICE it reports why and sends nothing.

Times include the round trip to ElevenLabs from wherever this machine is; run it from the
workstation, on its usual network.
"""

from __future__ import annotations

import asyncio
import time
from collections.abc import Callable
from typing import Any

from ghost_voice import VoiceSettings
from ghost_voice.elevenlabs import DEFAULT_URL, ElevenLabsTTS, tts_problems
from pipecat.frames.frames import EndFrame, TTSAudioRawFrame, TTSSpeakFrame, TTSStoppedFrame
from pipecat.observers.base_observer import BaseObserver, FramePushed
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineParams, PipelineWorker
from pipecat.workers.runner import WorkerRunner

LINE_TIMEOUT_SECS = 15.0


class _Clock(BaseObserver):
    """When the service pushes its first audio for a line, and when the line ends."""

    def __init__(self, tts: ElevenLabsTTS) -> None:
        super().__init__()
        self._tts = tts
        self.first: float | None = None
        self.audio_bytes = 0
        self.done = asyncio.Event()

    def reset(self) -> None:
        self.first = None
        self.audio_bytes = 0
        self.done.clear()

    async def on_push_frame(self, data: FramePushed) -> None:
        if data.source is not self._tts:
            return
        if isinstance(data.frame, TTSAudioRawFrame):
            if self.first is None:
                self.first = time.perf_counter()
            self.audio_bytes += len(data.frame.audio)
        elif isinstance(data.frame, TTSStoppedFrame):
            self.done.set()


async def first_audio(
    settings: VoiceSettings,
    lines: dict[str, str],
    n: int,
    stats: Callable[[list[float]], dict[str, float]],
    *,
    url: str = DEFAULT_URL,
) -> dict[str, Any]:
    """``{"skipped": why}``, ``{"failed": why}``, or the connect time and, per line, time
    to first audio and to the end of the line."""
    problems = tts_problems(settings.model_copy(update={"tts_engine": "elevenlabs"}))
    if problems:
        return {"skipped": "; ".join(problems)}
    tts = ElevenLabsTTS(
        api_key=settings.elevenlabs_api_key,
        voice=settings.elevenlabs_voice,
        model=settings.elevenlabs_model,
        clause_min_words=settings.tts_clause_min_words if settings.tts_clauses else None,
        url=url,
    )
    # No speaker here: nothing would lift the pause it takes after each line.
    tts._pause_frame_processing = False  # noqa: SLF001
    clock = _Clock(tts)
    connected = asyncio.Event()

    @tts.event_handler("on_connected")
    async def _connected(_tts) -> None:
        connected.set()

    async def _failed(_reason: str) -> None:  # stop waiting: there is nothing to time
        connected.set()
        clock.done.set()

    tts.on_written_off = _failed
    worker = PipelineWorker(
        Pipeline([tts]),
        params=PipelineParams(audio_out_sample_rate=24000),
        observers=[clock],
        cancel_on_idle_timeout=False,
        enable_rtvi=False,
    )
    runner = WorkerRunner(handle_sigint=False)
    started = time.perf_counter()
    await runner.add_workers(worker)
    run = asyncio.ensure_future(runner.run())
    out: dict[str, Any] = {"model": settings.elevenlabs_model, "lines": {}}
    try:
        try:
            await asyncio.wait_for(connected.wait(), LINE_TIMEOUT_SECS)
        except TimeoutError:
            pass
        if tts.written_off or not connected.is_set():
            return {"failed": tts.failure or "no connection"}
        out["connect"] = time.perf_counter() - started
        for label, text in lines.items():
            first, whole, audio = [], [], 0.0
            for _ in range(n):
                clock.reset()
                t = time.perf_counter()
                await worker.queue_frame(TTSSpeakFrame(text))
                try:
                    await asyncio.wait_for(clock.done.wait(), LINE_TIMEOUT_SECS)
                except TimeoutError:
                    pass
                if tts.written_off or clock.first is None:
                    return {"failed": tts.failure or f"no audio for {label!r}"}
                first.append(clock.first - t)
                whole.append(time.perf_counter() - t)
                audio = clock.audio_bytes / (2 * 24000)
            out["lines"][label] = {"first": stats(first), "whole": stats(whole), "audio": audio}
        return out
    finally:
        await worker.queue_frame(EndFrame())
        try:
            await asyncio.wait_for(run, 10)
        except TimeoutError:
            await runner.cancel()
