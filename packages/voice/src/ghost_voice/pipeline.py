"""Build one conversation session."""

from __future__ import annotations

import asyncio
import uuid
from collections.abc import Awaitable, Callable
from dataclasses import dataclass

from anthropic import AsyncAnthropic
from ghost_domain import WriteGate
from ghost_skills import Skill
from loguru import logger
from pipecat.audio.turn.smart_turn.local_smart_turn_v3 import LocalSmartTurnAnalyzerV3
from pipecat.audio.vad.silero import SileroVADAnalyzer
from pipecat.audio.vad.vad_analyzer import VADParams
from pipecat.frames.frames import LLMRunFrame
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineParams, PipelineWorker, ProcessorUnusablePolicy
from pipecat.processors.aggregators.llm_context import LLMContext
from pipecat.processors.aggregators.llm_response_universal import (
    LLMContextAggregatorPair,
    LLMUserAggregatorParams,
)
from pipecat.services.anthropic.llm import AnthropicLLMService
from pipecat.services.kokoro.tts import KokoroTTSService
from pipecat.transports.base_transport import BaseTransport
from pipecat.turns.user_stop.turn_analyzer_user_turn_stop_strategy import (
    TurnAnalyzerUserTurnStopStrategy,
)
from pipecat.turns.user_turn_strategies import UserTurnStrategies
from pipecat.workers.runner import WorkerRunner

from ghost_voice.config import VoiceSettings
from ghost_voice.confirmation import ConfirmationProcessor
from ghost_voice.parakeet import ParakeetSTTService
from ghost_voice.prompt import build_system_prompt
from ghost_voice.tools import register_tools
from ghost_voice.transport import SoundDeviceTransport, SoundDeviceTransportParams


@dataclass
class Models:
    """Loaded once by the daemon and shared across sessions (about 3 GB)."""

    stt: ParakeetSTTService
    tts: KokoroTTSService
    turn: LocalSmartTurnAnalyzerV3

    @classmethod
    def load(cls, s: VoiceSettings) -> Models:
        stt = ParakeetSTTService(
            model=s.stt_model, quantization=s.stt_quantization, threads=s.stt_threads
        )
        stt.load()
        # Prefer files already in Ghost's cache; otherwise Pipecat downloads them.
        model_path = s.tts_model_path or s.cache_dir / "kokoro" / "kokoro-v1.0.onnx"
        voices_path = s.tts_voices_path or s.cache_dir / "kokoro" / "voices-v1.0.bin"
        tts = KokoroTTSService(
            model_path=str(model_path) if model_path.exists() else None,
            voices_path=str(voices_path) if voices_path.exists() else None,
            settings=KokoroTTSService.Settings(voice=s.tts_voice, speed=s.tts_speed),
        )
        turn = LocalSmartTurnAnalyzerV3(cpu_count=s.smart_turn_threads)
        return cls(stt=stt, tts=tts, turn=turn)


@dataclass
class GhostSession:
    session_id: str
    worker: PipelineWorker
    runner: WorkerRunner
    _ended: asyncio.Event

    async def run(self) -> None:
        await self.runner.run()

    async def end(self) -> None:
        if not self._ended.is_set():
            self._ended.set()
            await self.runner.stop_when_done()

    async def cancel(self) -> None:
        self._ended.set()
        await self.runner.cancel()


def build_session(
    settings: VoiceSettings,
    *,
    models: Models,
    skills: list[Skill],
    gate: WriteGate,
    transport: BaseTransport | None = None,
    greeting: str | None = "Ghost. What do you need?",
    on_sleep: Callable[[], Awaitable[None]] | None = None,
) -> GhostSession:
    session_id = uuid.uuid4().hex
    ended = asyncio.Event()

    transport = transport or SoundDeviceTransport(
        SoundDeviceTransportParams(
            audio_in_enabled=True,
            audio_out_enabled=True,
            input_device=settings.input_device,
            output_device=settings.output_device,
        )
    )

    llm = AnthropicLLMService(
        api_key=settings.anthropic_auth_token,
        client=AsyncAnthropic(
            api_key=settings.anthropic_auth_token, base_url=settings.anthropic_base_url
        ),
        settings=AnthropicLLMService.Settings(
            model=settings.llm_model,
            system_instruction=build_system_prompt([s.prompt_fragment for s in skills]),
            max_tokens=settings.llm_max_tokens,
            enable_prompt_caching=True,
            thinking=AnthropicLLMService.ThinkingConfig(type=settings.llm_thinking),
        ),
    )

    session: GhostSession | None = None

    async def sleep() -> None:
        logger.info("go_to_sleep")
        if on_sleep:
            await on_sleep()
        if session:
            await session.end()

    tools = register_tools(llm, skills=skills, gate=gate, session_id=session_id, on_sleep=sleep)
    context = LLMContext(tools=tools)

    user_agg, assistant_agg = LLMContextAggregatorPair(
        context,
        user_params=LLMUserAggregatorParams(
            vad_analyzer=SileroVADAnalyzer(params=VADParams(stop_secs=settings.vad_stop_secs)),
            user_turn_strategies=UserTurnStrategies(
                stop=[TurnAnalyzerUserTurnStopStrategy(turn_analyzer=models.turn)]
            ),
            user_idle_timeout=settings.idle_timeout_secs,
        ),
    )

    @user_agg.event_handler("on_user_turn_idle")
    async def on_idle(_agg) -> None:
        logger.info(f"idle for {settings.idle_timeout_secs}s")
        await sleep()

    pipeline = Pipeline(
        [
            transport.input(),
            models.stt,
            ConfirmationProcessor(gate, session_id),
            user_agg,
            llm,
            models.tts,
            transport.output(),
            assistant_agg,
        ]
    )
    worker = PipelineWorker(
        pipeline,
        params=PipelineParams(audio_in_sample_rate=16000, enable_metrics=True),
        processor_unusable_policy=ProcessorUnusablePolicy.END,
    )
    runner = WorkerRunner(handle_sigint=False)
    session = GhostSession(session_id=session_id, worker=worker, runner=runner, _ended=ended)

    async def start() -> None:
        await runner.add_workers(worker)
        if greeting:
            from pipecat.frames.frames import TTSSpeakFrame

            await worker.queue_frames([TTSSpeakFrame(greeting)])
        else:
            await worker.queue_frames([LLMRunFrame()])

    session.start = start  # type: ignore[attr-defined]
    return session
