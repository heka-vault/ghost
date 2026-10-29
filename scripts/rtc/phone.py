"""The phone, automated: an aiortc peer that does what ``apps/mobile`` does on a call
(``docs/phone-protocol.md``), against ``crates/ghost-rtc``.

It offers one Opus audio track playing the marker tone (a 1 kHz burst once a second, the
same pattern as ``ghost_rtc::marker``) and a ``chat`` data channel, with no ICE servers;
sends ``ping`` once a second and ``client-ready`` when the channel opens; expects
``bot-ready``; listens to the desk's track for the bursts coming back; mid-call it can
redo the offer with ``restart_pc`` on a fresh peer connection; at the end it reads the
call's stats, says ``disconnect-bot`` and writes a JSON report.

Every burst's send time (the wall clock when the frame holding its first sample was handed
to aiortc) and every received burst's time (when the frame holding it came out of the
decoder) are in the report, so ``measure.py`` can pair them with the desk's own log and
read the one-way latency in each direction.

Signalling is either HTTP (``--url http://127.0.0.1:PORT``, the probe's unsigned
``/offer``) or stdio (``--stdio``: an offer as one JSON line on stdout, the answer as one
JSON line on stdin; used by the crate's integration tests). Run it with the venv's Python:

    /home/detroitpro/projects/active/ghost/.venv/bin/python -I scripts/rtc/phone.py --url ...

This file reads nothing but its own arguments; nothing it receives is kept but numbers.
"""

from __future__ import annotations

import argparse
import asyncio
import fractions
import json
import math
import re
import sys
import time
import urllib.request
from typing import Any

import numpy as np
from aiortc import RTCConfiguration, RTCPeerConnection, RTCSessionDescription
from aiortc.mediastreams import MediaStreamTrack
from av import AudioFrame

RATE = 48000
SAMPLES = 960  # 20 ms
PERIOD_MS = 1000
BURST_MS = 200
HZ = 1000.0
AMPLITUDE = 0.5
THRESHOLD = 4000
QUIET_MS = 300


def log(msg: str) -> None:
    print(f"phone: {msg}", file=sys.stderr, flush=True)


class MarkerTrack(MediaStreamTrack):
    """The marker tone at real-time pace, noting when each burst's first sample went out."""

    kind = "audio"

    def __init__(self, sent: list[float]) -> None:
        super().__init__()
        self._start: float | None = None
        self._ts = 0
        self._pos = 0
        self._period = RATE * PERIOD_MS // 1000
        self._burst = RATE * BURST_MS // 1000
        self.sent = sent
        n = np.arange(self._burst)
        self._tone = (AMPLITUDE * np.sin(2 * np.pi * HZ * n / RATE) * 32767).astype(np.int16)

    async def recv(self) -> AudioFrame:
        if self._start is None:
            self._start = time.time()
        else:
            wait = self._start + self._ts / RATE - time.time()
            if wait > 0:
                await asyncio.sleep(wait)
        pcm = np.zeros(SAMPLES, dtype=np.int16)
        onset = None
        for i in range(SAMPLES):
            if self._pos == 0:
                onset = i
            if self._pos < self._burst:
                pcm[i] = self._tone[self._pos]
            self._pos = (self._pos + 1) % self._period
        frame = AudioFrame(format="s16", layout="mono", samples=SAMPLES)
        frame.planes[0].update(pcm.tobytes())
        frame.sample_rate = RATE
        frame.pts = self._ts
        frame.time_base = fractions.Fraction(1, RATE)
        self._ts += SAMPLES
        if onset is not None:
            self.sent.append(time.time())
        return frame


class OnsetDetector:
    def __init__(self, rate: int) -> None:
        self.quiet_needed = rate * QUIET_MS // 1000
        self.quiet = self.quiet_needed
        self.count = 0

    def feed(self, samples: np.ndarray) -> bool:
        loud = np.abs(samples.astype(np.int32)) > THRESHOLD
        if not loud.any():
            self.quiet += len(samples)
            return False
        first = int(np.argmax(loud))
        found = self.quiet + first >= self.quiet_needed
        if found:
            self.count += 1
        last = int(len(samples) - 1 - np.argmax(loud[::-1]))
        self.quiet = len(samples) - 1 - last
        return found


class Phone:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.pc: RTCPeerConnection | None = None
        self.channel: Any = None
        self.pc_id: str | None = None
        self.sent: list[float] = []
        self.received: list[float] = []
        self.bot_ready: list[float] = []
        self.server_messages = 0
        self.other_messages = 0
        self.answers: list[dict[str, Any]] = []
        self.channel_opens: list[float] = []
        self.connected: list[float] = []
        self.errors: list[str] = []
        self._listen_tasks: list[asyncio.Task] = []
        self._ping_task: asyncio.Task | None = None
        self.stats_before_restart: dict[str, Any] | None = None

    # --- signalling -------------------------------------------------------------

    def _signal(self, body: dict[str, Any]) -> dict[str, Any]:
        if self.args.stdio:
            print(json.dumps({"type": "offer", **body}), flush=True)
            line = sys.stdin.readline()
            if not line:
                raise RuntimeError("no answer on stdin")
            return json.loads(line)
        req = urllib.request.Request(
            self.args.url.rstrip("/") + "/offer",
            data=json.dumps(body).encode(),
            headers={"content-type": "application/json"},
            method="POST",
        )
        with urllib.request.urlopen(req, timeout=10) as r:  # noqa: S310 - our own address
            return json.loads(r.read())

    async def _offer(self, restart: bool) -> None:
        pc = RTCPeerConnection(RTCConfiguration(iceServers=[]))
        self.pc = pc
        track = MarkerTrack(self.sent)
        pc.addTrack(track)
        channel = pc.createDataChannel("chat", ordered=True)
        self.channel = channel
        opened = asyncio.Event()

        @channel.on("open")
        def on_open() -> None:
            self.channel_opens.append(time.time())
            opened.set()
            channel.send(json.dumps({
                "label": "rtvi-ai", "type": "client-ready", "id": f"r{len(self.channel_opens)}",
                "data": {"version": "2.1.0", "about": {"library": "ghost-mobile"}},
            }))

        @channel.on("message")
        def on_message(message: Any) -> None:
            if not isinstance(message, str):
                self.other_messages += 1
                return
            try:
                msg = json.loads(message)
            except ValueError:
                self.other_messages += 1
                return
            kind = msg.get("type") if isinstance(msg, dict) else None
            if msg.get("label") != "rtvi-ai":
                self.other_messages += 1
            elif kind == "bot-ready":
                self.bot_ready.append(time.time())
            elif kind == "server-message":
                self.server_messages += 1
            else:
                self.other_messages += 1

        @pc.on("track")
        def on_track(remote: MediaStreamTrack) -> None:
            self._listen_tasks.append(asyncio.ensure_future(self._listen(remote)))

        @pc.on("connectionstatechange")
        def on_state() -> None:
            log(f"connection {pc.connectionState}")
            if pc.connectionState == "connected":
                self.connected.append(time.time())

        await pc.setLocalDescription(await pc.createOffer())
        body: dict[str, Any] = {"sdp": pc.localDescription.sdp, "type": "offer"}
        if restart:
            body["pc_id"] = self.pc_id
            body["restart_pc"] = True
        t0 = time.time()
        answer = self._signal(body)
        took = time.time() - t0
        if "error" in answer:
            raise RuntimeError(f"offer refused: {answer['error']}")
        sdp = answer["sdp"]
        self.pc_id = answer.get("pc_id", self.pc_id)
        fingerprint = None
        m = re.search(r"^a=fingerprint:(\S+)\s+(\S+)", sdp, re.M)
        if m:
            fingerprint = f"{m.group(1).lower()} {m.group(2).upper()}"
        self.answers.append({
            "at": t0, "took": took, "restart": restart, "fingerprint": fingerprint,
            "sha256": bool(fingerprint and fingerprint.startswith("sha-256 ")),
            "setup": (re.search(r"^a=setup:(\S+)", sdp, re.M) or [None, None])[1],
            "candidates": len(re.findall(r"^a=candidate:", sdp, re.M)),
            "ice_lite": bool(re.search(r"^a=ice-lite", sdp, re.M)),
        })
        if not fingerprint:
            raise RuntimeError("the answer has no fingerprint")
        await pc.setRemoteDescription(RTCSessionDescription(sdp, answer["type"]))
        try:
            await asyncio.wait_for(opened.wait(), 15)
        except TimeoutError:
            self.errors.append("the data channel never opened")
        if self._ping_task is None or self._ping_task.done():
            self._ping_task = asyncio.ensure_future(self._ping())

    async def _ping(self) -> None:
        while True:
            await asyncio.sleep(1)
            ch = self.channel
            if ch is not None and ch.readyState == "open":
                ch.send("ping")

    async def _listen(self, remote: MediaStreamTrack) -> None:
        detector = OnsetDetector(RATE)
        try:
            while True:
                frame = await remote.recv()
                pcm = frame.to_ndarray()
                mono = pcm[0] if pcm.ndim == 2 else pcm
                if frame.layout.name != "mono" and pcm.ndim == 2 and pcm.shape[0] == 1:
                    mono = pcm[0][::2]  # packed stereo: the left channel
                if detector.feed(mono):
                    self.received.append(time.time())
        except Exception:  # noqa: BLE001 - the track ends with the peer
            pass

    async def _stats(self) -> dict[str, Any]:
        out: dict[str, Any] = {}
        if self.pc is None:
            return out
        try:
            report = await self.pc.getStats()
        except Exception as e:  # noqa: BLE001
            self.errors.append(f"getStats: {type(e).__name__}")
            return out
        for s in report.values():
            if s.type == "inbound-rtp" and getattr(s, "kind", "") == "audio":
                out["inbound"] = {
                    "packets_received": s.packetsReceived, "packets_lost": s.packetsLost,
                    "jitter": s.jitter,
                }
            elif s.type == "outbound-rtp" and getattr(s, "kind", "") == "audio":
                out["outbound"] = {"packets_sent": s.packetsSent, "bytes_sent": s.bytesSent}
            elif s.type == "remote-inbound-rtp":
                out["remote_inbound"] = {
                    "packets_lost": s.packetsLost, "jitter": s.jitter,
                    "round_trip_time": s.roundTripTime, "fraction_lost": s.fractionLost,
                }
            elif s.type == "transport":
                out["transport"] = {
                    "packets_sent": s.packetsSent, "packets_received": s.packetsReceived,
                    "dtls_state": s.dtlsState,
                }
        return out

    async def _close_pc(self) -> None:
        for t in self._listen_tasks:
            t.cancel()
        await asyncio.gather(*self._listen_tasks, return_exceptions=True)
        self._listen_tasks = []
        if self.pc is not None:
            await self.pc.close()
            self.pc = None

    async def run(self) -> dict[str, Any]:
        started = time.time()
        await self._offer(restart=False)
        end = started + self.args.seconds
        restart_at = started + self.args.restart_at if self.args.restart_at > 0 else None
        restarted = False
        while time.time() < end:
            await asyncio.sleep(0.2)
            if restart_at is not None and not restarted and time.time() >= restart_at:
                restarted = True
                log("restart_pc")
                self.stats_before_restart = await self._stats()
                await self._close_pc()
                try:
                    await self._offer(restart=True)
                except Exception as e:  # noqa: BLE001
                    self.errors.append(f"restart: {type(e).__name__}: {e}")
                    break
        stats = await self._stats()
        ch = self.channel
        if ch is not None and ch.readyState == "open":
            ch.send(json.dumps({"label": "rtvi-ai", "type": "disconnect-bot", "id": "bye", "data": {}}))
            await asyncio.sleep(0.3)
        if self._ping_task:
            self._ping_task.cancel()
        await self._close_pc()
        return {
            "seconds": self.args.seconds,
            "restart_at": self.args.restart_at,
            "sent": self.sent,
            "received": self.received,
            "bot_ready": self.bot_ready,
            "server_messages": self.server_messages,
            "other_messages": self.other_messages,
            "answers": self.answers,
            "channel_opens": self.channel_opens,
            "connected": self.connected,
            "stats": stats,
            "stats_before_restart": self.stats_before_restart,
            "errors": self.errors,
        }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--url", help="the probe's address, http://127.0.0.1:PORT")
    group.add_argument("--stdio", action="store_true", help="offer on stdout, answer on stdin")
    parser.add_argument("--seconds", type=float, default=10.0)
    parser.add_argument("--restart-at", type=float, default=0.0, help="restart_pc after this many seconds (0: never)")
    parser.add_argument("--report", help="write the JSON report here (else the last stdout line)")
    args = parser.parse_args()
    phone = Phone(args)
    report = asyncio.run(phone.run())
    text = json.dumps(report)
    if args.report:
        with open(args.report, "w", encoding="utf-8") as f:
            f.write(text)
    else:
        print(json.dumps({"type": "report", **report}), flush=True)
    log(f"done: {len(phone.sent)} bursts sent, {len(phone.received)} heard, bot-ready x{len(phone.bot_ready)}, errors {phone.errors}")
    return 1 if phone.errors else 0


if __name__ == "__main__":
    sys.exit(main())
