"""Rendered user utterances and the transports that play them in and Ghost out.

``Voice`` renders a line with Kokoro (in a voice that is not Ghost's), resamples it to
16 kHz mono int16, and caches it under ``.scratch/perf/utterances``. A line may contain
``" ... "``: the parts are rendered separately and joined by a pause, to exercise the
turn hold ("what needs my sign-off ... in the app repo").

``ScriptedTransport`` stands in for the desk's sound devices: its input sends 20 ms
frames at real-time pace, silence unless an utterance is playing; its output accepts
Ghost's audio at the pace a sound card would and drops it.
"""

from __future__ import annotations

import asyncio
import hashlib
import time
import wave
from collections import deque
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from loguru import logger
from pipecat.frames.frames import InputAudioRawFrame, OutputAudioRawFrame, StartFrame
from pipecat.processors.frame_processor import FrameProcessor
from pipecat.transports.base_input import BaseInputTransport
from pipecat.transports.base_output import BaseOutputTransport
from pipecat.transports.base_transport import BaseTransport, TransportParams

IN_RATE = 16000
FRAME_SECS = 0.02
PAUSE_MARK = " ... "


@dataclass(frozen=True)
class Utterance:
    text: str
    pcm: bytes  # 16 kHz mono int16

    @property
    def seconds(self) -> float:
        return len(self.pcm) / 2 / IN_RATE


class Voice:
    """Renders and caches the operator's lines."""

    def __init__(
        self,
        kokoro,
        cache: Path,
        *,
        voice: str = "am_michael",
        speed: float = 1.0,
        pause_secs: float = 0.7,
    ) -> None:
        self._kokoro = kokoro
        self._cache = cache
        self._voice = voice
        self._speed = speed
        self._pause = pause_secs
        cache.mkdir(parents=True, exist_ok=True)

    def render(self, text: str) -> Utterance:
        key = hashlib.sha1(f"{self._voice}|{self._speed}|{self._pause}|{text}".encode()).hexdigest()
        path = self._cache / f"{key[:16]}.wav"
        if path.exists():
            return Utterance(text, _read_wav(path))
        parts = [p.strip() for p in text.split(PAUSE_MARK.strip()) if p.strip()]
        gap = np.zeros(int(IN_RATE * self._pause), dtype=np.float32)
        pieces: list[np.ndarray] = []
        for i, part in enumerate(parts):
            if i:
                pieces.append(gap)
            pieces.append(self._speak(part))
        audio = np.concatenate(pieces)
        peak = float(np.abs(audio).max()) or 1.0
        audio = audio * (0.5 / peak)  # a normal speaking level at the mic
        pcm = (audio * 32767).astype(np.int16).tobytes()
        _write_wav(path, pcm)
        return Utterance(text, pcm)

    def _speak(self, text: str) -> np.ndarray:
        import soxr

        samples, rate = self._kokoro.create(
            text, voice=self._voice, speed=self._speed, lang="en-us"
        )
        samples = np.asarray(samples, dtype=np.float32)
        return soxr.resample(samples, rate, IN_RATE).astype(np.float32)


def _read_wav(path: Path) -> bytes:
    with wave.open(str(path), "rb") as w:
        return w.readframes(w.getnframes())


def _write_wav(path: Path, pcm: bytes) -> None:
    tmp = path.with_suffix(".tmp")
    with wave.open(str(tmp), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(IN_RATE)
        w.writeframes(pcm)
    tmp.replace(path)


class Mouth:
    """What the scripted input plays: a queue of utterances, silence in between."""

    def __init__(self) -> None:
        self._chunks: deque[bytes] = deque()
        self._done: deque[asyncio.Event] = deque()
        self.playing = False

    def say(self, u: Utterance) -> asyncio.Event:
        """Queue ``u``; the event is set when its last frame has been sent."""
        n = int(IN_RATE * FRAME_SECS) * 2
        pcm = u.pcm + bytes((n - len(u.pcm) % n) % n)
        for i in range(0, len(pcm), n):
            self._chunks.append(pcm[i : i + n])
        done = asyncio.Event()
        self._chunks.append(b"")  # marks the end of this utterance
        self._done.append(done)
        return done

    def release(self) -> None:
        """The input stopped (the session ended): nothing queued will be heard."""
        self._chunks.clear()
        while self._done:
            self._done.popleft().set()
        self.playing = False

    def next_frame(self, n: int) -> bytes:
        while self._chunks:
            chunk = self._chunks.popleft()
            if chunk:
                self.playing = True
                return chunk
            self._done.popleft().set()
            self.playing = False
        return bytes(n)


class ScriptedInput(BaseInputTransport):
    def __init__(self, params: TransportParams, mouth: Mouth) -> None:
        super().__init__(params)
        self._mouth = mouth
        self._task: asyncio.Task | None = None

    async def start(self, frame: StartFrame) -> None:
        await super().start(frame)
        await self.set_transport_ready(frame)
        if self._task is None:
            self._task = self.create_task(self._feed(), "scripted-mic")

    async def _feed(self) -> None:
        n = int(self.sample_rate * FRAME_SECS) * 2
        next_t = time.monotonic()
        while True:
            audio = self._mouth.next_frame(n)
            await self.push_audio_frame(
                InputAudioRawFrame(audio=audio, sample_rate=self.sample_rate, num_channels=1)
            )
            next_t += FRAME_SECS
            delay = next_t - time.monotonic()
            if delay > 0:
                await asyncio.sleep(delay)
            elif delay < -0.5:  # fell far behind (a stall): don't burst to catch up
                logger.warning(f"perf: scripted mic {-delay:.2f}s behind")
                next_t = time.monotonic()

    async def _stop_feed(self) -> None:
        if self._task is not None:
            await self.cancel_task(self._task)
            self._task = None
        self._mouth.release()

    async def stop(self, frame) -> None:
        await self._stop_feed()
        await super().stop(frame)

    async def cancel(self, frame) -> None:
        await self._stop_feed()
        await super().cancel(frame)


LOUD = 0.05  # RMS, full scale 1.0: Ghost's voice, not the thinking loop or silence


class NullSpeaker(BaseOutputTransport):
    """Accepts audio at the pace a sound card would (a 40 ms device buffer), drops it,
    and notes when it turns loud: the moment the operator would hear Ghost."""

    LEAD = 0.04

    def __init__(self, params: TransportParams) -> None:
        super().__init__(params)
        self._until = 0.0
        self.seconds_played = 0.0
        self.first_loud: list[float] = []
        self._loud = False

    async def start(self, frame: StartFrame) -> None:
        await super().start(frame)
        await self.set_transport_ready(frame)

    async def write_audio_frame(self, frame: OutputAudioRawFrame) -> bool:
        secs = len(frame.audio) / 2 / frame.sample_rate / max(1, frame.num_channels)
        now = time.monotonic()
        if frame.audio:
            pcm = np.frombuffer(frame.audio, dtype=np.int16).astype(np.float32) / 32768.0
            rms = float(np.sqrt(np.mean(pcm * pcm)))
            if rms >= LOUD and not self._loud:
                # It plays once what is already queued in the device has played.
                self.first_loud.append(max(self._until, now))
            self._loud = rms >= LOUD or (self._loud and rms >= LOUD / 3)
        self._until = max(self._until, now) + secs
        self.seconds_played += secs
        wait = self._until - self.LEAD - now
        if wait > 0:
            await asyncio.sleep(wait)
        return True


def loud_after(times: list[float], t: float) -> float | None:
    later = [x for x in times if x > t]
    return later[0] - t if later else None


class ScriptedTransport(BaseTransport):
    def __init__(self, params: TransportParams, mouth: Mouth) -> None:
        super().__init__()
        self._params = params
        self._mouth = mouth
        self._input: ScriptedInput | None = None
        self._output: NullSpeaker | None = None

    def input(self) -> FrameProcessor:
        if self._input is None:
            self._input = ScriptedInput(self._params, self._mouth)
        return self._input

    def output(self) -> FrameProcessor:
        if self._output is None:
            self._output = NullSpeaker(self._params)
        return self._output

    def loud_after(self, t: float) -> float | None:
        return loud_after(self._output.first_loud, t) if self._output else None
