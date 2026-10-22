"""The ElevenLabs voice and its Kokoro fallback (proposed ADR 0026).

Nothing here leaves the machine. A fake ElevenLabs speaks the multi-stream websocket
protocol on 127.0.0.1 (``FakeElevenLabs``), a closed local port stands in for a network
that isn't there, and a fake model stands in for Kokoro, so the real ``ElevenLabsTTS`` and
Pipecat's switcher run end to end.
"""

from __future__ import annotations

import asyncio
import base64
import json
import socket
from http import HTTPStatus

import numpy as np
import pytest
from ghost_voice.config import VoiceSettings
from ghost_voice.elevenlabs import (
    ElevenLabsTTS,
    TTSFailover,
    Unheard,
    describe_tts,
    line_voices,
    session_tts,
)
from ghost_voice.kokoro import KokoroTTS
from loguru import logger
from pipecat.frames.frames import (
    ErrorFrame,
    LLMFullResponseEndFrame,
    LLMFullResponseStartFrame,
    LLMTextFrame,
    TTSAudioRawFrame,
    TTSSpeakFrame,
    TTSUpdateSettingsFrame,
)
from pipecat.pipeline.worker import PipelineParams
from pipecat.tests.utils import SleepFrame, run_test
from websockets.asyncio.server import serve

KEY = "sk_test_do_not_print_0123456789abcdef"


class FakeKokoro:
    voices = ["af_heart", "am_adam"]

    def __init__(self) -> None:
        self.spoken: list[tuple[str, str]] = []

    async def create_stream(self, text, *, voice, lang, speed):
        self.spoken.append((text, voice))
        yield np.zeros(2400, dtype=np.float32), 24000


def kokoro(fake: FakeKokoro | None = None) -> KokoroTTS:
    return KokoroTTS(kokoro=fake or FakeKokoro(), settings=KokoroTTS.Settings(voice="af_heart"))  # type: ignore[arg-type]


ENV = (
    "GHOST_TTS_ENGINE",
    "ELEVENLABS_API_KEY",
    "GHOST_ELEVENLABS_VOICE",
    "GHOST_ELEVENLABS_VOICES",
    "GHOST_ELEVENLABS_MODEL",
)


@pytest.fixture
def settings(monkeypatch):
    """VoiceSettings read from the environment, as the daemon reads them."""

    def make(**env: str) -> VoiceSettings:
        for name in ENV:
            monkeypatch.delenv(name, raising=False)
        for name, value in env.items():
            monkeypatch.setenv(name, value)
        return VoiceSettings()  # type: ignore[call-arg]

    return make


def closed_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class FakeElevenLabs:
    """Enough of ElevenLabs' multi-stream-input socket: audio with character alignment for
    each text message, ``isFinal`` when a context closes. ``drop_after`` closes the socket
    with an error once that many texts have been answered; ``reject`` refuses the
    handshake with that HTTP status."""

    def __init__(self, *, drop_after: int | None = None, reject: HTTPStatus | None = None):
        self.drop_after = drop_after
        self.reject = reject
        self.paths: list[str] = []
        self.keys: list[str | None] = []
        self.texts: list[str] = []
        self._server = None

    @property
    def url(self) -> str:
        host, port = self._server.sockets[0].getsockname()[:2]  # type: ignore[union-attr]
        return f"ws://{host}:{port}"

    async def __aenter__(self) -> FakeElevenLabs:
        self._server = await serve(self._handle, "127.0.0.1", 0, process_request=self._check)
        return self

    async def __aexit__(self, *exc) -> None:
        self._server.close()  # type: ignore[union-attr]
        await self._server.wait_closed()  # type: ignore[union-attr]

    def _check(self, connection, request):
        self.paths.append(request.path)
        self.keys.append(request.headers.get("xi-api-key"))
        if self.reject:
            return connection.respond(self.reject, "no\n")
        return None

    async def _handle(self, ws) -> None:
        answered = 0
        async for raw in ws:
            msg = json.loads(raw)
            ctx = msg.get("context_id")
            if msg.get("close_socket"):
                await ws.close()
                return
            if msg.get("close_context") or msg.get("flush"):
                await ws.send(json.dumps({"isFinal": True, "contextId": ctx}))
                continue
            text = msg.get("text", "")
            if not text.strip() or ctx is None:
                continue  # context init or keepalive
            self.texts.append(text)
            if self.drop_after is not None and answered >= self.drop_after:
                continue  # the network is gone: nothing comes back for this one
            spoken = text if text.endswith(" ") else text + " "
            await ws.send(
                json.dumps(
                    {
                        "audio": base64.b64encode(bytes(4800)).decode(),
                        "alignment": {
                            "chars": list(spoken),
                            "charStartTimesMs": [10 * i for i in range(len(spoken))],
                            "charDurationsMs": [10] * len(spoken),
                        },
                        "contextId": ctx,
                    }
                )
            )
            answered += 1
            if self.drop_after is not None and answered >= self.drop_after:
                await asyncio.sleep(0.3)  # let the heard words reach the switcher first
                await ws.close(code=1011, reason="gone")
                return


def failover(url: str, fake: FakeKokoro, **kw) -> TTSFailover:
    cloud = ElevenLabsTTS(api_key=KEY, voice="el_default", url=url, clause_min_words=4, **kw)
    # No output transport here, so no BotStoppedSpeakingFrame would ever lift the pause
    # ElevenLabs takes after each utterance.
    cloud._pause_frame_processing = False  # noqa: SLF001
    return TTSFailover(cloud, kokoro(fake).fork(clause_min_words=4))


async def run(tts, frames):
    return await asyncio.wait_for(
        run_test(
            tts,
            frames_to_send=frames,
            pipeline_params=PipelineParams(audio_out_sample_rate=24000),
        ),
        timeout=15,
    )


@pytest.fixture
def logs():
    lines: list[str] = []
    sink = logger.add(lambda m: lines.append(str(m)), level="TRACE")
    yield lines
    logger.remove(sink)


# --- choosing the engine --------------------------------------------------------------


def test_kokoro_is_the_default(settings):
    tts = session_tts(settings(), kokoro(), clause_min_words=4)
    assert isinstance(tts, KokoroTTS)


def test_elevenlabs_is_chosen_with_kokoro_behind_it(settings):
    s = settings(
        GHOST_TTS_ENGINE="ElevenLabs",
        ELEVENLABS_API_KEY=KEY,
        GHOST_ELEVENLABS_VOICE="el_default",
    )
    tts = session_tts(s, kokoro(), clause_min_words=4)
    assert isinstance(tts, TTSFailover)
    cloud = tts.primary
    assert isinstance(cloud, ElevenLabsTTS) and isinstance(tts.fallback, KokoroTTS)
    assert cloud._settings.model == "eleven_flash_v2_5"  # noqa: SLF001
    assert cloud._settings.voice == "el_default"  # noqa: SLF001
    assert cloud._init_sample_rate == 24000  # noqa: SLF001 - pcm_24000, like Kokoro
    assert type(cloud._text_aggregator).__name__ == "ClauseTextAggregator"  # noqa: SLF001
    assert type(cloud._text_filters[0]).__name__ == "SpokenTextFilter"  # noqa: SLF001
    assert cloud._auto_mode is True  # noqa: SLF001 - text arrives in whole clauses


def test_unknown_engine_is_refused(settings):
    with pytest.raises(ValueError):
        settings(GHOST_TTS_ENGINE="piper")


@pytest.mark.parametrize(
    "env",
    [
        {"GHOST_ELEVENLABS_VOICE": "el_default"},  # no key
        {"ELEVENLABS_API_KEY": KEY},  # no voice
    ],
)
def test_missing_key_or_voice_is_kokoro_logged_once(env, logs, settings, monkeypatch):
    monkeypatch.setattr("ghost_voice.elevenlabs._warned", set())  # once per process
    s = settings(GHOST_TTS_ENGINE="elevenlabs", **env)
    for _ in range(3):
        assert isinstance(session_tts(s, kokoro(), clause_min_words=4), KokoroTTS)
    warnings = [line for line in logs if "Kokoro speaks instead" in line]
    assert len(warnings) == 1
    assert KEY not in "".join(logs)


def test_the_key_stays_out_of_the_settings_repr_and_the_description(settings):
    s = settings(
        GHOST_TTS_ENGINE="elevenlabs",
        ELEVENLABS_API_KEY=KEY,
        GHOST_ELEVENLABS_VOICE="el_default",
    )
    assert s.elevenlabs_api_key == KEY
    assert KEY not in repr(s) and KEY not in str(s)
    assert KEY not in describe_tts(s)


def test_line_voices_are_read_by_slug():
    voices, bad = line_voices(" accounting = el_acc ,research=el_res,,oops, =x")
    assert voices == {"accounting": "el_acc", "research": "el_res"}
    assert bad == ["oops", "=x"]


# --- a line's voice ---------------------------------------------------------------------


def test_a_kokoro_voice_name_never_becomes_the_elevenlabs_voice():
    active = ["ghost"]
    cloud = ElevenLabsTTS(
        api_key=KEY,
        voice="el_default",
        voices={"accounting": "el_acc"},
        line=lambda: active[0],
    )
    delta = KokoroTTS.Settings(voice="am_adam", speed=1.0)
    assert cloud._line_delta(delta).voice == "el_default"  # noqa: SLF001
    active[0] = "accounting"
    out = cloud._line_delta(delta)  # noqa: SLF001
    assert out.voice == "el_acc"
    assert out.speed is None  # 1.0 is ElevenLabs' own default
    assert cloud._line_delta(KokoroTTS.Settings(speed=1.6)).speed == 1.2  # noqa: SLF001
    assert cloud._line_delta(KokoroTTS.Settings(speed=0.5)).speed == 0.7  # noqa: SLF001


async def test_a_transfer_reconnects_in_the_lines_elevenlabs_voice():
    active = ["ghost"]
    fake = FakeKokoro()
    async with FakeElevenLabs() as server:
        tts = failover(server.url, fake, voices={"accounting": "el_acc"}, line=lambda: active[0])
        active[0] = "accounting"  # what ConversationRuntime.switch does before the frames
        down, _ = await run(
            tts,
            [
                TTSSpeakFrame("Patching you through."),
                SleepFrame(sleep=0.3),
                # The runtime's voice frame: Kokoro settings for the line.
                TTSUpdateSettingsFrame(delta=KokoroTTS.Settings(voice="am_adam", speed=1.0)),
                SleepFrame(sleep=0.5),
            ],
        )
    assert server.paths[0].startswith("/v1/text-to-speech/el_default/multi-stream-input")
    assert any(p.startswith("/v1/text-to-speech/el_acc/") for p in server.paths)
    assert not any("am_adam" in p for p in server.paths)
    assert "model_id=eleven_flash_v2_5" in server.paths[0]
    assert "output_format=pcm_24000" in server.paths[0]
    # Sent as the header, never in the URL.
    assert server.keys and all(k == KEY for k in server.keys)
    assert not any(KEY in p for p in server.paths)
    assert fake.spoken == []  # ElevenLabs spoke; Kokoro only took the voice
    assert tts.fallback._settings.voice == "am_adam"  # noqa: SLF001
    assert any(isinstance(f, TTSAudioRawFrame) for f in down)


# --- failing over -----------------------------------------------------------------------


async def test_no_network_at_the_start_is_kokoro_for_the_session(logs):
    fake = FakeKokoro()
    tts = failover(f"ws://127.0.0.1:{closed_port()}", fake)
    down, up = await run(tts, [TTSSpeakFrame("Ghost here."), SleepFrame(sleep=0.3)])
    assert fake.spoken == [("Ghost here.", "af_heart")]
    assert any(isinstance(f, TTSAudioRawFrame) for f in down)
    assert tts.primary.written_off
    assert tts.strategy.active_service is tts.fallback
    assert not [f for f in up if isinstance(f, ErrorFrame)]  # kept inside the switcher
    assert "Kokoro speaks for the rest of this session" in "".join(logs)
    assert KEY not in "".join(logs)


async def test_a_rejected_key_is_kokoro_and_never_printed(logs):
    fake = FakeKokoro()
    async with FakeElevenLabs(reject=HTTPStatus.UNAUTHORIZED) as server:
        tts = failover(server.url, fake)
        _, up = await run(tts, [TTSSpeakFrame("Ghost here."), SleepFrame(sleep=0.3)])
    assert fake.spoken == [("Ghost here.", "af_heart")]
    assert "401" in tts.primary.failure
    assert not [f for f in up if isinstance(f, ErrorFrame)]
    assert KEY not in "".join(logs) and KEY not in tts.primary.failure


async def test_a_socket_that_drops_mid_reply_hands_kokoro_the_rest():
    fake = FakeKokoro()
    async with FakeElevenLabs(drop_after=1) as server:
        tts = failover(server.url, fake)
        down, up = await run(
            tts,
            [
                LLMFullResponseStartFrame(),
                LLMTextFrame("Three issues are waiting, "),
                LLMTextFrame("the oldest is from Monday."),
                LLMFullResponseEndFrame(),
                SleepFrame(sleep=1.0),
                TTSSpeakFrame("Anything else?"),
                SleepFrame(sleep=0.3),
            ],
        )
    assert server.texts[0].startswith("Three issues are waiting,")
    # The first clause was heard from ElevenLabs; Kokoro says the rest, then what follows.
    assert [t for t, _ in fake.spoken] == ["the oldest is from Monday.", "Anything else?"]
    assert tts.primary.written_off
    assert not [f for f in up if isinstance(f, ErrorFrame)]
    assert sum(isinstance(f, LLMFullResponseEndFrame) for f in down) == 1


async def test_a_drop_while_the_reply_is_streaming_continues_it_in_kokoro():
    fake = FakeKokoro()
    async with FakeElevenLabs(drop_after=1) as server:
        tts = failover(server.url, fake)
        await run(
            tts,
            [
                LLMFullResponseStartFrame(),
                # The first clause goes to ElevenLabs; "the old" waits for more.
                LLMTextFrame("Three issues are waiting, the old"),
                SleepFrame(sleep=0.8),  # the socket drops here, mid-response
                LLMTextFrame("est is "),
                LLMTextFrame("from Monday."),
                LLMFullResponseEndFrame(),
                SleepFrame(sleep=0.3),
            ],
        )
    assert server.texts == ["Three issues are waiting,"]
    # The unheard "the old" and the rest of the stream join up into one sentence.
    assert [t for t, _ in fake.spoken] == ["the oldest is from Monday."]


# --- what was heard ---------------------------------------------------------------------


def test_unheard_strikes_off_spoken_words_as_tokens_arrive():
    u = Unheard()
    for token in ["Three", " iss", "ues are", " waiting, the", " old"]:
        u.add(token)
    for word in ["Three", "issues", "are"]:
        u.heard(word)
    assert u.text() == "waiting, the old" and u.partial
    u.add("est.")
    u.end()
    assert u.text() == "waiting, the oldest." and not u.partial


def test_unheard_steps_over_what_the_filter_dropped():
    u = Unheard()
    u.add("- **Infra 17**: see [the PR](https://x.y/1) today. Then lunch. ")
    for word in ["Infra", "17:", "see", "the", "today.", "Then"]:
        u.heard(word)
    assert u.text() == "lunch."
