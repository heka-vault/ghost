"""Each session's TTS is a fresh processor over the one loaded Kokoro model.

Pipecat marks a processor that sees a CancelFrame as cancelling for good, so a
processor shared across sessions would leave every session after a cancelled one
mute. No real model is loaded here: a fake stands in for ``kokoro_onnx.Kokoro``.
"""

from __future__ import annotations

import numpy as np
import pytest
from ghost_voice.kokoro import KokoroTTS
from ghost_voice.turns import SmartTurn
from pipecat.audio.turn.smart_turn.base_smart_turn import BaseSmartTurn
from pipecat.frames.frames import CancelFrame, TTSAudioRawFrame, TTSSpeakFrame
from pipecat.pipeline.worker import PipelineParams
from pipecat.tests.utils import SleepFrame, run_test


class FakeKokoro:
    voices = ["af_heart", "am_adam"]

    def __init__(self) -> None:
        self.spoken: list[tuple[str, str]] = []

    async def create_stream(self, text, *, voice, lang, speed):
        self.spoken.append((text, voice))
        yield np.zeros(2400, dtype=np.float32), 24000


def template(kokoro: FakeKokoro) -> KokoroTTS:
    return KokoroTTS(kokoro=kokoro, settings=KokoroTTS.Settings(voice="af_heart"))  # type: ignore[arg-type]


async def speak(tts: KokoroTTS, *, then_cancel: bool = False):
    frames = [TTSSpeakFrame("hello"), SleepFrame(sleep=0.2)]
    if then_cancel:
        frames.append(CancelFrame())
    down, _ = await run_test(
        tts,
        frames_to_send=frames,
        send_end_frame=not then_cancel,
        pipeline_params=PipelineParams(audio_out_sample_rate=24000),
    )
    return [f for f in down if isinstance(f, TTSAudioRawFrame)]


@pytest.mark.asyncio
async def test_session_after_a_cancelled_one_still_speaks():
    kokoro = FakeKokoro()
    models_tts = template(kokoro)

    first = models_tts.fork()
    assert await speak(first, then_cancel=True)
    assert first._cancelling  # noqa: SLF001 - the latch the fork exists to sidestep

    second = models_tts.fork()
    assert second is not first
    assert not second._cancelling  # noqa: SLF001
    assert second.kokoro is kokoro  # the loaded model is reused, not reloaded
    assert await speak(second)
    assert kokoro.spoken == [("hello", "af_heart"), ("hello", "af_heart")]


def test_fork_keeps_the_template_settings():
    tts = KokoroTTS(
        kokoro=FakeKokoro(),  # type: ignore[arg-type]
        settings=KokoroTTS.Settings(voice="am_adam", speed=1.2),
    )
    fork = tts.fork()
    assert fork._settings.voice == "am_adam"  # noqa: SLF001
    assert fork._settings.speed == 1.2  # noqa: SLF001


def test_unknown_voice_is_rejected():
    with pytest.raises(ValueError):
        KokoroTTS(kokoro=FakeKokoro(), settings=KokoroTTS.Settings(voice="nope"))  # type: ignore[arg-type]


def test_smart_turn_fork_shares_the_model_but_not_the_audio():
    turn = SmartTurn.__new__(SmartTurn)  # skip loading the ONNX model
    BaseSmartTurn.__init__(turn)
    turn._session = object()  # noqa: SLF001
    turn._log_data = False  # noqa: SLF001
    turn._audio_buffer.append("half a turn")  # noqa: SLF001
    turn._speech_triggered = True  # noqa: SLF001

    fork = turn.fork()
    assert fork._session is turn._session  # noqa: SLF001
    assert fork._audio_buffer == [] and not fork.speech_triggered  # noqa: SLF001
