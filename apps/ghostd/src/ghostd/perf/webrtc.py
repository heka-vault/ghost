"""The phone path on loopback: an aiortc peer plays the scripted lines over WebRTC.

The desk side is what ``ghostd`` builds for a phone (ADR 0016): a ``SmallWebRTCConnection``
with no ICE servers and a ``SmallWebRTCTransport`` carrying Kokoro's 24 kHz speech and
the thinking sound. The peer sends Opus from the scripted ``Mouth`` at real-time pace and
listens to Ghost's track, noting when audio louder than ``LOUD`` first arrives after each
line: Ghost's voice as the phone would hear it, network and jitter buffer included.
Run it with the thinking sound off (``--set thinking_sound=false``) for that number to
mean Ghost's voice and not the loop.
"""

from __future__ import annotations

import asyncio
import fractions
import time
from typing import Any

import numpy as np
from loguru import logger

from ghostd.perf.audio import IN_RATE, Mouth

SAMPLES = int(IN_RATE * 0.02)
LOUD = 0.05  # RMS, full scale 1.0


class LoopbackPhone:
    def __init__(self, mouth: Mouth) -> None:
        self.mouth = mouth
        self.pc: Any = None
        self.conn: Any = None
        self._tasks: list[asyncio.Task] = []
        self.first_loud: list[float] = []  # monotonic times audio got loud after silence
        self._loud = False

    async def transport_factory(self, connected: asyncio.Event):
        from aiortc import RTCConfiguration, RTCPeerConnection, RTCSessionDescription
        from aiortc.mediastreams import MediaStreamTrack
        from av import AudioFrame
        from ghost_voice.pipeline import TTS_SAMPLE_RATE
        from pipecat.transports.base_transport import TransportParams
        from pipecat.transports.smallwebrtc.connection import SmallWebRTCConnection
        from pipecat.transports.smallwebrtc.transport import SmallWebRTCTransport

        mouth = self.mouth

        class MouthTrack(MediaStreamTrack):
            kind = "audio"

            def __init__(self) -> None:
                super().__init__()
                self._start: float | None = None
                self._ts = 0

            async def recv(self) -> AudioFrame:
                if self._start is None:
                    self._start = time.time()
                else:
                    wait = self._start + self._ts / IN_RATE - time.time()
                    if wait > 0:
                        await asyncio.sleep(wait)
                pcm = mouth.next_frame(SAMPLES * 2)
                frame = AudioFrame(format="s16", layout="mono", samples=SAMPLES)
                frame.planes[0].update(pcm)
                frame.sample_rate = IN_RATE
                frame.pts = self._ts
                frame.time_base = fractions.Fraction(1, IN_RATE)
                self._ts += SAMPLES
                return frame

        pc = RTCPeerConnection(RTCConfiguration(iceServers=[]))
        self.pc = pc
        pc.addTrack(MouthTrack())
        pc.createDataChannel("chat")

        @pc.on("track")
        def on_track(track) -> None:
            self._tasks.append(asyncio.ensure_future(self._listen(track)))

        await pc.setLocalDescription(await pc.createOffer())
        conn = SmallWebRTCConnection(ice_servers=[])
        self.conn = conn
        await conn.initialize(sdp=pc.localDescription.sdp, type=pc.localDescription.type)
        answer = conn.get_answer()
        await pc.setRemoteDescription(RTCSessionDescription(answer["sdp"], answer["type"]))

        def factory(mixer: Any) -> SmallWebRTCTransport:
            transport = SmallWebRTCTransport(
                webrtc_connection=conn,
                params=TransportParams(
                    audio_in_enabled=True,
                    audio_out_enabled=True,
                    audio_out_sample_rate=TTS_SAMPLE_RATE,
                    audio_out_mixer=mixer,
                ),
            )

            @transport.event_handler("on_client_connected")
            async def _connected(_t: Any, _c: Any) -> None:
                connected.set()

            return transport

        return factory

    async def _listen(self, track) -> None:
        try:
            while True:
                frame = await track.recv()
                pcm = frame.to_ndarray().astype(np.float32) / 32768.0
                rms = float(np.sqrt(np.mean(pcm * pcm))) if pcm.size else 0.0
                if rms >= LOUD and not self._loud:
                    self.first_loud.append(time.monotonic())
                self._loud = rms >= LOUD or (self._loud and rms >= LOUD / 3)
        except Exception:  # noqa: BLE001 - the track ends with the peer
            logger.debug("perf: phone track ended")

    def loud_after(self, t: float) -> float | None:
        """Seconds from ``t`` to the first loud audio the phone received after it."""
        later = [x for x in self.first_loud if x > t]
        return later[0] - t if later else None

    async def close(self) -> None:
        for t in self._tasks:
            t.cancel()
        await asyncio.gather(*self._tasks, return_exceptions=True)
        if self.pc is not None:
            await self.pc.close()
        if self.conn is not None:
            try:
                await self.conn.disconnect()
            except Exception:  # noqa: BLE001
                pass
